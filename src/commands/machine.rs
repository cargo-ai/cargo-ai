//! Explicit machine presentation and negotiation for application callers.
use clap::{Arg, ArgMatches, Command};
use serde_json::{json, Value};
use std::io::{self, Write};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{mpsc, Mutex};

const RESPONSE_LIMIT: usize = 8 * 1024 * 1024;
const EVENT_LIMIT: usize = 64 * 1024;
const THINKING_RECORD_LIMIT: usize = 256;
const THINKING_BYTES_LIMIT: usize = 512 * 1024;
static SELECTED: AtomicBool = AtomicBool::new(false);
static CANCELED: AtomicBool = AtomicBool::new(false);
static ACTIVE_LANES: AtomicUsize = AtomicUsize::new(0);
static OBSERVER: Mutex<Option<Observer>> = Mutex::new(None);

#[derive(Debug)]
pub(crate) struct Failure {
    pub code: &'static str,
    pub message: &'static str,
    pub data: Value,
    pub retryable: bool,
}
impl Failure {
    pub fn new(code: &'static str, message: &'static str) -> Self {
        Self {
            code,
            message,
            data: Value::Null,
            retryable: false,
        }
    }
    pub fn with_data(mut self, data: Value) -> Self {
        self.data = data;
        self
    }
}

struct Observer {
    id: String,
    sender: mpsc::SyncSender<(Vec<u8>, bool)>,
    stream: bool,
    content: bool,
    sequence: u64,
    output: Option<Value>,
    error: Option<Failure>,
    artifacts: Vec<Value>,
    thinking: Vec<Value>,
    thinking_bytes: usize,
    thinking_omitted: usize,
    child_instrumentation: bool,
    truncated: bool,
    client_action: Option<Value>,
    role_session: Option<crate::role_session::Session>,
}

