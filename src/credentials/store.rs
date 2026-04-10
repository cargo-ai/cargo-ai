//! Credential storage primitives for profile and account secrets.
//!
//! Storage strategy:
//! - `file` mode stores secrets only in `credentials.toml`.
//! - `keychain` mode stores secrets only in OS keychain backends.
//! - Legacy configs without an explicit mode keep keychain-first read
//!   compatibility to avoid silent auth regressions.

use crate::config::loader::load_config;
use crate::config::schema::{default_secret_store_mode, SecretStoreMode};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

const KEYCHAIN_SERVICE: &str = "cargo-ai";
const PROFILE_TOKEN_PREFIX: &str = "profile/";
const PROFILE_TOKEN_SUFFIX: &str = "/token";
const ACCOUNT_ACCESS_KEY: &str = "account/access_token";
const ACCOUNT_REFRESH_KEY: &str = "account/refresh_token";
const OPENAI_OAUTH_ACCESS_KEY: &str = "openai_oauth/access_token";
const OPENAI_OAUTH_REFRESH_KEY: &str = "openai_oauth/refresh_token";

#[derive(Debug, Clone)]
pub struct AccountTokens {
    pub access_token: String,
    pub refresh_token: Option<String>,
}

#[derive(Debug, Clone)]
pub struct OpenAiOAuthTokens {
    pub access_token: String,
    pub refresh_token: Option<String>,
}

#[derive(Debug, Clone)]
pub struct SecretStoreStatus {
    pub configured_mode: Option<SecretStoreMode>,
    pub default_mode: SecretStoreMode,
    pub file_credentials_present: bool,
    pub keychain_credentials_present: bool,
    pub keychain_backend_accessible: bool,
    pub keychain_probe_error: Option<String>,
}

#[derive(Debug, Clone)]
pub struct SecretStoreMigrationOutcome {
    pub source_mode: Option<SecretStoreMode>,
    pub target_mode: SecretStoreMode,
    pub migrated_profile_tokens: usize,
    pub migrated_account_tokens: bool,
    pub source_had_secrets: bool,
}

#[derive(Debug, Serialize, Deserialize, Default)]
struct CredentialsFile {
    #[serde(default)]
    profile_tokens: BTreeMap<String, String>,

    #[serde(default)]
    account: Option<CredentialsAccount>,

    #[serde(default)]
    openai_oauth: Option<CredentialsAccount>,
}

#[derive(Debug, Serialize, Deserialize, Default)]
struct CredentialsAccount {
    #[serde(default)]
    access_token: Option<String>,

    #[serde(default)]
    refresh_token: Option<String>,
}

#[derive(Debug, Default, Clone)]
struct SecretSnapshot {
    profile_tokens: BTreeMap<String, String>,
    account_tokens: Option<AccountTokens>,
    openai_oauth_tokens: Option<OpenAiOAuthTokens>,
}

impl SecretSnapshot {
    fn is_empty(&self) -> bool {
        self.profile_tokens.is_empty()
            && self.account_tokens.is_none()
            && self.openai_oauth_tokens.is_none()
    }

    fn merge_keychain_preferred(file_snapshot: Self, keychain_snapshot: Self) -> Self {
        let mut merged = file_snapshot;
        for (profile, token) in keychain_snapshot.profile_tokens {
            merged.profile_tokens.insert(profile, token);
        }

        if keychain_snapshot.account_tokens.is_some() {
            merged.account_tokens = keychain_snapshot.account_tokens;
        }

        if keychain_snapshot.openai_oauth_tokens.is_some() {
            merged.openai_oauth_tokens = keychain_snapshot.openai_oauth_tokens;
        }

        merged
    }
}

fn resolve_credentials_path(cargo_ai_root: std::path::PathBuf) -> std::path::PathBuf {
    cargo_ai_root.join("credentials.toml")
}

pub fn credentials_path() -> PathBuf {
    resolve_credentials_path(crate::config::paths::cargo_ai_root())
}

pub fn configured_secret_store_mode() -> Option<SecretStoreMode> {
    load_config().and_then(|cfg| cfg.secret_store)
}

fn profile_names_from_config() -> Vec<String> {
    match load_config() {
        Some(cfg) => cfg
            .profile
            .into_iter()
            .map(|profile| profile.name)
            .collect(),
        None => Vec::new(),
    }
}

fn keychain_account_for_profile(profile_name: &str) -> String {
    format!("{PROFILE_TOKEN_PREFIX}{profile_name}{PROFILE_TOKEN_SUFFIX}")
}

