//! Runtime behavior for `cargo ai run`.
use clap::ArgMatches;
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Path, PathBuf};

use super::definition_source::{
    load_definition_contents, read_definition_json_from_stdin, resolve_definition_source,
    AgentDefinitionSource,
};

#[cfg(test)]
fn resolve_run_definition_source_in_dir(
    name_or_path: Option<&str>,
    config_path: Option<&str>,
    inline_json: Option<&str>,
    stdin_json: Option<&str>,
    current_dir: &Path,
) -> Result<AgentDefinitionSource, String> {
    if let Some(inline_json) = inline_json {
        return Ok(AgentDefinitionSource::InlineJson(inline_json.to_string()));
    }

    if let Some(stdin_json) = stdin_json {
        return Ok(AgentDefinitionSource::StdinJson(stdin_json.to_string()));
    }

    if let Some(config_path) = config_path {
        return Ok(AgentDefinitionSource::LocalPath(config_path.to_string()));
    }

    let Some(name_or_path) = name_or_path else {
        return Err(
            "Missing agent reference. Use `cargo ai run <name-or-path>`, `cargo ai run --config <path-to-json>`, `cargo ai run --json <json>`, or `cargo ai run --stdin`."
                .to_string(),
        );
    };

    super::definition_source::resolve_definition_source_in_dir(name_or_path, current_dir, "run")
}

fn resolve_run_definition_source(sub_m: &ArgMatches) -> Result<AgentDefinitionSource, String> {
    if let Some(inline_json) = sub_m.get_one::<String>("json").map(String::as_str) {
        return Ok(AgentDefinitionSource::InlineJson(inline_json.to_string()));
    }

    if sub_m.get_flag("stdin") {
        return Ok(AgentDefinitionSource::StdinJson(
            read_definition_json_from_stdin()?,
        ));
    }

    if let Some(config_path) = sub_m.get_one::<String>("config").map(String::as_str) {
        return Ok(AgentDefinitionSource::LocalPath(config_path.to_string()));
    }

    let Some(name_or_path) = sub_m.get_one::<String>("name").map(String::as_str) else {
        return Err(
            "Missing agent reference. Use `cargo ai run <name-or-path>`, `cargo ai run --config <path-to-json>`, `cargo ai run --json <json>`, or `cargo ai run --stdin`."
                .to_string(),
        );
    };

    resolve_definition_source(name_or_path, "run", "run")
}

fn is_account_run_invocation(sub_m: &ArgMatches) -> bool {
    sub_m.get_flag("from_account")
        || sub_m.get_one::<String>("owner_handle").is_some()
        || sub_m.get_one::<String>("definition_path").is_some()
}

async fn load_run_definition_from_source(
    source: &AgentDefinitionSource,
) -> Result<(crate::runtime_definition::RuntimeAgentDefinition, String), String> {
    let contents = match source {
        AgentDefinitionSource::LocalPath(path) => fs::read_to_string(path)
            .map_err(|error| format!("failed to read '{}': {error}", Path::new(path).display()))?,
        AgentDefinitionSource::RegistryName(_)
        | AgentDefinitionSource::InlineJson(_)
        | AgentDefinitionSource::StdinJson(_) => load_definition_contents(source).await?,
    };
    let definition =
        crate::runtime_definition::RuntimeAgentDefinition::from_str(contents.as_str())?;
    Ok((definition, contents))
}

fn project_root_for_definition_source(
    source: &AgentDefinitionSource,
) -> Result<Option<PathBuf>, String> {
    match source {
        AgentDefinitionSource::LocalPath(path) => {
            crate::commands::package_dependencies::find_project_root(Path::new(path))
        }
        AgentDefinitionSource::RegistryName(_)
        | AgentDefinitionSource::InlineJson(_)
        | AgentDefinitionSource::StdinJson(_) => {
            let current_dir = std::env::current_dir()
                .map_err(|error| format!("Failed to inspect current project directory: {error}"))?;
            crate::commands::package_dependencies::find_project_root(current_dir.as_path())
        }
    }
}

fn runtime_context_path_for_definition_source(
    source: &AgentDefinitionSource,
) -> Result<PathBuf, String> {
    match source {
        AgentDefinitionSource::LocalPath(path) => Ok(PathBuf::from(path)),
        AgentDefinitionSource::RegistryName(_)
        | AgentDefinitionSource::InlineJson(_)
        | AgentDefinitionSource::StdinJson(_) => std::env::current_dir()
            .map_err(|error| format!("Failed to inspect current runtime directory: {error}")),
    }
}

