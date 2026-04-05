//! Runtime behavior for `cargo ai account agents`.
use clap::ArgMatches;
use serde_json::{json, Value};

use crate::agent_builder::build_target::BuildTarget;
use crate::infra_api;
use crate::ui;

use std::{
    fs,
    path::{Path, PathBuf},
};

use super::helpers::{
    apply_agents_list_display_limit, load_account_auth, persist_refreshed_access_token,
    refresh_access_token_for_retry, RefreshAccessError, INFRA_BASE_URL,
};

#[derive(Clone, Debug)]
struct AccountHatchCommand {
    source_name: String,
    local_name: String,
    owner_handle: Option<String>,
    definition_path: Option<String>,
    mode: crate::commands::hatch_pipeline::HatchMode,
    force_overwrite: bool,
    keep_project: bool,
    build_target: BuildTarget,
    output_dir: Option<PathBuf>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct AccountAgentSourceDetails {
    owner: String,
    agent: String,
    path: String,
}

fn hatch_mode_from_check_flag(check_only: bool) -> crate::commands::hatch_pipeline::HatchMode {
    if check_only {
        crate::commands::hatch_pipeline::HatchMode::Check
    } else {
        crate::commands::hatch_pipeline::HatchMode::Build
    }
}

fn is_supported_local_hatch_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_')
}

fn looks_like_local_path_input(input: &str) -> bool {
    input.starts_with("./")
        || input.starts_with("../")
        || input.starts_with("~/")
        || input.contains('/')
        || input.contains('\\')
}

fn validate_account_hatch_name(name: &str) -> Result<String, String> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err("Missing account hatch name. Provide positional NAME.".to_string());
    }
    if looks_like_local_path_input(trimmed) || trimmed.ends_with(".json") {
        return Err(format!(
            "Account hatch name '{}' looks like a path or .json file. Use positional NAME only for the local output/workspace name, use --agent <AGENT> for a different remote source, and use --output-dir <DIR> for export location.",
            trimmed
        ));
    }
    if !is_supported_local_hatch_name(trimmed) {
        return Err(format!(
            "Account hatch name '{}' is invalid. Use only letters, numbers, '-' or '_'.",
            trimmed
        ));
    }

    Ok(trimmed.to_string())
}

fn resolve_account_hatch_names(
    local_name_input: &str,
    source_name_override: Option<&str>,
) -> Result<(String, String), String> {
    let local_name = validate_account_hatch_name(local_name_input)?;

    let source_name = match source_name_override {
        Some(source_name_override) => validate_account_hatch_name(source_name_override)?,
        None => local_name.clone(),
    };

    Ok((source_name, local_name))
}

fn parse_hatch_command(hatch_m: &ArgMatches) -> Result<AccountHatchCommand, String> {
    let name = hatch_m
        .get_one::<String>("name")
        .map(String::as_str)
        .ok_or_else(|| "Missing account hatch name. Provide positional NAME.".to_string())?;

    let (source_name, local_name) =
        resolve_account_hatch_names(name, hatch_m.get_one::<String>("agent").map(String::as_str))?;

    let build_target =
        BuildTarget::from_cli(hatch_m.get_one::<String>("target").map(String::as_str))?;
    let output_dir = crate::commands::hatch_pipeline::resolve_output_dir(
        hatch_m.get_one::<String>("output_dir").map(String::as_str),
    )?;

    Ok(AccountHatchCommand {
        source_name,
        local_name,
        owner_handle: hatch_m
            .get_one::<String>("owner_handle")
            .map(|s| s.to_string()),
        definition_path: hatch_m
            .get_one::<String>("definition_path")
            .map(|s| s.to_string()),
        mode: hatch_mode_from_check_flag(hatch_m.get_flag("check")),
        force_overwrite: hatch_m.get_flag("force"),
        keep_project: hatch_m.get_flag("keep_project"),
        build_target,
        output_dir,
    })
}

async fn request_hatch_pull(
    access_token: &str,
    hatch: &AccountHatchCommand,
) -> Result<Value, String> {
    infra_api::account::agents::pull_agent(
        INFRA_BASE_URL,
        access_token,
        &hatch.source_name,
        hatch.owner_handle.as_deref(),
        hatch.definition_path.as_deref(),
    )
    .await
    .map_err(|error| format!("{error:?}"))
}

fn render_account_agents_response(response: &Value) {
    if !ui::account_status::render_backend_ui(response) {
        match serde_json::to_string_pretty(response) {
            Ok(pretty) => println!("{pretty}"),
            Err(_) => println!("{response:?}"),
        }
    }
}

