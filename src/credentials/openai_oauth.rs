//! OpenAI account-session helpers backed by Codex local auth storage.
//!
//! Cargo AI intentionally treats Codex auth state as the source of truth for
//! `openai_account` mode and does not import/persist duplicated OpenAI account
//! session tokens in Cargo AI credential backends.

use crate::config::loader::load_config;
use crate::config::settings as config_settings;
use crate::credentials::store;
use serde_json::Value;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

pub const OPENAI_ACCOUNT_RESPONSES_URL: &str = "https://chatgpt.com/backend-api/codex/responses";
pub const OPENAI_REFRESH_BUFFER_SEC: i64 = 30;
pub(crate) const MAX_AUTH_BYTES: usize = 64 * 1024;
pub(crate) const MAX_TOKEN_BYTES: usize = 16 * 1024;
pub(crate) const MAX_ACCOUNT_ID_BYTES: usize = 256;

#[derive(Clone)]
pub struct CodexSession {
    pub access_token: String,
    pub account_id: Option<String>,
    pub refresh_token: Option<String>,
    pub access_token_expires_at_unix: Option<i64>,
}

#[derive(Clone)]
pub struct ResolvedSession {
    pub access_token: String,
    pub account_id: Option<String>,
}

pub(crate) fn now_unix_seconds() -> i64 {
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

impl std::fmt::Debug for CodexSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CodexSession")
            .field("access_token", &"[redacted]")
            .field("account_id_present", &self.account_id.is_some())
            .field("refresh_token_present", &self.refresh_token.is_some())
            .field(
                "access_token_expires_at_unix",
                &self.access_token_expires_at_unix,
            )
            .finish()
    }
}
impl std::fmt::Debug for ResolvedSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ResolvedSession")
            .field("access_token", &"[redacted]")
            .field("account_id_present", &self.account_id.is_some())
            .finish()
    }
}

pub(crate) fn bounded_header(value: &str, limit: usize) -> bool {
    !value.is_empty() && value.len() <= limit && value.bytes().all(|byte| byte.is_ascii_graphic())
}

