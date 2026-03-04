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

    #[serde(default)]
    pub cargo_ai_token: Option<String>,

    #[serde(default)]
    pub default_profile: Option<String>,

    #[serde(default)]
    pub secret_store: Option<SecretStoreMode>,

    #[serde(default)]
    pub openai_auth: Option<OpenAiAuth>,

    #[serde(default)]
    pub web_resources: Option<WebResources>,
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

fn default_timeout() -> u64 {
    60
}