fn display_path(path: &Path) -> String {
    if path.is_relative() {
        return path.display().to_string();
    }

    match std::env::current_dir() {
        Ok(current_dir) => match path.strip_prefix(&current_dir) {
            Ok(relative) if relative.as_os_str().is_empty() => ".".to_string(),
            Ok(relative) => format!("./{}", relative.display()),
            Err(_) => path.display().to_string(),
        },
        Err(_) => path.display().to_string(),
    }
}

fn account_agent_source_details(
    source_name: &str,
    owner_handle: Option<&str>,
    definition_path: Option<&str>,
    response: &Value,
) -> AccountAgentSourceDetails {
    let owner = response
        .get("owner_handle")
        .and_then(|value| value.as_str())
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| owner_handle.unwrap_or("self"));
    let agent = response
        .get("agent")
        .and_then(|value| value.as_str())
        .filter(|value| !value.trim().is_empty())
        .unwrap_or(source_name);
    let path = response
        .get("definition_path")
        .and_then(|value| value.as_str())
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| definition_path.unwrap_or("/"));

    AccountAgentSourceDetails {
        owner: owner.to_string(),
        agent: agent.to_string(),
        path: path.to_string(),
    }
}

fn account_agent_selector_suffix(source: &AccountAgentSourceDetails) -> String {
    let mut args = Vec::new();

    if source.owner != "self" {
        args.push(format!("--owner-handle {}", source.owner));
    }
    if source.path != "/" {
        args.push(format!("--definition-path {}", source.path));
    }

    if args.is_empty() {
        String::new()
    } else {
        format!(" {}", args.join(" "))
    }
}

fn account_pull_success_ui_response(source: &AccountAgentSourceDetails, file_path: &Path) -> Value {
    let selector_suffix = account_agent_selector_suffix(source);
    let file_display = display_path(file_path);

    json!({
        "ui": {
            "schema": "1.0",
            "kind": "success",
            "icon": "✓",
            "title": "Agent definition saved",
            "summary": format!("Saved `{}` to `{}`.", source.agent, file_display),
            "sections": [
                {
                    "type": "kv",
                    "title": "Source",
                    "title_style": "plain",
                    "layout": "aligned",
                    "items": [
                        {"label": "Owner", "value": source.owner},
                        {"label": "Agent", "value": source.agent},
                        {"label": "Path", "value": source.path}
                    ]
                },
                {
                    "type": "kv",
                    "title": "Output",
                    "title_style": "plain",
                    "layout": "aligned",
                    "items": [
                        {"label": "File", "value": format!("`{file_display}`")}
                    ]
                },
                {
                    "type": "kv",
                    "title": "Available commands",
                    "title_style": "plain",
                    "layout": "aligned",
                    "items": [
                        {
                            "label": "Hatch agent",
                            "value": format!("`cargo ai account hatch {}{}`", source.agent, selector_suffix)
                        },
                        {
                            "label": "Print JSON",
                            "value": format!("`cargo ai account agents pull {} --stdout{}`", source.agent, selector_suffix)
                        }
                    ]
                }
            ]
        }
    })
}

fn restyle_agents_list_ui(response: &mut Value, truncation: Option<(usize, usize)>) {
    let agents = match response.get("agents").and_then(|value| value.as_array()) {
        Some(agents) => agents,
        None => return,
    };

    let count = agents.len();
    let summary = match truncation {
        Some((shown, total)) if total > shown => {
            format!("Showing {shown} of {total} account agents.")
        }
        _ if count == 0 => "No account agents found.".to_string(),
        _ => format!("{count} account agents found."),
    };

    let mut sections = Vec::new();

    if count == 0 {
        sections.push(json!({
            "type": "notice",
            "title": "Agents",
            "message": "No agents were found for this query."
        }));
        sections.push(json!({
            "type": "kv",
            "title": "Available commands",
            "title_style": "plain",
            "layout": "aligned",
            "items": [
                {"label": "Push agent", "value": "`cargo ai account agents push --name <agent> --json-file ./<agent>.json`"}
            ]
        }));
    } else {
        let items: Vec<Value> = agents
            .iter()
            .filter_map(|item| {
                let name = item
                    .get("agent_name")
                    .and_then(|value| value.as_str())?
                    .trim();
                let path = item
                    .get("definition_path")
                    .and_then(|value| value.as_str())
                    .unwrap_or("/")
                    .trim();
                if name.is_empty() || path.is_empty() {
                    return None;
                }

                let visibility = match item.get("is_public").and_then(|value| value.as_bool()) {
                    Some(true) => "public",
                    Some(false) => "private",
                    None => "unknown",
                };
                let archived_suffix =
                    match item.get("is_archived").and_then(|value| value.as_bool()) {
                        Some(true) => ", archived",
                        _ => "",
                    };

                Some(json!({
                    "label": name,
                    "value": format!("{path}  {visibility}{archived_suffix}")
                }))
            })
            .collect();

        sections.push(json!({
            "type": "kv",
            "title": "Agents",
            "title_style": "plain",
            "layout": "aligned",
            "items": items
        }));

        if let Some((shown, total)) = truncation {
            if total > shown {
                sections.push(json!({
                    "type": "notice",
                    "title": "Details",
                    "message": format!("Showing {shown} of {total} agents. Use `--limit <N>` or `--all` to adjust output.")
                }));
            }
        }

        sections.push(json!({
            "type": "kv",
            "title": "Available commands",
            "title_style": "plain",
            "layout": "aligned",
            "items": [
                {"label": "Pull agent", "value": "`cargo ai account agents pull <agent>`"},
                {"label": "Push agent", "value": "`cargo ai account agents push --name <agent> --json-file ./<agent>.json`"}
            ]
        }));
    }

    response["ui"] = json!({
        "schema": "1.0",
        "kind": "success",
        "icon": "✓",
        "title": "Agents",
        "summary": summary,
        "sections": sections
    });
}

