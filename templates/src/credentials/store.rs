//! Runtime secret lookup for scaffolded agents.
//!
//! Generated agents resolve profile secrets from keychain-first storage and
//! fall back to `credentials.toml` when keychain is unavailable.

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

fn resolve_credentials_path(cargo_home: Option<PathBuf>, home_dir: Option<PathBuf>) -> PathBuf {
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

#[cfg(any(
    target_os = "macos",
    target_os = "ios",
    target_os = "windows",
    target_os = "linux",
    target_os = "freebsd",
    target_os = "openbsd"
))]
fn load_profile_token_from_keychain(profile_name: &str) -> Option<String> {
    if !keychain_enabled() {
        return None;
    }

    let account = keychain_account_for_profile(profile_name);
    let entry = keyring::Entry::new(KEYCHAIN_SERVICE, &account).ok()?;
    match entry.get_password() {
        Ok(token) if !token.is_empty() => Some(token),
        Ok(_) | Err(keyring::Error::NoEntry) => None,
        Err(_) => None,
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
fn load_profile_token_from_keychain(_profile_name: &str) -> Option<String> {
    None
}

fn load_profile_token_from_file(profile_name: &str) -> Option<String> {
    let path = credentials_path();
    if !path.exists() {
        return None;
    }

    let raw = fs::read_to_string(path).ok()?;
    let parsed = toml::from_str::<CredentialsFile>(&raw).ok()?;
    parsed.profile_tokens.get(profile_name).cloned()
}

pub fn load_profile_token(profile_name: &str) -> Option<String> {
    load_profile_token_from_keychain(profile_name).or_else(|| load_profile_token_from_file(profile_name))
}
