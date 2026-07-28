//! Update-check policy and crates.io version lookup for `cargo-ai`.
//!
//! Responsibilities:
//! - Explicit update modes (`check` / `off`)
//! - 24-hour throttled background checks
//! - `cargo ai version --check` forced checks
//! - persisted local state in `config.toml`
use crate::config::loader::{config_path, load_config_from_path, ConfigLoad};
use crate::config::storage::persist_section_fields;
#[cfg(test)]
use crate::config::storage::persist_section_fields_at;
use reqwest::header::{ACCEPT, USER_AGENT};
use semver::Version;
use serde::Deserialize;
use std::path::Path;
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

fn now_unix_seconds() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn load_state() -> Result<PersistedState, String> {
    load_state_at(&config_path())
}

fn load_state_at(path: &Path) -> Result<PersistedState, String> {
    let loaded = load_config_from_path(path).map_err(|error| error.to_string())?;
    let update = match &loaded {
        ConfigLoad::Missing => None,
        ConfigLoad::Loaded(loaded) => loaded.config().update_check.as_ref(),
    };

    Ok(PersistedState {
        mode: UpdateMode::from_config_value(update.and_then(|u| u.mode.as_deref())),
        last_checked_unix_seconds: update.and_then(|u| u.last_checked_unix_seconds),
        latest_version: update.and_then(|u| u.latest_version.clone()),
    })
}

fn persist_update_mode(mode: UpdateMode) -> Result<(), String> {
    persist_section_fields(
        "update_check",
        &[("mode", Some(toml::Value::String(mode.as_str().to_string())))],
    )?;
    Ok(())
}

fn persist_check_result(
    last_checked_unix_seconds: i64,
    latest_version: Option<String>,
) -> Result<(), String> {
    persist_check_result_with(
        |fields| persist_section_fields("update_check", fields).map(|_| ()),
        last_checked_unix_seconds,
        latest_version,
    )
}

fn persist_check_result_with<Persist>(
    persist: Persist,
    last_checked_unix_seconds: i64,
    latest_version: Option<String>,
) -> Result<(), String>
where
    Persist: FnOnce(&[(&str, Option<toml::Value>)]) -> Result<(), String>,
{
    let checked = (
        "last_checked_unix_seconds",
        Some(toml::Value::Integer(last_checked_unix_seconds)),
    );
    match latest_version {
        Some(latest_version) => persist(&[
            checked,
            ("latest_version", Some(toml::Value::String(latest_version))),
        ]),
        None => persist(&[checked]),
    }
}

#[cfg(test)]
fn persist_check_result_at(
    path: &Path,
    last_checked_unix_seconds: i64,
    latest_version: Option<String>,
) -> Result<(), String> {
    persist_check_result_with(
        |fields| persist_section_fields_at(path, "update_check", fields).map(|_| ()),
        last_checked_unix_seconds,
        latest_version,
    )
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
    persist_update_mode(mode)
}

pub async fn force_check_and_persist() -> Result<VersionStatus, String> {
    load_state()?;
    let latest = fetch_latest_version().await?;
    let now = now_unix_seconds();

    persist_check_result(now, Some(latest.clone()))?;
    Ok(compare_versions(env!("CARGO_PKG_VERSION"), &latest))
}

