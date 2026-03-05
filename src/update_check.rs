//! Update-check policy and crates.io version lookup for `cargo-ai`.
//!
//! Phase 1 scope:
//! - Explicit update modes (`check` / `off`)
//! - 24-hour throttled background checks
//! - `cargo ai version --check` forced checks
//! - persisted local state in `config.toml`
use crate::config::loader::{config_path, load_config};
use crate::config::schema::{default_secret_store_mode, Config, UpdateCheck as UpdateCheckConfig};
use reqwest::header::{ACCEPT, USER_AGENT};
use semver::Version;
use serde::Deserialize;
use std::fs;
use std::time::{SystemTime, UNIX_EPOCH};

const CRATES_IO_BASE_URL: &str = "https://crates.io";
const CRATE_NAME: &str = "cargo-ai";
pub const UPDATE_CHECK_TTL_SECONDS: i64 = 24 * 60 * 60;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpdateMode {
    Check,
    Off,
}

impl UpdateMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Check => "check",
            Self::Off => "off",
        }
    }

    pub fn from_config_value(value: Option<&str>) -> Self {
        match value.map(str::trim).map(str::to_ascii_lowercase).as_deref() {
            Some("off") => Self::Off,
            _ => Self::Check,
        }
    }

    pub fn from_cli_value(value: &str) -> Self {
        match value.trim().to_ascii_lowercase().as_str() {
            "off" => Self::Off,
            _ => Self::Check,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VersionStatus {
    UpToDate { installed: String, latest: String },
    UpdateAvailable { installed: String, latest: String },
    UnknownVersionFormat { installed: String, latest: String },
}

#[derive(Debug, Clone)]
struct PersistedState {
    mode: UpdateMode,
    last_checked_unix_seconds: Option<i64>,
    latest_version: Option<String>,
}

#[derive(Debug, Deserialize)]
struct CratesIoResponse {
    #[serde(rename = "crate")]
    crate_info: CratesIoCrate,
}

#[derive(Debug, Deserialize)]
struct CratesIoCrate {
    #[serde(default)]
    max_version: Option<String>,

    #[serde(default)]
    max_stable_version: Option<String>,
}

fn default_config() -> Config {
    Config {
        profile: Vec::new(),
        cargo_ai_token: None,
        default_profile: None,
        secret_store: Some(default_secret_store_mode()),
        account: None,
        openai_auth: None,
        web_resources: None,
        update_check: None,
        cargo_ai_metadata: None,
    }
}

fn now_unix_seconds() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn load_state() -> PersistedState {
    let cfg = load_config();
    let update = cfg.as_ref().and_then(|c| c.update_check.as_ref());

    PersistedState {
        mode: UpdateMode::from_config_value(update.and_then(|u| u.mode.as_deref())),
        last_checked_unix_seconds: update.and_then(|u| u.last_checked_unix_seconds),
        latest_version: update.and_then(|u| u.latest_version.clone()),
    }
}

fn write_config(cfg: &Config) -> Result<(), String> {
    let path = config_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| {
            format!(
                "Failed to create config directory '{}': {e}",
                parent.display()
            )
        })?;
    }

    let serialized =
        toml::to_string_pretty(cfg).map_err(|e| format!("Failed to serialize config: {e}"))?;
    fs::write(&path, serialized)
        .map_err(|e| format!("Failed to write config '{}': {e}", path.display()))
}

fn persist_state(
    mode: UpdateMode,
    last_checked_unix_seconds: Option<i64>,
    latest_version: Option<String>,
) -> Result<(), String> {
    let mut cfg = load_config().unwrap_or_else(default_config);
    let update = cfg.update_check.get_or_insert(UpdateCheckConfig {
        mode: None,
        last_checked_unix_seconds: None,
        latest_version: None,
    });

    update.mode = Some(mode.as_str().to_string());
    update.last_checked_unix_seconds = last_checked_unix_seconds;
    update.latest_version = latest_version;

    write_config(&cfg)
}

fn ttl_expired(last_checked_unix_seconds: Option<i64>, now: i64) -> bool {
    match last_checked_unix_seconds {
        None => true,
        Some(last) => now.saturating_sub(last) >= UPDATE_CHECK_TTL_SECONDS,
    }
}

fn compare_versions(installed: &str, latest: &str) -> VersionStatus {
    let installed_version = Version::parse(installed);
    let latest_version = Version::parse(latest);

    match (installed_version, latest_version) {
        (Ok(installed_parsed), Ok(latest_parsed)) => {
            if latest_parsed > installed_parsed {
                VersionStatus::UpdateAvailable {
                    installed: installed.to_string(),
                    latest: latest.to_string(),
                }
            } else {
                VersionStatus::UpToDate {
                    installed: installed.to_string(),
                    latest: latest.to_string(),
                }
            }
        }
        _ => VersionStatus::UnknownVersionFormat {
            installed: installed.to_string(),
            latest: latest.to_string(),
        },
    }
}

