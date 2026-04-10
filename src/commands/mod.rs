//! Command execution modules for the `cargo-ai` CLI.
//!
//! Each submodule owns one command area and keeps `main.rs` dispatch-only.
pub mod account;
pub mod add;
pub mod auth;
pub mod credentials;
pub mod definition_source;
pub mod hatch;
pub mod hatch_pipeline;
pub mod init;
pub mod new;
pub mod preflight;
pub mod preflight_actions;
pub mod profile;
pub mod run;
pub mod scaffold;
pub mod version;
