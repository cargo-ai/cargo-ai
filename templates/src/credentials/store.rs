//! Runtime secret lookup for scaffolded agents.
//!
//! Generated agents resolve profile secrets according to global `secret_store`
//! mode in Cargo-AI config.

use crate::config::loader::load_config;
use crate::config::schema::SecretStoreMode;
use serde::Deserialize;
use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

const KEYCHAIN_SERVICE: &str = "cargo-ai";
const PROFILE_TOKEN_PREFIX: &str = "profile/";
const PROFILE_TOKEN_SUFFIX: &str = "/token";

#[derive(Debug, Deserialize, Default)]
struct CredentialsFile {
    #[serde(default)]
    profile_tokens: BTreeMap<String, String>,
}

fn resolve_credentials_path(
    cargo_ai_home: Option<PathBuf>,
    cargo_home: Option<PathBuf>,
    home_dir: Option<PathBuf>,
) -> PathBuf {
    if let Some(cargo_ai_home) = cargo_ai_home {
        return cargo_ai_home.join("credentials.toml");
    }

    if let Some(cargo_home) = cargo_home {
        return cargo_home.join(".cargo-ai/credentials.toml");
    }

    if let Some(home_dir) = home_dir {
        return home_dir.join(".cargo/.cargo-ai/credentials.toml");
    }

    PathBuf::from(".cargo/.cargo-ai/credentials.toml")
}

fn credentials_path() -> PathBuf {
    resolve_credentials_path(
        std::env::var_os("CARGO_AI_HOME").map(PathBuf::from),
        std::env::var_os("CARGO_HOME").map(PathBuf::from),
        dirs::home_dir(),
    )
}

fn configured_secret_store_mode() -> Option<SecretStoreMode> {
    load_config().and_then(|cfg| cfg.secret_store)
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
fn load_profile_token_from_keychain(profile_name: &str) -> Result<Option<String>, String> {
    let _interaction = super::role_context::keychain_interaction_lock()?;
    if !keychain_enabled() {
        return Err("keychain usage is disabled by CARGO_AI_DISABLE_KEYCHAIN".to_string());
    }

    let account = keychain_account_for_profile(profile_name);
    let entry = keyring::Entry::new(KEYCHAIN_SERVICE, &account)
        .map_err(|error| format!("failed to initialize keyring entry for '{account}': {error}"))?;

    match entry.get_password() {
        Ok(token) if !token.is_empty() => Ok(Some(token)),
        Ok(_) | Err(keyring::Error::NoEntry) => Ok(None),
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
fn load_profile_token_from_keychain(_profile_name: &str) -> Result<Option<String>, String> {
    Err("keychain backend is unavailable on this platform".to_string())
}

fn load_profile_token_from_file(profile_name: &str) -> Result<Option<String>, String> {
    let path = credentials_path();
    if !path.exists() {
        return Ok(None);
    }

    let raw = fs::read_to_string(path)
        .map_err(|error| format!("failed to read credentials file: {error}"))?;
    let parsed = toml::from_str::<CredentialsFile>(&raw)
        .map_err(|error| format!("failed to parse credentials file: {error}"))?;

    Ok(parsed.profile_tokens.get(profile_name).and_then(|value| {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        }
    }))
}

pub fn load_profile_token(profile_name: &str) -> Result<Option<String>, String> {
    match configured_secret_store_mode() {
        Some(SecretStoreMode::File) => load_profile_token_from_file(profile_name),
        Some(SecretStoreMode::Keychain) => load_profile_token_from_keychain(profile_name),
        None => match load_profile_token_from_keychain(profile_name) {
            Ok(Some(token)) => Ok(Some(token)),
            Ok(None) | Err(_) => load_profile_token_from_file(profile_name),
        },
    }
}

pub(crate) fn read_context_records(
    root: &std::path::Path,
    mode: SecretStoreMode,
) -> Result<Option<super::role_context::ContextRecords>, String> {
    let raw = match mode {
        SecretStoreMode::File => {
            let value = context_credentials_file(root)?;
            value
                .get("role_contexts")
                .and_then(toml::Value::as_str)
                .map(str::to_owned)
        }
        SecretStoreMode::Keychain => {
            context_keychain_get(&super::role_context::metadata_key(root))?
        }
    };
    raw.map(|v| {
        serde_json::from_str(&v).map_err(|_| "Private context metadata is invalid.".to_owned())
    })
    .transpose()
}

fn context_credentials_file(root: &std::path::Path) -> Result<toml::Value, String> {
    let path = root.join("credentials.toml");
    for candidate in [root, path.as_path()] {
        match fs::symlink_metadata(candidate) {
            Ok(metadata) => {
                if metadata.file_type().is_symlink() {
                    return Err("Unsafe credential path.".to_owned());
                }
                #[cfg(windows)]
                {
                    use std::os::windows::fs::MetadataExt;
                    if metadata.file_attributes() & 0x400 != 0 {
                        return Err("Unsafe credential path.".to_owned());
                    }
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Ok(toml::Value::Table(Default::default()))
            }
            Err(_) => return Err("Credential state unavailable.".to_owned()),
        }
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if fs::metadata(&path)
            .map_err(|_| "Credential state unavailable.")?
            .permissions()
            .mode()
            & 0o077
            != 0
        {
            return Err("Unsafe credential permissions.".to_owned());
        }
    }
    let raw = fs::read_to_string(path).map_err(|_| "Credential state unavailable.")?;
    toml::from_str(&raw).map_err(|_| "Credential state unavailable.".to_owned())
}

#[cfg(any(
    target_os = "macos",
    target_os = "ios",
    target_os = "windows",
    target_os = "linux",
    target_os = "freebsd",
    target_os = "openbsd"
))]
fn context_keychain_get(account: &str) -> Result<Option<String>, String> {
    if !keychain_enabled() {
        return Err("Selected credential store is unavailable.".to_owned());
    }
    super::role_context::noninteractive_keychain_read(|| {
        let entry = keyring::Entry::new(KEYCHAIN_SERVICE, account)
            .map_err(|_| "Selected credential store is unavailable.")?;
        match entry.get_password() {
            Ok(v) => Ok(Some(v)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(_) => Err("Selected credential store is unavailable.".to_owned()),
        }
    })
}

#[cfg(not(any(
    target_os = "macos",
    target_os = "ios",
    target_os = "windows",
    target_os = "linux",
    target_os = "freebsd",
    target_os = "openbsd"
)))]
fn context_keychain_get(_: &str) -> Result<Option<String>, String> {
    Err("Selected credential store is unavailable.".to_owned())
}

pub(crate) fn context_profile_token(
    root: &std::path::Path,
    mode: SecretStoreMode,
    name: &str,
) -> Result<Option<String>, String> {
    match mode {
        SecretStoreMode::File => Ok(context_credentials_file(root)?
            .get("profile_tokens")
            .and_then(|v| v.get(name))
            .and_then(toml::Value::as_str)
            .map(str::to_owned)),
        SecretStoreMode::Keychain => context_keychain_get(&keychain_account_for_profile(name)),
    }
}

pub(crate) fn context_account_comparison() -> Result<String, String> {
    let session = crate::load_codex_session()
        .map_err(|_| "Selected session is unavailable.".to_owned())?
        .ok_or("Selected session is unavailable.")?;
    if session.access_token_expires_at_unix.is_none()
        || crate::codex_access_token_expired_or_near(session.access_token_expires_at_unix)
    {
        return Err("Selected session freshness is unavailable.".to_owned());
    }
    super::role_context::session_file_comparison(&crate::codex_auth_path()?)
}
