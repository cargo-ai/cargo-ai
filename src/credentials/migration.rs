//! One-time legacy credential migration.
//!
//! This module intentionally stays isolated from normal runtime read/write
//! paths so migration behavior is explicit, testable, and easy to retire.

use crate::config::loader::{load_config_strict, ConfigLoad, LoadedConfig};
use crate::config::schema::SecretStoreMode;
use crate::config::storage::{
    persist_legacy_credential_scrub, reconcile_managed_backup_before_credential_scrub,
};
use crate::credentials::store;
use std::collections::BTreeSet;

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct MigrationOutcome {
    pub migrated_profile_tokens: usize,
    pub migrated_account_tokens: bool,
}

impl MigrationOutcome {
    pub fn changed(self) -> bool {
        self.migrated_profile_tokens > 0 || self.migrated_account_tokens
    }
}

pub fn run_legacy_credential_migration() -> Result<MigrationOutcome, String> {
    let loaded = match load_config_strict().map_err(|error| error.to_string())? {
        ConfigLoad::Missing => return Ok(MigrationOutcome::default()),
        ConfigLoad::Loaded(loaded) => loaded,
    };

    migrate_loaded_config(
        loaded,
        store::commit_legacy_config_tokens,
        reconcile_managed_backup_before_credential_scrub,
        |loaded| persist_legacy_credential_scrub(loaded).map(|_| ()),
    )
}

