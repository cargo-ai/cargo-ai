//! One-time legacy credential migration.
//!
//! This module intentionally stays isolated from normal runtime read/write
//! paths so migration behavior is explicit, testable, and easy to retire.

use crate::config::loader::{config_path, load_config};
use crate::credentials::store;
use std::fs;

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

pub fn run_phase1_migration() -> Result<MigrationOutcome, String> {
    let mut cfg = match load_config() {
        Some(cfg) => cfg,
        None => return Ok(MigrationOutcome::default()),
    };

    let mut legacy_profile_tokens = Vec::new();
    for profile in cfg.profile.iter_mut() {
        if let Some(token) = profile
            .token
            .take()
            .map(|token| token.trim().to_string())
            .filter(|token| !token.is_empty())
        {
            legacy_profile_tokens.push((profile.name.clone(), token));
        }
    }

    let mut legacy_account_access = None::<String>;
    let mut legacy_account_refresh = None::<String>;
    if let Some(account) = cfg.account.as_mut() {
        legacy_account_access = account
            .access_token
            .take()
            .map(|token| token.trim().to_string())
            .filter(|token| !token.is_empty());
        legacy_account_refresh = account
            .refresh_token
            .take()
            .map(|token| token.trim().to_string())
            .filter(|token| !token.is_empty());
    }

    if legacy_profile_tokens.is_empty() && legacy_account_access.is_none() {
        return Ok(MigrationOutcome::default());
    }

    for (profile_name, token) in &legacy_profile_tokens {
        store::store_profile_token(profile_name, token)?;
    }

    if let Some(access_token) = legacy_account_access.as_deref() {
        store::store_account_tokens(access_token, legacy_account_refresh.as_deref())?;
    }

    let serialized = toml::to_string_pretty(&cfg)
        .map_err(|error| format!("failed to serialize migrated config: {error}"))?;
    fs::write(config_path(), serialized)
        .map_err(|error| format!("failed to write migrated config: {error}"))?;

    Ok(MigrationOutcome {
        migrated_profile_tokens: legacy_profile_tokens.len(),
        migrated_account_tokens: legacy_account_access.is_some(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::credentials::store::{credentials_path, load_account_tokens, load_profile_token};
    use std::path::Path;
    use std::sync::Mutex;
    use std::time::{SystemTime, UNIX_EPOCH};

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    fn with_test_env<F>(test: F)
    where
        F: FnOnce(&Path),
    {
        let _guard = ENV_LOCK
            .lock()
            .expect("environment lock should not be poisoned");

        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time should be valid")
            .as_nanos();
        let root = std::env::temp_dir().join(format!("cargo-ai-migration-test-{unique}"));
        fs::create_dir_all(&root).expect("temp root should be created");

        let original_cargo_home = std::env::var_os("CARGO_HOME");
        let original_disable_keychain = std::env::var_os("CARGO_AI_DISABLE_KEYCHAIN");

        std::env::set_var("CARGO_HOME", &root);
        std::env::set_var("CARGO_AI_DISABLE_KEYCHAIN", "1");

        test(&root);

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

    #[test]
    fn migrates_legacy_config_secrets_and_scrubs_config_file() {
        with_test_env(|root| {
            let config_dir = root.join(".cargo-ai");
            fs::create_dir_all(&config_dir).expect("config dir should exist");
            let config_file = config_dir.join("config.toml");

            fs::write(
                &config_file,
                r#"
profile = [
  { name = "openai-dev", server = "openai", model = "gpt-4o", token = "profile-secret", timeout_in_sec = 60 }
]
default_profile = "openai-dev"

[account]
email = "user@example.com"
access_token = "account-access"
refresh_token = "account-refresh"
access_token_expires_in = 3600
access_token_issued_at = 1700000000
"#,
            )
            .expect("legacy config should be written");

            let outcome = run_phase1_migration().expect("migration should succeed");
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

            assert!(
                credentials_path().exists(),
                "credentials.toml should be created during migration"
            );
        });
    }

    #[test]
    fn migration_is_noop_when_no_legacy_secrets_exist() {
        with_test_env(|root| {
            let config_dir = root.join(".cargo-ai");
            fs::create_dir_all(&config_dir).expect("config dir should exist");
            let config_file = config_dir.join("config.toml");

            fs::write(
                &config_file,
                r#"
profile = [
  { name = "openai-dev", server = "openai", model = "gpt-4o", timeout_in_sec = 60 }
]
default_profile = "openai-dev"
"#,
            )
            .expect("config should be written");

            let outcome = run_phase1_migration().expect("migration should succeed");
            assert_eq!(outcome, MigrationOutcome::default());

            let creds_exists = credentials_path().exists();
            assert!(!creds_exists);
        });
    }
}
