//! OpenAI account-session helpers backed by Codex local auth storage.
//!
//! Cargo AI intentionally treats Codex auth state as the source of truth for
//! `openai_account` mode and does not import/persist duplicated OpenAI account
//! session tokens in Cargo AI credential backends.

use crate::config::settings as config_settings;
use crate::config::loader::load_config;
use crate::credentials::store;
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};

pub const OPENAI_ACCOUNT_RESPONSES_URL: &str = "https://chatgpt.com/backend-api/codex/responses";
pub const OPENAI_REFRESH_BUFFER_SEC: i64 = 30;

#[derive(Debug, Clone)]
pub struct CodexSession {
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub access_token_expires_at_unix: Option<i64>,
}

#[derive(Debug, Clone)]
pub struct ResolvedSession {
    pub access_token: String,
}

fn now_unix_seconds() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or(0)
}

fn parse_unix_timestamp(value: &Value) -> Option<i64> {
    if let Some(seconds) = value.as_i64() {
        return Some(if seconds > 1_000_000_000_000 {
            seconds / 1000
        } else {
            seconds
        });
    }

    value
        .as_str()
        .map(str::trim)
        .filter(|raw| !raw.is_empty())
        .and_then(|raw| raw.parse::<i64>().ok())
        .map(|seconds| {
            if seconds > 1_000_000_000_000 {
                seconds / 1000
            } else {
                seconds
            }
        })
}

fn parse_non_empty_token(container: &Value, key: &str) -> Option<String> {
    container
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|token| !token.is_empty())
        .map(str::to_string)
}

fn parse_access_token(tokens: &Value) -> Result<String, String> {
    parse_non_empty_token(tokens, "access_token").ok_or_else(|| {
        "Codex auth payload did not include a non-empty access token. Re-run `codex login`."
            .to_string()
    })
}

fn parse_access_token_expires_at_unix(tokens: &Value) -> Option<i64> {
    let keys = [
        "access_token_expires_at_unix",
        "access_token_expires_at",
        "expires_at",
        "expiresAt",
    ];

    for key in keys {
        if let Some(value) = tokens.get(key) {
            if let Some(parsed) = parse_unix_timestamp(value) {
                return Some(parsed);
            }
        }
    }

    None
}

pub fn codex_auth_path() -> Result<PathBuf, String> {
    if let Ok(codex_home) = std::env::var("CODEX_HOME") {
        let trimmed = codex_home.trim();
        if !trimmed.is_empty() {
            return Ok(PathBuf::from(trimmed).join("auth.json"));
        }
    }

    let home_dir = dirs::home_dir()
        .ok_or_else(|| "failed to resolve home directory for Codex auth lookup".to_string())?;
    Ok(home_dir.join(".codex").join("auth.json"))
}

pub fn parse_codex_auth_payload(raw: &str) -> Result<CodexSession, String> {
    let parsed = serde_json::from_str::<Value>(raw)
        .map_err(|error| format!("failed to parse Codex auth JSON: {error}"))?;

    let tokens = parsed.get("tokens").ok_or_else(|| {
        "Codex auth payload did not include a `tokens` object. Re-run `codex login`.".to_string()
    })?;

    let access_token = parse_access_token(tokens)?;
    let refresh_token = parse_non_empty_token(tokens, "refresh_token");
    let access_token_expires_at_unix = parse_access_token_expires_at_unix(tokens);

    Ok(CodexSession {
        access_token,
        refresh_token,
        access_token_expires_at_unix,
    })
}

fn load_codex_auth_tokens_from_file(path: &Path) -> Result<CodexSession, String> {
    let raw = fs::read_to_string(path)
        .map_err(|error| format!("failed to read '{}': {error}", path.display()))?;
    parse_codex_auth_payload(raw.as_str())
}

pub fn load_codex_session() -> Result<Option<CodexSession>, String> {
    let path = codex_auth_path()?;
    if !path.exists() {
        return Ok(None);
    }

    load_codex_auth_tokens_from_file(path.as_path()).map(Some)
}

pub fn access_token_expired_or_near(access_token_expires_at_unix: Option<i64>, now: i64) -> bool {
    match access_token_expires_at_unix {
        Some(expires_at) => expires_at.saturating_sub(OPENAI_REFRESH_BUFFER_SEC) <= now,
        None => false,
    }
}

pub fn openai_account_locally_disabled() -> bool {
    load_config()
        .and_then(|cfg| cfg.openai_auth)
        .and_then(|openai_auth| openai_auth.locally_disabled)
        .unwrap_or(false)
}

pub async fn resolve_session_for_runtime() -> Result<ResolvedSession, String> {
    if openai_account_locally_disabled() {
        return Err(
            "OpenAI account auth is logged out for Cargo AI locally. Run `cargo ai auth login openai` to re-enable, or use `profile token set` for api_key mode."
                .to_string(),
        );
    }

    let Some(session) = load_codex_session()? else {
        return Err(
            "OpenAI authentication is missing. Install Codex and run `codex login`, or use `profile token set` for api_key mode."
                .to_string(),
        );
    };

    if access_token_expired_or_near(session.access_token_expires_at_unix, now_unix_seconds()) {
        return Err(
            "OpenAI account session in Codex cache is expired or near expiry. Re-run `codex login`."
                .to_string(),
        );
    }

    Ok(ResolvedSession {
        access_token: session.access_token,
    })
}

pub fn clear_legacy_openai_session_tokens() {
    let _ = store::clear_openai_oauth_tokens();
}

pub fn clear_local_session() -> Result<(), String> {
    clear_legacy_openai_session_tokens();
    config_settings::clear_openai_auth_metadata()
        .map_err(|error| format!("failed to clear OpenAI session metadata: {error}"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{access_token_expired_or_near, parse_codex_auth_payload};

    #[test]
    fn parse_codex_auth_payload_extracts_tokens_and_expiry() {
        let payload = r#"{
            "tokens": {
                "access_token": "access-123",
                "refresh_token": "refresh-456",
                "expires_at": 1700000000
            }
        }"#;

        let parsed = parse_codex_auth_payload(payload).expect("payload should parse");
        assert_eq!(parsed.access_token, "access-123");
        assert_eq!(parsed.refresh_token.as_deref(), Some("refresh-456"));
        assert_eq!(parsed.access_token_expires_at_unix, Some(1700000000));
    }

    #[test]
    fn parse_codex_auth_payload_converts_millisecond_expiry() {
        let payload = r#"{
            "tokens": {
                "access_token": "access-123",
                "expires_at": 1700000000000
            }
        }"#;

        let parsed = parse_codex_auth_payload(payload).expect("payload should parse");
        assert_eq!(parsed.access_token_expires_at_unix, Some(1700000000));
    }

    #[test]
    fn parse_codex_auth_payload_rejects_missing_access_token() {
        let payload = r#"{
            "tokens": {
                "refresh_token": "refresh-only"
            }
        }"#;

        let err = parse_codex_auth_payload(payload).expect_err("missing access token must fail");
        assert!(err.contains("non-empty access token"));
    }

    #[test]
    fn access_token_expired_or_near_applies_safety_buffer() {
        // safety buffer is 30, threshold is 130
        assert!(!access_token_expired_or_near(Some(160), 129));
        assert!(access_token_expired_or_near(Some(160), 130));
    }
}
