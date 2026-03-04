//! Secret-material storage for Cargo-AI runtime credentials.
//!
//! This module keeps secret values out of `config.toml` and routes reads/writes
//! through a keychain-first store with deterministic file fallback.

pub mod migration;
pub mod store;