fn account_hatch_presentation(
    hatch: &AccountHatchCommand,
    response: &Value,
) -> crate::commands::hatch_pipeline::HatchPresentation {
    let source = account_agent_source_details(
        hatch.source_name.as_str(),
        hatch.owner_handle.as_deref(),
        hatch.definition_path.as_deref(),
        response,
    );

    crate::commands::hatch_pipeline::HatchPresentation {
        source: crate::commands::hatch_pipeline::HatchSource::Account {
            owner: source.owner,
            agent: source.agent,
            path: source.path,
        },
    }
}

fn continue_hatch_from_response(hatch: &AccountHatchCommand, response: &Value) -> bool {
    let is_pull_success = response
        .get("type")
        .and_then(|v| v.as_str())
        .map(|t| t == "account_agents_pull_succeeded")
        .unwrap_or(false);

    if !is_pull_success {
        render_account_agents_response(response);
        return false;
    }

    let definition_json = match response.get("definition_json") {
        Some(value) => value,
        None => {
            eprintln!(
                "❌ Hatch could not continue because response did not include 'definition_json'."
            );
            return false;
        }
    };

    let definition_json_str = match serde_json::to_string_pretty(definition_json) {
        Ok(pretty) => pretty,
        Err(error) => {
            eprintln!("❌ Failed to serialize pulled definition JSON: {error}");
            return false;
        }
    };

    let request = crate::commands::hatch_pipeline::HatchRequest::new(
        hatch.local_name.clone(),
        definition_json_str,
        hatch.mode,
        hatch.force_overwrite,
        hatch.keep_project,
        hatch.build_target.clone(),
        hatch.output_dir.clone(),
        account_hatch_presentation(hatch, response),
    );

    crate::commands::hatch_pipeline::run_hatch_pipeline(request)
}