fn keychain_enabled() -> bool {
    match std::env::var("CARGO_AI_DISABLE_KEYCHAIN") {
        Ok(value) => {
            let normalized = value.trim().to_ascii_lowercase();
            normalized != "1" && normalized != "true" && normalized != "yes"
        }
        Err(_) => true,
    }
}

#[cfg(any(
    target_os = "macos",
    target_os = "ios",
    target_os = "windows",
    target_os = "linux",
    target_os = "freebsd",
    target_os = "openbsd"
))]
fn keychain_supported_on_target() -> bool {
    true
}

#[cfg(not(any(
    target_os = "macos",
    target_os = "ios",
    target_os = "windows",
    target_os = "linux",
    target_os = "freebsd",
    target_os = "openbsd"
)))]
fn keychain_supported_on_target() -> bool {
    false
}

fn read_credentials_file(path: &Path) -> Result<CredentialsFile, String> {
    if !path.exists() {
        return Ok(CredentialsFile::default());
    }

    validate_file_permissions(path)?;

    let raw = fs::read_to_string(path)
        .map_err(|error| format!("failed to read '{}': {error}", path.display()))?;

    toml::from_str::<CredentialsFile>(&raw)
        .map_err(|error| format!("failed to parse '{}': {error}", path.display()))
}

fn write_credentials_file(path: &Path, credentials: &CredentialsFile) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| {
            format!(
                "failed to create credentials directory '{}': {error}",
                parent.display()
            )
        })?;
    }

    if path.exists() {
        validate_file_permissions(path)?;
    }

    let serialized = toml::to_string_pretty(credentials)
        .map_err(|error| format!("failed to serialize credentials: {error}"))?;

    fs::write(path, serialized)
        .map_err(|error| format!("failed to write '{}': {error}", path.display()))?;

    lock_down_file_permissions(path)?;
    Ok(())
}

#[cfg(unix)]
fn lock_down_file_permissions(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;

    let mut permissions = fs::metadata(path)
        .map_err(|error| format!("failed to read metadata for '{}': {error}", path.display()))?
        .permissions();
    permissions.set_mode(0o600);
    fs::set_permissions(path, permissions)
        .map_err(|error| format!("failed to set permissions on '{}': {error}", path.display()))?;

    validate_file_permissions(path)
}

#[cfg(not(unix))]
fn lock_down_file_permissions(_path: &Path) -> Result<(), String> {
    Ok(())
}

#[cfg(unix)]
fn validate_file_permissions(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;

    let mode = fs::metadata(path)
        .map_err(|error| format!("failed to read metadata for '{}': {error}", path.display()))?
        .permissions()
        .mode();

    if mode & 0o077 != 0 {
        return Err(format!(
            "refusing insecure permissions on '{}' (expected owner-only permissions)",
            path.display()
        ));
    }

    Ok(())
}

#[cfg(not(unix))]
fn validate_file_permissions(_path: &Path) -> Result<(), String> {
    Ok(())
}

#[cfg(any(
    target_os = "macos",
    target_os = "ios",
    target_os = "windows",
    target_os = "linux",
    target_os = "freebsd",
    target_os = "openbsd"
))]
fn keyring_entry(account: &str) -> Result<keyring::Entry, String> {
    keyring::Entry::new(KEYCHAIN_SERVICE, account)
        .map_err(|error| format!("failed to initialize keyring entry for '{account}': {error}"))
}

#[cfg(any(
    target_os = "macos",
    target_os = "ios",
    target_os = "windows",
    target_os = "linux",
    target_os = "freebsd",
    target_os = "openbsd"
))]
fn keychain_get(account: &str) -> Result<Option<String>, String> {
    if !keychain_enabled() {
        return Err("keychain usage is disabled by CARGO_AI_DISABLE_KEYCHAIN".to_string());
    }

    let entry = keyring_entry(account)?;
    match entry.get_password() {
        Ok(value) => {
            if value.is_empty() {
                Ok(None)
            } else {
                Ok(Some(value))
            }
        }
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(error) => Err(format!("keyring lookup failed for '{account}': {error}")),
    }
}

#[cfg(not(any(
    target_os = "macos",
    target_os = "ios",
    target_os = "windows",
    target_os = "linux",
    target_os = "freebsd",
    target_os = "openbsd"
)))]
fn keychain_get(_account: &str) -> Result<Option<String>, String> {
    Err("keychain backend is unavailable on this platform".to_string())
}