fn caller_project_root_for_installed_context(
    package_context: Option<&crate::commands::local_packages::InstalledPackageRuntimeContext>,
    current_dir: &Path,
) -> Result<Option<PathBuf>, String> {
    if package_context.is_none() {
        return Ok(None);
    }
    crate::commands::package_dependencies::find_project_root(current_dir)
}

fn usage_agent_info_for_definition_source(
    source: &AgentDefinitionSource,
    definition_json: &str,
    project_root: Option<&Path>,
) -> Value {
    let mut value = json!({
        "generated": false,
    });

    match source {
        AgentDefinitionSource::LocalPath(path) => {
            value["source"] = json!("local_path");
            value["artifact"] = json!(path);
            if let Some(name) = derived_agent_name_from_path(path) {
                value["name"] = json!(name);
            }
        }
        AgentDefinitionSource::RegistryName(name) => {
            value["source"] = json!("registry");
            value["name"] = json!(name);
        }
        AgentDefinitionSource::InlineJson(_) => {
            value["source"] = json!("inline_json");
        }
        AgentDefinitionSource::StdinJson(_) => {
            value["source"] = json!("stdin_json");
        }
    }

    if let Ok(definition_sha256) = definition_sha256_from_json_str(definition_json) {
        value["definition_sha256"] = json!(definition_sha256);
    }

    if let Some(project_root) = project_root {
        value["project_root"] = json!(project_root.display().to_string());
    }

    value
}

fn usage_agent_info_for_package_entrypoint(
    resolved: &crate::commands::local_packages::ResolvedPackageEntrypoint,
    definition_json: &str,
) -> Value {
    let mut value = json!({
        "source": "installed_package",
        "generated": false,
        "package_alias": resolved.alias.as_str(),
        "entrypoint": resolved.entrypoint.as_str(),
        "artifact": resolved.definition_path.display().to_string(),
        "name": resolved.entrypoint.as_str(),
        "project_root": resolved.package_root.display().to_string(),
        "package": {
            "name": resolved.package_name.as_str(),
            "version": resolved.package_version.as_str(),
            "content_sha256": resolved.content_sha256.as_str(),
            "source_kind": resolved.source_kind.as_str(),
            "data_root": resolved.package_data_root.display().to_string(),
            "permissions": {
                "package_payload": resolved.permissions.package_payload.as_str(),
                "package_data": resolved.permissions.package_data.as_str(),
                "project_workspace": resolved.permissions.project_workspace.as_str(),
                "subprocess": resolved.permissions.subprocess.as_str(),
            },
        }
    });

    if let Ok(definition_sha256) = definition_sha256_from_json_str(definition_json) {
        value["definition_sha256"] = json!(definition_sha256);
    }

    value
}

fn derived_agent_name_from_path(path: &str) -> Option<String> {
    Path::new(path)
        .file_stem()
        .map(|name| name.to_string_lossy().trim().to_string())
        .filter(|name| !name.is_empty())
}

fn definition_sha256_from_json_str(json_str: &str) -> Result<String, String> {
    let root = serde_json::from_str::<Value>(json_str)
        .map_err(|error| format!("failed to parse agent JSON for usage metadata: {error}"))?;
    let canonical = canonicalize_json_value(&root);
    let serialized = serde_json::to_string(&canonical).map_err(|error| {
        format!("failed to serialize canonical agent JSON for usage metadata: {error}")
    })?;
    Ok(sha256_hex(serialized.as_str()))
}

fn authored_project_identity(project_root: Option<&Path>) -> (Option<String>, Option<String>) {
    let Some(root) = project_root else {
        return (None, None);
    };
    let Ok(contents) = fs::read_to_string(root.join(".cargo-ai/project.toml")) else {
        return (None, None);
    };
    let Ok(document) = toml::from_str::<toml::Value>(&contents) else {
        return (None, None);
    };
    let project = document.get("project");
    (
        project
            .and_then(|project| project.get("id"))
            .and_then(toml::Value::as_str)
            .map(str::to_string),
        project
            .and_then(|project| project.get("version"))
            .and_then(toml::Value::as_str)
            .map(str::to_string),
    )
}