/// Executes account-agent operations (list/push/pull/hatch/visibility/archive).
pub async fn run(agents_m: &ArgMatches) -> bool {
    enum AgentsCommand {
        List {
            owner_handle: Option<String>,
            include_archived: bool,
            display_limit: Option<usize>,
        },
        Push {
            name: String,
            definition_path: Option<String>,
            definition_json: serde_json::Value,
        },
        Pull {
            name: String,
            owner_handle: Option<String>,
            definition_path: Option<String>,
            json_file: Option<String>,
            stdout: bool,
            force: bool,
        },
        Hatch(AccountHatchCommand),
        Visibility {
            name: String,
            definition_path: Option<String>,
            is_public: bool,
            public_from: Option<String>,
            public_until: Option<String>,
        },
        Archive {
            name: String,
            definition_path: Option<String>,
            is_archived: bool,
        },
    }

    let agents_command = if let Some(list_m) = agents_m.subcommand_matches("list") {
        let display_limit = if list_m.get_flag("all") {
            None
        } else {
            Some(list_m.get_one::<u32>("limit").copied().unwrap_or(20) as usize)
        };

        AgentsCommand::List {
            owner_handle: list_m
                .get_one::<String>("owner_handle")
                .map(|s| s.to_string()),
            include_archived: list_m.get_flag("include_archived"),
            display_limit,
        }
    } else if let Some(push_m) = agents_m.subcommand_matches("push") {
        // TODO: Keep future push shortcuts routed through this branch so
        // name inference, validation, and request payload stay consistent.
        let json_file_path = push_m
            .get_one::<String>("json_file")
            .map(|s| s.to_string())
            .or_else(|| {
                push_m
                    .get_one::<String>("input_file")
                    .map(|s| s.to_string())
            });

        let is_valid_inferred_name = |candidate: &str| {
            let normalized = candidate.trim().to_lowercase();
            if normalized.len() < 3 || normalized.len() > 32 {
                return false;
            }

            let mut chars = normalized.chars();
            match chars.next() {
                Some(c) if c.is_ascii_lowercase() || c.is_ascii_digit() => {}
                _ => return false,
            }

            chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-')
        };
        let looks_like_file_path = |candidate: &str| {
            let trimmed = candidate.trim();
            let normalized = trimmed.to_lowercase();

            trimmed.starts_with("~/")
                || trimmed.starts_with("./")
                || trimmed.starts_with("../")
                || trimmed.contains('/')
                || trimmed.contains('\\')
                || normalized.ends_with(".json")
        };

        let name = if let Some(name) = push_m.get_one::<String>("name") {
            let trimmed_name = name.trim();
            if looks_like_file_path(trimmed_name) {
                eprintln!(
                    "❌ The value passed to --name ('{}') looks like a file path. Use --json-file <FILE> for file input and keep --name for the agent name.",
                    name
                );
                return false;
            }
            trimmed_name.to_string()
        } else if let Some(file_path) = json_file_path.as_deref() {
            let stem = match Path::new(file_path)
                .file_stem()
                .and_then(|s| s.to_str())
                .map(str::trim)
                .filter(|s| !s.is_empty())
            {
                Some(s) => s,
                None => {
                    eprintln!(
                        "❌ Could not infer agent name from file '{}'. Use --name explicitly.",
                        file_path
                    );
                    return false;
                }
            };

            if !is_valid_inferred_name(stem) {
                eprintln!(
                    "❌ Inferred agent name '{}' from '{}' is invalid. Use --name explicitly.",
                    stem, file_path
                );
                return false;
            }

            println!("ℹ️ Using inferred agent name from file: {}", stem);
            stem.to_string()
        } else {
            eprintln!("❌ Missing agent name. Provide --name or use --json-file <FILE> (or positional FILE).");
            return false;
        };

        let definition_path = push_m
            .get_one::<String>("definition_path")
            .map(|s| s.to_string());
        let definition_json_raw = if let Some(raw) = push_m.get_one::<String>("json") {
            raw.to_string()
        } else if let Some(file_path) = json_file_path.as_deref() {
            match fs::read_to_string(file_path) {
                Ok(contents) => contents,
                Err(e) => {
                    eprintln!("❌ Failed to read JSON file '{}': {e}", file_path);
                    return false;
                }
            }
        } else {
            eprintln!("❌ Missing required input: provide --json, --json-file <FILE>, or positional FILE.");
            return false;
        };

        let definition_json = match serde_json::from_str::<serde_json::Value>(&definition_json_raw)
        {
            Ok(v) => v,
            Err(e) => {
                eprintln!("❌ Invalid JSON provided for agent definition: {e}");
                return false;
            }
        };

        AgentsCommand::Push {
            name,
            definition_path,
            definition_json,
        }
    } else if let Some(pull_m) = agents_m.subcommand_matches("pull") {
        let name = pull_m
            .get_one::<String>("name")
            .or_else(|| pull_m.get_one::<String>("name_positional"))
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| {
                eprintln!("❌ Missing agent name. Provide positional NAME or --name <NAME>.");
                String::new()
            });
        if name.is_empty() {
            return false;
        }

        AgentsCommand::Pull {
            name,
            owner_handle: pull_m
                .get_one::<String>("owner_handle")
                .map(|s| s.to_string()),
            definition_path: pull_m
                .get_one::<String>("definition_path")
                .map(|s| s.to_string()),
            json_file: pull_m.get_one::<String>("json_file").map(|s| s.to_string()),
            stdout: pull_m.get_flag("stdout"),
            force: pull_m.get_flag("force"),
        }
    } else if let Some(hatch_m) = agents_m.subcommand_matches("hatch") {
        match parse_hatch_command(hatch_m) {
            Ok(hatch) => AgentsCommand::Hatch(hatch),
            Err(error) => {
                eprintln!("❌ {}", error);
                return false;
            }
        }
    } else if let Some(visibility_m) = agents_m.subcommand_matches("visibility") {
        let Some(name) = visibility_m.get_one::<String>("name") else {
            eprintln!("❌ Missing agent name. Provide --name <NAME>.");
            return false;
        };
        AgentsCommand::Visibility {
            name: name.to_string(),
            definition_path: visibility_m
                .get_one::<String>("definition_path")
                .map(|s| s.to_string()),
            is_public: visibility_m.get_flag("public"),
            public_from: visibility_m
                .get_one::<String>("public_from")
                .map(|s| s.to_string()),
            public_until: visibility_m
                .get_one::<String>("public_until")
                .map(|s| s.to_string()),
        }
    } else if let Some(archive_m) = agents_m.subcommand_matches("archive") {
        let Some(name) = archive_m.get_one::<String>("name") else {
            eprintln!("❌ Missing agent name. Provide --name <NAME>.");
            return false;
        };
        AgentsCommand::Archive {
            name: name.to_string(),
            definition_path: archive_m
                .get_one::<String>("definition_path")
                .map(|s| s.to_string()),
            is_archived: archive_m.get_flag("archive"),
        }
    } else {
        println!(
            "No agents subcommand found. Try 'cargo ai account agents list|push|pull|hatch|visibility|archive'."
        );
        return false;
    };

    let auth = match load_account_auth() {
        Ok(auth) => auth,
        Err(message) => {
            eprintln!("{message}");
            return false;
        }
    };
    let access_token_owned = auth.access_token;
    let refresh_token = auth.refresh_token;

    if let AgentsCommand::Hatch(hatch) = &agents_command {
        crate::commands::hatch_pipeline::print_hatch_start(&hatch.local_name, hatch.mode);
        crate::commands::hatch_pipeline::print_hatch_progress(
            crate::commands::hatch_pipeline::HatchProgressStep::PreparingDefinition,
        );
    }

    // 4. Execute first attempt using current access token.
    let mut response = match &agents_command {
        AgentsCommand::List {
            owner_handle,
            include_archived,
            ..
        } => match infra_api::account::agents::list_agents(
            INFRA_BASE_URL,
            access_token_owned.as_str(),
            owner_handle.as_deref(),
            *include_archived,
        )
        .await
        {
            Ok(r) => r,
            Err(e) => {
                eprintln!("❌ Request failed: {e:?}");
                return false;
            }
        },
        AgentsCommand::Push {
            name,
            definition_path,
            definition_json,
        } => match infra_api::account::agents::push_agent(
            INFRA_BASE_URL,
            access_token_owned.as_str(),
            name,
            definition_path.as_deref(),
            definition_json.clone(),
        )
        .await
        {
            Ok(r) => r,
            Err(e) => {
                eprintln!("❌ Request failed: {e:?}");
                return false;
            }
        },
        AgentsCommand::Pull {
            name,
            owner_handle,
            definition_path,
            ..
        } => match infra_api::account::agents::pull_agent(
            INFRA_BASE_URL,
            access_token_owned.as_str(),
            name,
            owner_handle.as_deref(),
            definition_path.as_deref(),
        )
        .await
        {
            Ok(r) => r,
            Err(e) => {
                eprintln!("❌ Request failed: {e:?}");
                return false;
            }
        },
        AgentsCommand::Hatch(hatch) => {
            match request_hatch_pull(access_token_owned.as_str(), hatch).await {
                Ok(r) => r,
                Err(e) => {
                    eprintln!("❌ Request failed: {e}");
                    return false;
                }
            }
        }
        AgentsCommand::Visibility {
            name,
            definition_path,
            is_public,
            public_from,
            public_until,
        } => match infra_api::account::agents::set_agent_visibility(
            INFRA_BASE_URL,
            access_token_owned.as_str(),
            name,
            definition_path.as_deref(),
            *is_public,
            public_from.as_deref(),
            public_until.as_deref(),
        )
        .await
        {
            Ok(r) => r,
            Err(e) => {
                eprintln!("❌ Request failed: {e:?}");
                return false;
            }
        },
        AgentsCommand::Archive {
            name,
            definition_path,
            is_archived,
        } => match infra_api::account::agents::set_agent_archive(
            INFRA_BASE_URL,
            access_token_owned.as_str(),
            name,
            definition_path.as_deref(),
            *is_archived,
        )
        .await
        {
            Ok(r) => r,
            Err(e) => {
                eprintln!("❌ Request failed: {e:?}");
                return false;
            }
        },
    };

    // 5. If access token is expired, refresh via status and retry agents once.
    let is_expired_error = response
        .get("type")
        .and_then(|v| v.as_str())
        .map(|t| t == "access_token_expired")
        .unwrap_or(false);

    if is_expired_error {
        match refresh_access_token_for_retry(access_token_owned.as_str(), refresh_token.as_deref())
            .await
        {
            Err(RefreshAccessError::MissingRefreshToken) => {
                eprintln!("⚠️ Access token expired, and no refresh token exists in credential store. Run `cargo ai account status` or re-confirm account.");
                if !ui::account_status::render_backend_ui(&response) {
                    match serde_json::to_string_pretty(&response) {
                        Ok(pretty) => println!("{pretty}"),
                        Err(_) => println!("{response:?}"),
                    }
                }
                return false;
            }
            Err(RefreshAccessError::RequestFailed(error)) => {
                eprintln!("❌ Request failed while refreshing session: {error}");
                return false;
            }
            Err(RefreshAccessError::MissingRefreshedToken(refresh_response)) => {
                eprintln!("⚠️ Session refresh did not return a new access token. Cannot retry agents request.");
                if !ui::account_status::render_backend_ui(&refresh_response) {
                    match serde_json::to_string_pretty(&refresh_response) {
                        Ok(pretty) => println!("{pretty}"),
                        Err(_) => println!("{refresh_response:?}"),
                    }
                }
                return false;
            }
            Ok((retry_access_token, refreshed_expires_in)) => {
                if let Some(rt) = refresh_token.as_deref() {
                    persist_refreshed_access_token(
                        retry_access_token.as_str(),
                        rt,
                        refreshed_expires_in,
                    );
                }

                response = match &agents_command {
                    AgentsCommand::List {
                        owner_handle,
                        include_archived,
                        ..
                    } => match infra_api::account::agents::list_agents(
                        INFRA_BASE_URL,
                        retry_access_token.as_str(),
                        owner_handle.as_deref(),
                        *include_archived,
                    )
                    .await
                    {
                        Ok(r) => r,
                        Err(e) => {
                            eprintln!("❌ Request failed after session refresh: {e:?}");
                            return false;
                        }
                    },
                    AgentsCommand::Push {
                        name,
                        definition_path,
                        definition_json,
                    } => match infra_api::account::agents::push_agent(
                        INFRA_BASE_URL,
                        retry_access_token.as_str(),
                        name,
                        definition_path.as_deref(),
                        definition_json.clone(),
                    )
                    .await
                    {
                        Ok(r) => r,
                        Err(e) => {
                            eprintln!("❌ Request failed after session refresh: {e:?}");
                            return false;
                        }
                    },
                    AgentsCommand::Pull {
                        name,
                        owner_handle,
                        definition_path,
                        ..
                    } => match infra_api::account::agents::pull_agent(
                        INFRA_BASE_URL,
                        retry_access_token.as_str(),
                        name,
                        owner_handle.as_deref(),
                        definition_path.as_deref(),
                    )
                    .await
                    {
                        Ok(r) => r,
                        Err(e) => {
                            eprintln!("❌ Request failed after session refresh: {e:?}");
                            return false;
                        }
                    },
                    AgentsCommand::Hatch(hatch) => {
                        match request_hatch_pull(retry_access_token.as_str(), hatch).await {
                            Ok(r) => r,
                            Err(e) => {
                                eprintln!("❌ Request failed after session refresh: {e}");
                                return false;
                            }
                        }
                    }
                    AgentsCommand::Visibility {
                        name,
                        definition_path,
                        is_public,
                        public_from,
                        public_until,
                    } => match infra_api::account::agents::set_agent_visibility(
                        INFRA_BASE_URL,
                        retry_access_token.as_str(),
                        name,
                        definition_path.as_deref(),
                        *is_public,
                        public_from.as_deref(),
                        public_until.as_deref(),
                    )
                    .await
                    {
                        Ok(r) => r,
                        Err(e) => {
                            eprintln!("❌ Request failed after session refresh: {e:?}");
                            return false;
                        }
                    },
                    AgentsCommand::Archive {
                        name,
                        definition_path,
                        is_archived,
                    } => match infra_api::account::agents::set_agent_archive(
                        INFRA_BASE_URL,
                        retry_access_token.as_str(),
                        name,
                        definition_path.as_deref(),
                        *is_archived,
                    )
                    .await
                    {
                        Ok(r) => r,
                        Err(e) => {
                            eprintln!("❌ Request failed after session refresh: {e:?}");
                            return false;
                        }
                    },
                };
            }
        }
    }

    let mut pull_stdout_payload: Option<String> = None;
    if let AgentsCommand::Pull {
        name,
        owner_handle,
        definition_path,
        json_file,
        stdout,
        force,
        ..
    } = &agents_command
    {
        let is_pull_success = response
            .get("type")
            .and_then(|v| v.as_str())
            .map(|t| t == "account_agents_pull_succeeded")
            .unwrap_or(false);

        if is_pull_success {
            let definition_json = match response.get("definition_json") {
                Some(value) => value,
                None => {
                    eprintln!("❌ Pull succeeded but response did not include 'definition_json'.");
                    return false;
                }
            };

            let pretty_definition = match serde_json::to_string_pretty(definition_json) {
                Ok(pretty) => pretty,
                Err(e) => {
                    eprintln!("❌ Failed to serialize pulled definition JSON: {e}");
                    return false;
                }
            };

            let output_path = if let Some(path) = json_file {
                Some(path.clone())
            } else if *stdout {
                None
            } else {
                Some(format!("{}.json", name))
            };
            let source = account_agent_source_details(
                name.as_str(),
                owner_handle.as_deref(),
                definition_path.as_deref(),
                &response,
            );

            if let Some(path) = output_path {
                if Path::new(&path).exists() && !*force {
                    eprintln!(
                        "❌ Output file '{}' already exists. Use --force to overwrite or --json-file <FILE> to choose another path.",
                        path
                    );
                    return false;
                }

                if let Err(e) = fs::write(&path, format!("{pretty_definition}\n")) {
                    eprintln!(
                        "❌ Failed to write pulled definition JSON to '{}': {e}",
                        path
                    );
                    return false;
                }

                if *stdout {
                    eprintln!(
                        "Saved agent definition to '{}'.",
                        display_path(Path::new(&path))
                    );
                } else {
                    render_account_agents_response(&account_pull_success_ui_response(
                        &source,
                        Path::new(&path),
                    ));
                    return true;
                }
            }

            if *stdout {
                pull_stdout_payload = Some(pretty_definition);
            }
        }
    }

    if let Some(pretty_definition) = pull_stdout_payload {
        println!("{pretty_definition}");
        return true;
    }

    if let AgentsCommand::Hatch(hatch) = &agents_command {
        return continue_hatch_from_response(hatch, &response);
    }

    let list_display_truncation = match &agents_command {
        AgentsCommand::List { display_limit, .. } => {
            apply_agents_list_display_limit(&mut response, *display_limit)
        }
        _ => None,
    };

    let is_list_success = response
        .get("type")
        .and_then(|value| value.as_str())
        .map(|value| value == "account_agents_list_succeeded")
        .unwrap_or(false);
    if is_list_success {
        restyle_agents_list_ui(&mut response, list_display_truncation);
    }

    // 6. Render backend-provided UI when available, fallback to raw JSON.
    render_account_agents_response(&response);

    response
        .get("status")
        .and_then(|v| v.as_str())
        .map(|s| s.eq_ignore_ascii_case("success"))
        .unwrap_or(false)
}