#[cfg(any(
    target_os = "macos",
    target_os = "ios",
    target_os = "windows",
    target_os = "linux",
    target_os = "freebsd",
    target_os = "openbsd"
))]
fn keychain_set(account: &str, secret: &str) -> Result<(), String> {
    if !keychain_enabled() {
        return Err("keychain usage is disabled by CARGO_AI_DISABLE_KEYCHAIN".to_string());
    }

    let entry = keyring_entry(account)?;
    entry
        .set_password(secret)
        .map_err(|error| format!("keyring write failed for '{account}': {error}"))
}

#[cfg(not(any(
    target_os = "macos",
    target_os = "ios",
    target_os = "windows",
    target_os = "linux",
    target_os = "freebsd",
    target_os = "openbsd"
)))]
fn keychain_set(_account: &str, _secret: &str) -> Result<(), String> {
    Err("keychain backend is unavailable on this platform".to_string())
}

#[cfg(any(
    target_os = "macos",
    target_os = "ios",
    target_os = "windows",
    target_os = "linux",
    target_os = "freebsd",
    target_os = "openbsd"
))]
fn keychain_delete(account: &str) -> Result<(), String> {
    if !keychain_enabled() {
        return Err("keychain usage is disabled by CARGO_AI_DISABLE_KEYCHAIN".to_string());
    }

    let entry = keyring_entry(account)?;
    match entry.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(error) => Err(format!("keyring delete failed for '{account}': {error}")),
    }
}

#[cfg(not(any(
    target_os = "macos",
    target_os = "ios",
    target_os = "windows",
    target_os = "linux",
    target_os = "freebsd",
    target_os = "openbsd"
)))]
fn keychain_delete(_account: &str) -> Result<(), String> {
    Err("keychain backend is unavailable on this platform".to_string())
}

fn load_profile_token_from_file_with_path(
    path: &Path,
    profile_name: &str,
) -> Result<Option<String>, String> {
    let credentials = read_credentials_file(path)?;
    Ok(credentials
        .profile_tokens
        .get(profile_name)
        .and_then(|value| {
            let trimmed = value.trim();
            if trimmed.is_empty() {
                None
            } else {
                Some(trimmed.to_string())
            }
        }))
}

fn store_profile_token_in_file_with_path(
    path: &Path,
    profile_name: &str,
    token: &str,
) -> Result<(), String> {
    let mut credentials = read_credentials_file(path)?;
    credentials
        .profile_tokens
        .insert(profile_name.to_string(), token.to_string());
    write_credentials_file(path, &credentials)
}

fn clear_profile_token_in_file_with_path(path: &Path, profile_name: &str) -> Result<(), String> {
    let mut credentials = read_credentials_file(path)?;
    credentials.profile_tokens.remove(profile_name);
    write_credentials_file(path, &credentials)
}

fn load_account_tokens_from_file_with_path(path: &Path) -> Result<Option<AccountTokens>, String> {
    let credentials = read_credentials_file(path)?;
    let account = match credentials.account {
        Some(account) => account,
        None => return Ok(None),
    };

    let access_token = match account.access_token {
        Some(value) if !value.trim().is_empty() => value,
        _ => return Ok(None),
    };

    let refresh_token = account.refresh_token.and_then(|value| {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        }
    });

    Ok(Some(AccountTokens {
        access_token,
        refresh_token,
    }))
}

fn store_account_tokens_in_file_with_path(
    path: &Path,
    access_token: &str,
    refresh_token: Option<&str>,
) -> Result<(), String> {
    let mut credentials = read_credentials_file(path)?;
    credentials.account = Some(CredentialsAccount {
        access_token: Some(access_token.to_string()),
        refresh_token: refresh_token
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string),
    });
    write_credentials_file(path, &credentials)
}

fn clear_account_tokens_in_file_with_path(path: &Path) -> Result<(), String> {
    let mut credentials = read_credentials_file(path)?;
    credentials.account = None;
    write_credentials_file(path, &credentials)
}

#[allow(dead_code)]
fn load_openai_oauth_tokens_from_file_with_path(
    path: &Path,
) -> Result<Option<OpenAiOAuthTokens>, String> {
    let credentials = read_credentials_file(path)?;
    let openai_oauth = match credentials.openai_oauth {
        Some(tokens) => tokens,
        None => return Ok(None),
    };

    let access_token = match openai_oauth.access_token {
        Some(value) if !value.trim().is_empty() => value,
        _ => return Ok(None),
    };

    let refresh_token = openai_oauth.refresh_token.and_then(|value| {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        }
    });

    Ok(Some(OpenAiOAuthTokens {
        access_token,
        refresh_token,
    }))
}