fn migrate_loaded_config<CommitCredentials, ReconcileBackup, ScrubConfig>(
    loaded: LoadedConfig,
    commit_credentials: CommitCredentials,
    reconcile_backup: ReconcileBackup,
    scrub_config: ScrubConfig,
) -> Result<MigrationOutcome, String>
where
    CommitCredentials: FnOnce(
        Option<SecretStoreMode>,
        &[(String, String)],
        Option<(&str, Option<&str>)>,
    ) -> Result<(), String>,
    ReconcileBackup: FnOnce(&LoadedConfig) -> Result<(), String>,
    ScrubConfig: FnOnce(&LoadedConfig) -> Result<(), String>,
{
    let cfg = loaded.config();

    let mut legacy_profile_tokens = Vec::new();
    let mut legacy_profile_names = BTreeSet::new();
    for profile in &cfg.profile {
        if let Some(token) = profile
            .token
            .as_deref()
            .map(|token| token.trim().to_string())
            .filter(|token| !token.is_empty())
        {
            if !legacy_profile_names.insert(profile.name.clone()) {
                return Err(format!(
                    "multiple legacy credential entries use profile name {:?} in '{}'; Cargo AI left the config unchanged",
                    profile.name,
                    loaded.path().display()
                ));
            }
            legacy_profile_tokens.push((profile.name.clone(), token));
        }
    }

    let mut legacy_account_access = None::<String>;
    let mut legacy_account_refresh = None::<String>;
    if let Some(account) = cfg.account.as_ref() {
        legacy_account_access = account
            .access_token
            .as_deref()
            .map(|token| token.trim().to_string())
            .filter(|token| !token.is_empty());
        legacy_account_refresh = account
            .refresh_token
            .as_deref()
            .map(|token| token.trim().to_string())
            .filter(|token| !token.is_empty());
    }

    if legacy_account_access.is_none() && legacy_account_refresh.is_some() {
        return Err(format!(
            "legacy account refresh token in '{}' has no access token; Cargo AI left the config unchanged",
            loaded.path().display()
        ));
    }

    let account_tokens = legacy_account_access
        .as_deref()
        .map(|access_token| (access_token, legacy_account_refresh.as_deref()));
    commit_credentials(cfg.secret_store, &legacy_profile_tokens, account_tokens)?;
    reconcile_backup(&loaded)?;
    scrub_config(&loaded)?;

    Ok(MigrationOutcome {
        migrated_profile_tokens: legacy_profile_tokens.len(),
        migrated_account_tokens: legacy_account_access.is_some(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::loader::{load_config_from_path, ConfigLoad};
    use crate::credentials::store::{credentials_path, load_account_tokens, load_profile_token};
    use std::cell::{Cell, RefCell};
    use std::fs;
    use std::path::Path;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn with_test_env<F>(test: F)
    where
        F: FnOnce(&Path),
    {
        let _guard = crate::commands::runtime_actions::TEST_ENV_LOCK
            .lock()
            .expect("environment lock should not be poisoned");

        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time should be valid")
            .as_nanos();
        let root = std::env::temp_dir().join(format!("cargo-ai-migration-test-{unique}"));
        fs::create_dir_all(&root).expect("temp root should be created");

        let original_cargo_ai_home = std::env::var_os("CARGO_AI_HOME");
        let original_cargo_home = std::env::var_os("CARGO_HOME");
        let original_disable_keychain = std::env::var_os("CARGO_AI_DISABLE_KEYCHAIN");

        std::env::set_var("CARGO_AI_HOME", root.join(".cargo-ai"));
        std::env::set_var("CARGO_HOME", &root);
        std::env::set_var("CARGO_AI_DISABLE_KEYCHAIN", "1");

        test(&root);

        match original_cargo_ai_home {
            Some(value) => std::env::set_var("CARGO_AI_HOME", value),
            None => std::env::remove_var("CARGO_AI_HOME"),
        }
        match original_cargo_home {
            Some(value) => std::env::set_var("CARGO_HOME", value),
            None => std::env::remove_var("CARGO_HOME"),
        }
        match original_disable_keychain {
            Some(value) => std::env::set_var("CARGO_AI_DISABLE_KEYCHAIN", value),
            None => std::env::remove_var("CARGO_AI_DISABLE_KEYCHAIN"),
        }

        let _ = fs::remove_dir_all(root);
    }

    fn assert_duplicate_legacy_profile_tokens_are_rejected(second_token: &str) {
        with_test_env(|root| {
            let config_dir = root.join(".cargo-ai");
            fs::create_dir_all(&config_dir).expect("config dir should exist");
            let config_file = config_dir.join("config.toml");
            let credentials_file = config_dir.join("credentials.toml");
            let first_token = "first-profile-secret";
            let legacy_config = format!(
                r#"profile = [
  {{ name = "duplicate", server = "openai", model = "gpt-4o", token = "{first_token}", timeout_in_sec = 60 }},
  {{ name = "duplicate", server = "openai", model = "gpt-4o-mini", token = "{second_token}", timeout_in_sec = 60 }}
]
secret_store = "file"
"#
            );
            let original_credentials = b"profile_tokens = { existing = \"existing-secret\" }\n";
            fs::write(&config_file, &legacy_config).expect("legacy config should be written");
            fs::write(&credentials_file, original_credentials)
                .expect("existing credentials should be written");
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(&credentials_file, fs::Permissions::from_mode(0o600))
                    .expect("test credentials should be private");
            }

            let error = run_legacy_credential_migration()
                .expect_err("duplicate legacy profile tokens must be rejected");

            assert!(error.contains("profile name \"duplicate\""));
            assert!(!error.contains(first_token));
            assert!(!error.contains(second_token));
            assert_eq!(
                fs::read_to_string(&config_file).expect("config should remain readable"),
                legacy_config
            );
            assert_eq!(
                fs::read(&credentials_file).expect("credentials should remain readable"),
                original_credentials
            );
        });
    }

    #[test]
    fn migrates_legacy_config_secrets_and_scrubs_config_file() {
        with_test_env(|root| {
            let config_dir = root.join(".cargo-ai");
            fs::create_dir_all(&config_dir).expect("config dir should exist");
            let config_file = config_dir.join("config.toml");
            let managed_backup = config_dir.join("config.toml.bak");
            let sibling_recovery = root.join(".cargo-ai-20260621");
            let sibling_sentinel = sibling_recovery.join("sentinel.txt");
            fs::create_dir_all(&sibling_recovery).expect("sibling recovery should exist");
            fs::write(&sibling_sentinel, "preserve-sibling-recovery")
                .expect("sibling sentinel should be written");
            fs::write(
                &managed_backup,
                r#"profile = [
  { name = "old", server = "openai", model = "gpt-4o", token = "backup-profile-secret", timeout_in_sec = 60 }
]
"#,
            )
            .expect("unsafe managed backup should be written");

            fs::write(
                &config_file,
                r#"
profile = [
  { name = "openai-dev", server = "openai", model = "gpt-4o", token = "profile-secret", timeout_in_sec = 60 }
]
default_profile = "openai-dev"
cargo_ai_token = "preserve-unmigrated-token"
unknown_top_level = "preserve-top-level"

[account]
email = "user@example.com"
access_token = "account-access"
refresh_token = "account-refresh"
access_token_expires_in = 3600
access_token_issued_at = 1700000000
unknown_account_field = "preserve-account-field"
"#,
            )
            .expect("legacy config should be written");

            let outcome = run_legacy_credential_migration().expect("migration should succeed");
            assert_eq!(outcome.migrated_profile_tokens, 1);
            assert!(outcome.migrated_account_tokens);

            let profile_token =
                load_profile_token("openai-dev").expect("profile token should be readable");
            assert_eq!(profile_token.as_deref(), Some("profile-secret"));

            let account_tokens = load_account_tokens()
                .expect("account credentials should load")
                .expect("account credentials should exist");
            assert_eq!(account_tokens.access_token, "account-access");
            assert_eq!(
                account_tokens.refresh_token.as_deref(),
                Some("account-refresh")
            );

            let migrated_config =
                fs::read_to_string(&config_file).expect("config should be readable");
            assert!(
                !migrated_config.contains("profile-secret"),
                "profile token must not remain in config.toml"
            );
            assert!(
                !migrated_config.contains("account-access"),
                "account access token must not remain in config.toml"
            );
            assert!(
                !migrated_config.contains("account-refresh"),
                "account refresh token must not remain in config.toml"
            );
            assert!(migrated_config.contains("preserve-unmigrated-token"));
            assert!(migrated_config.contains("preserve-top-level"));
            assert!(migrated_config.contains("preserve-account-field"));
            assert!(
                !managed_backup.exists(),
                "unsafe managed backup must be removed before the active scrub"
            );
            assert_eq!(
                fs::read_to_string(&sibling_sentinel)
                    .expect("sibling recovery sentinel should remain readable"),
                "preserve-sibling-recovery"
            );

            for entry in fs::read_dir(&config_dir).expect("config directory should be readable") {
                let entry = entry.expect("directory entry should be readable");
                let file_name = entry.file_name().to_string_lossy().to_string();
                if file_name != "credentials.toml" {
                    let contents = fs::read_to_string(entry.path())
                        .expect("non-credential migration artifact should be readable");
                    assert!(!contents.contains("profile-secret"));
                    assert!(!contents.contains("account-access"));
                    assert!(!contents.contains("account-refresh"));
                }
            }

            assert!(
                credentials_path().exists(),
                "credentials.toml should be created during migration"
            );
        });
    }

    #[test]
    fn clean_active_config_removes_unsafe_managed_backup_only() {
        with_test_env(|root| {
            let config_dir = root.join(".cargo-ai");
            let sibling_recovery = root.join(".cargo-ai-maintained-recovery");
            fs::create_dir_all(&config_dir).expect("config dir should exist");
            fs::create_dir_all(&sibling_recovery).expect("sibling recovery should exist");
            let config_file = config_dir.join("config.toml");
            let managed_backup = config_dir.join("config.toml.bak");
            let sibling_sentinel = sibling_recovery.join("sentinel.txt");
            let active = "profile = []\nsecret_store = \"file\"\n";
            let unsafe_backup = r#"profile = []
secret_store = "file"

[account]
access_token = "backup-account-secret"
"#;
            fs::write(&config_file, active).expect("active config should be written");
            fs::write(&managed_backup, unsafe_backup)
                .expect("unsafe managed backup should be written");
            fs::write(&sibling_sentinel, "preserve-sibling-recovery")
                .expect("sibling sentinel should be written");

            let outcome = run_legacy_credential_migration()
                .expect("unsafe managed backup cleanup should succeed");

            assert_eq!(outcome, MigrationOutcome::default());
            assert_eq!(
                fs::read_to_string(&config_file).expect("active config should remain readable"),
                active
            );
            assert!(!managed_backup.exists());
            assert_eq!(
                fs::read_to_string(&sibling_sentinel)
                    .expect("sibling recovery sentinel should remain readable"),
                "preserve-sibling-recovery"
            );
        });
    }

    #[test]
    fn clean_active_config_preserves_provably_safe_managed_backup() {
        with_test_env(|root| {
            let config_dir = root.join(".cargo-ai");
            fs::create_dir_all(&config_dir).expect("config dir should exist");
            let config_file = config_dir.join("config.toml");
            let managed_backup = config_dir.join("config.toml.bak");
            let active = "profile = []\nsecret_store = \"file\"\n";
            let safe_backup = "profile = []\nsecret_store = \"file\"\n";
            fs::write(&config_file, active).expect("active config should be written");
            fs::write(&managed_backup, safe_backup).expect("safe managed backup should be written");

            let outcome = run_legacy_credential_migration()
                .expect("safe managed backup inspection should succeed");

            assert_eq!(outcome, MigrationOutcome::default());
            assert_eq!(
                fs::read_to_string(&managed_backup)
                    .expect("safe managed backup should remain readable"),
                safe_backup
            );
        });
    }

    #[test]
    fn managed_backup_cleanup_failure_blocks_active_scrub_after_credential_commit() {
        with_test_env(|root| {
            let config_dir = root.join(".cargo-ai");
            fs::create_dir_all(&config_dir).expect("config dir should exist");
            let config_file = config_dir.join("config.toml");
            let managed_backup = config_dir.join("config.toml.bak");
            let legacy_config = r#"profile = [
  { name = "openai-dev", server = "openai", model = "gpt-4o", token = "profile-secret", timeout_in_sec = 60 }
]
secret_store = "file"
"#;
            fs::write(&config_file, legacy_config).expect("legacy config should be written");
            fs::create_dir(&managed_backup)
                .expect("directory fixture should block managed backup removal");

            let error = run_legacy_credential_migration()
                .expect_err("managed backup cleanup failure must block active scrub");

            assert!(error.contains("failed to remove unsafe managed"));
            assert_eq!(
                fs::read_to_string(&config_file).expect("legacy config should remain readable"),
                legacy_config
            );
            assert!(managed_backup.is_dir());
            assert_eq!(
                load_profile_token("openai-dev")
                    .expect("committed credential should load")
                    .as_deref(),
                Some("profile-secret"),
                "credential commit must precede managed backup cleanup"
            );
        });
    }

    #[test]
    fn migration_is_noop_when_no_legacy_secrets_exist() {
        with_test_env(|root| {
            let config_dir = root.join(".cargo-ai");
            fs::create_dir_all(&config_dir).expect("config dir should exist");
            let config_file = config_dir.join("config.toml");

            let original_config = r#"
profile = [
  { name = "openai-dev", server = "openai", model = "gpt-4o", timeout_in_sec = 60 }
]
default_profile = "openai-dev"
"#;
            fs::write(&config_file, original_config).expect("config should be written");

            let credentials_file = config_dir.join("credentials.toml");
            let original_credentials = b"profile_tokens = { existing = \"existing-secret\" }\n";
            fs::write(&credentials_file, original_credentials)
                .expect("existing credentials should be written");
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(&credentials_file, fs::Permissions::from_mode(0o600))
                    .expect("test credentials should be private");
            }

            let outcome = run_legacy_credential_migration().expect("migration should succeed");
            assert_eq!(outcome, MigrationOutcome::default());
            assert_eq!(
                fs::read_to_string(&config_file).expect("config should remain readable"),
                original_config
            );
            assert_eq!(
                fs::read(&credentials_file).expect("credentials should remain readable"),
                original_credentials,
                "normal non-auth startup must not rewrite established credentials"
            );
        });
    }

    #[test]
    fn malformed_config_and_established_credentials_are_left_byte_identical() {
        with_test_env(|root| {
            let config_dir = root.join(".cargo-ai");
            fs::create_dir_all(&config_dir).expect("config dir should exist");
            let config_file = config_dir.join("config.toml");
            let credentials_file = config_dir.join("credentials.toml");
            let malformed_config = b"profile = [\n";
            let original_credentials = b"profile_tokens = { existing = \"existing-secret\" }\n";
            fs::write(&config_file, malformed_config).expect("malformed config should be written");
            fs::write(&credentials_file, original_credentials)
                .expect("existing credentials should be written");
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(&credentials_file, fs::Permissions::from_mode(0o600))
                    .expect("test credentials should be private");
            }

            let error = run_legacy_credential_migration()
                .expect_err("malformed config must block migration");

            assert!(error.contains(&config_file.display().to_string()));
            assert_eq!(
                fs::read(&config_file).expect("config should remain"),
                malformed_config
            );
            assert_eq!(
                fs::read(&credentials_file).expect("credentials should remain"),
                original_credentials
            );
        });
    }

    #[test]
    fn credential_commit_failure_leaves_legacy_config_recoverable() {
        with_test_env(|root| {
            let config_dir = root.join(".cargo-ai");
            fs::create_dir_all(&config_dir).expect("config dir should exist");
            let config_file = config_dir.join("config.toml");
            let credentials_file = config_dir.join("credentials.toml");
            let legacy_config = r#"profile = [
  { name = "openai-dev", server = "openai", model = "gpt-4o", token = "profile-secret", timeout_in_sec = 60 }
]
secret_store = "file"
"#;
            fs::write(&config_file, legacy_config).expect("legacy config should be written");
            fs::write(&credentials_file, "profile_tokens = [not-valid\n")
                .expect("malformed credentials should be written");
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(&credentials_file, fs::Permissions::from_mode(0o600))
                    .expect("test credentials should be private");
            }

            let error = run_legacy_credential_migration()
                .expect_err("credential persistence failure must block config scrub");

            assert!(error.contains("failed to parse"));
            assert_eq!(
                fs::read_to_string(&config_file).expect("legacy config should remain readable"),
                legacy_config
            );
            assert!(
                !config_dir.join("config.toml.bak").exists(),
                "failed migration must not create a secret-bearing backup"
            );
        });
    }

    #[test]
    fn keychain_failure_leaves_config_and_file_fallback_untouched() {
        with_test_env(|root| {
            let config_dir = root.join(".cargo-ai");
            fs::create_dir_all(&config_dir).expect("config dir should exist");
            let config_file = config_dir.join("config.toml");
            let credentials_file = config_dir.join("credentials.toml");
            let legacy_config = r#"profile = [
  { name = "openai-dev", server = "openai", model = "gpt-4o", token = "profile-secret", timeout_in_sec = 60 }
]
secret_store = "keychain"
"#;
            let fallback_credentials =
                b"profile_tokens = { existing = \"existing-file-secret\" }\n";
            fs::write(&config_file, legacy_config).expect("legacy config should be written");
            fs::write(&credentials_file, fallback_credentials)
                .expect("file fallback should be written");
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(&credentials_file, fs::Permissions::from_mode(0o600))
                    .expect("test credentials should be private");
            }

            let error = run_legacy_credential_migration()
                .expect_err("disabled keychain must block configured keychain migration");

            assert!(error.contains("keychain usage is disabled"));
            assert_eq!(
                fs::read_to_string(&config_file).expect("legacy config should remain readable"),
                legacy_config
            );
            assert_eq!(
                fs::read(&credentials_file).expect("fallback credentials should remain readable"),
                fallback_credentials
            );
        });
    }

    #[test]
    fn refresh_without_access_token_is_rejected_without_writes() {
        with_test_env(|root| {
            let config_dir = root.join(".cargo-ai");
            fs::create_dir_all(&config_dir).expect("config dir should exist");
            let config_file = config_dir.join("config.toml");
            let legacy_config = r#"profile = []
secret_store = "file"

[account]
refresh_token = "orphan-refresh"
"#;
            fs::write(&config_file, legacy_config).expect("legacy config should be written");

            let error = run_legacy_credential_migration()
                .expect_err("orphan refresh token must block migration");

            assert!(error.contains("has no access token"));
            assert_eq!(
                fs::read_to_string(&config_file).expect("legacy config should remain readable"),
                legacy_config
            );
            assert!(!credentials_path().exists());
        });
    }

    #[test]
    fn duplicate_legacy_profile_tokens_with_same_value_are_rejected() {
        assert_duplicate_legacy_profile_tokens_are_rejected("first-profile-secret");
    }

    #[test]
    fn duplicate_legacy_profile_tokens_with_conflicting_values_are_rejected() {
        assert_duplicate_legacy_profile_tokens_are_rejected("second-profile-secret");
    }

    #[test]
    fn scrub_failure_occurs_after_credential_commit_and_leaves_source_untouched() {
        let root = std::env::temp_dir().join(format!(
            "cargo-ai-migration-order-test-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("system time should be valid")
                .as_nanos()
        ));
        fs::create_dir_all(&root).expect("test root should exist");
        let config_file = root.join("config.toml");
        let legacy_config = r#"profile = [
  { name = "openai-dev", server = "openai", model = "gpt-4o", token = "profile-secret", timeout_in_sec = 60 }
]
secret_store = "file"
"#;
        fs::write(&config_file, legacy_config).expect("legacy config should be written");
        let ConfigLoad::Loaded(loaded) =
            load_config_from_path(&config_file).expect("legacy config should load")
        else {
            panic!("legacy config should exist");
        };
        let calls = RefCell::new(Vec::new());

        let error = migrate_loaded_config(
            loaded,
            |_, _, _| {
                calls.borrow_mut().push("credentials");
                Ok(())
            },
            |_| {
                calls.borrow_mut().push("backup");
                Ok(())
            },
            |_| {
                calls.borrow_mut().push("config");
                Err("injected config replacement failure".to_string())
            },
        )
        .expect_err("config scrub failure should be returned");

        assert!(error.contains("injected config replacement failure"));
        assert_eq!(*calls.borrow(), vec!["credentials", "backup", "config"]);
        assert_eq!(
            fs::read_to_string(&config_file).expect("legacy config should remain readable"),
            legacy_config
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn credential_commit_failure_prevents_config_scrub() {
        let root = std::env::temp_dir().join(format!(
            "cargo-ai-migration-commit-failure-test-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("system time should be valid")
                .as_nanos()
        ));
        fs::create_dir_all(&root).expect("test root should exist");
        let config_file = root.join("config.toml");
        fs::write(
            &config_file,
            r#"profile = [
  { name = "openai-dev", server = "openai", model = "gpt-4o", token = "profile-secret", timeout_in_sec = 60 }
]
"#,
        )
        .expect("legacy config should be written");
        let ConfigLoad::Loaded(loaded) =
            load_config_from_path(&config_file).expect("legacy config should load")
        else {
            panic!("legacy config should exist");
        };
        let backup_called = Cell::new(false);
        let scrub_called = Cell::new(false);

        let error = migrate_loaded_config(
            loaded,
            |_, _, _| Err("injected credential failure".to_string()),
            |_| {
                backup_called.set(true);
                Ok(())
            },
            |_| {
                scrub_called.set(true);
                Ok(())
            },
        )
        .expect_err("credential failure should be returned");

        assert!(error.contains("injected credential failure"));
        assert!(!backup_called.get());
        assert!(!scrub_called.get());
        let _ = fs::remove_dir_all(root);
    }
}