pub async fn run_hatch(hatch_m: &ArgMatches) -> bool {
    let hatch = match parse_hatch_command(hatch_m) {
        Ok(hatch) => hatch,
        Err(error) => {
            eprintln!("❌ {}", error);
            return false;
        }
    };

    let auth = match load_account_auth() {
        Ok(auth) => auth,
        Err(message) => {
            eprintln!("{message}");
            return false;
        }
    };
    let access_token_owned = auth.access_token;
    let refresh_token = auth.refresh_token;

    crate::commands::hatch_pipeline::print_hatch_start(&hatch.local_name, hatch.mode);
    crate::commands::hatch_pipeline::print_hatch_progress(
        crate::commands::hatch_pipeline::HatchProgressStep::PreparingDefinition,
    );

    let mut response = match request_hatch_pull(access_token_owned.as_str(), &hatch).await {
        Ok(response) => response,
        Err(error) => {
            eprintln!("❌ Request failed: {error}");
            return false;
        }
    };

    let is_expired_error = response
        .get("type")
        .and_then(|v| v.as_str())
        .map(|t| t == "access_token_expired")
        .unwrap_or(false);

    if is_expired_error {
        match refresh_access_token_for_retry(access_token_owned.as_str(), refresh_token.as_deref())
            .await
        {
            Err(RefreshAccessError::MissingRefreshToken) => {
                eprintln!("⚠️ Access token expired, and no refresh token exists in credential store. Run `cargo ai account status` or re-confirm account.");
                if !ui::account_status::render_backend_ui(&response) {
                    match serde_json::to_string_pretty(&response) {
                        Ok(pretty) => println!("{pretty}"),
                        Err(_) => println!("{response:?}"),
                    }
                }
                return false;
            }
            Err(RefreshAccessError::RequestFailed(error)) => {
                eprintln!("❌ Request failed while refreshing session: {error}");
                return false;
            }
            Err(RefreshAccessError::MissingRefreshedToken(refresh_response)) => {
                eprintln!("⚠️ Session refresh did not return a new access token. Cannot retry account hatch.");
                if !ui::account_status::render_backend_ui(&refresh_response) {
                    match serde_json::to_string_pretty(&refresh_response) {
                        Ok(pretty) => println!("{pretty}"),
                        Err(_) => println!("{refresh_response:?}"),
                    }
                }
                return false;
            }
            Ok((retry_access_token, refreshed_expires_in)) => {
                if let Some(rt) = refresh_token.as_deref() {
                    persist_refreshed_access_token(
                        retry_access_token.as_str(),
                        rt,
                        refreshed_expires_in,
                    );
                }

                response = match request_hatch_pull(retry_access_token.as_str(), &hatch).await {
                    Ok(response) => response,
                    Err(error) => {
                        eprintln!("❌ Request failed after session refresh: {error}");
                        return false;
                    }
                };
            }
        }
    }

    continue_hatch_from_response(&hatch, &response)
}