#[allow(dead_code)]
fn store_openai_oauth_tokens_in_file_with_path(
    path: &Path,
    access_token: &str,
    refresh_token: Option<&str>,
) -> Result<(), String> {
    let mut credentials = read_credentials_file(path)?;
    credentials.openai_oauth = Some(CredentialsAccount {
        access_token: Some(access_token.to_string()),
        refresh_token: refresh_token
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string),
    });
    write_credentials_file(path, &credentials)
}

fn clear_openai_oauth_tokens_in_file_with_path(path: &Path) -> Result<(), String> {
    let mut credentials = read_credentials_file(path)?;
    credentials.openai_oauth = None;
    write_credentials_file(path, &credentials)
}

fn load_account_tokens_from_keychain() -> Result<Option<AccountTokens>, String> {
    let access_token = match keychain_get(ACCOUNT_ACCESS_KEY)? {
        Some(value) => value,
        None => return Ok(None),
    };

    let refresh_token = keychain_get(ACCOUNT_REFRESH_KEY)?.and_then(|value| {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        }
    });

    Ok(Some(AccountTokens {
        access_token,
        refresh_token,
    }))
}

fn load_openai_oauth_tokens_from_keychain() -> Result<Option<OpenAiOAuthTokens>, String> {
    let access_token = match keychain_get(OPENAI_OAUTH_ACCESS_KEY)? {
        Some(value) => value,
        None => return Ok(None),
    };

    let refresh_token = keychain_get(OPENAI_OAUTH_REFRESH_KEY)?.and_then(|value| {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        }
    });

    Ok(Some(OpenAiOAuthTokens {
        access_token,
        refresh_token,
    }))
}

fn load_snapshot_from_file(path: &Path) -> Result<SecretSnapshot, String> {
    let credentials = read_credentials_file(path)?;

    let mut snapshot = SecretSnapshot::default();

    for (profile, token) in credentials.profile_tokens {
        let trimmed = token.trim();
        if !trimmed.is_empty() {
            snapshot.profile_tokens.insert(profile, trimmed.to_string());
        }
    }

    if let Some(account) = credentials.account {
        if let Some(access_token) = account
            .access_token
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
        {
            let refresh_token = account
                .refresh_token
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty());
            snapshot.account_tokens = Some(AccountTokens {
                access_token,
                refresh_token,
            });
        }
    }

    if let Some(openai_oauth) = credentials.openai_oauth {
        if let Some(access_token) = openai_oauth
            .access_token
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
        {
            let refresh_token = openai_oauth
                .refresh_token
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty());
            snapshot.openai_oauth_tokens = Some(OpenAiOAuthTokens {
                access_token,
                refresh_token,
            });
        }
    }

    Ok(snapshot)
}

fn load_snapshot_from_keychain(profile_names: &[String]) -> Result<SecretSnapshot, String> {
    let mut snapshot = SecretSnapshot::default();

    for profile_name in profile_names {
        let account = keychain_account_for_profile(profile_name);
        if let Some(token) = keychain_get(&account)? {
            snapshot.profile_tokens.insert(profile_name.clone(), token);
        }
    }

    snapshot.account_tokens = load_account_tokens_from_keychain()?;
    snapshot.openai_oauth_tokens = load_openai_oauth_tokens_from_keychain()?;
    Ok(snapshot)
}

fn write_snapshot_to_file(path: &Path, snapshot: &SecretSnapshot) -> Result<(), String> {
    let mut credentials = CredentialsFile::default();
    credentials.profile_tokens = snapshot.profile_tokens.clone();
    credentials.account = snapshot
        .account_tokens
        .as_ref()
        .map(|tokens| CredentialsAccount {
            access_token: Some(tokens.access_token.clone()),
            refresh_token: tokens.refresh_token.clone(),
        });
    credentials.openai_oauth =
        snapshot
            .openai_oauth_tokens
            .as_ref()
            .map(|tokens| CredentialsAccount {
                access_token: Some(tokens.access_token.clone()),
                refresh_token: tokens.refresh_token.clone(),
            });

    write_credentials_file(path, &credentials)
}

fn clear_file_snapshot(path: &Path) -> Result<(), String> {
    write_credentials_file(path, &CredentialsFile::default())
}

