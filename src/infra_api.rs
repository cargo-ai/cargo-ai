// src/infra_api.rs
//
// Reusable client surface for communicating with cargo-ai-infra (api.cargo-ai.org).
// Keep this module free of CLI concepts (no clap, no printing) so it can be extracted
// into a standalone library later.

pub mod account;
// pub mod public; // placeholder for later