pub(crate) fn read_auth_snapshot(path: &Path) -> Result<Option<String>, String> {
    match fs::metadata(path) {
        Ok(metadata) => {
            if !metadata.is_file() {
                return Err("Codex auth storage is not a supported regular file.".into());
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err("Codex auth storage could not be read.".into()),
    }
    let file =
        fs::File::open(path).map_err(|_| "Codex auth storage could not be read.".to_string())?;
    let mut bytes = Vec::new();
    file.take((MAX_AUTH_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|_| "Codex auth storage could not be read.".to_string())?;
    if bytes.len() > MAX_AUTH_BYTES {
        return Err("Codex auth storage exceeds the 64 KiB limit.".into());
    }
    String::from_utf8(bytes)
        .map(Some)
        .map_err(|_| "Codex auth storage is not UTF-8 JSON.".into())
}

pub fn parse_codex_auth_payload(raw: &str) -> Result<CodexSession, String> {
    if raw.len() > MAX_AUTH_BYTES {
        return Err("Codex auth storage exceeds the 64 KiB limit.".into());
    }
    let parsed = serde_json::from_str::<Value>(raw)
        .map_err(|_| "Codex auth JSON is invalid. Re-run `codex login`.".to_string())?;

    let tokens = parsed.get("tokens").ok_or_else(|| {
        "Codex auth payload did not include a `tokens` object. Re-run `codex login`.".to_string()
    })?;

    let access_token = parse_access_token(tokens)?;
    if !bounded_header(&access_token, MAX_TOKEN_BYTES) {
        return Err("Codex access token is invalid bounded credential input.".into());
    }
    let account_id = match tokens.get("account_id") {
        None | Some(Value::Null) => None,
        Some(Value::String(value)) if bounded_header(value, MAX_ACCOUNT_ID_BYTES) => {
            Some(value.clone())
        }
        _ => return Err("Codex selected account context is invalid.".into()),
    };
    let refresh_token = parse_non_empty_token(tokens, "refresh_token");
    let access_token_expires_at_unix = parse_access_token_expires_at_unix(tokens);

    Ok(CodexSession {
        access_token,
        account_id,
        refresh_token,
        access_token_expires_at_unix,
    })
}

pub fn load_codex_session() -> Result<Option<CodexSession>, String> {
    let path = codex_auth_path()?;
    read_auth_snapshot(&path)?
        .map(|raw| parse_codex_auth_payload(&raw))
        .transpose()
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
            "OpenAI account auth is logged out for Cargo AI locally. Run `cargo ai auth login openai` to re-enable, or use `cargo ai profile set <name> --token <TOKEN> --auth api_key`."
                .to_string(),
        );
    }

    let Some(session) = load_codex_session()? else {
        return Err(
            "OpenAI authentication is missing. Install Codex and run `codex login`, or use `cargo ai profile set <name> --token <TOKEN> --auth api_key`."
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
        account_id: session.account_id,
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

#[cfg(test)]
mod bounded_session_tests {
    use super::*;
    #[test]
    fn runtime_legacy_session_and_optional_selected_context_remain_compatible() {
        let legacy =
            parse_codex_auth_payload(r#"{"tokens":{"access_token":"legacy-token"}}"#).unwrap();
        assert_eq!(legacy.account_id, None);
        assert!(!access_token_expired_or_near(
            legacy.access_token_expires_at_unix,
            now_unix_seconds()
        ));
        let selected = parse_codex_auth_payload(
            r#"{"tokens":{"access_token":"legacy-token","account_id":"synthetic-workspace"}}"#,
        )
        .unwrap();
        assert_eq!(selected.account_id.as_deref(), Some("synthetic-workspace"));
        let debug = format!("{selected:?}");
        assert!(!debug.contains("legacy-token"));
        assert!(!debug.contains("synthetic-workspace"));
        for account in ["", "bad\nheader", "nonascii-é"] {
            let error = parse_codex_auth_payload(
                &serde_json::json!({"tokens":{"access_token":"legacy-token","account_id":account}})
                    .to_string(),
            )
            .unwrap_err();
            assert_eq!(error, "Codex selected account context is invalid.");
        }
        assert!(parse_codex_auth_payload(
            &serde_json::json!({"tokens":{"access_token":"x".repeat(MAX_TOKEN_BYTES+1)}})
                .to_string()
        )
        .is_err());
        assert!(!parse_codex_auth_payload("{secret-invalid-json")
            .unwrap_err()
            .contains("secret-invalid-json"));
    }
    #[test]
    fn auth_snapshot_is_bounded_read_only_and_preserves_selected_links() {
        let root =
            std::env::temp_dir().join(format!("cargo-ai-session-bound-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&root).unwrap();
        let path = root.join("auth.json");
        assert_eq!(read_auth_snapshot(&path).unwrap(), None);
        std::fs::write(&path, vec![b'x'; MAX_AUTH_BYTES + 1]).unwrap();
        assert!(read_auth_snapshot(&path).unwrap_err().contains("64 KiB"));
        assert_eq!(
            std::fs::metadata(&path).unwrap().len(),
            (MAX_AUTH_BYTES + 1) as u64
        );
        #[cfg(unix)]
        {
            let link = root.join("linked.json");
            std::os::unix::fs::symlink(&path, &link).unwrap();
            assert!(read_auth_snapshot(&link).unwrap_err().contains("64 KiB"));
            let raw = r#"{"tokens":{"access_token":"synthetic-linked-token"}}"#;
            std::fs::write(&path, raw).unwrap();
            assert_eq!(read_auth_snapshot(&link).unwrap().as_deref(), Some(raw));
            assert!(std::fs::symlink_metadata(&link)
                .unwrap()
                .file_type()
                .is_symlink());
            let linked_home = root.join("linked-home");
            std::os::unix::fs::symlink(&root, &linked_home).unwrap();
            assert_eq!(
                read_auth_snapshot(&linked_home.join("auth.json"))
                    .unwrap()
                    .as_deref(),
                Some(raw)
            );
            assert_eq!(std::fs::read_to_string(&path).unwrap(), raw);
        }
        std::fs::remove_dir_all(root).unwrap();
    }
}
