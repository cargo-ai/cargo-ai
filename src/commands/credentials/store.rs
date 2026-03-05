//! Runtime behavior for `cargo ai credentials store`.
use clap::ArgMatches;
use std::io::{self, Write};

use crate::config::schema::SecretStoreMode;
use crate::config::settings as config_settings;
use crate::credentials::store;

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

fn run_status() -> bool {
    let status = match store::secret_store_status() {
        Ok(status) => status,
        Err(error) => {
            eprintln!("❌ Failed to inspect credential-store status: {error}");
            return false;
        }
    };

    let configured = match status.configured_mode {
        Some(mode) => mode.as_str().to_string(),
        None => "(unset; legacy compatibility mode)".to_string(),
    };
    println!("Configured mode: {configured}");
    println!("Default for new installs: {}", status.default_mode.as_str());

    let effective_reads = match status.configured_mode {
        Some(SecretStoreMode::File) => "file only",
        Some(SecretStoreMode::Keychain) => "keychain only",
        None => "legacy keychain-first, then file fallback",
    };
    println!("Effective read behavior: {effective_reads}");
    println!(
        "File credentials present: {}",
        if status.file_credentials_present {
            "yes"
        } else {
            "no"
        }
    );
    println!(
        "Keychain credentials present: {}",
        if status.keychain_credentials_present {
            "yes"
        } else {
            "no"
        }
    );
    println!(
        "Keychain backend accessible: {}",
        if status.keychain_backend_accessible {
            "yes"
        } else {
            "no"
        }
    );

    if let Some(error) = status.keychain_probe_error {
        println!("Keychain probe detail: {error}");
    }

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
