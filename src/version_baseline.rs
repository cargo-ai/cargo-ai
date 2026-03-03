//! Local Cargo-AI/tooling baseline persistence for generated-agent drift checks.
//!
//! Phase 2 scope:
//! - Persist current `cargo-ai` version in global config.
//! - Persist current template/schema version in global config.
//! - Keep behavior deterministic and local-only (no network calls).
use crate::config::loader::{config_path, load_config};
use crate::config::schema::{Config, VersionBaseline as VersionBaselineConfig};
use crate::schema_version;
use std::fs;
use std::path::Path;

fn default_config() -> Config {
    Config {
        profile: Vec::new(),
        cargo_ai_token: None,
        default_profile: None,
        account: None,
        web_resources: None,
        update_check: None,
        version_baseline: None,
    }
}

fn current_template_schema_version() -> String {
    schema_version::current_schema_version()
}

fn write_config_at_path(path: &Path, cfg: &Config) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| {
            format!(
                "Failed to create config directory '{}': {error}",
                parent.display()
            )
        })?;
    }

    let serialized = toml::to_string_pretty(cfg)
        .map_err(|error| format!("Failed to serialize config: {error}"))?;

    fs::write(path, serialized)
        .map_err(|error| format!("Failed to write config '{}': {error}", path.display()))
}

fn persist_baseline_in_config(
    cfg: &mut Config,
    cargo_ai_version: &str,
    template_schema_version: &str,
) {
    let baseline = cfg.version_baseline.get_or_insert(VersionBaselineConfig {
        cargo_ai_version: None,
        template_schema_version: None,
    });

    baseline.cargo_ai_version = Some(cargo_ai_version.to_string());
    baseline.template_schema_version = Some(template_schema_version.to_string());
}

fn persist_baseline_values(
    cargo_ai_version: &str,
    template_schema_version: &str,
) -> Result<(), String> {
    let mut cfg = load_config().unwrap_or_else(default_config);
    persist_baseline_in_config(&mut cfg, cargo_ai_version, template_schema_version);
    write_config_at_path(&config_path(), &cfg)
}

pub fn persist_current_baseline() -> Result<(), String> {
    persist_baseline_values(
        env!("CARGO_PKG_VERSION"),
        &current_template_schema_version(),
    )
}

#[cfg(test)]
mod tests {
    use super::{persist_baseline_in_config, write_config_at_path};
    use crate::config::schema::Config;
    use crate::schema_version;
    use std::fs;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn default_test_config() -> Config {
        Config {
            profile: Vec::new(),
            cargo_ai_token: None,
            default_profile: None,
            account: None,
            web_resources: None,
            update_check: None,
            version_baseline: None,
        }
    }

    fn temp_file_path(stem: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time should be after epoch")
            .as_nanos();
        std::env::temp_dir().join(format!("cargo-ai-version-baseline-{stem}-{nanos}.toml"))
    }

    #[test]
    fn template_schema_version_uses_date_revision_format_when_present() {
        let value = schema_version::extract_schema_version_from_agentcfg(
            r#"{"version":"2026-03-03.r2","prompt":"x","agent_schema":{"type":"object","properties":{}},"resource_urls":[],"actions":[]}"#,
        );
        assert_eq!(value.as_deref(), Some("2026-03-03.r2"));
    }

    #[test]
    fn template_schema_version_rejects_legacy_semver_values() {
        let value = schema_version::extract_schema_version_from_agentcfg(
            r#"{"version":"0.0.11","prompt":"x","agent_schema":{"type":"object","properties":{}},"resource_urls":[],"actions":[]}"#,
        );
        assert!(value.is_none());
    }

    #[test]
    fn template_schema_version_falls_back_to_current_schema_version_when_missing() {
        let value = schema_version::extract_schema_version_from_agentcfg(
            r#"{"prompt":"x","agent_schema":{"type":"object","properties":{}},"resource_urls":[],"actions":[]}"#,
        );
        assert!(value.is_none());
        let fallback = schema_version::current_schema_version();
        assert!(schema_version::is_valid_schema_version(&fallback));
    }

    #[test]
    fn persist_baseline_sets_both_versions_deterministically() {
        let mut cfg = default_test_config();

        persist_baseline_in_config(&mut cfg, "0.0.11", "2026-03-03.r1");
        let baseline = cfg
            .version_baseline
            .as_ref()
            .expect("version baseline should be initialized");

        assert_eq!(baseline.cargo_ai_version.as_deref(), Some("0.0.11"));
        assert_eq!(
            baseline.template_schema_version.as_deref(),
            Some("2026-03-03.r1")
        );
    }

    #[test]
    fn write_config_persists_version_baseline_section() {
        let mut cfg = default_test_config();
        persist_baseline_in_config(&mut cfg, "0.0.11", "2026-03-03.r1");

        let path = temp_file_path("write");
        write_config_at_path(&path, &cfg).expect("config should be written");

        let written = fs::read_to_string(&path).expect("written config should be readable");
        assert!(written.contains("version_baseline"));
        assert!(written.contains("cargo_ai_version = \"0.0.11\""));
        assert!(written.contains("template_schema_version = \"2026-03-03.r1\""));

        let _ = fs::remove_file(path);
    }
}
