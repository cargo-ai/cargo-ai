//! Backup integration with the CLI's existing credential and configuration stores.
use crate::usage_backup::Auth;
pub(crate) fn load_auth() -> Result<Auth, String> {
    let tokens = crate::credentials::store::load_account_tokens()
        .map_err(|_| "Cannot read account credentials for usage backup")?
        .ok_or("Sign in explicitly before using cloud backup")?;
    Ok(Auth {
        access_token: tokens.access_token,
        refresh_token: tokens.refresh_token,
    })
}
pub(crate) fn save_settings(settings: &crate::usage_store::UsageSettings) -> Result<(), String> {
    crate::config::settings::mutate_usage_backup_settings(settings)
}