pub async fn maybe_run_background_check(skip_for_invocation: bool) {
    if skip_for_invocation {
        return;
    }

    let state = match load_state() {
        Ok(state) => state,
        Err(error) => {
            eprintln!("Warning: update check skipped because {error}");
            return;
        }
    };
    if state.mode == UpdateMode::Off {
        return;
    }

    let now = now_unix_seconds();
    let mut latest_known_version = state.latest_version.clone();

    if ttl_expired(state.last_checked_unix_seconds, now) {
        match fetch_latest_version().await {
            Ok(latest) => {
                latest_known_version = Some(latest.clone());
                if let Err(error) = persist_check_result(now, Some(latest)) {
                    eprintln!("⚠️ Failed to persist update-check state: {error}");
                }
            }
            Err(_) => {
                // Keep command behavior non-blocking and throttle retry attempts by
                // persisting last-check timestamp even when the request fails.
                if let Err(error) = persist_check_result(now, None) {
                    eprintln!("Warning: failed to persist update-check state: {error}");
                }
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
        compare_versions, fetch_latest_version_from_base, load_state_at, persist_check_result_at,
        ttl_expired, UpdateMode, VersionStatus, UPDATE_CHECK_TTL_SECONDS,
    };
    use std::fs;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    const CURRENT_CARGO_AI_VERSION: &str = env!("CARGO_PKG_VERSION");
    const PREVIOUS_CARGO_AI_VERSION: &str = "0.0.11";

    fn temp_config_path(stem: &str) -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock should be valid")
            .as_nanos();
        std::env::temp_dir()
            .join(format!("cargo-ai-update-check-{stem}-{unique}"))
            .join("config.toml")
    }

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
    fn state_loader_propagates_malformed_config_error() {
        let path = temp_config_path("malformed");
        fs::create_dir_all(path.parent().expect("test path should have a parent"))
            .expect("test directory should be created");
        let original = "profile = [\n";
        fs::write(&path, original).expect("test config should be written");

        let error = load_state_at(&path).expect_err("malformed config should fail closed");

        assert!(error.contains(&path.display().to_string()));
        assert_eq!(
            fs::read_to_string(&path).expect("config should remain"),
            original
        );
        let _ = fs::remove_dir_all(path.parent().expect("test path should have a parent"));
    }

    #[test]
    fn state_writer_preserves_unknown_fields() {
        let path = temp_config_path("preserve");
        fs::create_dir_all(path.parent().expect("test path should have a parent"))
            .expect("test directory should be created");
        fs::write(
            &path,
            "profile = []\nunknown = \"keep\"\n\n[update_check]\nmode = \"off\"\nfuture = 7\n",
        )
        .expect("test config should be written");

        persist_check_result_at(&path, 123, Some("1.2.3".to_string()))
            .expect("state should persist");

        let written = fs::read_to_string(&path).expect("config should be readable");
        assert!(written.contains("unknown = \"keep\""));
        assert!(written.contains("future = 7"));
        assert!(written.contains("mode = \"off\""));
        assert!(written.contains("last_checked_unix_seconds = 123"));
        assert!(written.contains("latest_version = \"1.2.3\""));
        let _ = fs::remove_dir_all(path.parent().expect("test path should have a parent"));
    }

    #[test]
    fn version_compare_identifies_update_and_up_to_date() {
        assert!(matches!(
            compare_versions(PREVIOUS_CARGO_AI_VERSION, CURRENT_CARGO_AI_VERSION),
            VersionStatus::UpdateAvailable { .. }
        ));
        assert!(matches!(
            compare_versions(CURRENT_CARGO_AI_VERSION, CURRENT_CARGO_AI_VERSION),
            VersionStatus::UpToDate { .. }
        ));
    }

    #[test]
    fn version_compare_handles_unparseable_versions() {
        assert!(matches!(
            compare_versions(CURRENT_CARGO_AI_VERSION, "not-a-version"),
            VersionStatus::UnknownVersionFormat { .. }
        ));
    }

    #[tokio::test]
    async fn fetch_latest_version_uses_max_version_field() {
        let mut server = mockito::Server::new_async().await;
        let response_body = format!(
            r#"{{"crate":{{"id":"cargo-ai","max_version":"{CURRENT_CARGO_AI_VERSION}","max_stable_version":"{PREVIOUS_CARGO_AI_VERSION}"}}}}"#
        );
        let _mock = server
            .mock("GET", "/api/v1/crates/cargo-ai")
            .match_header("user-agent", mockito::Matcher::Regex("^cargo-ai/".into()))
            .match_header("accept", mockito::Matcher::Regex("application/json".into()))
            .with_status(200)
            .with_body(response_body)
            .create_async()
            .await;

        let latest = fetch_latest_version_from_base(&server.url())
            .await
            .expect("mock response should parse");

        assert_eq!(latest, CURRENT_CARGO_AI_VERSION);
    }
}
