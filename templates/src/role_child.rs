//! Bounded parent-owned native pipes. Only verified native children receive bootstrap state.
use crate::role_runtime::{Bootstrap, Context};
use crate::role_session::{Boundary, Permit};
use serde::Deserialize;
use serde_json::{json, Value};
use std::{collections::BTreeMap, io, process::Stdio, time::Duration};
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};

#[cfg(all(windows, not(cargo_ai_cli)))]
#[path = "owned_process_windows.rs"]
mod windows;
const MAX_CHANNEL_BYTES: usize = 4 * 1024 * 1024;
const MAX_FRAMES: usize = 1024;

#[cfg(cargo_ai_cli)]
type Owned = crate::commands::machine_process::OwnedChild;
#[cfg(not(cargo_ai_cli))]
struct Owned {
    pid: u32,
    #[cfg(windows)]
    handle: usize,
}
#[cfg(not(cargo_ai_cli))]
impl Owned {
    fn attach(child: &tokio::process::Child) -> io::Result<Self> {
        let pid = child
            .id()
            .ok_or_else(|| io::Error::other("Missing owned child identity"))?;
        #[cfg(windows)]
        let handle = windows::attach(child)?;
        let owned = Self {
            pid,
            #[cfg(windows)]
            handle,
        };
        #[cfg(windows)]
        windows::resume(pid)?;
        Ok(owned)
    }
    fn terminate(&self) -> io::Result<()> {
        #[cfg(unix)]
        {
            unsafe extern "C" {
                fn kill(pid: i32, signal: i32) -> i32;
            }
            if unsafe { kill(-(self.pid as i32), 9) } == 0 {
                return Ok(());
            }
            let error = io::Error::last_os_error();
            if error.raw_os_error() == Some(3) {
                Ok(())
            } else {
                Err(error)
            }
        }
        #[cfg(windows)]
        {
            windows::terminate(self.handle)
        }
        #[cfg(not(any(unix, windows)))]
        {
            Err(io::Error::other("Native descendant ownership unavailable"))
        }
    }
}
#[cfg(not(cargo_ai_cli))]
impl Drop for Owned {
    fn drop(&mut self) {
        let _ = self.terminate();
        #[cfg(windows)]
        windows::close(self.handle);
    }
}

fn prepare(command: &mut tokio::process::Command) -> io::Result<()> {
    #[cfg(cargo_ai_cli)]
    {
        crate::commands::machine_process::prepare(command)?;
    }
    #[cfg(not(cargo_ai_cli))]
    {
        command.kill_on_drop(true);
        #[cfg(unix)]
        command.process_group(0);
        #[cfg(windows)]
        command.creation_flags(windows::CREATE_SUSPENDED);
    }
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    command
        .env_remove(crate::execution_policy::CHILD_POLICY_ENV)
        .env_remove("CARGO_AI_EXECUTION_POLICY_REQUIRED_V1");
    Ok(())
}
fn invalid() -> String {
    crate::role_runtime::failure("role.invalid_child_control")
}

pub fn verify_command(
    command: &tokio::process::Command,
    bootstrap: &Bootstrap,
    cli_run: bool,
) -> Result<(), String> {
    let program = std::path::Path::new(command.as_std().get_program());
    let capabilities = crate::generated_capabilities::capabilities_for_artifact(program)
        .map_err(|_| crate::role_runtime::failure("role.child_rebuild_required"))?;
    if !capabilities.supports_native_roles() || capabilities.is_cli_run() != cli_run {
        return Err(crate::role_runtime::failure("role.child_rebuild_required"));
    }
    if !cli_run {
        let path = bootstrap.package_root.join(&bootstrap.definition);
        let bytes = read_definition(&path)?;
        let value = crate::business_schema::strict_json_bounded(&bytes, 1024 * 1024, 64)
            .map_err(|_| invalid())?;
        let expected = crate::role_contract::canonical_identity(&value).map_err(|_| invalid())?;
        if crate::generated_capabilities::definition_for_artifact(program)
            .ok()
            .as_deref()
            != Some(&expected)
        {
            return Err(crate::role_runtime::failure("role.child_rebuild_required"));
        }
    }
    Ok(())
}

fn read_definition(path: &std::path::Path) -> Result<Vec<u8>, String> {
    use std::io::Read;
    let file = std::fs::File::open(path).map_err(|_| invalid())?;
    let mut bytes = Vec::new();
    file.take(1024 * 1024 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| invalid())?;
    if bytes.len() > 1024 * 1024 {
        return Err(invalid());
    }
    Ok(bytes)
}

