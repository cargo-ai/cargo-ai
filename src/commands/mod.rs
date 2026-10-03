//! Command execution modules for the `cargo-ai` CLI.
//!
//! Each submodule owns one command area and keeps `main.rs` dispatch-only.
pub mod account;
pub(crate) mod action_artifacts;
pub mod add;
pub mod agents;
pub mod auth;
#[cfg(feature = "developer-tools")]
pub mod build;
pub(crate) mod client_actions;
pub mod credentials;
pub mod definition_source;
#[cfg(feature = "developer-tools")]
pub mod hatch;
#[cfg(feature = "developer-tools")]
mod hatch_compatibility;
pub mod hatch_pipeline;
pub mod init;
pub mod local_packages;
pub(crate) mod machine;
pub(crate) mod machine_process;
pub mod mail;
pub mod models;
pub mod new;
#[cfg(feature = "developer-tools")]
pub mod package;
pub(crate) mod package_dependencies;
pub(crate) mod package_inspection;
pub(crate) mod package_lock;
pub(crate) mod package_metadata;
#[cfg(feature = "developer-tools")]
pub(crate) mod package_publication;
pub mod packages;
pub mod profile;
pub mod requirements;
pub mod run;
pub mod runtime;
pub mod runtime_actions;
#[path = "../../templates/src/runtime_data.rs"]
pub(crate) mod runtime_data;
pub mod scaffold;
pub(crate) mod secret_input;
pub(crate) mod structured_results;
pub mod tools;
pub mod version;

pub(crate) mod usage;
