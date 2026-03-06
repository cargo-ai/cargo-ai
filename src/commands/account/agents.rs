//! Runtime behavior for `cargo ai account agents`.
use clap::ArgMatches;

use crate::agent_builder::build_target::BuildTarget;
use crate::infra_api;
use crate::ui;

use std::{fs, path::Path};

use super::helpers::{
    apply_agents_list_display_limit, load_account_auth, persist_refreshed_access_token,
    refresh_access_token_for_retry, RefreshAccessError, INFRA_BASE_URL,
};

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

fn resolve_local_hatch_name(
    source_name: &str,
    local_name_override: Option<&str>,
) -> Result<String, String> {
    let Some(source_trimmed) = Some(source_name.trim()).filter(|s| !s.is_empty()) else {
        return Err("Missing agent name. Provide positional AGENT.".to_string());
    };

    let Some(local_name) = local_name_override else {
        return Ok(source_trimmed.to_string());
    };

    let trimmed = local_name.trim();
    if trimmed.is_empty() {
        return Err("Local name cannot be empty. Provide --local-name <NAME>.".to_string());
    }
    if looks_like_local_path_input(trimmed) {
        return Err(format!(
            "Local name '{}' looks like a path. Use --local-name for name-only local output and --definition-path for remote account definition path selection.",
            trimmed
        ));
    }
    if !is_supported_local_hatch_name(trimmed) {
        return Err(format!(
            "Local name '{}' is invalid. Use only letters, numbers, '-' or '_' for --local-name.",
            trimmed
        ));
    }

    Ok(trimmed.to_string())
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
        Hatch {
            source_name: String,
            local_name: String,
            owner_handle: Option<String>,
            definition_path: Option<String>,
            force_overwrite: bool,
            keep_project: bool,
            build_target: BuildTarget,
        },
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
        let source_name = hatch_m
            .get_one::<String>("agent")
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| {
                eprintln!("❌ Missing agent name. Provide positional AGENT.");
                String::new()
            });
        if source_name.is_empty() {
            return false;
        }

        let local_name = match resolve_local_hatch_name(
            &source_name,
            hatch_m.get_one::<String>("local_name").map(String::as_str),
        ) {
            Ok(name) => name,
            Err(error) => {
                eprintln!("❌ {}", error);
                return false;
            }
        };

        AgentsCommand::Hatch {
            source_name,
            local_name,
            owner_handle: hatch_m
                .get_one::<String>("owner_handle")
                .map(|s| s.to_string()),
            definition_path: hatch_m
                .get_one::<String>("definition_path")
                .map(|s| s.to_string()),
            force_overwrite: hatch_m.get_flag("force"),
            keep_project: hatch_m.get_flag("keep_project"),
            build_target: match BuildTarget::from_cli(
                hatch_m.get_one::<String>("target").map(String::as_str),
            ) {
                Ok(build_target) => build_target,
                Err(error) => {
                    eprintln!("❌ {}", error);
                    return false;
                }
            },
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
        AgentsCommand::Hatch {
            source_name,
            owner_handle,
            definition_path,
            ..
        } => match infra_api::account::agents::pull_agent(
            INFRA_BASE_URL,
            access_token_owned.as_str(),
            source_name,
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
                    AgentsCommand::Hatch {
                        source_name,
                        owner_handle,
                        definition_path,
                        ..
                    } => match infra_api::account::agents::pull_agent(
                        INFRA_BASE_URL,
                        retry_access_token.as_str(),
                        source_name,
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
                    eprintln!("ℹ️ Saved pulled definition to '{}'.", path);
                } else {
                    println!("✅ Saved pulled definition to '{}'.", path);
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

    if let AgentsCommand::Hatch {
        source_name,
        local_name,
        owner_handle,
        definition_path,
        force_overwrite,
        keep_project,
        build_target,
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
                    eprintln!(
                        "❌ Hatch could not continue because response did not include 'definition_json'."
                    );
                    return false;
                }
            };

            let definition_json_str = match serde_json::to_string_pretty(definition_json) {
                Ok(pretty) => pretty,
                Err(e) => {
                    eprintln!("❌ Failed to serialize pulled definition JSON: {e}");
                    return false;
                }
            };

            let owner_label = owner_handle.as_deref().unwrap_or("self");
            let path_label = definition_path.as_deref().unwrap_or("/");
            println!(
                "📦 Using account agent definition: owner='{}', name='{}', path='{}'",
                owner_label, source_name, path_label
            );
            if local_name != source_name {
                println!(
                    "ℹ️ Local hatch name override: source='{}' local='{}'.",
                    source_name, local_name
                );
            }
            println!("Build new cargo agent: {local_name}");
            let request = crate::commands::hatch_pipeline::HatchRequest::new(
                local_name.to_string(),
                definition_json_str,
                crate::commands::hatch_pipeline::HatchMode::Build,
                *force_overwrite,
                *keep_project,
                build_target.clone(),
            );
            return crate::commands::hatch_pipeline::run_hatch_pipeline(request);
        }
    }

    let list_display_truncation = match &agents_command {
        AgentsCommand::List { display_limit, .. } => {
            apply_agents_list_display_limit(&mut response, *display_limit)
        }
        _ => None,
    };

    // 6. Render backend-provided UI when available, fallback to raw JSON.
    if !ui::account_status::render_backend_ui(&response) {
        match serde_json::to_string_pretty(&response) {
            Ok(pretty) => println!("{pretty}"),
            Err(_) => println!("{response:?}"),
        }
    }

    if let Some((shown, total)) = list_display_truncation {
        println!(
            "ℹ️ Showing {} of {} agents. Use --limit <N> or --all to adjust output.",
            shown, total
        );
    }

    response
        .get("status")
        .and_then(|v| v.as_str())
        .map(|s| s.eq_ignore_ascii_case("success"))
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::resolve_local_hatch_name;

    #[test]
    fn local_hatch_name_defaults_to_source_name() {
        let resolved = resolve_local_hatch_name("weather_agent", None)
            .expect("default local hatch name should resolve");
        assert_eq!(resolved, "weather_agent");
    }

    #[test]
    fn local_hatch_name_accepts_valid_override() {
        let resolved = resolve_local_hatch_name("weather_agent", Some("weather_agent_v2"))
            .expect("valid local hatch override should resolve");
        assert_eq!(resolved, "weather_agent_v2");
    }

    #[test]
    fn local_hatch_name_rejects_path_like_override() {
        let err = resolve_local_hatch_name("weather_agent", Some("./bin/weather_agent_v2"))
            .expect_err("path-like override should fail");
        assert!(err.contains("looks like a path"));
    }

    #[test]
    fn local_hatch_name_rejects_invalid_name_override() {
        let err = resolve_local_hatch_name("weather_agent", Some("weather.agent.v2"))
            .expect_err("invalid override should fail");
        assert!(err.contains("invalid"));
    }
}
