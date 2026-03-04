//! OpenAI OAuth session resolution helpers.
//!
//! This module keeps OpenAI account-session behavior deterministic and shared
//! across command surfaces that need runtime credentials.

use crate::config::loader::load_config;
use crate::config::settings as config_settings;
use crate::credentials::store;
use crate::infra_api;

pub const OPENAI_INFRA_BASE_URL: &str = "https://api.cargo-ai.org";
pub const OPENAI_REFRESH_BUFFER_SEC: i64 = 30;

#[derive(Debug, Clone, Copy, Default)]
pub struct SessionMetadata {
    pub access_token_expires_in: Option<i32>,
    pub access_token_issued_at: Option<i64>,
}

#[derive(Debug, Clone)]
pub struct ResolvedSession {
    pub access_token: String,
    pub refreshed: bool,
}

fn now_unix_seconds() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or(0)
}

pub fn load_metadata() -> SessionMetadata {
    load_config()
        .and_then(|cfg| cfg.openai_auth)
        .map(|openai_auth| SessionMetadata {
            access_token_expires_in: openai_auth.access_token_expires_in,
            access_token_issued_at: openai_auth.access_token_issued_at,
        })
        .unwrap_or_default()
}

pub fn expires_at_unix(metadata: SessionMetadata) -> Option<i64> {
    let issued_at = metadata.access_token_issued_at?;
    let expires_in = metadata.access_token_expires_in? as i64;
    if issued_at <= 0 || expires_in <= 0 {
        return None;
    }

    Some(issued_at.saturating_add(expires_in))
}

pub fn token_expired_or_near(metadata: SessionMetadata, now: i64) -> bool {
    match expires_at_unix(metadata) {
        Some(expires_at) => expires_at.saturating_sub(OPENAI_REFRESH_BUFFER_SEC) <= now,
        None => false,
    }
}

pub async fn resolve_session_for_runtime() -> Result<ResolvedSession, String> {
    let Some(tokens) = store::load_openai_oauth_tokens()
        .map_err(|error| format!("failed to load OpenAI OAuth session: {error}"))?
    else {
        return Err(
            "OpenAI account session is not available. Run `cargo ai auth login openai` or use `profile token set` for api_key mode."
                .to_string(),
        );
    };

    let mut access_token = tokens.access_token;
    let refresh_token = tokens.refresh_token;
    let mut refreshed = false;

    let now = now_unix_seconds();
    let metadata = load_metadata();

    if token_expired_or_near(metadata, now) {
        let Some(existing_refresh_token) = refresh_token.as_deref() else {
            return Err(
                "OpenAI account session has expired and no refresh token is available. Re-run `cargo ai auth login openai`."
                    .to_string(),
            );
        };

        let refreshed_session = infra_api::auth::openai::session_status(
            OPENAI_INFRA_BASE_URL,
            access_token.as_str(),
            Some(existing_refresh_token),
        )
        .await
        .map_err(|error| format!("failed to refresh OpenAI session: {error}"))?;

        let session = refreshed_session.session.ok_or_else(|| {
            refreshed_session.message.unwrap_or_else(|| {
                "OpenAI refresh response did not include session details".to_string()
            })
        })?;

        let new_access_token = session
            .access_token
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| {
                "OpenAI refresh response did not include a non-empty access token".to_string()
            })?
            .to_string();

        let new_refresh_token = session
            .refresh_token
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
            .or(refresh_token.clone());

        let new_expires_in = session.expires_in_seconds;
        store::store_openai_oauth_tokens(&new_access_token, new_refresh_token.as_deref())
            .map_err(|error| format!("failed to persist refreshed OpenAI session: {error}"))?;

        if new_expires_in.is_some() {
            config_settings::set_openai_auth_metadata(new_expires_in, Some(now_unix_seconds()))
                .map_err(|error| {
                    format!("failed to persist refreshed OpenAI session metadata: {error}")
                })?;
        }

        access_token = new_access_token;
        refreshed = true;
    }

    Ok(ResolvedSession {
        access_token,
        refreshed,
    })
}

pub fn clear_local_session() -> Result<(), String> {
    store::clear_openai_oauth_tokens()
        .map_err(|error| format!("failed to clear OpenAI session secrets: {error}"))?;
    config_settings::clear_openai_auth_metadata()
        .map_err(|error| format!("failed to clear OpenAI session metadata: {error}"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{expires_at_unix, token_expired_or_near, SessionMetadata};

    #[test]
    fn expires_at_unix_returns_none_without_complete_metadata() {
        let metadata = SessionMetadata {
            access_token_expires_in: Some(3600),
            access_token_issued_at: None,
        };
        assert_eq!(expires_at_unix(metadata), None);
    }

    #[test]
    fn expires_at_unix_returns_expected_value() {
        let metadata = SessionMetadata {
            access_token_expires_in: Some(3600),
            access_token_issued_at: Some(100),
        };
        assert_eq!(expires_at_unix(metadata), Some(3700));
    }

    #[test]
    fn token_expired_or_near_applies_safety_buffer() {
        let metadata = SessionMetadata {
            access_token_expires_in: Some(60),
            access_token_issued_at: Some(100),
        };

        // expiry is 160, safety buffer is 30, so threshold is 130
        assert!(!token_expired_or_near(metadata, 129));
        assert!(token_expired_or_near(metadata, 130));
    }
}
