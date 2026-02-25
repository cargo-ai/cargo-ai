// src/infra_api/account.rs
//
// Account operations against cargo-ai-infra.
// Keep this module free of CLI concerns so it can be extracted into a library later.

pub mod register;
pub mod confirm;
pub mod status;
pub mod handle;
pub mod agents;
pub mod send_mail;
pub mod mail_preferences;