fn relative_agent_key(path: &Path, project_root: Option<&Path>) -> Option<String> {
    let root = project_root?;
    let canonical_root = fs::canonicalize(root).ok()?;
    let canonical_path = fs::canonicalize(path).ok()?;
    let relative = canonical_path.strip_prefix(canonical_root).ok()?;
    Some(relative.to_string_lossy().replace('\\', "/"))
}

fn attribution_path(path: &Path, caller_dir: Option<&Path>) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else if let Some(caller_dir) = caller_dir {
        caller_dir.join(path)
    } else {
        path.to_path_buf()
    }
}

fn attribution_for_definition_source(
    source: &AgentDefinitionSource,
    definition_json: &str,
    project_root: Option<&Path>,
    package_context: Option<&crate::commands::local_packages::InstalledPackageRuntimeContext>,
) -> crate::usage_attribution::AttributionInput {
    let caller_dir = std::env::current_dir().ok();
    attribution_for_definition_source_in_dir(
        source,
        definition_json,
        project_root,
        package_context,
        caller_dir.as_deref(),
    )
}

fn attribution_for_definition_source_in_dir(
    source: &AgentDefinitionSource,
    definition_json: &str,
    project_root: Option<&Path>,
    package_context: Option<&crate::commands::local_packages::InstalledPackageRuntimeContext>,
    caller_dir: Option<&Path>,
) -> crate::usage_attribution::AttributionInput {
    let local_definition = matches!(source, AgentDefinitionSource::LocalPath(_));
    let discovered_root = project_root
        .filter(|_| local_definition)
        .map(|root| attribution_path(root, caller_dir));
    let (authored_package_id, package_version) =
        authored_project_identity(discovered_root.as_deref());
    let package_context = package_context.filter(|_| local_definition);
    let package_root = discovered_root.or_else(|| match source {
        AgentDefinitionSource::LocalPath(path) => Some(attribution_path(
            Path::new(path)
                .parent()
                .filter(|parent| !parent.as_os_str().is_empty())
                .unwrap_or_else(|| Path::new(".")),
            caller_dir,
        )),
        _ => None,
    });
    let agent_key = match source {
        AgentDefinitionSource::LocalPath(path) => {
            let source_path = attribution_path(Path::new(path), caller_dir);
            relative_agent_key(source_path.as_path(), package_root.as_deref())
        }
        AgentDefinitionSource::RegistryName(name) => Some(format!("registry/{name}")),
        _ => None,
    };
    crate::usage_attribution::AttributionInput {
        authored_package_id: package_context
            .and_then(|context| context.project_id.clone())
            .or(authored_package_id),
        hosted_source_id: package_context.and_then(|context| context.hosted_source_id.clone()),
        package_version: package_context
            .map(|context| context.package_version.clone())
            .or(package_version),
        package_content_digest: package_context.map(|context| context.content_sha256.clone()),
        hosted_version_id: package_context.and_then(|context| context.hosted_version_id.clone()),
        package_root,
        agent_key,
        definition_hash: definition_sha256_from_json_str(definition_json).ok(),
        workspace: Some(crate::usage_attribution::capture_workspace()),
        ..Default::default()
    }
}

fn canonicalize_json_value(value: &Value) -> Value {
    match value {
        Value::Array(items) => Value::Array(items.iter().map(canonicalize_json_value).collect()),
        Value::Object(map) => {
            let mut keys = map.keys().cloned().collect::<Vec<_>>();
            keys.sort();

            let mut canonical = Map::new();
            for key in keys {
                if let Some(entry) = map.get(&key) {
                    canonical.insert(key, canonicalize_json_value(entry));
                }
            }

            Value::Object(canonical)
        }
        _ => value.clone(),
    }
}

fn sha256_hex(contents: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(contents.as_bytes());
    format!("{:x}", hasher.finalize())
}

/// Executes the interpreted runtime flow from a local or registry JSON definition.
pub(crate) async fn machine_run(
    sub_m: &ArgMatches,
) -> Result<serde_json::Value, super::machine::Failure> {
    if sub_m.get_one::<String>("token").is_some() {
        return Err(super::machine::Failure::new("cli.invalid_input", "Application runtime authentication uses the configured profile; argv tokens are not accepted."));
    }
    let succeeded = run(sub_m).await;
    super::machine::runtime_outcome(succeeded)
}