async fn fetch_latest_version_from_base(base_url: &str) -> Result<String, String> {
    let url = format!(
        "{}/api/v1/crates/{}",
        base_url.trim_end_matches('/'),
        CRATE_NAME
    );

    let response = reqwest::Client::new()
        .get(&url)
        .header(USER_AGENT, update_check_user_agent())
        .header(ACCEPT, "application/json")
        .send()
        .await
        .map_err(|e| format!("Request failed: {e}"))?;

    if !response.status().is_success() {
        return Err(format!(
            "Request to crates.io failed with status {}.",
            response.status()
        ));
    }

    let payload = response
        .json::<CratesIoResponse>()
        .await
        .map_err(|e| format!("Invalid crates.io response: {e}"))?;

    if let Some(version) = payload
        .crate_info
        .max_version
        .as_deref()
        .map(str::trim)
        .filter(|v| !v.is_empty())
    {
        return Ok(version.to_string());
    }

    if let Some(version) = payload
        .crate_info
        .max_stable_version
        .as_deref()
        .map(str::trim)
        .filter(|v| !v.is_empty())
    {
        return Ok(version.to_string());
    }

    Err("crates.io response did not include max version metadata.".to_string())
}

fn update_check_user_agent() -> String {
    format!(
        "cargo-ai/{} (+https://cargo-ai.org)",
        env!("CARGO_PKG_VERSION")
    )
}

async fn fetch_latest_version() -> Result<String, String> {
    fetch_latest_version_from_base(CRATES_IO_BASE_URL).await
}

pub fn set_update_mode(mode: UpdateMode) -> Result<(), String> {
    let state = load_state();
    persist_state(mode, state.last_checked_unix_seconds, state.latest_version)
}

pub async fn force_check_and_persist() -> Result<VersionStatus, String> {
    let state = load_state();
    let latest = fetch_latest_version().await?;
    let now = now_unix_seconds();

    persist_state(state.mode, Some(now), Some(latest.clone()))?;
    Ok(compare_versions(env!("CARGO_PKG_VERSION"), &latest))
}

pub async fn maybe_run_background_check(skip_for_invocation: bool) {
    if skip_for_invocation {
        return;
    }

    let state = load_state();
    if state.mode == UpdateMode::Off {
        return;
    }

    let now = now_unix_seconds();
    let mut latest_known_version = state.latest_version.clone();

    if ttl_expired(state.last_checked_unix_seconds, now) {
        match fetch_latest_version().await {
            Ok(latest) => {
                latest_known_version = Some(latest.clone());
                if let Err(error) = persist_state(state.mode, Some(now), Some(latest)) {
                    eprintln!("⚠️ Failed to persist update-check state: {error}");
                }
            }
            Err(_) => {
                // Keep command behavior non-blocking and throttle retry attempts by
                // persisting last-check timestamp even when the request fails.
                let _ = persist_state(state.mode, Some(now), state.latest_version.clone());
            }
        }
    }

    if let Some(latest) = latest_known_version {
        if let VersionStatus::UpdateAvailable { installed, latest } =
            compare_versions(env!("CARGO_PKG_VERSION"), &latest)
        {
            eprintln!(
                "⚠️ Update available for cargo-ai: {installed} -> {latest}. Run `cargo install cargo-ai --locked` to update."
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        compare_versions, fetch_latest_version_from_base, ttl_expired, UpdateMode, VersionStatus,
        UPDATE_CHECK_TTL_SECONDS,
    };

    #[test]
    fn update_mode_defaults_to_check_for_missing_or_unknown_values() {
        assert_eq!(UpdateMode::from_config_value(None), UpdateMode::Check);
        assert_eq!(
            UpdateMode::from_config_value(Some("unexpected-mode")),
            UpdateMode::Check
        );
    }

    #[test]
    fn update_mode_parses_off_value() {
        assert_eq!(UpdateMode::from_config_value(Some("off")), UpdateMode::Off);
        assert_eq!(UpdateMode::from_cli_value("off"), UpdateMode::Off);
        assert_eq!(UpdateMode::from_cli_value("check"), UpdateMode::Check);
    }

    #[test]
    fn ttl_check_behaves_deterministically() {
        let now = 2_000_000_i64;
        assert!(ttl_expired(None, now));
        assert!(!ttl_expired(Some(now - UPDATE_CHECK_TTL_SECONDS + 1), now));
        assert!(ttl_expired(Some(now - UPDATE_CHECK_TTL_SECONDS), now));
    }

    #[test]
    fn version_compare_identifies_update_and_up_to_date() {
        assert!(matches!(
            compare_versions("0.0.10", "0.0.11"),
            VersionStatus::UpdateAvailable { .. }
        ));
        assert!(matches!(
            compare_versions("0.0.11", "0.0.11"),
            VersionStatus::UpToDate { .. }
        ));
    }

    #[test]
    fn version_compare_handles_unparseable_versions() {
        assert!(matches!(
            compare_versions("0.0.10", "not-a-version"),
            VersionStatus::UnknownVersionFormat { .. }
        ));
    }

    #[tokio::test]
    async fn fetch_latest_version_uses_max_version_field() {
        let mut server = mockito::Server::new_async().await;
        let _mock = server
            .mock("GET", "/api/v1/crates/cargo-ai")
            .match_header("user-agent", mockito::Matcher::Regex("^cargo-ai/".into()))
            .match_header("accept", mockito::Matcher::Regex("application/json".into()))
            .with_status(200)
            .with_body(
                r#"{"crate":{"id":"cargo-ai","max_version":"0.0.11","max_stable_version":"0.0.10"}}"#,
            )
            .create_async()
            .await;

        let latest = fetch_latest_version_from_base(&server.url())
            .await
            .expect("mock response should parse");

        assert_eq!(latest, "0.0.11");
    }
}