pub(crate) fn selected() -> bool {
    SELECTED.load(Ordering::Relaxed)
}
pub(crate) fn canceled() -> bool {
    selected() && CANCELED.load(Ordering::Relaxed)
}
pub(crate) struct LaneCompletion(bool);
pub(crate) fn lane_completion() -> LaneCompletion {
    let tracked = selected();
    if tracked {
        ACTIVE_LANES.fetch_add(1, Ordering::AcqRel);
    }
    LaneCompletion(tracked)
}
impl Drop for LaneCompletion {
    fn drop(&mut self) {
        if self.0 {
            ACTIVE_LANES.fetch_sub(1, Ordering::AcqRel);
        }
    }
}
async fn settle_lanes() -> bool {
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        while ACTIVE_LANES.load(Ordering::Acquire) != 0 {
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    })
    .await
    .is_ok()
}
pub(crate) fn arguments(command: Command) -> Command {
    command
        .arg(
            Arg::new("output_format")
                .long("output-format")
                .global(true)
                .value_name("FORMAT")
                .help("Select the application contract: json or ndjson"),
        )
        .arg(
            Arg::new("output_schema_version")
                .long("output-schema-version")
                .global(true)
                .value_name("VERSION")
                .requires("output_format")
                .help("Application envelope revision (1)"),
        )
        .arg(
            Arg::new("include_result_content")
                .long("include-result-content")
                .global(true)
                .requires("output_format")
                .action(clap::ArgAction::SetTrue)
                .help("Explicitly include private runtime result content on stdout"),
        )
}
pub(crate) fn command() -> Command {
    Command::new("capabilities")
        .about("Inspect compiled application contracts without reading state or using network")
}
pub(crate) fn requested(words: &[std::ffi::OsString]) -> bool {
    words.iter().any(|s| {
        s == "--output-format"
            || s == "--output-schema-version"
            || s == "--include-result-content"
            || s.to_str().is_some_and(|s| {
                s.starts_with("--output-format=") || s.starts_with("--output-schema-version=")
            })
    })
}
pub(crate) fn parser_contract_supported(words: &[std::ffi::OsString]) -> bool {
    let mut format_present = false;
    let mut format_required = false;
    for (index, word) in words.iter().enumerate() {
        let Some(word) = word.to_str() else { continue };
        let value = |flag: &str| {
            if word == flag {
                Some(words.get(index + 1).and_then(|value| value.to_str()))
            } else {
                word.strip_prefix(&format!("{flag}=")).map(Some)
            }
        };
        if let Some(format) = value("--output-format") {
            format_present = true;
            // Help/version are finite passive responses, never run streams.
            if format != Some("json") {
                return false;
            }
        }
        if let Some(schema) = value("--output-schema-version") {
            format_required = true;
            if schema != Some("1") {
                return false;
            }
        }
        if word == "--include-result-content" {
            format_required = true;
        }
    }
    !format_required || format_present
}
fn command_path(matches: &ArgMatches) -> String {
    let mut m = matches;
    let mut names = Vec::new();
    while let Some((name, child)) = m.subcommand() {
        names.push(name);
        m = child;
    }
    names.join(" ")
}
fn timestamp() -> String {
    time::OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_default()
}
fn build() -> Value {
    json!({"version":env!("CARGO_PKG_VERSION"),"developer_tools":cfg!(feature="developer-tools"),"target_os":std::env::consts::OS,"target_arch":std::env::consts::ARCH})
}
fn context() -> Value {
    json!({"home_selection":if std::env::var_os("CARGO_AI_HOME").is_some(){"explicit"}else{"default"},"keychain_scope":"shared_profile_name_not_home_isolated"})
}
fn outcome(error: Option<&Failure>) -> &'static str {
    match error.map(|e| e.code) {
        None => "succeeded",
        Some("cli.interaction_required") => "requires_interaction",
        Some("cli.canceled") => "canceled",
        Some("operation.partial") => "partial",
        Some(_)
            if error
                .is_some_and(|e| e.data.get("partial").and_then(Value::as_bool) == Some(true)) =>
        {
            "partial"
        }
        Some(_) => "failed",
    }
}
fn payload_schema(command: &str) -> String {
    let revision = if matches!(command, "actions list" | "actions validate") {
        2
    } else {
        1
    };
    format!("cargo-ai.{}.v{revision}", command.replace(' ', "."))
}
fn envelope(
    command: &str,
    id: &str,
    data: Value,
    error: Option<&Failure>,
    warnings: Vec<Value>,
) -> Value {
    json!({"schema_version":1,"payload_schema":if matches!(command,"actions list"|"actions validate") && data.get("schema_version")==Some(&json!(3)){format!("cargo-ai.{}.v3",command.replace(' ',"."))}else{payload_schema(command)},"command":command,"build":build(),"request_id":id,"context":context(),"outcome":outcome(error),"data":data,"warnings":warnings,"error":error.map(|e|json!({"code":e.code,"message":e.message,"retryable":e.retryable})),"completion":{"terminal":true,"complete":true}})
}
fn write_value(value: &Value, limit: usize) -> io::Result<()> {
    let bytes = serde_json::to_vec(value)?;
    if bytes.len() > limit {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Machine response exceeds its limit",
        ));
    }
    let mut out = io::stdout().lock();
    out.write_all(&bytes)?;
    out.write_all(b"\n")?;
    out.flush()
}
pub(crate) fn parser_failure(kind: clap::error::ErrorKind, supported: bool) -> ! {
    let (command, data, error, exit) = if !supported {
        (
            "parse",
            Value::Null,
            Some(Failure::new(
                "cli.unsupported_contract",
                "Requested parser response format or schema is unavailable.",
            )),
            2,
        )
    } else {
        match kind {
            clap::error::ErrorKind::DisplayVersion => ("version", build(), None, 0),
            clap::error::ErrorKind::DisplayHelp => ("capabilities", capabilities(), None, 0),
            _ => (
                "parse",
                Value::Null,
                Some(Failure::new(
                    "cli.invalid_input",
                    "Invalid command arguments; inspect capabilities or command help.",
                )),
                2,
            ),
        }
    };
    let value = envelope(
        command,
        &uuid::Uuid::new_v4().to_string(),
        data,
        error.as_ref(),
        vec![],
    );
    let (sender, ack) = writer();
    let bytes = serde_json::to_vec(&value).unwrap_or_default();
    let delivered = sender.try_send((bytes, true)).is_ok()
        && ack
            .recv_timeout(std::time::Duration::from_secs(2))
            .unwrap_or(false);
    std::process::exit(if delivered { exit } else { 1 })
}

fn writer() -> (mpsc::SyncSender<(Vec<u8>, bool)>, mpsc::Receiver<bool>) {
    let (sender, receiver) = mpsc::sync_channel::<(Vec<u8>, bool)>(64);
    let (ack, acknowledgments) = mpsc::channel();
    std::thread::spawn(move || {
        while let Ok((bytes, terminal)) = receiver.recv() {
            let result = (|| {
                let mut out = io::stdout().lock();
                out.write_all(&bytes)?;
                out.write_all(b"\n")?;
                out.flush()
            })();
            if terminal || result.is_err() {
                let _ = ack.send(result.is_ok());
                break;
            }
        }
    });
    (sender, acknowledgments)
}
async fn writer_ack(receiver: &mpsc::Receiver<bool>) -> bool {
    loop {
        match receiver.try_recv() {
            Ok(result) => return result,
            Err(mpsc::TryRecvError::Disconnected) => return false,
            Err(mpsc::TryRecvError::Empty) => {
                tokio::time::sleep(std::time::Duration::from_millis(10)).await
            }
        }
    }
}
fn progress_capacity(observer: &mut Observer) -> bool {
    if observer.sequence >= 63 {
        observer.truncated = true;
        false
    } else {
        true
    }
}