pub async fn run(sub_m: &ArgMatches) -> bool {
    match crate::execution_policy::scope_inherited(Box::pin(run_scoped(sub_m))).await {
        Ok(succeeded) => succeeded,
        Err(_) => {
            super::machine::record_error(super::machine::Failure::new(
                "action.execution_policy_invalid",
                "The inherited execution policy is invalid or unavailable.",
            ));
            eprintln!("action.execution_policy_invalid: The inherited execution policy is invalid or unavailable.");
            false
        }
    }
}

async fn run_scoped(sub_m: &ArgMatches) -> bool {
    if sub_m.get_one::<String>("action").is_some() {
        return run_client_action(sub_m).await;
    }
    if is_account_run_invocation(sub_m) {
        return crate::commands::account::run_account_agent(sub_m).await;
    }

    if sub_m.get_one::<String>("config").is_none()
        && sub_m.get_one::<String>("json").is_none()
        && !sub_m.get_flag("stdin")
    {
        if let Some(name_or_path) = sub_m.get_one::<String>("name").map(String::as_str) {
            match crate::commands::local_packages::resolve_entrypoint_reference(name_or_path, false)
            {
                Ok(Some(resolved)) => {
                    let source = AgentDefinitionSource::LocalPath(
                        resolved.definition_path.display().to_string(),
                    );
                    let (definition, definition_json) =
                        match load_run_definition_from_source(&source).await {
                            Ok(loaded) => loaded,
                            Err(error) => {
                                super::machine::record_error(super::machine::Failure::new(
                                "runtime.invalid_definition",
                                "The selected agent definition could not be loaded or validated.",
                            ));
                                eprintln!("x {error}");
                                return false;
                            }
                        };
                    let usage_agent_info = usage_agent_info_for_package_entrypoint(
                        &resolved,
                        definition_json.as_str(),
                    );
                    let attribution = crate::usage_attribution::AttributionInput {
                        authored_package_id: resolved.project_id.clone(),
                        hosted_source_id: resolved.hosted_source_id.clone(),
                        package_version: Some(resolved.package_version.clone()),
                        package_content_digest: Some(resolved.content_sha256.clone()),
                        hosted_version_id: resolved.hosted_version_id.clone(),
                        package_root: Some(resolved.package_root.clone()),
                        agent_key: relative_agent_key(
                            resolved.definition_path.as_path(),
                            Some(resolved.package_root.as_path()),
                        ),
                        definition_hash: definition_sha256_from_json_str(&definition_json).ok(),
                        workspace: Some(crate::usage_attribution::capture_workspace()),
                        ..Default::default()
                    };
                    let package_context =
                        match crate::commands::local_packages::runtime_context_for_resolved_entrypoint(
                            &resolved,
                        ) {
                            Ok(context) => context,
                            Err(error) => {
                                eprintln!("x {error}");
                                return false;
                            }
                        };
                    let _package_lease = resolved.lease.clone();
                    let declaring_project_root = Some(resolved.package_root.clone());
                    return super::runtime_actions::scope_declaring_project_root(
                        declaring_project_root,
                        super::runtime::run_with_definition_in_context_and_usage_agent(
                            sub_m,
                            &definition,
                            Some(resolved.package_root.clone()),
                            Some(usage_agent_info),
                            Some(package_context),
                            Some(attribution),
                        ),
                    )
                    .await;
                }
                Ok(None) => {}
                Err(error) => {
                    eprintln!("x {error}");
                    return false;
                }
            }
        }
    }

    let definition_source = match resolve_run_definition_source(sub_m) {
        Ok(source) => source,
        Err(error) => {
            eprintln!("x {error}");
            return false;
        }
    };
    let runtime_context_path = match runtime_context_path_for_definition_source(&definition_source)
    {
        Ok(path) => path,
        Err(error) => {
            eprintln!("x {error}");
            return false;
        }
    };
    let required_capability = matches!(definition_source, AgentDefinitionSource::LocalPath(_))
        .then_some(crate::commands::local_packages::InstalledEntrypointCapability::Run);
    let checked_package_runtime =
        match crate::commands::local_packages::checked_runtime_lease_for_path(
            runtime_context_path.as_path(),
            required_capability,
        ) {
            Ok(context) => context,
            Err(error) => {
                eprintln!("x {error}");
                return false;
            }
        };
    let (package_context, _package_lease) = match checked_package_runtime {
        Some(checked) => (Some(checked.context), Some(checked.lease)),
        None => (None, None),
    };
    if let Some(context) = package_context.as_ref() {
        let caller_project_root = match std::env::current_dir() {
            Ok(current_dir) => match caller_project_root_for_installed_context(
                package_context.as_ref(),
                current_dir.as_path(),
            ) {
                Ok(project_root) => project_root,
                Err(error) => {
                    eprintln!("x {error}");
                    return false;
                }
            },
            Err(error) => {
                eprintln!("x Failed to inspect current project directory: {error}");
                return false;
            }
        };
        if let Some(caller_project_root) = caller_project_root.as_deref() {
            let same_project = std::fs::canonicalize(caller_project_root)
                .ok()
                .zip(std::fs::canonicalize(&context.package_payload_root).ok())
                .map(|(caller, payload)| caller == payload)
                .unwrap_or(false);
            if !same_project {
                if let Err(error) =
                    crate::commands::local_packages::validate_installed_alias_dependency_for_project(
                        context.alias.as_str(),
                        caller_project_root,
                    )
                {
                    eprintln!("x {error}");
                    return false;
                }
            }
        }
    }
    let project_root = match package_context.as_ref() {
        Some(context) => Some(context.package_payload_root.clone()),
        None => match project_root_for_definition_source(&definition_source) {
            Ok(project_root) => project_root,
            Err(error) => {
                eprintln!("x {error}");
                return false;
            }
        },
    };

    let (definition, definition_json) =
        match load_run_definition_from_source(&definition_source).await {
            Ok(loaded) => loaded,
            Err(error) => {
                eprintln!("x {error}");
                return false;
            }
        };
    let usage_agent_info = usage_agent_info_for_definition_source(
        &definition_source,
        definition_json.as_str(),
        project_root.as_deref(),
    );
    let attribution = attribution_for_definition_source(
        &definition_source,
        definition_json.as_str(),
        project_root.as_deref(),
        package_context.as_ref(),
    );

    super::runtime_actions::scope_declaring_project_root(
        project_root.clone(),
        super::runtime::run_with_definition_in_context_and_usage_agent(
            sub_m,
            &definition,
            project_root,
            Some(usage_agent_info),
            package_context,
            Some(attribution),
        ),
    )
    .await
}