#[cfg(test)]
mod tests {
    use super::{
        account_agent_selector_suffix, account_agent_source_details,
        account_pull_success_ui_response, hatch_mode_from_check_flag, resolve_account_hatch_names,
        restyle_agents_list_ui, AccountAgentSourceDetails,
    };
    use serde_json::json;
    use std::path::Path;

    #[test]
    fn account_hatch_name_defaults_remote_source_to_local_name() {
        let (source_name, local_name) = resolve_account_hatch_names("weather_test", None)
            .expect("default account hatch names should resolve");
        assert_eq!(source_name, "weather_test");
        assert_eq!(local_name, "weather_test");
    }

    #[test]
    fn account_hatch_name_accepts_remote_source_override() {
        let (source_name, local_name) =
            resolve_account_hatch_names("weather_test_local", Some("weather_test_remote"))
                .expect("remote source override should resolve");
        assert_eq!(source_name, "weather_test_remote");
        assert_eq!(local_name, "weather_test_local");
    }

    #[test]
    fn account_hatch_name_rejects_path_like_local_name() {
        let err = resolve_account_hatch_names("./bin/weather_test", None)
            .expect_err("path-like local name should fail");
        assert!(err.contains("looks like a path"));
    }

    #[test]
    fn account_hatch_name_rejects_json_like_local_name() {
        let err = resolve_account_hatch_names("weather_test.json", None)
            .expect_err(".json-like local name should fail");
        assert!(err.contains(".json"));
    }

