//! Runtime behavior for `cargo ai credentials store`.
use clap::ArgMatches;
use serde_json::{json, Value};
use std::io::{self, Write};

use crate::config::schema::SecretStoreMode;
use crate::config::settings as config_settings;
use crate::credentials::store;
use crate::ui;

fn parse_mode(raw: &str) -> Option<SecretStoreMode> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "file" => Some(SecretStoreMode::File),
        "keychain" => Some(SecretStoreMode::Keychain),
        _ => None,
    }
}

fn confirm(message: &str) -> Result<bool, String> {
    print!("{message} [y/N]: ");
    io::stdout()
        .flush()
        .map_err(|error| format!("failed to flush stdout: {error}"))?;
    let mut input = String::new();
    io::stdin()
        .read_line(&mut input)
        .map_err(|error| format!("failed to read confirmation input: {error}"))?;
    Ok(matches!(
        input.trim().to_ascii_lowercase().as_str(),
        "y" | "yes"
    ))
}

fn source_has_credentials_for_mode(
    configured_mode: Option<SecretStoreMode>,
    status: &store::SecretStoreStatus,
) -> bool {
    match configured_mode {
        Some(SecretStoreMode::File) => status.file_credentials_present,
        Some(SecretStoreMode::Keychain) => status.keychain_credentials_present,
        None => status.file_credentials_present || status.keychain_credentials_present,
    }
}

fn credential_store_summary(
    status: &store::SecretStoreStatus,
) -> (&'static str, &'static str, String) {
    if status.configured_mode == Some(SecretStoreMode::Keychain)
        && !status.keychain_backend_accessible
    {
        return (
            "info",
            "!",
            "Keychain mode is configured, but the keychain backend is unavailable.".to_string(),
        );
    }

    if status.configured_mode.is_none() {
        return (
            "info",
            "!",
            "Credential-store mode is unset; legacy compatibility reads remain active.".to_string(),
        );
    }

    (
        "success",
        "✓",
        "Credential-store status is healthy.".to_string(),
    )
}

fn credential_store_status_ui_response(status: &store::SecretStoreStatus) -> Value {
    let (kind, icon, summary) = credential_store_summary(status);
    let configured = match status.configured_mode {
        Some(mode) => mode.as_str().to_string(),
        None => "(unset; legacy compatibility mode)".to_string(),
    };
    let effective_reads = match status.configured_mode {
        Some(SecretStoreMode::File) => "file only",
        Some(SecretStoreMode::Keychain) => "keychain only",
        None => "legacy keychain-first, then file fallback",
    };

    let mut sections = vec![
        json!({
            "type": "kv",
            "title": "Configuration",
            "title_style": "plain",
            "layout": "aligned",
            "items": [
                {"label": "Configured mode", "value": configured},
                {"label": "Default mode", "value": status.default_mode.as_str()},
                {"label": "Effective reads", "value": effective_reads}
            ]
        }),
        json!({
            "type": "kv",
            "title": "Availability",
            "title_style": "plain",
            "layout": "aligned",
            "items": [
                {
                    "label": "File credentials",
                    "value": if status.file_credentials_present { "Yes" } else { "No" }
                },
                {
                    "label": "Keychain credentials",
                    "value": if status.keychain_credentials_present { "Yes" } else { "No" }
                },
                {
                    "label": "Keychain backend",
                    "value": if status.keychain_backend_accessible { "Available" } else { "Unavailable" }
                }
            ]
        }),
    ];

    if let Some(error) = &status.keychain_probe_error {
        sections.push(json!({
            "type": "notice",
            "title": "Details",
            "title_style": "plain",
            "message": error
        }));
    }

    if status.configured_mode.is_none()
        || (status.configured_mode == Some(SecretStoreMode::Keychain)
            && !status.keychain_backend_accessible)
    {
        sections.push(json!({
            "type": "kv",
            "title": "Next steps",
            "title_style": "plain",
            "layout": "aligned",
            "items": [
                {"label": "Set mode", "value": "cargo ai credentials store set <file|keychain>"}
            ]
        }));
    }

    json!({
        "ui": {
            "schema": "1.0",
            "kind": kind,
            "icon": icon,
            "title": "Credential-store status",
            "summary": summary,
            "sections": sections
        }
    })
}

fn run_status() -> bool {
    let status = match store::secret_store_status() {
        Ok(status) => status,
        Err(error) => {
            eprintln!("❌ Failed to inspect credential-store status: {error}");
            return false;
        }
    };

    ui::account_status::render_backend_ui(&credential_store_status_ui_response(&status));

    true
}