async fn read_line(reader: &mut (impl AsyncBufRead + Unpin)) -> io::Result<Option<Vec<u8>>> {
    let mut bytes = Vec::new();
    loop {
        let available = reader.fill_buf().await?;
        if available.is_empty() {
            return if bytes.is_empty() {
                Ok(None)
            } else {
                Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "Truncated child frame",
                ))
            };
        }
        let count = available
            .iter()
            .position(|b| *b == b'\n')
            .map(|i| i + 1)
            .unwrap_or(available.len());
        if bytes.len() + count > crate::role_session::MAX_FRAME_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Child frame limit",
            ));
        }
        bytes.extend_from_slice(&available[..count]);
        reader.consume(count);
        if bytes.last() == Some(&b'\n') {
            return Ok(Some(bytes));
        }
    }
}
async fn write_frame(
    writer: &mut (impl tokio::io::AsyncWrite + Unpin),
    value: &Value,
) -> Result<(), String> {
    let bytes = serde_json::to_vec(value).map_err(|_| invalid())?;
    if bytes.len() + 1 > crate::role_session::MAX_FRAME_BYTES {
        return Err(invalid());
    }
    writer.write_all(&bytes).await.map_err(|_| invalid())?;
    writer.write_all(b"\n").await.map_err(|_| invalid())?;
    writer.flush().await.map_err(|_| invalid())
}
async fn drain(stderr: tokio::process::ChildStderr) -> Result<(), String> {
    let mut bytes = Vec::new();
    stderr
        .take(MAX_CHANNEL_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .await
        .map_err(|_| invalid())?;
    if bytes.len() > MAX_CHANNEL_BYTES {
        return Err(invalid());
    }
    Ok(())
}

pub async fn describe(
    mut command: tokio::process::Command,
    context: Context,
    tool_name: &str,
    remaining: Duration,
) -> Result<Vec<u8>, String> {
    context.validate_context()?;
    let permit = context
        .session
        .admit(Boundary {
            invocation_id: context.session.invocation_id.clone(),
            binding_revision: context.session.binding_revision.clone(),
            parent_permit_id: context.parent_permit_id.clone(),
            call_site: format!("tools.{tool_name}"),
            agent: context.locator.definition.clone(),
            target_agent: None,
            kind: "tool".into(),
        })
        .await?;
    prepare(&mut command).map_err(|_| invalid())?;
    command.arg("describe").stdin(Stdio::null());
    let mut child = command.spawn().map_err(|_| invalid())?;
    let owned = Owned::attach(&child).map_err(|_| invalid())?;
    let stdout = child.stdout.take().ok_or_else(invalid)?;
    let stderr = child.stderr.take().ok_or_else(invalid)?;
    let work = async {
        let read = async {
            let mut bytes = Vec::new();
            stdout
                .take(MAX_CHANNEL_BYTES as u64 + 1)
                .read_to_end(&mut bytes)
                .await
                .map_err(|_| invalid())?;
            if bytes.len() > MAX_CHANNEL_BYTES {
                return Err(invalid());
            }
            Ok(bytes)
        };
        let (bytes, (), status) = tokio::try_join!(read, drain(stderr), async {
            child.wait().await.map_err(|_| invalid())
        })?;
        if !status.success() {
            return Err(crate::role_runtime::failure("role.tool_describe_failed"));
        }
        Ok(bytes)
    };
    let outcome = tokio::select! {result=tokio::time::timeout(remaining,work)=>result.unwrap_or_else(|_|Err(crate::role_runtime::failure("role.timeout"))),_=context.session.canceled()=>Err(crate::role_runtime::failure("role.canceled"))};
    if outcome.is_err() {
        let _ = owned.terminate();
        let _ = child.kill().await;
        let _ = tokio::time::timeout(Duration::from_secs(2), child.wait()).await;
    }
    if outcome.is_ok() {
        permit.finish("completed", None)
    } else {
        permit.finish("completion_unknown", Some("interrupted"))
    }
    outcome
}
#[derive(Clone, Debug)]
pub struct ChildResult {
    pub result: Value,
    pub error: Option<Value>,
}

#[derive(Clone)]
pub struct ChildOptions {
    pub current_depth: u32,
    pub max_depth: u32,
    pub max_runtime_secs: u64,
    pub started_at_ms: u64,
    pub deadline_ms: u64,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ToolRequest {
    protocol_version: u32,
    #[serde(rename = "type")]
    kind: String,
    request_id: String,
    call_site: String,
    inputs: Value,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ToolResult {
    protocol_version: u32,
    #[serde(rename = "type")]
    kind: String,
    result: Value,
}

async fn invoke_tool_child(
    context: Context,
    tool_name: &str,
    request: &ToolRequest,
    options: &ChildOptions,
    remaining: Duration,
) -> Result<ChildResult, String> {
    let call = context
        .bootstrap
        .resolution
        .calls
        .iter()
        .find(|call| {
            call.call_site.id == request.call_site
                && call.call_site.kind == crate::role_contract::CallKind::ToolChild
                && call.call_site.locator.definition == context.locator.definition
                && call
                    .call_site
                    .locator
                    .site
                    .starts_with(&format!("tools.{tool_name}."))
        })
        .ok_or_else(|| crate::role_runtime::failure("role.undeclared_call_site"))?
        .clone();
    let at = context.at(call.call_site.locator.site.clone());
    at.scope(async {
        at.selected()?;
        let schema = call.call_site.input_schema.as_ref().ok_or_else(invalid)?;
        crate::business_schema::validate_value_bounds(&request.inputs, 0).map_err(|_| invalid())?;
        crate::business_schema::validate_value(schema, &request.inputs, 0)
            .map_err(|_| crate::role_runtime::failure("role.invalid_tool_inputs"))?;
        let target = call.call_site.target.as_deref().ok_or_else(invalid)?;
        let bootstrap = at.child_bootstrap(target)?;
        if options.current_depth >= options.max_depth {
            return Err(crate::role_runtime::failure("role.depth_limit"));
        }
        let mut command = if let Some(artifact) = &call.call_site.artifact {
            tokio::process::Command::new(context.bootstrap.package_root.join(artifact))
        } else {
            let mut command = tokio::process::Command::new("cargo-ai");
            command
                .arg("run")
                .arg(context.bootstrap.package_root.join(target));
            command
        };
        let cli_run = call.call_site.artifact.is_none();
        crate::execution_policy::propagate_child(&mut command, cli_run)?;
        verify_command(&command, &bootstrap, cli_run)?;
        for (key, value) in request.inputs.as_object().ok_or_else(invalid)? {
            let value = value
                .as_str()
                .map(str::to_owned)
                .unwrap_or_else(|| value.to_string());
            command.arg("--run-var").arg(format!("{key}={value}"));
        }
        command.current_dir(&context.bootstrap.package_root);
        command.env(
            "CARGO_AI_AGENT_ACTION_DEPTH",
            (options.current_depth + 1).to_string(),
        );
        command.env(
            "CARGO_AI_AGENT_ACTION_MAX_DEPTH",
            options.max_depth.to_string(),
        );
        command.env(
            "CARGO_AI_AGENT_MAX_RUNTIME_SECS",
            options.max_runtime_secs.to_string(),
        );
        command.env(
            "CARGO_AI_AGENT_RUNTIME_STARTED_AT_MS",
            options.started_at_ms.to_string(),
        );
        command.env(
            "CARGO_AI_AGENT_RUNTIME_DEADLINE_MS",
            options.deadline_ms.to_string(),
        );
        native(command, at.clone(), bootstrap, remaining).await
    })
    .await
}

/// A package tool receives only business arguments and safe child results.
pub async fn tool(
    mut command: tokio::process::Command,
    mut context: Context,
    tool_name: &str,
    params: Value,
    options: ChildOptions,
    remaining: Duration,
) -> Result<std::process::Output, String> {
    context.validate_context()?;
    let launch = context
        .session
        .admit(Boundary {
            invocation_id: context.session.invocation_id.clone(),
            binding_revision: context.session.binding_revision.clone(),
            parent_permit_id: context.parent_permit_id.clone(),
            call_site: format!("tools.{tool_name}"),
            agent: context.locator.definition.clone(),
            target_agent: None,
            kind: "tool".into(),
        })
        .await?;
    context.parent_permit_id = Some(launch.id().to_owned());
    prepare(&mut command).map_err(|_| invalid())?;
    let mut child = command
        .spawn()
        .map_err(|_| crate::role_runtime::failure("role.tool_launch_failed"))?;
    let owned = Owned::attach(&child).map_err(|_| invalid())?;
    let mut stdin = child.stdin.take().ok_or_else(invalid)?;
    let mut stdout = BufReader::new(child.stdout.take().ok_or_else(invalid)?);
    let stderr = child.stderr.take().ok_or_else(invalid)?;
    let started = std::time::Instant::now();
    let work = async {
        let conversation = async {
            write_frame(
                &mut stdin,
                &json!({"protocol_version":2,"type":"invoke","params":params}),
            )
            .await?;
            let mut result = None;
            let mut total = 0;
            let mut count = 0;
            let mut requests = std::collections::BTreeSet::new();
            while let Some(bytes) = read_line(&mut stdout).await.map_err(|_| invalid())? {
                count += 1;
                total += bytes.len();
                if count > 256 || total > MAX_CHANNEL_BYTES || result.is_some() {
                    return Err(invalid());
                }
                let frame = crate::business_schema::strict_json_bounded(
                    &bytes,
                    crate::role_session::MAX_FRAME_BYTES,
                    32,
                )
                .map_err(|_| invalid())?;
                match frame["type"].as_str() {
                    Some("child_request") => {
                        let request: ToolRequest =
                            serde_json::from_value(frame).map_err(|_| invalid())?;
                        if request.protocol_version != 2
                            || request.kind != "child_request"
                            || !crate::role_contract::identifier(&request.request_id)
                            || !requests.insert(request.request_id.clone())
                        {
                            return Err(invalid());
                        }
                        let left = remaining
                            .checked_sub(started.elapsed())
                            .ok_or_else(|| crate::role_runtime::failure("role.timeout"))?;
                        let response = match invoke_tool_child(
                            context.clone(),
                            tool_name,
                            &request,
                            &options,
                            left,
                        )
                        .await
                        {
                            Ok(child) => {
                                json!({"protocol_version":2,"type":"child_result","request_id":request.request_id,"result":child.result,"error":child.error.as_ref().and_then(|e|e.get("code")).and_then(Value::as_str)})
                            }
                            Err(_) => {
                                json!({"protocol_version":2,"type":"child_result","request_id":request.request_id,"result":null,"error":"role.child_failed"})
                            }
                        };
                        write_frame(&mut stdin, &response).await?;
                    }
                    Some("result") => {
                        let value: ToolResult =
                            serde_json::from_value(frame).map_err(|_| invalid())?;
                        if value.protocol_version != 2
                            || value.kind != "result"
                            || !(value.result.is_string() || value.result.is_null())
                        {
                            return Err(invalid());
                        }
                        result = Some(value.result);
                    }
                    _ => return Err(invalid()),
                }
            }
            result.ok_or_else(invalid)
        };
        let (result, (), status) = tokio::try_join!(conversation, drain(stderr), async {
            child.wait().await.map_err(|_| invalid())
        })?;
        Ok(std::process::Output {
            status,
            stdout: serde_json::to_vec(&json!({"protocol_version":1,"result":result}))
                .map_err(|_| invalid())?,
            stderr: Vec::new(),
        })
    };
    let outcome = tokio::select! {result=tokio::time::timeout(remaining,work)=>result.unwrap_or_else(|_|Err(crate::role_runtime::failure("role.timeout"))),_=context.session.canceled()=>Err(crate::role_runtime::failure("role.canceled"))};
    if outcome.is_err() {
        let _ = owned.terminate();
        let _ = child.kill().await;
        let _ = tokio::time::timeout(Duration::from_secs(2), child.wait()).await;
    }
    match &outcome {
        Ok(output) if output.status.success() => launch.finish("completed", None),
        Ok(_) => launch.finish("failed", Some("tool_failed")),
        Err(_) => launch.finish("completion_unknown", Some("interrupted")),
    }
    outcome
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Admission {
    protocol: String,
    version: u32,
    #[serde(rename = "type")]
    kind: String,
    request_id: String,
    boundary: Boundary,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Settlement {
    protocol: String,
    version: u32,
    #[serde(rename = "type")]
    kind: String,
    permit_id: String,
    state: String,
    cause: Option<String>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Denial {
    protocol: String,
    version: u32,
    #[serde(rename = "type")]
    kind: String,
    boundary: Boundary,
    code: String,
}

// Only this pipe's launch and its live descendants can own forwarded work.
// Each descendant was checked when admitted; a sibling pipe's permit is absent.
fn validate_lineage(
    boundary: &Boundary,
    bootstrap: &Bootstrap,
    launch: &Permit,
    permits: &BTreeMap<String, Permit>,
) -> Result<(), String> {
    if boundary.invocation_id != bootstrap.invocation_id
        || boundary.binding_revision != bootstrap.binding_revision
        || boundary.call_site.is_empty()
        || boundary.call_site.len() > 256
        || boundary.agent.len() > 1024
        || !matches!(boundary.kind.as_str(), "provider" | "native_child" | "tool")
    {
        return Err(invalid());
    }
    let parent = boundary.parent_permit_id.as_deref().ok_or_else(invalid)?;
    let agent = if parent == launch.id() {
        bootstrap.definition.as_str()
    } else {
        let parent = permits.get(parent).ok_or_else(invalid)?.boundary();
        match parent.kind.as_str() {
            "native_child" => parent.target_agent.as_deref().ok_or_else(invalid)?,
            "tool" => &parent.agent,
            _ => return Err(invalid()),
        }
    };
    if boundary.agent != agent {
        return Err(invalid());
    }
    let call = bootstrap.resolution.calls.iter().find(|call| {
        call.call_site.locator.definition == boundary.agent
            && call.call_site.id == boundary.call_site
    });
    if boundary.target_agent.as_deref() != call.and_then(|call| call.call_site.target.as_deref())
        || (boundary.kind == "tool" && boundary.target_agent.is_some())
        || (boundary.kind == "native_child" && boundary.target_agent.is_none())
        || (boundary.kind == "provider" && boundary.target_agent.is_some())
    {
        return Err(invalid());
    }
    Ok(())
}

/// Launch only after caller verifies executable provenance and its declared target.
pub async fn native(
    mut command: tokio::process::Command,
    context: Context,
    mut bootstrap: Bootstrap,
    remaining: Duration,
) -> Result<ChildResult, String> {
    let launch = context
        .session
        .admit(context.boundary("native_child")?)
        .await?;
    bootstrap.parent_permit_id = Some(launch.id().to_owned());
    prepare(&mut command).map_err(|_| invalid())?;
    command.arg("--native-role-child");
    let mut child = command
        .spawn()
        .map_err(|_| crate::role_runtime::failure("role.child_launch_failed"))?;
    let owned = Owned::attach(&child).map_err(|_| invalid())?;
    let mut stdin = child.stdin.take().ok_or_else(invalid)?;
    let mut stdout = BufReader::new(child.stdout.take().ok_or_else(invalid)?);
    let stderr = child.stderr.take().ok_or_else(invalid)?;
    let mut permits = BTreeMap::<String, Permit>::new();
    let work = async {
        let conversation = async {
            write_frame(&mut stdin,&json!({"protocol":"cargo_ai_native_role","version":1,"type":"bootstrap","bootstrap":bootstrap})).await?;
            let mut result = None;
            let mut total = 0;
            let mut count = 0;
            let mut requests = std::collections::BTreeSet::new();
            while let Some(bytes) = read_line(&mut stdout).await.map_err(|_| invalid())? {
                total += bytes.len();
                count += 1;
                if total > MAX_CHANNEL_BYTES || count > MAX_FRAMES {
                    return Err(invalid());
                }
                let Ok(frame) = crate::business_schema::strict_json_bounded(
                    &bytes,
                    crate::role_session::MAX_FRAME_BYTES,
                    32,
                ) else {
                    continue;
                };
                if frame["protocol"] != "cargo_ai_native_role" {
                    continue;
                }
                if frame["version"] != 1 || result.is_some() {
                    return Err(invalid());
                }
                match frame["type"].as_str() {
                    Some("admit") => {
                        let request: Admission =
                            serde_json::from_value(frame).map_err(|_| invalid())?;
                        if request.protocol != "cargo_ai_native_role"
                            || request.version != 1
                            || request.kind != "admit"
                            || !crate::role_contract::identifier(&request.request_id)
                            || !requests.insert(request.request_id.clone())
                            || permits.len() >= 256
                        {
                            return Err(invalid());
                        }
                        validate_lineage(&request.boundary, &bootstrap, &launch, &permits)?;
                        let permission = if request.boundary.kind == "tool" {
                            let name = request
                                .boundary
                                .call_site
                                .strip_prefix("tools.")
                                .filter(|name| crate::role_contract::identifier(name))
                                .ok_or_else(invalid)?;
                            let source = bootstrap.package_root.join(&request.boundary.agent);
                            if !bootstrap
                                .content_identities
                                .contains_key(source.to_string_lossy().as_ref())
                            {
                                return Err(invalid());
                            }
                            let bytes = read_definition(&source)?;
                            let definition = crate::business_schema::strict_json_bounded(
                                &bytes,
                                1024 * 1024,
                                64,
                            )
                            .map_err(|_| invalid())?;
                            if !definition["actions"]
                                .as_array()
                                .into_iter()
                                .flatten()
                                .flat_map(|action| action["run"].as_array().into_iter().flatten())
                                .any(|step| step["kind"] == "tool" && step["name"] == name)
                            {
                                return Err(invalid());
                            }
                            context.validate_context().map(|_| request.boundary.clone())
                        } else {
                            let call = bootstrap
                                .resolution
                                .calls
                                .iter()
                                .find(|c| {
                                    c.call_site.locator.definition == request.boundary.agent
                                        && c.call_site.id == request.boundary.call_site
                                })
                                .ok_or_else(invalid)?;
                            let mut at = context.clone();
                            at.locator = call.call_site.locator.clone();
                            at.parent_permit_id = request.boundary.parent_permit_id.clone();
                            at.scope(async {
                                at.selected().and_then(|selected| {
                                    if request.boundary.kind == "provider"
                                        && selected.selection.is_none()
                                    {
                                        return Err(crate::role_runtime::failure(
                                            "role.unresolved",
                                        ));
                                    }
                                    Ok(request.boundary.clone())
                                })
                            })
                            .await
                        };
                        let admission = match permission {
                            Ok(boundary) => context.session.admit(boundary).await,
                            Err(error) => Err(error),
                        };
                        let reply = match admission {
                            Ok(permit) => {
                                let id = permit.id().to_owned();
                                permits.insert(id.clone(), permit);
                                json!({"protocol":"cargo_ai_native_role","version":1,"type":"admission","request_id":request.request_id,"allowed":true,"permit_id":id})
                            }
                            Err(_) => {
                                json!({"protocol":"cargo_ai_native_role","version":1,"type":"admission","request_id":request.request_id,"allowed":false})
                            }
                        };
                        write_frame(&mut stdin, &reply).await?;
                    }
                    Some("settled") => {
                        let value: Settlement =
                            serde_json::from_value(frame).map_err(|_| invalid())?;
                        if value.protocol != "cargo_ai_native_role"
                            || value.version != 1
                            || value.kind != "settled"
                            || ![
                                "completed",
                                "failed",
                                "not_dispatched",
                                "completion_unknown",
                            ]
                            .contains(&value.state.as_str())
                            || value
                                .cause
                                .as_ref()
                                .is_some_and(|s| !crate::role_contract::identifier(s))
                        {
                            return Err(invalid());
                        }
                        if permits.values().any(|permit| {
                            permit.boundary().parent_permit_id.as_deref() == Some(&value.permit_id)
                        }) {
                            return Err(invalid());
                        }
                        permits
                            .remove(&value.permit_id)
                            .ok_or_else(invalid)?
                            .finish(&value.state, value.cause.as_deref());
                    }
                    Some("denied") => {
                        let denial: Denial =
                            serde_json::from_value(frame).map_err(|_| invalid())?;
                        if denial.protocol != "cargo_ai_native_role"
                            || denial.version != 1
                            || denial.kind != "denied"
                            || denial.boundary.invocation_id != context.session.invocation_id
                            || denial.boundary.binding_revision != context.session.binding_revision
                            || !crate::role_contract::identifier(&denial.code)
                            || !bootstrap.content_identities.contains_key(
                                bootstrap
                                    .package_root
                                    .join(&denial.boundary.agent)
                                    .to_string_lossy()
                                    .as_ref(),
                            )
                        {
                            return Err(invalid());
                        }
                        validate_lineage(&denial.boundary, &bootstrap, &launch, &permits)?;
                        context.session.record_denial(denial.boundary, &denial.code);
                        crate::role_runtime::note_error(&denial.code);
                    }
                    Some("result") => {
                        if frame.as_object().is_none_or(|o| {
                            o.keys().any(|k| {
                                ![
                                    "protocol",
                                    "version",
                                    "type",
                                    "result",
                                    "error",
                                    "invocation_id",
                                    "binding_revision",
                                    "native_outcomes",
                                ]
                                .contains(&k.as_str())
                            })
                        }) {
                            return Err(invalid());
                        }
                        if frame["invocation_id"] != context.session.invocation_id
                            || frame["binding_revision"] != context.session.binding_revision
                        {
                            return Err(invalid());
                        }
                        if let Some(code) = frame.pointer("/error/code").and_then(Value::as_str) {
                            if !crate::role_contract::identifier(code) {
                                return Err(invalid());
                            }
                            crate::role_runtime::note_error(code);
                        }
                        result = Some(ChildResult {
                            result: frame.get("result").cloned().unwrap_or(Value::Null),
                            error: frame.get("error").filter(|v| !v.is_null()).cloned(),
                        });
                    }
                    _ => return Err(invalid()),
                }
            }
            result.ok_or_else(|| crate::role_runtime::failure("role.child_completion_unknown"))
        };
        let (result, (), status) = tokio::try_join!(conversation, drain(stderr), async {
            child.wait().await.map_err(|_| invalid())
        })?;
        if !permits.is_empty() {
            return Err(crate::role_runtime::failure(
                "role.child_completion_unknown",
            ));
        }
        if !status.success() && result.error.is_none() {
            return Err(crate::role_runtime::failure("role.child_failed"));
        }
        Ok(result)
    };
    let outcome = tokio::select! {result=tokio::time::timeout(remaining,work)=>result.unwrap_or_else(|_|Err(crate::role_runtime::failure("role.timeout"))),_=context.session.canceled()=>Err(crate::role_runtime::failure("role.canceled"))};
    if outcome.is_err() {
        let _ = owned.terminate();
        let _ = child.kill().await;
        let _ = tokio::time::timeout(Duration::from_secs(2), child.wait()).await;
    }
    drop(permits);
    match &outcome {
        Ok(value) if value.error.is_none() => launch.finish("completed", None),
        Ok(_) => launch.finish("failed", Some("child_failed")),
        Err(_) => launch.finish("completion_unknown", Some("interrupted")),
    }
    outcome
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(unix)]
    struct ProcessFixture {
        context: Context,
        root: std::path::PathBuf,
    }
    #[cfg(unix)]
    impl ProcessFixture {
        fn new() -> Self {
            use sha2::{Digest, Sha256};
            let root =
                std::env::temp_dir().join(format!("cargo-ai-role-pipe-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir(&root).unwrap();
            let root = root.canonicalize().unwrap();
            let source = root.join("agent.json");
            std::fs::write(&source, b"{}").unwrap();
            let session = crate::role_session::Session::new("revision-a".into());
            let bindings = json!({"version":1,"revision":"revision-a","bindings":[]});
            let binding_identity = crate::role_contract::canonical_identity(&bindings).unwrap();
            let fixed_identity =
                crate::providers::operation_metadata::fixed_context_identity(&root, &[]).unwrap();
            let connection_identity =
                crate::providers::operation_metadata::connection_context_identity(
                    &serde_json::from_value(bindings.clone()).unwrap(),
                    &fixed_identity,
                )
                .unwrap();
            let limits = json!({"max_runtime_secs":60,"max_output_tokens":512,"max_agent_depth":4});
            let bootstrap: Bootstrap = serde_json::from_value(json!({
                "version":1,"bindings":bindings,"definition":"agent.json","package_root":root,
                "content_identities":{source.to_string_lossy().as_ref():format!("{:x}",Sha256::digest(b"{}"))},
                "invocation_id":session.invocation_id,"binding_revision":session.binding_revision,
                "policy":{"version":1,"allowed":[],"limits":limits},
                "resolution":{"version":1,"identity":"test-only","binding_revision":"revision-a","binding_identity":binding_identity,
                    "context":{"project_identity":"test","environment_identity":"test","package_identity":"test","runtime_contract":"test","connection_context_identity":connection_identity,"consent_identity":"test"},
                    "scope":{"key":{"action":"run","interface":"native","mode":"default"},"call_sites":["child"],"resources":[],"data_scopes":[],"limits":limits},
                    "calls":[{"call_site":{"id":"child","locator":{"definition":"agent.json","site":"actions.0.run.0"},"kind":"child","target":"child.json","requirements":{"operation":"text_generation"}},"selection":null,"settings":{},"compatibility":"compatible","reason":"structural_native_child","evidence":[]}],
                    "required_selections":[],"ready":true,"execution_authorized":false,"invocation_access":"unverified"}
            })).unwrap();
            let context = Context::new(bootstrap, session)
                .unwrap()
                .at("actions.0.run.0".into());
            Self { context, root }
        }
        fn command(&self, body: &str) -> tokio::process::Command {
            let script = self.root.join("fixture.sh");
            std::fs::write(&script, body).unwrap();
            let mut command = tokio::process::Command::new("/bin/sh");
            command.arg(script).current_dir(&self.root);
            command
        }
        fn completion(&self) -> String {
            json!({"protocol":"cargo_ai_native_role","version":1,"type":"result","invocation_id":self.context.session.invocation_id,"binding_revision":"revision-a","result":{"ok":true},"error":null}).to_string()
        }
    }
    #[cfg(unix)]
    impl Drop for ProcessFixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn native_role_revocation_prevents_spawn_and_timeout_settles_unknown() {
        let fixture = ProcessFixture::new();
        fixture.context.session.revoke("revision-a").unwrap();
        let command = fixture.command("touch launched\n");
        assert!(native(
            command,
            fixture.context.clone(),
            (*fixture.context.bootstrap).clone(),
            Duration::from_secs(1)
        )
        .await
        .is_err());
        assert!(!fixture.root.join("launched").exists());
        let fixture = ProcessFixture::new();
        let command = fixture.command("read bootstrap\nsleep 30 &\nwait\n");
        assert!(native(
            command,
            fixture.context.clone(),
            (*fixture.context.bootstrap).clone(),
            Duration::from_millis(50)
        )
        .await
        .is_err());
        let snapshot = fixture.context.session.snapshot();
        assert!(snapshot["outstanding"].as_object().unwrap().is_empty());
        assert_eq!(snapshot["outcomes"][0]["state"], "completion_unknown");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn native_role_output_overflow_and_cancel_leave_no_owned_descendants() {
        unsafe extern "C" {
            fn kill(pid: i32, signal: i32) -> i32;
        }
        for case in ["overflow", "partial_frame", "blocked_bootstrap"] {
            let cancel = case != "overflow";
            let fixture = ProcessFixture::new();
            let body = match case {
                "partial_frame" => "read bootstrap\nsleep 30 &\nprintf '%s' $! > children\nprintf '{'\nwait\n",
                "blocked_bootstrap" => "sleep 30 &\nprintf '%s' $! > children\nwait\n",
                _ => "read bootstrap\nsleep 30 &\nworker=$!\ncat /dev/zero &\nprintf '%s %s' $worker $! > children\nwait\n",
            };
            let command = fixture.command(body);
            let context = fixture.context.clone();
            let mut bootstrap = (*context.bootstrap).clone();
            if case == "blocked_bootstrap" {
                bootstrap.resolution.context.environment_identity = "x".repeat(512 * 1024);
            }
            let running = tokio::spawn(async move {
                native(command, context, bootstrap, Duration::from_secs(5)).await
            });
            let children = tokio::time::timeout(Duration::from_secs(2), async {
                loop {
                    if let Ok(contents) = std::fs::read_to_string(fixture.root.join("children")) {
                        let ids: Vec<i32> = contents
                            .split_whitespace()
                            .map(|v| v.parse().unwrap())
                            .collect();
                        if ids.len() == if cancel { 1 } else { 2 } {
                            break ids;
                        }
                    }
                    tokio::time::sleep(Duration::from_millis(5)).await;
                }
            })
            .await
            .unwrap();
            if cancel {
                fixture.context.session.cancel();
            }
            let result = tokio::time::timeout(Duration::from_secs(3), running)
                .await
                .unwrap()
                .unwrap();
            assert!(
                result.is_err(),
                "Interrupted output cannot imply completion"
            );
            tokio::time::timeout(Duration::from_secs(2), async {
                loop {
                    if children.iter().all(|pid| {
                        assert!(*pid > 1);
                        let status = unsafe { kill(*pid, 0) };
                        status == -1 && std::io::Error::last_os_error().raw_os_error() == Some(3)
                    }) {
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            })
            .await
            .expect("Owned descendants remained alive after driver cleanup");
            let snapshot = fixture.context.session.snapshot();
            assert!(snapshot["outstanding"].as_object().unwrap().is_empty());
            assert!(snapshot["outcomes"]
                .as_array()
                .unwrap()
                .iter()
                .all(|o| o["state"] != "completed"));
            assert_eq!(snapshot["outcomes"][0]["state"], "completion_unknown");
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn native_role_pipe_relays_admission_and_requires_settlement() {
        let fixture = ProcessFixture::new();
        let admit = json!({"protocol":"cargo_ai_native_role","version":1,"type":"admit","request_id":"one","boundary":fixture.context.boundary("native_child").unwrap()}).to_string().replace("\"parent_permit_id\":null", "\"parent_permit_id\":\"%s\"");
        let script = format!(
            r#"read bootstrap
parent=${{bootstrap##*\"parent_permit_id\":\"}}
parent=${{parent%%\"*}}
printf '{}\n' "$parent"
read reply
permit=${{reply#*\"permit_id\":\"}}
permit=${{permit%%\"*}}
printf '{{"protocol":"cargo_ai_native_role","version":1,"type":"settled","permit_id":"%s","state":"completed","cause":null}}\n' "$permit"
printf '%s\n' '{}'
"#,
            admit,
            fixture.completion()
        );
        let result = native(
            fixture.command(&script),
            fixture.context.clone(),
            (*fixture.context.bootstrap).clone(),
            Duration::from_secs(3),
        )
        .await
        .unwrap();
        assert_eq!(result.result, json!({"ok":true}));
        assert!(result.error.is_none());
        let snapshot = fixture.context.session.snapshot();
        assert_eq!(snapshot["outcomes"].as_array().unwrap().len(), 2);
        assert!(snapshot["outcomes"]
            .as_array()
            .unwrap()
            .iter()
            .all(|o| o["state"] == "completed"));
        let script = format!(
            r#"read bootstrap
parent=${{bootstrap##*\"parent_permit_id\":\"}}
parent=${{parent%%\"*}}
printf '{}\n' "$parent"
read reply
printf '%s\n' '{}'
"#,
            admit,
            fixture.completion()
        );
        assert!(native(
            fixture.command(&script),
            fixture.context.clone(),
            (*fixture.context.bootstrap).clone(),
            Duration::from_secs(3)
        )
        .await
        .is_err());
        assert!(fixture.context.session.snapshot()["outstanding"]
            .as_object()
            .unwrap()
            .is_empty());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn native_role_lineage_rejects_other_launches_and_preserves_denials() {
        let fixture = ProcessFixture::new();
        let session = &fixture.context.session;
        let launch = session
            .admit(fixture.context.boundary("native_child").unwrap())
            .await
            .unwrap();
        let sibling = session
            .admit(fixture.context.boundary("native_child").unwrap())
            .await
            .unwrap();
        let bootstrap = &fixture.context.bootstrap;
        let mut forwarded = fixture.context.boundary("native_child").unwrap();
        forwarded.parent_permit_id = Some(launch.id().into());
        assert!(validate_lineage(&forwarded, bootstrap, &launch, &BTreeMap::new()).is_ok());
        session.record_denial(forwarded.clone(), "role.revoked");
        for parent in [None, Some("forged".into()), Some(sibling.id().into())] {
            forwarded.parent_permit_id = parent;
            assert!(validate_lineage(&forwarded, bootstrap, &launch, &BTreeMap::new()).is_err());
        }
        forwarded.parent_permit_id = Some(launch.id().into());
        forwarded.target_agent = Some("another.json".into());
        assert!(validate_lineage(&forwarded, bootstrap, &launch, &BTreeMap::new()).is_err());
        let snapshot = session.snapshot();
        assert!(snapshot["outcomes"]
            .as_array()
            .unwrap()
            .iter()
            .any(|o| o["cause"] == "role.revoked"
                && o["boundary"]["parent_permit_id"] == launch.id()));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn native_role_tool_cannot_hide_denied_child_with_success_result() {
        let fixture = ProcessFixture::new();
        let command = fixture.command(r#"read invocation
printf '%s\n' '{"protocol_version":2,"type":"child_request","request_id":"one","call_site":"undeclared","inputs":{}}'
read response
printf '%s\n' '{"protocol_version":2,"type":"result","result":"apparently successful"}'
"#);
        let options = ChildOptions {
            current_depth: 0,
            max_depth: 4,
            max_runtime_secs: 60,
            started_at_ms: 0,
            deadline_ms: 0,
        };
        let result = fixture
            .context
            .scope(tool(
                command,
                fixture.context.clone(),
                "fixture",
                json!({}),
                options,
                Duration::from_secs(3),
            ))
            .await
            .unwrap();
        assert!(result.status.success());
        assert!(fixture.context.session.snapshot()["outcomes"]
            .as_array()
            .unwrap()
            .iter()
            .any(|o| o["state"] == "not_dispatched" && o["cause"] == "role.undeclared_call_site"));
    }

    #[tokio::test]
    async fn native_role_pipe_reader_rejects_unbounded_and_truncated_frames() {
        let bytes = vec![b'x'; crate::role_session::MAX_FRAME_BYTES + 1];
        assert!(read_line(&mut BufReader::new(bytes.as_slice()))
            .await
            .is_err());
        assert!(
            read_line(&mut BufReader::new(b"{\"type\":\"admit\"}".as_slice()))
                .await
                .is_err()
        );
        let mut reader = BufReader::new(b"{}\n{}\n".as_slice());
        assert_eq!(
            read_line(&mut reader).await.unwrap(),
            Some(b"{}\n".to_vec())
        );
        assert_eq!(
            read_line(&mut reader).await.unwrap(),
            Some(b"{}\n".to_vec())
        );
        assert!(read_line(&mut reader).await.unwrap().is_none());
    }
    #[test]
    fn native_role_tool_frames_cannot_smuggle_selector_or_binding_fields() {
        let request = json!({"protocol_version":2,"type":"child_request","request_id":"one","call_site":"child","inputs":{}});
        assert!(serde_json::from_value::<ToolRequest>(request.clone()).is_ok());
        for key in [
            "profile",
            "model",
            "execution_policy",
            "binding_revision",
            "runtime_context",
            "target",
        ] {
            let mut value = request.clone();
            value[key] = json!("unexpected");
            assert!(serde_json::from_value::<ToolRequest>(value).is_err());
        }
    }
}