pub(crate) fn event(kind: &'static str, data: Value) {
    if !selected() || canceled() {
        return;
    }
    let mut guard = OBSERVER.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(o) = guard.as_mut() {
        if !o.stream {
            return;
        }
        // Reserving one queue slot keeps essential terminal state independent
        // of progress volume. Slow pipes never hold the observer mutex.
        if !progress_capacity(o) {
            return;
        }
        let mut value = json!({"schema_version":1,"event_type":kind,"operation_id":o.id,"root_invocation_id":o.id,"invocation_id":o.id,"sequence":o.sequence,"timestamp":timestamp(),"data":data});
        if let Some(action) = &o.client_action {
            value["client_action"] = action.clone();
        }
        match serde_json::to_vec(&value) {
            Ok(bytes) if bytes.len() <= EVENT_LIMIT => {
                if o.sender.try_send((bytes, false)).is_ok() {
                    o.sequence += 1;
                } else {
                    o.truncated = true;
                }
            }
            _ => o.truncated = true,
        }
    }
}
async fn stop_operation(operation: &mut tokio::task::JoinHandle<Result<Value, Failure>>) -> bool {
    CANCELED.store(true, Ordering::Relaxed);
    let first = super::machine_process::terminate_all().is_ok();
    let settled = if tokio::time::timeout(std::time::Duration::from_secs(2), &mut *operation)
        .await
        .is_ok()
    {
        true
    } else {
        operation.abort();
        tokio::time::timeout(std::time::Duration::from_millis(250), &mut *operation)
            .await
            .is_ok()
    };
    let lanes_settled = settle_lanes().await;
    let swept = super::machine_process::terminate_all().is_ok();
    first && settled && lanes_settled && swept
}
pub(crate) fn record_error(error: Failure) {
    crate::role_runtime::note_error(error.code);
    if selected() {
        if let Some(o) = OBSERVER.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
            if o.error.is_none() {
                o.error = Some(error);
            }
        }
    }
}

/// Associates a validated declaration with this execution, without retaining inputs.
pub(crate) fn record_client_action(action: Value) {
    if selected() {
        if let Some(observer) = OBSERVER.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
            observer.client_action = Some(action.clone());
        }
        event("client_action_resolved", action);
    }
}
pub(crate) fn record_process_error(error: &io::Error) {
    let code = match error.kind() {
        io::ErrorKind::TimedOut => "runtime.timeout",
        io::ErrorKind::Interrupted => "cli.canceled",
        io::ErrorKind::InvalidData => "runtime.output_limit",
        _ => "runtime.subprocess_failed",
    };
    record_error(Failure::new(
        code,
        "Owned subprocess execution did not complete.",
    ));
}
pub(crate) fn record_result(value: &Value) {
    crate::role_runtime::capture_business_result(value);
    if selected() {
        if let Some(o) = OBSERVER.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
            if o.content
                && serde_json::to_vec(value).map_or(true, |bytes| bytes.len() > 4 * 1024 * 1024)
            {
                o.error = Some(
                    Failure::new(
                        "runtime.result_limit",
                        "Private result content exceeds its delivery bound.",
                    )
                    .with_data(json!({"partial":true})),
                );
                o.output = Some(
                    json!({"availability":"available","content_included":false,"omission":"content_limit"}),
                );
                return;
            }
            o.output = Some(if o.content {
                json!({"availability":"available","content":value})
            } else {
                json!({"availability":"available","content_included":false})
            });
        }
    }
}
/// Retains authorized export metadata only when private terminal content was requested.
pub(crate) fn record_result_artifacts(references: Value, grants: Value) {
    if !selected() {
        return;
    }
    if let Some(observer) = OBSERVER.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
        if observer.content {
            if let Some(output) = observer.output.as_mut() {
                output["artifact_references"] = references;
                output["artifact_read_grants"] = grants;
            }
        }
    }
}
pub(crate) fn record_artifact(value: Value) {
    if selected() {
        if let Some(o) = OBSERVER.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
            if o.artifacts.len() < 256 {
                o.artifacts.push(value)
            } else {
                o.truncated = true;
            }
        }
    }
}

pub(crate) fn record_thinking(value: Value) {
    if !selected() {
        return;
    }
    let retained = {
        let mut guard = OBSERVER.lock().unwrap_or_else(|e| e.into_inner());
        let Some(observer) = guard.as_mut() else {
            return;
        };
        retain_thinking(observer, &value)
    };
    if retained {
        event("thinking_resolved", value);
    }
}

fn retain_thinking(observer: &mut Observer, value: &Value) -> bool {
    let size = serde_json::to_vec(value).map_or(usize::MAX, |bytes| bytes.len());
    if size <= 16 * 1024
        && observer.thinking.len() < THINKING_RECORD_LIMIT
        && size <= THINKING_BYTES_LIMIT.saturating_sub(observer.thinking_bytes)
    {
        observer.thinking_bytes += size;
        observer.thinking.push(value.clone());
        true
    } else {
        observer.thinking_omitted += 1;
        false
    }
}