    #[test]
    fn account_hatch_name_rejects_invalid_remote_source_override() {
        let err = resolve_account_hatch_names("weather_test", Some("weather.test.v2"))
            .expect_err("invalid remote source override should fail");
        assert!(err.contains("invalid"));
    }

    #[test]
    fn hatch_check_flag_maps_to_check_mode() {
        assert_eq!(
            hatch_mode_from_check_flag(true),
            crate::commands::hatch_pipeline::HatchMode::Check
        );
        assert_eq!(
            hatch_mode_from_check_flag(false),
            crate::commands::hatch_pipeline::HatchMode::Build
        );
    }

    #[test]
    fn account_pull_source_defaults_to_self_and_root_path() {
        let source = account_agent_source_details(
            "weather_test",
            None,
            None,
            &json!({
                "agent": "weather_test",
                "owner_handle": null,
                "definition_path": "/"
            }),
        );

        assert_eq!(
            source,
            AccountAgentSourceDetails {
                owner: "self".to_string(),
                agent: "weather_test".to_string(),
                path: "/".to_string(),
            }
        );
    }

    #[test]
    fn account_pull_success_ui_includes_selector_specific_available_commands() {
        let response = account_pull_success_ui_response(
            &AccountAgentSourceDetails {
                owner: "shared".to_string(),
                agent: "weather_test".to_string(),
                path: "/agents/public".to_string(),
            },
            Path::new("./weather_test.json"),
        );

        let next_step_items = response["ui"]["sections"][2]["items"]
            .as_array()
            .expect("next steps should be rendered as kv items");
        assert_eq!(
            response["ui"]["sections"][2]["title"].as_str(),
            Some("Available commands")
        );
        assert_eq!(
            next_step_items[0]["value"].as_str(),
            Some(
                "`cargo ai account hatch weather_test --owner-handle shared --definition-path /agents/public`"
            )
        );
        assert_eq!(
            next_step_items[1]["value"].as_str(),
            Some(
                "`cargo ai account agents pull weather_test --stdout --owner-handle shared --definition-path /agents/public`"
            )
        );
    }

