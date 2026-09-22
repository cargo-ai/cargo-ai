//! Standalone backup uses the same existing account credential backend as the agent.
use crate::usage_backup::Auth;
pub(crate) fn load_auth() -> Result<Auth, String> {
    let mode = crate::config::loader::load_config().and_then(|config| config.secret_store);
    let tokens = crate::load_account_auth(mode)
        .map_err(|_| "Cannot read account credentials for usage backup")?;
    Ok(Auth {
        access_token: tokens.access_token,
        refresh_token: tokens.refresh_token,
    })
}
pub(crate) fn save_settings(_: &crate::usage_store::UsageSettings) -> Result<(), String> {
    Err("Manage usage backup consent through `cargo ai usage backup`".into())
}