fn thinking_data(observer: &Observer) -> Value {
    json!({"records":observer.thinking,"omitted_records":observer.thinking_omitted,
        "coverage":if observer.thinking_omitted == 0 {"complete_observed_scopes"} else {"bounded_partial"}})
}

pub(crate) fn operation_id() -> Option<String> {
    OBSERVER
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_ref()
        .map(|observer| observer.id.clone())
}
pub(crate) fn record_role_session(session: crate::role_session::Session) {
    if let Some(observer) = OBSERVER.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
        observer.role_session = Some(session)
    }
}
fn merge_thinking(mut data: Value, observer: &Observer) -> Value {
    if let Some(session) = &observer.role_session {
        if !data.is_object() {
            data = json!({"operation_data":data});
        }
        data["native_role_execution"] = session.snapshot();
    }
    if let Some(action) = &observer.client_action {
        if !data.is_object() {
            data = json!({"operation_data":data});
        }
        data["client_action"] = action.clone();
    }
    if observer.thinking.is_empty() && observer.thinking_omitted == 0 {
        return data;
    }
    if !data.is_object() {
        data = json!({"operation_data":data});
    }
    data["thinking"] = thinking_data(observer);
    data
}

fn thinking_warnings(observer: &Observer) -> Vec<Value> {
    let mut warnings = Vec::new();
    for record in &observer.thinking {
        if let Some(fallback) = record.pointer("/outcome/fallback").and_then(Value::as_str) {
            let code = match fallback {
                "unavailable_choice" => "thinking.choice_unavailable",
                "unsupported_control" => "thinking.control_unsupported",
                _ => "thinking.support_unknown",
            };
            warnings.push(json!({"code":code,"message":"The requested thinking setting was not applied; provider default is used.",
                "scope":record.get("scope"),"requested":record.pointer("/outcome/requested"),
                "reason":fallback,"effective":{"mode":"provider_default"}}));
        } else if record.get("disposition").and_then(Value::as_str) == Some("not_forwarded") {
            warnings.push(json!({"code":"thinking.child_capability_unavailable",
                "message":"The child does not declare support for the requested thinking setting; its existing configuration is preserved and its effective setting is unverified.",
                "scope":record.get("scope"),"requested":record.get("requested"),"effective":"child_unverified"}));
        }
    }
    if observer.thinking_omitted != 0 {
        warnings.push(json!({"code":"thinking.outcomes_truncated","message":"Some thinking outcomes exceeded the collection bound.","omitted_records":observer.thinking_omitted}));
    }
    warnings
}
pub(crate) fn opaque_child() {
    if selected() {
        if let Some(o) = OBSERVER.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
            o.child_instrumentation = false;
        }
    }
}