fn write_snapshot_to_keychain(snapshot: &SecretSnapshot) -> Result<(), String> {
    for (profile_name, token) in &snapshot.profile_tokens {
        let account = keychain_account_for_profile(profile_name);
        keychain_set(&account, token)?;
    }

    match &snapshot.account_tokens {
        Some(tokens) => {
            keychain_set(ACCOUNT_ACCESS_KEY, &tokens.access_token)?;
            match tokens.refresh_token.as_deref() {
                Some(refresh) => keychain_set(ACCOUNT_REFRESH_KEY, refresh)?,
                None => {
                    keychain_delete(ACCOUNT_REFRESH_KEY)?;
                }
            }
        }
        None => {
            keychain_delete(ACCOUNT_ACCESS_KEY)?;
            keychain_delete(ACCOUNT_REFRESH_KEY)?;
        }
    }

    match &snapshot.openai_oauth_tokens {
        Some(tokens) => {
            keychain_set(OPENAI_OAUTH_ACCESS_KEY, &tokens.access_token)?;
            match tokens.refresh_token.as_deref() {
                Some(refresh) => keychain_set(OPENAI_OAUTH_REFRESH_KEY, refresh)?,
                None => {
                    keychain_delete(OPENAI_OAUTH_REFRESH_KEY)?;
                }
            }
        }
        None => {
            keychain_delete(OPENAI_OAUTH_ACCESS_KEY)?;
            keychain_delete(OPENAI_OAUTH_REFRESH_KEY)?;
        }
    }

    Ok(())
}

fn clear_keychain_snapshot(profile_names: &[String]) -> Result<(), String> {
    keychain_delete(ACCOUNT_ACCESS_KEY)?;
    keychain_delete(ACCOUNT_REFRESH_KEY)?;
    keychain_delete(OPENAI_OAUTH_ACCESS_KEY)?;
    keychain_delete(OPENAI_OAUTH_REFRESH_KEY)?;

    for profile_name in profile_names {
        keychain_delete(&keychain_account_for_profile(profile_name))?;
    }

    Ok(())
}

fn legacy_snapshot(profile_names: &[String]) -> Result<SecretSnapshot, String> {
    let file_snapshot = load_snapshot_from_file(&credentials_path())?;
    let keychain_snapshot = match load_snapshot_from_keychain(profile_names) {
        Ok(snapshot) => snapshot,
        Err(_) => SecretSnapshot::default(),
    };

    Ok(SecretSnapshot::merge_keychain_preferred(
        file_snapshot,
        keychain_snapshot,
    ))
}

fn source_snapshot_for_mode(mode: Option<SecretStoreMode>) -> Result<SecretSnapshot, String> {
    let path = credentials_path();
    let profile_names = profile_names_from_config();

    match mode {
        Some(SecretStoreMode::File) => load_snapshot_from_file(&path),
        Some(SecretStoreMode::Keychain) => load_snapshot_from_keychain(&profile_names),
        None => legacy_snapshot(&profile_names),
    }
}

fn clear_source_after_migration(
    mode: Option<SecretStoreMode>,
    target: SecretStoreMode,
) -> Result<(), String> {
    let path = credentials_path();
    let profile_names = profile_names_from_config();

    match mode {
        Some(SecretStoreMode::File) if target != SecretStoreMode::File => {
            clear_file_snapshot(&path)
        }
        Some(SecretStoreMode::Keychain) if target != SecretStoreMode::Keychain => {
            clear_keychain_snapshot(&profile_names)
        }
        None => match target {
            SecretStoreMode::File => clear_keychain_snapshot(&profile_names),
            SecretStoreMode::Keychain => clear_file_snapshot(&path),
        },
        _ => Ok(()),
    }
}

pub fn secret_store_status() -> Result<SecretStoreStatus, String> {
    let configured_mode = configured_secret_store_mode();
    let default_mode = default_secret_store_mode();

    let file_snapshot = load_snapshot_from_file(&credentials_path())?;
    let file_credentials_present = !file_snapshot.is_empty();

    let profile_names = profile_names_from_config();
    let (keychain_credentials_present, keychain_backend_accessible, keychain_probe_error) =
        if !keychain_supported_on_target() {
            (
                false,
                false,
                Some("keychain backend is unavailable on this platform".to_string()),
            )
        } else if !keychain_enabled() {
            (
                false,
                false,
                Some("keychain usage is disabled by CARGO_AI_DISABLE_KEYCHAIN".to_string()),
            )
        } else {
            match load_snapshot_from_keychain(&profile_names) {
                Ok(snapshot) => (!snapshot.is_empty(), true, None),
                Err(error) => (false, false, Some(error)),
            }
        };

    Ok(SecretStoreStatus {
        configured_mode,
        default_mode,
        file_credentials_present,
        keychain_credentials_present,
        keychain_backend_accessible,
        keychain_probe_error,
    })
}

