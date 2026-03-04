// This module defines the configuration schema for Cargo-AI.
// Originally named `profile.rs`, it was renamed to `schema.rs` to better reflect
// its future role in housing additional sections beyond profiles, such as
// defaults or user tokens for Cargo-AI.org. For now, it only includes profiles.

#![allow(dead_code)]

use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
pub struct Config {
    pub profile: Vec<Profile>,

    #[serde(default)]
    pub cargo_ai_token: Option<String>,

    #[serde(default)]
    pub default_profile: Option<String>,

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
