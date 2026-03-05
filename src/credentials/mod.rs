//! Secret-material storage for Cargo-AI runtime credentials.
//!
//! This module keeps secret values out of `config.toml` and routes reads/writes
//! through an explicit secret-store mode (`file` or `keychain`) with legacy
//! read compatibility for older configs that do not yet set a mode.

pub mod migration;
pub mod openai_oauth;
pub mod store;