async fn run_client_action(selected: &ArgMatches) -> bool {
    match prepare_client_action(selected) {
        Ok((prepared, settings, policy)) => {
            let root = Some(prepared.project_root.clone());
            let source = AgentDefinitionSource::LocalPath(
                prepared.definition_path.to_string_lossy().into_owned(),
            );
            let definition = match crate::runtime_definition::RuntimeAgentDefinition::from_str(
                &prepared.definition_json,
            ) {
                Ok(definition) => definition,
                Err(_) => {
                    return reject_client_action(super::machine::Failure::new(
                        "runtime.invalid_definition",
                        "The action definition is invalid.",
                    ))
                }
            };
            let context = prepared
                .installed
                .as_ref()
                .map(|installed| installed.context.clone());
            let usage = usage_agent_info_for_definition_source(
                &source,
                &prepared.definition_json,
                root.as_deref(),
            );
            let attribution = attribution_for_definition_source(
                &source,
                &prepared.definition_json,
                root.as_deref(),
                context.as_ref(),
            );
            super::machine::record_client_action(json!({
                "interface": prepared.interface, "action": prepared.action,
                "binding": prepared.binding,
            }));
            // Keep the validated package lease alive through the entire invocation.
            let _lease = prepared.installed;
            super::client_actions::scope_result_artifacts(
                prepared.artifact_context,
                super::runtime_actions::scope_declaring_project_root(
                    root.clone(),
                    policy.scope(
                        super::runtime::run_with_definition_in_context_and_usage_agent(
                            &settings,
                            &definition,
                            root,
                            Some(usage),
                            context,
                            Some(attribution),
                        ),
                    ),
                ),
            )
            .await
        }
        Err(error) => reject_client_action(error),
    }
}

fn reject_client_action(error: super::machine::Failure) -> bool {
    eprintln!("x {}", error.message);
    super::machine::record_error(error);
    false
}

