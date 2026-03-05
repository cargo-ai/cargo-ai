// This module defines the configuration schema for Cargo-AI.
// Originally named `profile.rs`, it was renamed to `schema.rs` to better reflect
// its future role in housing additional sections beyond profiles, such as
// defaults or user tokens for Cargo-AI.org. For now, it only includes profiles.

#![allow(dead_code)]

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SecretStoreMode {
    File,
    Keychain,
}

impl SecretStoreMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::File => "file",
            Self::Keychain => "keychain",
        }
    }
}

pub fn default_secret_store_mode() -> SecretStoreMode {
    SecretStoreMode::File
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ProfileAuthMode {
    None,
    ApiKey,
    OpenaiAccount,
}

impl ProfileAuthMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::ApiKey => "api_key",
            Self::OpenaiAccount => "openai_account",
        }
    }
}

pub fn default_profile_auth_mode() -> ProfileAuthMode {
    ProfileAuthMode::None
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Config {
    pub profile: Vec<Profile>,

    // Reserved for future install or account identification and management (currently unused).
    #[serde(default)]
    pub cargo_ai_token: Option<String>,

    #[serde(default)]
    pub default_profile: Option<String>,

    #[serde(default)]
    pub secret_store: Option<SecretStoreMode>,

    #[serde(default)]
    pub account: Option<Account>,

    #[serde(default)]
    pub openai_auth: Option<OpenAiAuth>,

    #[serde(default)]
    pub web_resources: Option<WebResources>,

    #[serde(default)]
    pub update_check: Option<UpdateCheck>,

    #[serde(default)]
    pub cargo_ai_metadata: Option<CargoAiMetadata>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Profile {
    pub name: String,
    pub server: String,
    pub model: String,

    #[serde(default)]
    pub url: Option<String>,

    #[serde(default)]
    #[serde(skip_serializing)]
    pub token: Option<String>,

    #[serde(default = "default_timeout")]
    pub timeout_in_sec: u64,

    #[serde(default)]
    pub description: Option<String>,

    #[serde(default = "default_profile_auth_mode")]
    pub auth_mode: ProfileAuthMode,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Account {
    #[serde(default)]
    pub email: Option<String>,

    #[serde(default)]
    #[serde(skip_serializing)]
    pub access_token: Option<String>,

    #[serde(default)]
    #[serde(skip_serializing)]
    pub refresh_token: Option<String>,

    #[serde(default)]
    pub access_token_expires_in: Option<i32>,

    // Unix epoch seconds when the access token was last obtained.
    #[serde(default)]
    pub access_token_issued_at: Option<i64>,
}

#[derive(Debug, Serialize, Deserialize, Default)]
pub struct OpenAiAuth {
    #[serde(default)]
    pub access_token_expires_in: Option<i32>,

    #[serde(default)]
    pub access_token_issued_at: Option<i64>,

    #[serde(default)]
    pub locally_disabled: Option<bool>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct WebResources {
    #[serde(default)]
    pub max_attempts: Option<u32>,

    #[serde(default)]
    pub base_backoff_ms: Option<u64>,

    #[serde(default)]
    pub retry_on_empty_body: Option<bool>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct UpdateCheck {
    #[serde(default)]
    pub mode: Option<String>,

    #[serde(default)]
    pub last_checked_unix_seconds: Option<i64>,

    #[serde(default)]
    pub latest_version: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CargoAiMetadata {
    #[serde(default)]
    pub cargo_ai_version: Option<String>,

    // Agent-definition schema version (format: YYYY-MM-DD.rN), decoupled from cargo-ai semver.
    #[serde(default)]
    pub template_schema_version: Option<String>,

    #[serde(default)]
    pub cargo_ai_build_target: Option<String>,

    #[serde(default)]
    pub cargo_ai_install_id: Option<String>,

    #[serde(default)]
    pub cargo_ai_binary_sha256: Option<String>,
}

fn default_timeout() -> u64 {
    60
}