fn run_set(sub_m: &ArgMatches) -> bool {
    let Some(raw_mode) = sub_m.get_one::<String>("mode") else {
        eprintln!("❌ Missing mode. Use `cargo ai credentials store set <file|keychain>`.");
        return false;
    };

    let Some(target_mode) = parse_mode(raw_mode) else {
        eprintln!("❌ Invalid mode '{raw_mode}'. Use `file` or `keychain`.");
        return false;
    };

    let migrate = sub_m.get_flag("migrate");
    let dry_run = sub_m.get_flag("dry_run");
    let yes = sub_m.get_flag("yes");

    let status = match store::secret_store_status() {
        Ok(status) => status,
        Err(error) => {
            eprintln!("❌ Failed to inspect credential-store status: {error}");
            return false;
        }
    };

    if target_mode == SecretStoreMode::Keychain && !status.keychain_backend_accessible {
        let detail = status
            .keychain_probe_error
            .as_deref()
            .unwrap_or("keychain backend is unavailable");
        eprintln!("❌ Cannot switch to keychain mode: {detail}");
        return false;
    }

    let configured_mode = status.configured_mode;
    if configured_mode == Some(SecretStoreMode::Keychain) && !status.keychain_backend_accessible {
        let detail = status
            .keychain_probe_error
            .as_deref()
            .unwrap_or("keychain backend is unavailable");
        eprintln!("❌ Keychain credentials cannot be inspected right now: {detail}");
        eprintln!("Unlock/allow keychain access, then retry the mode switch.");
        return false;
    }

    let source_has_credentials = source_has_credentials_for_mode(configured_mode, &status);
    let changing_mode = configured_mode != Some(target_mode);

    if changing_mode && source_has_credentials && !migrate {
        eprintln!("❌ Existing credentials were detected for the current mode.");
        eprintln!(
            "Re-run with `--migrate` to copy credentials into '{}' before switching.",
            target_mode.as_str()
        );
        return false;
    }

    if migrate {
        if !dry_run && !yes {
            let confirmed = match confirm("Migrate credentials and switch credential-store mode?") {
                Ok(confirmed) => confirmed,
                Err(error) => {
                    eprintln!("❌ {error}");
                    return false;
                }
            };
            if !confirmed {
                println!("Operation canceled.");
                return true;
            }
        }

        let outcome = match store::migrate_secret_store(target_mode, dry_run) {
            Ok(outcome) => outcome,
            Err(error) => {
                eprintln!("❌ Failed to migrate credential store: {error}");
                return false;
            }
        };

        if dry_run {
            println!("Dry-run migration plan:");
            println!(
                "  Source mode: {}",
                outcome
                    .source_mode
                    .map(|mode| mode.as_str().to_string())
                    .unwrap_or_else(|| "legacy compatibility".to_string())
            );
            println!("  Target mode: {}", outcome.target_mode.as_str());
            println!(
                "  Profile tokens to migrate: {}",
                outcome.migrated_profile_tokens
            );
            println!(
                "  Account tokens to migrate: {}",
                if outcome.migrated_account_tokens {
                    "yes"
                } else {
                    "no"
                }
            );
            println!(
                "  Source contains credentials: {}",
                if outcome.source_had_secrets {
                    "yes"
                } else {
                    "no"
                }
            );
            return true;
        }

        if let Err(error) = config_settings::set_secret_store_mode(target_mode) {
            eprintln!("❌ Failed to persist credential-store mode: {error}");
            return false;
        }

        println!(
            "✅ Credential-store mode set to '{}'. Migrated {} profile token(s); account tokens migrated: {}.",
            target_mode.as_str(),
            outcome.migrated_profile_tokens,
            if outcome.migrated_account_tokens {
                "yes"
            } else {
                "no"
            }
        );
        return true;
    }

    if let Err(error) = config_settings::set_secret_store_mode(target_mode) {
        eprintln!("❌ Failed to persist credential-store mode: {error}");
        return false;
    }

    if changing_mode {
        println!(
            "✅ Credential-store mode set to '{}'. No credentials required migration.",
            target_mode.as_str()
        );
    } else {
        println!(
            "Credential-store mode is already '{}'.",
            target_mode.as_str()
        );
    }

    true
}

#[cfg(test)]
mod tests {
    use super::{credential_store_status_ui_response, credential_store_summary};
    use crate::config::schema::{default_secret_store_mode, SecretStoreMode};
    use crate::credentials::store::SecretStoreStatus;

    #[test]
    fn credential_store_unset_mode_has_next_step() {
        let response = credential_store_status_ui_response(&SecretStoreStatus {
            configured_mode: None,
            default_mode: default_secret_store_mode(),
            file_credentials_present: false,
            keychain_credentials_present: false,
            keychain_backend_accessible: true,
            keychain_probe_error: None,
        });

        assert_eq!(response["ui"]["icon"].as_str(), Some("!"));
        assert_eq!(
            response["ui"]["sections"][2]["items"][0]["value"].as_str(),
            Some("cargo ai credentials store set <file|keychain>")
        );
    }

    #[test]
    fn credential_store_keychain_failure_uses_warning_summary() {
        let (_, icon, summary) = credential_store_summary(&SecretStoreStatus {
            configured_mode: Some(SecretStoreMode::Keychain),
            default_mode: default_secret_store_mode(),
            file_credentials_present: false,
            keychain_credentials_present: false,
            keychain_backend_accessible: false,
            keychain_probe_error: Some("locked".to_string()),
        });

        assert_eq!(icon, "!");
        assert!(summary.contains("keychain backend is unavailable"));
    }
}

/// Executes `cargo ai credentials store ...`.
pub fn run(sub_m: &ArgMatches) -> bool {
    if sub_m.subcommand_matches("status").is_some() {
        run_status()
    } else if let Some(set_m) = sub_m.subcommand_matches("set") {
        run_set(set_m)
    } else {
        eprintln!(
            "No credential-store subcommand found. Try 'cargo ai credentials store status' or 'cargo ai credentials store set <file|keychain>'."
        );
        false
    }
}