fn contracts() -> Vec<Value> {
    let finite = [
        "capabilities",
        "version",
        "actions list",
        "actions resolve",
        "actions validate",
        "actions resource",
        "actions artifact",
        "profile list",
        "profile show",
        "profile refresh-context",
        "profile validate-context",
        "profile add",
        "profile set",
        "profile remove",
        "account status",
        "account register",
        "account confirm",
        "account deactivate",
        "auth login openai",
        "new",
        "packages list",
        "packages inspect",
        "packages install",
        "packages pull",
        "packages visibility",
        "packages publish",
        "agents list",
        "agents pull",
        "agents push",
        "agents visibility",
        "usage context",
        "usage summary",
        "usage runs",
        "usage show",
        "usage settings",
        "usage backup status",
        "usage backup enable",
        "usage backup disable",
        "usage backup sync",
        "usage backup include-history",
        "models list",
        "models thinking",
    ];
    let mut values: Vec<Value> = finite.iter().map(|name| {
        let effects=match *name {
            "capabilities"|"version"=>vec!["none"],
            "actions list"|"actions validate"|"actions resource"|"actions artifact"=>vec!["explicit_project_or_installed_package_read"],
            "actions resolve"=>vec!["explicit_project_or_installed_package_read","protected_connection_context_read"],
            "profile refresh-context"=>vec!["protected_context_metadata_mutation","selected_credential_read","public_verification_keys_read_for_account_profiles","no_provider_invocation"],
            "profile validate-context"=>vec!["protected_connection_context_read","selected_credential_read","explicit_renew_public_keys_and_proven_context_metadata","no_provider_invocation","no_provider_credential_mutation"],
            "profile list"=>vec!["configuration_read"],
            "profile show"=>vec!["configuration_read","credential_presence_lookup"],
            "models list"|"models thinking"=>vec!["connection_read","provider_catalog_read"],
            "profile add"|"profile set"|"profile remove"=>vec!["configuration_or_credentials_mutation"],
            "new"|"package"=>vec!["project_files_mutation"],
            "account status"=>vec!["account_read","possible_session_refresh_and_persistence"],
            "account register"|"account confirm"|"account deactivate"=>vec!["remote_account_mutation","local_account_persistence"],
            "auth login openai"=>vec!["none_until_existing_terminal_flow"],
            name if name.starts_with("usage backup")=>vec!["local_queue_or_consent_maintenance","explicit_remote_operation_when_applicable"],
            name if name.starts_with("usage")=>vec!["local_usage_read_or_explicit_settings_mutation"],
            _=>vec!["selected_local_or_account_operation"],
        };
        let variants=match *name {
            "packages list"=>json!([{"selector":"installed","pagination":"limit_and_all","legacy_default_limit":20},{"selector":"account","pagination":"all_and_existing_limit"}]),
            "actions list"|"actions resolve"|"actions validate"|"actions resource"|"actions artifact"=>json!([{"selector":"explicit_project"},{"selector":"installed_package_alias"}]),
            "packages inspect"=>json!([{"selector":"installed_alias"},{"selector":"account_name_and_optional_version"}]),
            "models list"|"models thinking"=>json!([{"selector":"saved_profile","api_key_store":"explicit_file_only","auth_modes":["none","api_key","openai_account"]},{"selector":"draft_server_auth","api_key_input":"stdin","auth_modes":["none","api_key","openai_account"],"account_provider":"openai"}]),
            "usage summary"|"usage runs"|"usage show"=>json!([{"domain_schema_version":1},{"domain_schema_version":2}]),
            "account deactivate"=>json!([{"deletion_request":false},{"deletion_request":true,"requires":"matching_confirm_email"}]),
            "profile validate-context"=>json!([{"selector":"without_renew","effects":"read_only"},{"selector":"--renew","effects":"public_key_fetch_and_proven_context_metadata_update"}]),
            _=>json!([]),
        };
        let mut contract = json!({"command":name,"payload_schema":payload_schema(name),"formats":["json"],"schema_versions":[1],"effects":effects,"variants":variants,"interaction":if *name=="auth login openai"{"existing_terminal_protocol_exception"}else{"noninteractive_explicit_consent_required_when_applicable"}});
        if matches!(*name, "actions list" | "actions validate") {
            let selector = if *name == "actions list" { "catalog_version" } else { "request_version" };
            contract["payload_schemas"] = json!([
                {"payload_schema":payload_schema(name),"condition":{(selector):2}},
                {"payload_schema":format!("cargo-ai.{}.v3",name.replace(' ',".")),"condition":{(selector):3}}
            ]);
        }
        if *name == "profile validate-context" {
            contract["continuity_contract_version"] = json!(1);
            contract["validation_does_not_authorize_execution"] = json!(true);
        }
        contract
    }).collect();
    values.push(json!({"command":"run","formats":["json","ndjson"],"schema_versions":[1],"private_content":"explicit_opt_in","opaque_children":"exit_status_only","thinking":{"selection":"tagged_provider_default_or_exact_choice","settings":["choice","provider_default","on","off"],"flags":["--thinking","--thinking-provider-default","--thinking-choice"],"terminal_outcomes":true,"action_definition_revision":"2026-10-01.r1"}}));
    let run = values.last_mut().expect("run contract was appended");
    run["client_actions"] = json!({"schema_version":2,"catalog_versions":[2,3],"request_versions":[2,3],"selector":"--action","request_input":"--action-request-stdin","execution_policy":"required","definition_binding":"required","client_action_correlation":true,"idempotency":false,"limits":super::client_actions::limits()});
    run["native_roles"] = json!({"definition_revision":"2026-10-06.r1","role_contract_version":1,"catalog_version":3,"request_version":3,"resolver":"actions resolve","profile_context_refresh":"profile refresh-context","binding_authorizes_execution":false,"operation_access":"unverified_until_invocation","session":{"version":1,"selector":"--role-session","input":"bounded_json_lines","output":"ndjson","first_frame":"start","controls":["revoke","cancel"],"revision_specific":true,"eof":"closes_admission_and_cancels_invocation","revocation":"admitted_work_may_settle","control_acknowledgements":"reliable_separate_from_progress_budget","frame_bytes":crate::role_session::MAX_FRAME_BYTES,"control_ack_deadline_ms":crate::role_transport::CONTROL_ACK_DEADLINE_MS,"max_control_frames":crate::role_transport::MAX_CONTROL_FRAMES,"control_frames_exclude_start":true,"control_output_failure":"closes_admission_cancels_and_requests_owned_cleanup"},"tool_protocol":2,"generated_runtime":"verified_capability_and_source_identity_required","private_records":"host_owned_outside_package_payload_and_data"});
    run["native_roles"]["continuity"] = json!({"version":1,"command":"profile validate-context","renew_selector":"--renew","approval_expires":false,"unavailable":"pause_and_retain_choices","changed":"affected_review","revoked":"no_reuse","credential_renewal_owner":"existing_credential_owner","validation_does_not_authorize_execution":true});
    run["structured_results"] = json!({"definition_revision":"2026-10-03.r1","source":"selected_root_tool","business_types":["object","array","string","integer","number","boolean","null"],"nullable_union":"T_or_null","private_content":"--include-result-content","descendants":"no_root_publication","artifact_access":{"permission_version":1,"descriptor_version":1,"read_grant_version":1,"read_command":"actions artifact","request_version":1,"native_confinement":cfg!(any(target_os="linux",target_os="macos",windows)),"limits":super::client_actions::limits()["artifact"]}});
    if cfg!(feature = "developer-tools") {
        values.push(json!({"command":"package","formats":["json"],"schema_versions":[1]}));
    }
    values
}
pub(crate) fn capabilities() -> Value {
    let runtime_capabilities = crate::generated_capabilities::decode_record(std::hint::black_box(
        &crate::CLI_RUN_RUNTIME_CAPABILITY_RECORD,
    ))
    .ok();
    json!({"build":build(),"contracts":contracts(),"runtime_capabilities":runtime_capabilities,"installed_capability_is_not_live_access":true,"legacy_formats":{"version_and_help":"text","usage_json":"unchanged_v1_v2","agent_pull_stdout":"raw_definition_json","run_json":"input_definition"},"limits":{"response_bytes":RESPONSE_LIMIT,"event_bytes":EVENT_LIMIT,"progress_records":63,"private_result_bytes":4*1024*1024,"terminal_delivery_seconds":2},"storage":{"keychain":"shared_profile_name","discovery_saved_api_key":"explicit_file_only"},"interaction":"noninteractive_by_default","generated_children":"instrumentation_unavailable_unless_explicitly_supported"})
}