    #[test]
    fn restyle_agents_list_ui_uses_aligned_sections_and_truncation_notice() {
        let mut response = json!({
            "agents": [
                {
                    "agent_name": "weather",
                    "definition_path": "/",
                    "is_public": true,
                    "is_archived": false
                }
            ]
        });

        restyle_agents_list_ui(&mut response, Some((1, 2)));

        assert_eq!(response["ui"]["title"].as_str(), Some("Agents"));
        assert_eq!(
            response["ui"]["summary"].as_str(),
            Some("Showing 1 of 2 account agents.")
        );
        assert_eq!(
            response["ui"]["sections"][0]["title"].as_str(),
            Some("Agents")
        );
        assert_eq!(
            response["ui"]["sections"][0]["items"][0]["value"].as_str(),
            Some("/  public")
        );
        assert_eq!(
            response["ui"]["sections"][1]["title"].as_str(),
            Some("Details")
        );
        assert_eq!(
            response["ui"]["sections"][2]["title"].as_str(),
            Some("Available commands")
        );
        assert_eq!(
            response["ui"]["sections"][2]["items"][0]["value"].as_str(),
            Some("`cargo ai account agents pull <agent>`")
        );
    }

    #[test]
    fn selector_suffix_omits_self_root_defaults() {
        let suffix = account_agent_selector_suffix(&AccountAgentSourceDetails {
            owner: "self".to_string(),
            agent: "weather_test".to_string(),
            path: "/".to_string(),
        });

        assert!(suffix.is_empty());
    }
}