pub fn migrate_secret_store(
    target_mode: SecretStoreMode,
    dry_run: bool,
) -> Result<SecretStoreMigrationOutcome, String> {
    let source_mode = configured_secret_store_mode();
    let source_snapshot = source_snapshot_for_mode(source_mode)?;

    let migrated_profile_tokens = source_snapshot.profile_tokens.len();
    let migrated_account_tokens = source_snapshot.account_tokens.is_some();
    let source_had_secrets = !source_snapshot.is_empty();

    if dry_run || source_snapshot.is_empty() || source_mode == Some(target_mode) {
        return Ok(SecretStoreMigrationOutcome {
            source_mode,
            target_mode,
            migrated_profile_tokens,
            migrated_account_tokens,
            source_had_secrets,
        });
    }

    match target_mode {
        SecretStoreMode::File => {
            write_snapshot_to_file(&credentials_path(), &source_snapshot)?;
        }
        SecretStoreMode::Keychain => {
            write_snapshot_to_keychain(&source_snapshot)?;
        }
    }

    clear_source_after_migration(source_mode, target_mode)?;

    Ok(SecretStoreMigrationOutcome {
        source_mode,
        target_mode,
        migrated_profile_tokens,
        migrated_account_tokens,
        source_had_secrets,
    })
}

pub fn load_profile_token(profile_name: &str) -> Result<Option<String>, String> {
    let keychain_account = keychain_account_for_profile(profile_name);

    match configured_secret_store_mode() {
        Some(SecretStoreMode::File) => {
            load_profile_token_from_file_with_path(&credentials_path(), profile_name)
        }
        Some(SecretStoreMode::Keychain) => keychain_get(&keychain_account),
        None => match keychain_get(&keychain_account) {
            Ok(Some(value)) => Ok(Some(value)),
            Ok(None) | Err(_) => {
                load_profile_token_from_file_with_path(&credentials_path(), profile_name)
            }
        },
    }
}

pub fn store_profile_token(profile_name: &str, token: &str) -> Result<(), String> {
    if token.trim().is_empty() {
        return clear_profile_token(profile_name);
    }

    let keychain_account = keychain_account_for_profile(profile_name);

    match configured_secret_store_mode() {
        Some(SecretStoreMode::File) => {
            store_profile_token_in_file_with_path(&credentials_path(), profile_name, token)
        }
        Some(SecretStoreMode::Keychain) => {
            keychain_set(&keychain_account, token)?;
            let _ = clear_profile_token_in_file_with_path(&credentials_path(), profile_name);
            Ok(())
        }
        None => {
            if keychain_set(&keychain_account, token).is_ok() {
                let _ = clear_profile_token_in_file_with_path(&credentials_path(), profile_name);
                return Ok(());
            }

            store_profile_token_in_file_with_path(&credentials_path(), profile_name, token)
        }
    }
}

pub fn clear_profile_token(profile_name: &str) -> Result<(), String> {
    let keychain_account = keychain_account_for_profile(profile_name);
    let _ = keychain_delete(&keychain_account);
    clear_profile_token_in_file_with_path(&credentials_path(), profile_name)
}

pub fn load_account_tokens() -> Result<Option<AccountTokens>, String> {
    match configured_secret_store_mode() {
        Some(SecretStoreMode::File) => load_account_tokens_from_file_with_path(&credentials_path()),
        Some(SecretStoreMode::Keychain) => load_account_tokens_from_keychain(),
        None => {
            let access_from_keychain = keychain_get(ACCOUNT_ACCESS_KEY);
            let refresh_from_keychain = keychain_get(ACCOUNT_REFRESH_KEY);

            if let Ok(Some(access_token)) = access_from_keychain {
                let refresh_token = refresh_from_keychain
                    .ok()
                    .and_then(|value| value.filter(|token| !token.trim().is_empty()));
                return Ok(Some(AccountTokens {
                    access_token,
                    refresh_token,
                }));
            }

            load_account_tokens_from_file_with_path(&credentials_path())
        }
    }
}

