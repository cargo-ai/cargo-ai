// src/infra_api/account.rs
//
// Account operations against cargo-ai-infra.
// Keep this module free of CLI concerns so it can be extracted into a library later.

pub mod agents;
pub mod confirm;
pub mod handle;
pub mod mail_preferences;
pub mod register;
pub mod send_mail;
pub mod status;