fn prepare_client_action(
    selected: &ArgMatches,
) -> Result<
    (
        super::client_actions::PreparedAction,
        ArgMatches,
        crate::execution_policy::ExecutionPolicy,
    ),
    super::machine::Failure,
> {
    use super::client_actions as actions;
    let raw = actions::read_request_stdin().map_err(|_| {
        super::machine::Failure::new(
            "action.invalid_request",
            "The action request must be bounded UTF-8 JSON.",
        )
    })?;
    let request = actions::parse_request(&raw).map_err(actions::action_failure)?;
    if selected.get_one::<String>("interface") != Some(&request.interface)
        || selected.get_one::<String>("action") != Some(&request.action)
    {
        return Err(super::machine::Failure::new(
            "action.invalid_request",
            "Action request selectors do not match the CLI selectors.",
        ));
    }
    let target = actions::load_target(
        selected.get_one::<String>("project").map(Path::new),
        selected.get_one::<String>("package").map(String::as_str),
    )
    .map_err(actions::action_failure)?;
    let prepared = actions::prepare(&target.root, &request).map_err(actions::action_failure)?;
    if let Some(installed) = prepared.installed.as_ref() {
        let current = std::env::current_dir().map_err(|_| {
            super::machine::Failure::new(
                "action.invalid_target",
                "The caller project could not be inspected.",
            )
        })?;
        let caller = caller_project_root_for_installed_context(Some(&installed.context), &current)
            .map_err(|_| {
                super::machine::Failure::new(
                    "action.invalid_target",
                    "The caller project is invalid.",
                )
            })?;
        if let Some(caller) = caller {
            if fs::canonicalize(&caller).ok()
                != fs::canonicalize(&installed.context.package_payload_root).ok()
            {
                super::local_packages::validate_installed_alias_dependency_for_project(
                    &installed.context.alias,
                    &caller,
                )
                .map_err(|_| {
                    super::machine::Failure::new(
                        "action.invalid_target",
                        "The installed action package does not satisfy the caller dependency.",
                    )
                })?;
            }
        }
    }
    let policy = crate::execution_policy::ExecutionPolicy::parse(&prepared.execution_policy)
        .map_err(|_| {
            super::machine::Failure::new(
                "action.execution_policy_invalid",
                "The action execution policy is invalid.",
            )
        })?;
    let settings = action_runtime_matches(selected, &prepared.run_vars, &prepared.input_overrides)?;
    Ok((prepared, settings, policy))
}

/// Preserves admitted runtime settings while supplying only validated business mappings.
fn action_runtime_matches(
    selected: &ArgMatches,
    run_vars: &[String],
    input_overrides: &[String],
) -> Result<ArgMatches, super::machine::Failure> {
    let mut words = vec![std::ffi::OsString::from("action-runtime")];
    for (id, flag) in [
        ("profile", "--profile"),
        ("model", "--model"),
        ("thinking", "--thinking"),
        ("thinking_choice", "--thinking-choice"),
        ("action_execution", "--action-execution"),
        ("render_mode", "--render-mode"),
        ("usage_log", "--usage-log"),
    ] {
        if let Some(value) = selected.get_one::<String>(id) {
            words.extend([flag.into(), value.into()]);
        }
    }
    for (id, flag) in [
        ("max_output_tokens", "--max-output-tokens"),
        ("max_agent_depth", "--max-agent-depth"),
    ] {
        if let Some(value) = selected.get_one::<u32>(id) {
            words.extend([flag.into(), value.to_string().into()]);
        }
    }
    for (id, flag) in [
        ("inference_timeout_in_sec", "--inference-timeout-in-sec"),
        ("max_runtime_in_sec", "--max-runtime-in-sec"),
    ] {
        if let Some(value) = selected.get_one::<u64>(id) {
            words.extend([flag.into(), value.to_string().into()]);
        }
    }
    for (id, flag) in [
        ("ignore_tools", "--ignore-tools"),
        ("thinking_provider_default", "--thinking-provider-default"),
    ] {
        if selected.get_flag(id) {
            words.push(flag.into());
        }
    }
    for assignment in run_vars {
        words.extend(["--run-var".into(), assignment.into()]);
    }
    for assignment in input_overrides {
        words.extend(["--input-override".into(), assignment.into()]);
    }
    crate::args::runtime_common::runtime_command("action-runtime", "Validated action runtime")
        .try_get_matches_from(words)
        .map_err(|_| {
            super::machine::Failure::new(
                "action.invalid_inputs",
                "Validated action inputs could not be applied to the runtime.",
            )
        })
}