pub fn store_account_tokens(access_token: &str, refresh_token: Option<&str>) -> Result<(), String> {
    if access_token.trim().is_empty() {
        return clear_account_tokens();
    }

    match configured_secret_store_mode() {
        Some(SecretStoreMode::File) => {
            store_account_tokens_in_file_with_path(&credentials_path(), access_token, refresh_token)
        }
        Some(SecretStoreMode::Keychain) => {
            keychain_set(ACCOUNT_ACCESS_KEY, access_token)?;
            match refresh_token {
                Some(token) if !token.trim().is_empty() => {
                    keychain_set(ACCOUNT_REFRESH_KEY, token)?
                }
                _ => {
                    keychain_delete(ACCOUNT_REFRESH_KEY)?;
                }
            }
            let _ = clear_account_tokens_in_file_with_path(&credentials_path());
            Ok(())
        }
        None => {
            let keychain_access_result = keychain_set(ACCOUNT_ACCESS_KEY, access_token);
            let keychain_refresh_result = match refresh_token {
                Some(token) if !token.trim().is_empty() => keychain_set(ACCOUNT_REFRESH_KEY, token),
                _ => keychain_delete(ACCOUNT_REFRESH_KEY),
            };

            if keychain_access_result.is_ok() && keychain_refresh_result.is_ok() {
                let _ = clear_account_tokens_in_file_with_path(&credentials_path());
                return Ok(());
            }

            store_account_tokens_in_file_with_path(&credentials_path(), access_token, refresh_token)
        }
    }
}

pub fn clear_account_tokens() -> Result<(), String> {
    let _ = keychain_delete(ACCOUNT_ACCESS_KEY);
    let _ = keychain_delete(ACCOUNT_REFRESH_KEY);
    clear_account_tokens_in_file_with_path(&credentials_path())
}

#[allow(dead_code)]
pub fn load_openai_oauth_tokens() -> Result<Option<OpenAiOAuthTokens>, String> {
    match configured_secret_store_mode() {
        Some(SecretStoreMode::File) => {
            load_openai_oauth_tokens_from_file_with_path(&credentials_path())
        }
        Some(SecretStoreMode::Keychain) => load_openai_oauth_tokens_from_keychain(),
        None => {
            let access_from_keychain = keychain_get(OPENAI_OAUTH_ACCESS_KEY);
            let refresh_from_keychain = keychain_get(OPENAI_OAUTH_REFRESH_KEY);

            if let Ok(Some(access_token)) = access_from_keychain {
                let refresh_token = refresh_from_keychain
                    .ok()
                    .and_then(|value| value.filter(|token| !token.trim().is_empty()));
                return Ok(Some(OpenAiOAuthTokens {
                    access_token,
                    refresh_token,
                }));
            }

            load_openai_oauth_tokens_from_file_with_path(&credentials_path())
        }
    }
}

#[allow(dead_code)]
pub fn store_openai_oauth_tokens(
    access_token: &str,
    refresh_token: Option<&str>,
) -> Result<(), String> {
    if access_token.trim().is_empty() {
        return clear_openai_oauth_tokens();
    }

    match configured_secret_store_mode() {
        Some(SecretStoreMode::File) => store_openai_oauth_tokens_in_file_with_path(
            &credentials_path(),
            access_token,
            refresh_token,
        ),
        Some(SecretStoreMode::Keychain) => {
            keychain_set(OPENAI_OAUTH_ACCESS_KEY, access_token)?;
            match refresh_token {
                Some(token) if !token.trim().is_empty() => {
                    keychain_set(OPENAI_OAUTH_REFRESH_KEY, token)?
                }
                _ => {
                    keychain_delete(OPENAI_OAUTH_REFRESH_KEY)?;
                }
            }
            let _ = clear_openai_oauth_tokens_in_file_with_path(&credentials_path());
            Ok(())
        }
        None => {
            let keychain_access_result = keychain_set(OPENAI_OAUTH_ACCESS_KEY, access_token);
            let keychain_refresh_result = match refresh_token {
                Some(token) if !token.trim().is_empty() => {
                    keychain_set(OPENAI_OAUTH_REFRESH_KEY, token)
                }
                _ => keychain_delete(OPENAI_OAUTH_REFRESH_KEY),
            };

            if keychain_access_result.is_ok() && keychain_refresh_result.is_ok() {
                let _ = clear_openai_oauth_tokens_in_file_with_path(&credentials_path());
                return Ok(());
            }

            store_openai_oauth_tokens_in_file_with_path(
                &credentials_path(),
                access_token,
                refresh_token,
            )
        }
    }
}