fn terminal_event(id: &str, observer: &Observer, data: Value) -> Value {
    let mut value = json!({"schema_version":1,"event_type":"operation_completed","operation_id":id,"root_invocation_id":id,"invocation_id":id,"sequence":observer.sequence,"timestamp":timestamp(),"data":data});
    if let Some(action) = &observer.client_action {
        value["client_action"] = action.clone();
    }
    value
}

pub(crate) async fn dispatch(matches: &ArgMatches) -> Option<i32> {
    let path = command_path(matches);
    let selected_format = matches
        .get_one::<String>("output_format")
        .map(String::as_str);
    if selected_format.is_none()
        && !matches!(
            path.as_str(),
            "capabilities" | "models list" | "models thinking"
        )
    {
        return None;
    }
    SELECTED.store(true, Ordering::Relaxed);
    let format = selected_format.unwrap_or("json");
    let id = uuid::Uuid::new_v4().to_string();
    let unsupported = matches
        .get_one::<String>("output_schema_version")
        .is_some_and(|v| v != "1")
        || !matches!(format, "json" | "ndjson")
        || (format == "ndjson" && path != "run")
        || !contracts().iter().any(|v| v["command"] == path);
    if unsupported {
        let error = Failure::new(
            "cli.unsupported_contract",
            "Requested command, format or schema is unavailable.",
        );
        let _ = write_value(
            &envelope(&path, &id, Value::Null, Some(&error), vec![]),
            RESPONSE_LIMIT,
        );
        return Some(2);
    }
    let (sender, acknowledgments) = writer();
    *OBSERVER.lock().unwrap_or_else(|e| e.into_inner()) = Some(Observer {
        id: id.clone(),
        sender: sender.clone(),
        stream: format == "ndjson",
        content: matches.get_flag("include_result_content"),
        sequence: 0,
        output: None,
        error: None,
        artifacts: vec![],
        thinking: vec![],
        thinking_bytes: 0,
        thinking_omitted: 0,
        child_instrumentation: true,
        truncated: false,
        client_action: None,
        role_session: None,
    });
    event("operation_started", json!({"command":path}));
    let owned_matches = matches.clone();
    let mut operation = tokio::spawn(async move { execute(&owned_matches).await });
    let mut result = tokio::select! {
        result=&mut operation=>result.unwrap_or_else(|_|Err(Failure::new("cli.execution_incomplete","The operation stopped before producing a terminal result."))),
        _=writer_ack(&acknowledgments)=>{
            let _=stop_operation(&mut operation).await;
            return Some(1);
        },
        signal=tokio::signal::ctrl_c()=>{
            if signal.is_err(){
                let cleanup=stop_operation(&mut operation).await;
                Err(Failure::new("cli.cancellation_unavailable","Cancellation signal could not be observed.").with_data(json!({"effects":"may_have_been_applied","owned_child_cleanup":if cleanup{"completed"}else{"unconfirmed"}})))
            }else{
                let cleanup=stop_operation(&mut operation).await;
                Err(Failure::new("cli.canceled","Invocation canceled; reconcile already applied effects before retrying.").with_data(json!({"effects":"may_have_been_applied","owned_child_cleanup":if cleanup{"completed"}else{"unconfirmed"}})))
            }
        }
    };
    if !settle_lanes().await {
        CANCELED.store(true, Ordering::Relaxed);
        let _ = super::machine_process::terminate_all();
        result = Err(Failure::new(
            "cli.execution_incomplete",
            "Parallel execution did not settle; reconcile effects before retrying.",
        )
        .with_data(json!({"effects":"may_have_been_applied","owned_child_cleanup":"unconfirmed"})));
    }
    let observer = OBSERVER
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .take()
        .unwrap();
    let (mut data, error) = match result {
        Ok(data) => (data, None),
        Err(e) => (e.data.clone(), Some(e)),
    };
    // Cancellation and interrupted tasks bypass runtime_outcome. Preserve any
    // already validated root result without changing the terminal failure.
    if path == "run" && data.get("result").is_none() {
        if let Some(known_result) = &observer.output {
            if !data.is_object() {
                data = json!({});
            }
            data["result"] = known_result.clone();
            data["execution"] = json!({"completed": false});
        }
    }
    let mut warnings = thinking_warnings(&observer);
    if observer.truncated {
        warnings.push(json!({"code":"cli.output_truncated","message":"Some bounded diagnostic or artifact metadata was omitted."}));
    }
    let value = envelope(
        &path,
        &id,
        merge_thinking(data, &observer),
        error.as_ref(),
        warnings,
    );
    let output = if format == "ndjson" {
        terminal_event(&id, &observer, value)
    } else {
        value
    };
    let mut delivery_error = false;
    let bytes = match serde_json::to_vec(&output) {
        Ok(bytes) if bytes.len() <= RESPONSE_LIMIT => bytes,
        _ => {
            delivery_error = true;
            let failure = Failure::new(
                "cli.output_limit",
                "Response exceeds its output bound; operation effects require reconciliation.",
            );
            let value = envelope(
                &path,
                &id,
                merge_thinking(Value::Null, &observer),
                Some(&failure),
                thinking_warnings(&observer),
            );
            let fallback = if format == "ndjson" {
                terminal_event(&id, &observer, value)
            } else {
                value
            };
            serde_json::to_vec(&fallback).unwrap()
        }
    };
    if sender.try_send((bytes, true)).is_err()
        || !tokio::time::timeout(
            std::time::Duration::from_secs(2),
            writer_ack(&acknowledgments),
        )
        .await
        .unwrap_or(false)
    {
        return Some(1);
    }
    if delivery_error {
        return Some(1);
    }
    Some(if error.is_none() {
        0
    } else if error.as_ref().is_some_and(|e| e.code == "cli.canceled") {
        130
    } else {
        1
    })
}