#[cfg(test)]
mod tests {
    use super::{
        attribution_for_definition_source, attribution_for_definition_source_in_dir,
        caller_project_root_for_installed_context, definition_sha256_from_json_str,
        project_root_for_definition_source, resolve_run_definition_source_in_dir,
        usage_agent_info_for_definition_source, AgentDefinitionSource,
    };
    use std::fs;
    use std::path::Path;

    fn minimal_definition_json() -> &'static str {
        r#"{
            "agent_definition_schema_version": "2026-03-11.r1",
            "inputs": [{"type": "text", "text": "Return a tiny answer."}],
            "agent_schema": {
                "type": "object",
                "properties": {
                    "answer": {
                        "type": "string"
                    }
                }
            },
            "actions": []
        }"#
    }

    #[test]
    fn explicit_config_uses_local_path_directly() {
        let resolution = resolve_run_definition_source_in_dir(
            None,
            Some("./adder_test.json"),
            None,
            None,
            Path::new("/tmp"),
        )
        .expect("resolution should succeed");

        assert_eq!(
            resolution,
            AgentDefinitionSource::LocalPath("./adder_test.json".to_string())
        );
    }

    #[test]
    fn missing_name_and_config_is_rejected() {
        let error = resolve_run_definition_source_in_dir(None, None, None, None, Path::new("/tmp"))
            .expect_err("missing run target should fail");

        assert!(error.contains("Missing agent reference"));
        assert!(error.contains("cargo ai run"));
    }

    #[test]
    fn inline_json_uses_inline_source_directly() {
        let resolution = resolve_run_definition_source_in_dir(
            None,
            None,
            Some(r#"{"agent_definition_schema_version":"2026-03-03.r1"}"#),
            None,
            Path::new("/tmp"),
        )
        .expect("inline json source should succeed");

        assert_eq!(
            resolution,
            AgentDefinitionSource::InlineJson(
                r#"{"agent_definition_schema_version":"2026-03-03.r1"}"#.to_string()
            )
        );
    }

    #[test]
    fn stdin_json_uses_stdin_source_directly() {
        let resolution = resolve_run_definition_source_in_dir(
            None,
            None,
            None,
            Some(r#"{"agent_definition_schema_version":"2026-03-03.r1"}"#),
            Path::new("/tmp"),
        )
        .expect("stdin json source should succeed");

        assert_eq!(
            resolution,
            AgentDefinitionSource::StdinJson(
                r#"{"agent_definition_schema_version":"2026-03-03.r1"}"#.to_string()
            )
        );
    }

    #[test]
    fn usage_agent_info_for_local_path_includes_concrete_definition_identity() {
        let info = usage_agent_info_for_definition_source(
            &AgentDefinitionSource::LocalPath("./child_gemma_branch.json".to_string()),
            minimal_definition_json(),
            Some(Path::new(".")),
        );

        assert_eq!(info["source"], "local_path");
        assert_eq!(info["generated"], false);
        assert_eq!(info["artifact"], "./child_gemma_branch.json");
        assert_eq!(info["name"], "child_gemma_branch");
        assert_eq!(info["project_root"], ".");
        let hash = info["definition_sha256"]
            .as_str()
            .expect("definition hash should be present");
        assert_eq!(hash.len(), 64);
        assert!(hash.chars().all(|ch| ch.is_ascii_hexdigit()));
    }

    #[test]
    fn usage_agent_info_for_registry_name_keeps_registry_identity() {
        let info = usage_agent_info_for_definition_source(
            &AgentDefinitionSource::RegistryName("invoice_reviewer".to_string()),
            minimal_definition_json(),
            None,
        );

        assert_eq!(info["source"], "registry");
        assert_eq!(info["generated"], false);
        assert_eq!(info["name"], "invoice_reviewer");
        assert!(info.get("artifact").is_none());
        assert!(info["definition_sha256"].as_str().is_some());
    }

    #[test]
    fn interpreted_definition_hash_uses_canonical_json_ordering() {
        let left = r#"{"b":2,"a":{"z":3,"y":[{"d":4,"c":5}]}}"#;
        let right = r#"{
            "a": {
                "y": [
                    {
                        "c": 5,
                        "d": 4
                    }
                ],
                "z": 3
            },
            "b": 2
        }"#;

        assert_eq!(
            definition_sha256_from_json_str(left).expect("left hash should compute"),
            definition_sha256_from_json_str(right).expect("right hash should compute")
        );
    }

    #[test]
    fn ordinary_absolute_definition_ignores_malformed_caller_project_marker() {
        let unique = uuid::Uuid::new_v4();
        let project_a = std::env::temp_dir().join(format!("cargo-ai-run-project-a-{unique}"));
        let project_b = std::env::temp_dir().join(format!("cargo-ai-run-project-b-{unique}"));
        fs::create_dir_all(project_a.join(".cargo-ai"))
            .expect("project A metadata dir should exist");
        fs::write(
            project_a.join(".cargo-ai/project.toml"),
            "format_version = 1\n",
        )
        .expect("project A metadata should exist");
        let definition_path = project_a.join("agent.json");
        fs::write(&definition_path, minimal_definition_json())
            .expect("project A definition should exist");
        fs::create_dir_all(project_b.join(".cargo-ai/project.toml"))
            .expect("project B malformed marker should exist");

        assert_eq!(
            caller_project_root_for_installed_context(None, project_b.as_path())
                .expect("ordinary runs must not inspect caller package bindings"),
            None
        );
        let source =
            AgentDefinitionSource::LocalPath(definition_path.to_string_lossy().to_string());
        assert_eq!(
            project_root_for_definition_source(&source)
                .expect("source project discovery should succeed"),
            Some(project_a.clone())
        );

        let _ = fs::remove_dir_all(project_a);
        let _ = fs::remove_dir_all(project_b);
    }

    #[test]
    fn copied_project_keeps_authored_package_and_relative_agent_key() {
        let id = uuid::Uuid::new_v4().to_string();
        let base =
            std::env::temp_dir().join(format!("cargo-ai-run-copies-{}", uuid::Uuid::new_v4()));
        for folder in ["first", "second"] {
            let root = base.join(folder);
            fs::create_dir_all(root.join(".cargo-ai")).unwrap();
            fs::create_dir_all(root.join("agents")).unwrap();
            fs::write(
                root.join(".cargo-ai/project.toml"),
                format!("format_version = 1\n[project]\nid = \"{id}\"\nversion = \"2.0.0\"\n"),
            )
            .unwrap();
            let definition = root.join("agents/reports.json");
            fs::write(&definition, minimal_definition_json()).unwrap();
            let source = AgentDefinitionSource::LocalPath(definition.display().to_string());
            let input = attribution_for_definition_source(
                &source,
                minimal_definition_json(),
                Some(root.as_path()),
                None,
            );
            assert_eq!(input.authored_package_id.as_deref(), Some(id.as_str()));
            assert_eq!(input.agent_key.as_deref(), Some("agents/reports.json"));
            assert_eq!(input.package_version.as_deref(), Some("2.0.0"));
            assert_eq!(input.package_root.as_deref(), Some(root.as_path()));
        }
        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn bare_config_in_cwd_resolves_empty_discovered_root_for_attribution() {
        let root = std::env::temp_dir().join(format!("cargo-ai-run-cwd-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(root.join(".cargo-ai")).unwrap();
        let id = uuid::Uuid::new_v4().to_string();
        fs::write(
            root.join(".cargo-ai/project.toml"),
            format!("format_version = 1\n[project]\nid = \"{id}\"\n"),
        )
        .unwrap();
        fs::write(root.join("agent.json"), minimal_definition_json()).unwrap();
        let source = AgentDefinitionSource::LocalPath("agent.json".to_string());
        let input = attribution_for_definition_source_in_dir(
            &source,
            minimal_definition_json(),
            Some(Path::new("")),
            None,
            Some(root.as_path()),
        );
        assert_eq!(input.authored_package_id.as_deref(), Some(id.as_str()));
        assert_eq!(input.package_root.as_deref(), Some(root.as_path()));
        assert_eq!(input.agent_key.as_deref(), Some("agent.json"));
        fs::remove_dir_all(root).unwrap();
    }
}