pub fn clear_openai_oauth_tokens() -> Result<(), String> {
    let _ = keychain_delete(OPENAI_OAUTH_ACCESS_KEY);
    let _ = keychain_delete(OPENAI_OAUTH_REFRESH_KEY);
    clear_openai_oauth_tokens_in_file_with_path(&credentials_path())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::schema::SecretStoreMode;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn with_temp_credentials_path<F>(test: F)
    where
        F: FnOnce(&Path),
    {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time should be valid")
            .as_nanos();
        let root = std::env::temp_dir().join(format!("cargo-ai-credentials-file-test-{unique}"));
        fs::create_dir_all(&root).expect("temp root should be created");
        let path = root.join("credentials.toml");
        test(path.as_path());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn profile_token_fallback_roundtrip() {
        with_temp_credentials_path(|path| {
            store_profile_token_in_file_with_path(path, "openai-dev", "secret-1")
                .expect("profile token should persist");
            let loaded = load_profile_token_from_file_with_path(path, "openai-dev")
                .expect("profile token should load");
            assert_eq!(loaded.as_deref(), Some("secret-1"));

            clear_profile_token_in_file_with_path(path, "openai-dev")
                .expect("profile token should clear");
            let loaded = load_profile_token_from_file_with_path(path, "openai-dev")
                .expect("profile token should load");
            assert!(loaded.is_none());
        });
    }

    #[test]
    fn account_token_fallback_roundtrip() {
        with_temp_credentials_path(|path| {
            store_account_tokens_in_file_with_path(path, "access-1", Some("refresh-1"))
                .expect("account tokens should persist");

            let loaded =
                load_account_tokens_from_file_with_path(path).expect("account tokens should load");
            let loaded = loaded.expect("account tokens should be present");
            assert_eq!(loaded.access_token, "access-1");
            assert_eq!(loaded.refresh_token.as_deref(), Some("refresh-1"));

            clear_account_tokens_in_file_with_path(path).expect("account tokens should clear");
            let loaded =
                load_account_tokens_from_file_with_path(path).expect("account tokens should load");
            assert!(loaded.is_none());
        });
    }

    #[test]
    fn merge_keychain_preferred_uses_keychain_values() {
        let mut file_snapshot = SecretSnapshot::default();
        file_snapshot
            .profile_tokens
            .insert("openai-dev".to_string(), "file-token".to_string());
        file_snapshot.account_tokens = Some(AccountTokens {
            access_token: "file-access".to_string(),
            refresh_token: Some("file-refresh".to_string()),
        });

        let mut keychain_snapshot = SecretSnapshot::default();
        keychain_snapshot
            .profile_tokens
            .insert("openai-dev".to_string(), "keychain-token".to_string());
        keychain_snapshot.account_tokens = Some(AccountTokens {
            access_token: "keychain-access".to_string(),
            refresh_token: Some("keychain-refresh".to_string()),
        });

        let merged = SecretSnapshot::merge_keychain_preferred(file_snapshot, keychain_snapshot);
        assert_eq!(
            merged.profile_tokens.get("openai-dev").map(String::as_str),
            Some("keychain-token")
        );
        assert_eq!(
            merged
                .account_tokens
                .as_ref()
                .map(|tokens| tokens.access_token.as_str()),
            Some("keychain-access")
        );
    }

    #[test]
    fn dry_run_migration_reports_counts_without_writes() {
        with_temp_credentials_path(|path| {
            store_profile_token_in_file_with_path(path, "openai-dev", "file-token")
                .expect("profile token should persist");
            store_account_tokens_in_file_with_path(path, "access-1", Some("refresh-1"))
                .expect("account tokens should persist");

            // Simulate file-mode source snapshot without touching process-global config.
            let snapshot = load_snapshot_from_file(path).expect("snapshot should load");
            assert!(!snapshot.is_empty());

            let outcome = SecretStoreMigrationOutcome {
                source_mode: Some(SecretStoreMode::File),
                target_mode: SecretStoreMode::Keychain,
                migrated_profile_tokens: snapshot.profile_tokens.len(),
                migrated_account_tokens: snapshot.account_tokens.is_some(),
                source_had_secrets: true,
            };

            assert_eq!(outcome.migrated_profile_tokens, 1);
            assert!(outcome.migrated_account_tokens);
            assert!(outcome.source_had_secrets);
            assert_eq!(outcome.target_mode, SecretStoreMode::Keychain);
        });
    }

    #[cfg(unix)]
    #[test]
    fn rejects_insecure_fallback_permissions() {
        use std::os::unix::fs::PermissionsExt;

        with_temp_credentials_path(|path| {
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).expect("credentials parent should exist");
            }
            fs::write(path, "profile_tokens = {}\n").expect("credentials file should be written");
            fs::set_permissions(path, fs::Permissions::from_mode(0o644))
                .expect("insecure permissions should be set for test");

            let result = load_profile_token_from_file_with_path(path, "openai-dev");
            assert!(result.is_err());
        });
    }
}