async fn execute(matches: &ArgMatches) -> Result<Value, Failure> {
    match matches.subcommand() {
        Some(("capabilities", _)) => Ok(capabilities()),
        Some(("version", m))
            if !m.get_flag("check") && m.get_one::<String>("update_mode").is_none() =>
        {
            Ok(build())
        }
        Some(("profile", m)) => super::profile::machine_run(m),
        Some(("actions", m)) => super::client_actions::machine_run(m),
        Some(("new", m)) => super::new::machine_run(m),
        Some(("usage", m)) => {
            if let Some(b) = m.subcommand_matches("backup") {
                crate::usage_backup_cli::machine_run(b).await
            } else {
                super::usage::machine_run(m)
            }
        }
        Some(("models", m)) => {
            let data = super::models::run(m.subcommand().unwrap().1)
                .await
                .map_err(|e| Failure {
                    code: discovery_code(e.code),
                    message: e.message,
                    retryable: e.retryable,
                    data: Value::Null,
                })?;
            if data["complete"] == false {
                Err(Failure::new("discovery.partial","Model discovery reached its bounded request or time budget; the returned catalog is partial.").with_data(json!({"partial":true,"catalog":data})))
            } else {
                Ok(data)
            }
        }
        Some(("account", m)) => super::account::machine_run(m).await,
        Some(("packages", m)) => super::packages::machine_run(m).await,
        Some(("agents", m)) => super::agents::machine_run(m).await,
        #[cfg(feature = "developer-tools")]
        Some(("package", m)) => super::package::machine_run(m),
        Some(("auth", m)) => super::auth::machine_run(m).await,
        Some(("run", m)) => super::run::machine_run(m).await,
        _ => Err(Failure::new(
            "cli.unsupported_contract",
            "No application contract is available for this command.",
        )),
    }
}

