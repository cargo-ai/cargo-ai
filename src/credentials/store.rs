//! Credential storage primitives for profile and account secrets.
//!
//! Storage strategy:
//! - Prefer OS secure credential backends via the `keyring` crate.
//! - Fall back to a local `credentials.toml` file when keychain is unavailable.
//! - Enforce strict fallback file permissions on Unix platforms.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

const KEYCHAIN_SERVICE: &str = "cargo-ai";
const PROFILE_TOKEN_PREFIX: &str = "profile/";
const PROFILE_TOKEN_SUFFIX: &str = "/token";
const ACCOUNT_ACCESS_KEY: &str = "account/access_token";
const ACCOUNT_REFRESH_KEY: &str = "account/refresh_token";

#[derive(Debug, Clone)]
pub struct AccountTokens {
    pub access_token: String,
    pub refresh_token: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Default)]
struct CredentialsFile {
    #[serde(default)]
    profile_tokens: BTreeMap<String, String>,

    #[serde(default)]
    account: Option<CredentialsAccount>,
}

#[derive(Debug, Serialize, Deserialize, Default)]
struct CredentialsAccount {
    #[serde(default)]
    access_token: Option<String>,

    #[serde(default)]
    refresh_token: Option<String>,
}

fn resolve_credentials_path(cargo_home: Option<PathBuf>, home_dir: Option<PathBuf>) -> PathBuf {
    if let Some(cargo_home) = cargo_home {
        return cargo_home.join(".cargo-ai/credentials.toml");
    }

    if let Some(home_dir) = home_dir {
        return home_dir.join(".cargo/.cargo-ai/credentials.toml");
    }

    PathBuf::from(".cargo/.cargo-ai/credentials.toml")
}

pub fn credentials_path() -> PathBuf {
    resolve_credentials_path(
        std::env::var_os("CARGO_HOME").map(PathBuf::from),
        dirs::home_dir(),
    )
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
    Ok(credentials.profile_tokens.get(profile_name).cloned())
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

    let refresh_token = account
        .refresh_token
        .filter(|value| !value.trim().is_empty());
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
        refresh_token: refresh_token.map(str::to_string),
    });
    write_credentials_file(path, &credentials)
}

fn clear_account_tokens_in_file_with_path(path: &Path) -> Result<(), String> {
    let mut credentials = read_credentials_file(path)?;
    credentials.account = None;
    write_credentials_file(path, &credentials)
}

pub fn load_profile_token(profile_name: &str) -> Result<Option<String>, String> {
    let keychain_account = keychain_account_for_profile(profile_name);
    match keychain_get(&keychain_account) {
        Ok(Some(value)) => Ok(Some(value)),
        Ok(None) | Err(_) => {
            load_profile_token_from_file_with_path(&credentials_path(), profile_name)
        }
    }
}

pub fn store_profile_token(profile_name: &str, token: &str) -> Result<(), String> {
    if token.trim().is_empty() {
        return clear_profile_token(profile_name);
    }

    let keychain_account = keychain_account_for_profile(profile_name);
    if keychain_set(&keychain_account, token).is_ok() {
        let _ = clear_profile_token_in_file_with_path(&credentials_path(), profile_name);
        return Ok(());
    }

    store_profile_token_in_file_with_path(&credentials_path(), profile_name, token)
}

pub fn clear_profile_token(profile_name: &str) -> Result<(), String> {
    let keychain_account = keychain_account_for_profile(profile_name);
    let _ = keychain_delete(&keychain_account);
    clear_profile_token_in_file_with_path(&credentials_path(), profile_name)
}

pub fn load_account_tokens() -> Result<Option<AccountTokens>, String> {
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

pub fn store_account_tokens(access_token: &str, refresh_token: Option<&str>) -> Result<(), String> {
    if access_token.trim().is_empty() {
        return clear_account_tokens();
    }

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

pub fn clear_account_tokens() -> Result<(), String> {
    let _ = keychain_delete(ACCOUNT_ACCESS_KEY);
    let _ = keychain_delete(ACCOUNT_REFRESH_KEY);
    clear_account_tokens_in_file_with_path(&credentials_path())
}

#[cfg(test)]
mod tests {
    use super::*;
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
