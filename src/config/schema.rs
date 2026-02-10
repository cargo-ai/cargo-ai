// This module defines the configuration schema for Cargo-AI.
// Originally named `profile.rs`, it was renamed to `schema.rs` to better reflect
// its future role in housing additional sections beyond profiles, such as
// defaults or user tokens for Cargo-AI.org. For now, it only includes profiles.

#![allow(dead_code)]

use serde::{Serialize, Deserialize};

#[derive(Debug, Serialize, Deserialize)]
pub struct Config {
    pub profile: Vec<Profile>,

    // Reserved for future install or account identification and management (currently unused).
    #[serde(default)]
    pub cargo_ai_token: Option<String>,

    #[serde(default)]
    pub default_profile: Option<String>,

    #[serde(default)]
    pub account: Option<Account>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Profile {
    pub name: String,
    pub server: String,
    pub model: String,

    #[serde(default)]
    pub url: Option<String>, 

    #[serde(default)]
    pub token: Option<String>,

    #[serde(default = "default_timeout")]
    pub timeout_in_sec: u64,

    #[serde(default)]
    pub description: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Account {
    #[serde(default)]
    pub email: Option<String>,

    #[serde(default)]
    pub access_token: Option<String>,

    #[serde(default)]
    pub refresh_token: Option<String>,

    #[serde(default)]
    pub access_token_expires_in: Option<i32>,

    // Unix epoch seconds when the access token was last obtained.
    #[serde(default)]
    pub access_token_issued_at: Option<i64>,
}

fn default_timeout() -> u64 {
    60
}
