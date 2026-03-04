//! Command execution modules for the `cargo-ai` CLI.
//!
//! Each submodule owns one command area and keeps `main.rs` dispatch-only.
pub mod account;
pub mod hatch;
pub mod hatch_pipeline;
pub mod init;
pub mod new;
pub mod preflight;
pub mod preflight_actions;
pub mod profile;
pub mod scaffold;
pub mod settings;
pub mod shipyard;
pub mod version;