fn discovery_code(code: &str) -> &'static str {
    match code {
        "unsupported_secret_store" => "discovery.unsupported_secret_store",
        "unsupported_connection" | "unsupported_endpoint" | "unsupported_provider" => {
            "discovery.unsupported_connection"
        }
        "profile_not_found" => "discovery.profile_not_found",
        "missing_credentials" | "authentication_required" => "discovery.credentials_required",
        "invalid_credentials" => "discovery.invalid_credentials",
        "expired_credentials" => "discovery.credentials_expired",
        "locally_disabled" => "discovery.locally_disabled",
        "changed_connection" => "discovery.connection_changed",
        "invalid_configuration" => "discovery.invalid_configuration",
        "invalid_connection" | "invalid_request" => "cli.invalid_input",
        "unauthorized" | "authentication_failed" => "discovery.authentication_failed",
        "forbidden" | "authorization_failed" => "discovery.authorization_failed",
        "rate_limited" => "discovery.rate_limited",
        "timeout" => "discovery.timeout",
        "connectivity" | "network_error" | "network_failed" => "discovery.network_failed",
        "response_too_large" => "discovery.response_limit",
        "redirect_refused" | "redirect_blocked" => "discovery.redirect_blocked",
        "cancelled" => "cli.canceled",
        "provider_error" => "discovery.provider_failed",
        "unsupported_pagination" => "discovery.unsupported_pagination",
        _ => "discovery.invalid_response",
    }
}

pub(crate) fn runtime_outcome(succeeded: bool) -> Result<Value, Failure> {
    let mut guard = OBSERVER.lock().unwrap_or_else(|e| e.into_inner());
    let Some(o) = guard.as_mut() else {
        return Err(Failure::new(
            "cli.canceled",
            "Invocation no longer accepts runtime results.",
        ));
    };
    let data = json!({"execution":{"completed":succeeded},"result":o.output.clone().unwrap_or_else(||json!({"availability":"not_produced"})),"artifacts":o.artifacts,"child_instrumentation":if o.child_instrumentation{"no_opaque_children"}else{"unavailable"},"effects":{"external":"may_have_been_applied","replay_safe":false}});
    if succeeded && o.error.is_none() {
        Ok(data)
    } else {
        let mut failure = o.error.take().unwrap_or_else(|| {
            Failure::new(
                "runtime.failed",
                "Execution did not complete; reconcile external effects before retrying.",
            )
        });
        let partial = failure.data.get("partial") == Some(&Value::Bool(true));
        failure.data = data;
        if partial {
            failure.data["partial"] = Value::Bool(true);
        }
        Err(failure)
    }
}

#[cfg(test)]
mod thinking_tests {
    use super::*;

    fn observer() -> Observer {
        let (sender, _) = mpsc::sync_channel(64);
        Observer {
            id: "fixture".into(),
            sender,
            stream: true,
            content: false,
            sequence: 63,
            output: None,
            error: None,
            artifacts: vec![],
            thinking: vec![],
            thinking_bytes: 0,
            thinking_omitted: 0,
            child_instrumentation: true,
            truncated: false,
            client_action: None,
            role_session: None,
        }
    }

    #[test]
    fn terminal_thinking_retains_fallback_after_dropped_progress_and_failure() {
        let outcome = crate::providers::thinking::resolve(
            Some(&crate::providers::thinking::ThinkingSetting::Choice {
                value: "max".into(),
            }),
            "invocation",
            crate::providers::thinking::ThinkingSupport::Unsupported {
                reason: "fixture has no named control".into(),
                evidence: None,
            },
        );
        let mut o = observer();
        assert!(retain_thinking(
            &mut o,
            &json!({"scope":{"kind":"invocation"},"outcome":outcome})
        ));
        assert!(!progress_capacity(&mut o));
        assert!(o.truncated);
        let failure = Failure::new("runtime.provider_failed", "Fixture inference failed.");
        let response = envelope(
            "run",
            "fixture",
            merge_thinking(failure.data.clone(), &o),
            Some(&failure),
            thinking_warnings(&o),
        );
        assert_eq!(response["outcome"], "failed");
        assert_eq!(
            response["data"]["thinking"]["records"][0]["outcome"]["effective"]["mode"],
            "provider_default"
        );
        assert_eq!(
            response["warnings"][0]["code"],
            "thinking.control_unsupported"
        );
        assert_eq!(response["data"]["thinking"]["omitted_records"], 0);
    }

    #[test]
    fn terminal_thinking_discloses_bounded_collection_omissions() {
        let mut o = observer();
        assert!(!retain_thinking(
            &mut o,
            &json!({"oversized":"x".repeat(17*1024)})
        ));
        assert_eq!(thinking_data(&o)["coverage"], "bounded_partial");
        assert_eq!(thinking_warnings(&o)[0]["omitted_records"], 1);
    }
}
