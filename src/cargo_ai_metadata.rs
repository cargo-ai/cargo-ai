//! Local Cargo-AI metadata persistence for generated-agent drift checks.
//!
//! Responsibilities:
//! - Persist current `cargo-ai` version and template/schema version in global config.
//! - Persist local install/build metadata for the current `cargo-ai` binary.
//! - Keep behavior deterministic and local-only (no network calls).
use crate::config::loader::{config_path, load_config, load_config_from_path, ConfigLoad};
use crate::config::schema::CargoAiMetadata as CargoAiMetadataConfig;
#[cfg(test)]
use crate::config::schema::Config;
use crate::config::storage::{persist_loaded_section_fields, persist_section_fields_at};
use crate::schema_version;
use sha2::{Digest, Sha256};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use uuid::Uuid;

fn current_template_schema_version() -> String {
    schema_version::current_schema_version()
}

pub fn current_build_target() -> String {
    let arch = std::env::consts::ARCH;
    let vendor = if cfg!(target_vendor = "apple") {
        "apple"
    } else if cfg!(target_vendor = "pc") {
        "pc"
    } else {
        "unknown"
    };
    let os = match std::env::consts::OS {
        "macos" => "darwin",
        other => other,
    };
    let env = if cfg!(target_env = "msvc") {
        Some("msvc")
    } else if cfg!(target_env = "gnu") {
        Some("gnu")
    } else if cfg!(target_env = "musl") {
        Some("musl")
    } else {
        None
    };

    match env {
        Some(env) => format!("{arch}-{vendor}-{os}-{env}"),
        None => format!("{arch}-{vendor}-{os}"),
    }
}

fn current_binary_path() -> Result<PathBuf, String> {
    std::env::current_exe()
        .map_err(|error| format!("Failed to resolve current binary path: {error}"))
}

fn binary_sha256_for_path(path: &Path) -> Result<String, String> {
    let mut file = fs::File::open(path)
        .map_err(|error| format!("Failed to open binary '{}': {error}", path.display()))?;
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 8192];

    loop {
        let bytes_read = file
            .read(&mut buffer)
            .map_err(|error| format!("Failed to read binary '{}': {error}", path.display()))?;
        if bytes_read == 0 {
            break;
        }
        hasher.update(&buffer[..bytes_read]);
    }

    Ok(format!("{:x}", hasher.finalize()))
}

pub fn current_binary_sha256() -> Result<String, String> {
    let path = current_binary_path()?;
    binary_sha256_for_path(&path)
}

fn install_id_or_new(existing: Option<&str>) -> String {
    existing
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| Uuid::new_v4().to_string())
}

#[cfg(test)]
fn persist_metadata_in_config(
    cfg: &mut Config,
    cargo_ai_version: &str,
    template_schema_version: &str,
    cargo_ai_build_target: &str,
    cargo_ai_binary_sha256: &str,
) {
    let metadata = cfg
        .cargo_ai_metadata
        .get_or_insert_with(CargoAiMetadataConfig::default);

    metadata.cargo_ai_version = Some(cargo_ai_version.to_string());
    metadata.template_schema_version = Some(template_schema_version.to_string());
    metadata.cargo_ai_build_target = Some(cargo_ai_build_target.to_string());
    metadata.cargo_ai_install_id = Some(install_id_or_new(metadata.cargo_ai_install_id.as_deref()));
    metadata.cargo_ai_binary_sha256 = Some(cargo_ai_binary_sha256.to_string());
}

fn persist_metadata_values(
    cargo_ai_version: &str,
    template_schema_version: &str,
    cargo_ai_build_target: &str,
    cargo_ai_binary_sha256: &str,
) -> Result<(), String> {
    persist_metadata_values_at(
        &config_path(),
        cargo_ai_version,
        template_schema_version,
        cargo_ai_build_target,
        cargo_ai_binary_sha256,
    )
}

fn persist_metadata_values_at(
    path: &Path,
    cargo_ai_version: &str,
    template_schema_version: &str,
    cargo_ai_build_target: &str,
    cargo_ai_binary_sha256: &str,
) -> Result<(), String> {
    match load_config_from_path(path).map_err(|error| error.to_string())? {
        ConfigLoad::Missing => {
            let fields = metadata_fields(
                cargo_ai_version,
                template_schema_version,
                cargo_ai_build_target,
                cargo_ai_binary_sha256,
                install_id_or_new(None),
            );
            persist_section_fields_at(path, "cargo_ai_metadata", &fields)?;
        }
        ConfigLoad::Loaded(loaded) => {
            let existing_install_id = loaded
                .config()
                .cargo_ai_metadata
                .as_ref()
                .and_then(|metadata| metadata.cargo_ai_install_id.as_deref())
                .map(str::to_string);
            let fields = metadata_fields(
                cargo_ai_version,
                template_schema_version,
                cargo_ai_build_target,
                cargo_ai_binary_sha256,
                install_id_or_new(existing_install_id.as_deref()),
            );
            persist_loaded_section_fields(&loaded, "cargo_ai_metadata", &fields)?;
        }
    }
    Ok(())
}

fn metadata_fields(
    cargo_ai_version: &str,
    template_schema_version: &str,
    cargo_ai_build_target: &str,
    cargo_ai_binary_sha256: &str,
    install_id: String,
) -> [(&'static str, Option<toml::Value>); 5] {
    [
        (
            "cargo_ai_version",
            Some(toml::Value::String(cargo_ai_version.to_string())),
        ),
        (
            "template_schema_version",
            Some(toml::Value::String(template_schema_version.to_string())),
        ),
        (
            "cargo_ai_build_target",
            Some(toml::Value::String(cargo_ai_build_target.to_string())),
        ),
        ("cargo_ai_install_id", Some(toml::Value::String(install_id))),
        (
            "cargo_ai_binary_sha256",
            Some(toml::Value::String(cargo_ai_binary_sha256.to_string())),
        ),
    ]
}

pub fn persist_current_metadata() -> Result<(), String> {
    persist_metadata_values(
        env!("CARGO_PKG_VERSION"),
        &current_template_schema_version(),
        &current_build_target(),
        &current_binary_sha256()?,
    )
}

fn trim_optional_string(value: Option<String>) -> Option<String> {
    value
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn normalized_metadata(mut metadata: CargoAiMetadataConfig) -> Option<CargoAiMetadataConfig> {
    metadata.cargo_ai_version = trim_optional_string(metadata.cargo_ai_version);
    metadata.template_schema_version = trim_optional_string(metadata.template_schema_version);
    metadata.cargo_ai_build_target = trim_optional_string(metadata.cargo_ai_build_target);
    metadata.cargo_ai_install_id = trim_optional_string(metadata.cargo_ai_install_id);
    metadata.cargo_ai_binary_sha256 = trim_optional_string(metadata.cargo_ai_binary_sha256);

    if metadata.cargo_ai_version.is_none()
        && metadata.template_schema_version.is_none()
        && metadata.cargo_ai_build_target.is_none()
        && metadata.cargo_ai_install_id.is_none()
        && metadata.cargo_ai_binary_sha256.is_none()
    {
        None
    } else {
        Some(metadata)
    }
}

pub fn load_request_metadata() -> Option<CargoAiMetadataConfig> {
    load_config()
        .and_then(|cfg| cfg.cargo_ai_metadata)
        .and_then(normalized_metadata)
}

#[cfg(test)]
mod tests {
    use super::{
        binary_sha256_for_path, normalized_metadata, persist_metadata_in_config,
        persist_metadata_values_at,
    };
    use crate::config::schema::{CargoAiMetadata, Config};
    use crate::schema_version;
    use std::fs;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};
    use uuid::Uuid;

    const CURRENT_CARGO_AI_VERSION: &str = env!("CARGO_PKG_VERSION");
    const DIFFERENT_CARGO_AI_VERSION: &str = "9.9.9";

    fn default_test_config() -> Config {
        Config {
            profile: Vec::new(),
            cargo_ai_token: None,
            default_profile: None,
            secret_store: Some(crate::config::schema::default_secret_store_mode()),
            account: None,
            openai_auth: None,
            web_resources: None,
            update_check: None,
            cargo_ai_metadata: None,
        }
    }

    fn temp_file_path(stem: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time should be after epoch")
            .as_nanos();
        std::env::temp_dir().join(format!("cargo-ai-metadata-{stem}-{nanos}.tmp"))
    }

    #[test]
    fn template_schema_version_uses_date_revision_format_when_present() {
        let value = schema_version::extract_schema_version_from_agentcfg(
            r#"{"version":"2026-03-03.r2","inputs":[{"type":"text","text":"x"}],"agent_schema":{"type":"object","properties":{}},"actions":[]}"#,
        );
        assert_eq!(value.as_deref(), Some("2026-03-03.r2"));
    }

    #[test]
    fn template_schema_version_rejects_legacy_semver_values() {
        let agentcfg = format!(
            r#"{{"version":"{CURRENT_CARGO_AI_VERSION}","inputs":[{{"type":"text","text":"x"}}],"agent_schema":{{"type":"object","properties":{{}}}},"actions":[]}}"#
        );
        let value = schema_version::extract_schema_version_from_agentcfg(&agentcfg);
        assert!(value.is_none());
    }

    #[test]
    fn template_schema_version_falls_back_to_current_schema_version_when_missing() {
        let value = schema_version::extract_schema_version_from_agentcfg(
            r#"{"inputs":[{"type":"text","text":"x"}],"agent_schema":{"type":"object","properties":{}},"actions":[]}"#,
        );
        assert!(value.is_none());
        let fallback = schema_version::current_schema_version();
        assert!(schema_version::is_valid_schema_version(&fallback));
    }

    #[test]
    fn persist_metadata_sets_all_fields_deterministically() {
        let mut cfg = default_test_config();

        persist_metadata_in_config(
            &mut cfg,
            CURRENT_CARGO_AI_VERSION,
            "2026-03-03.r1",
            "aarch64-apple-darwin",
            "abc123",
        );
        let metadata = cfg
            .cargo_ai_metadata
            .as_ref()
            .expect("cargo-ai metadata should be initialized");

        assert_eq!(
            metadata.cargo_ai_version.as_deref(),
            Some(CURRENT_CARGO_AI_VERSION)
        );
        assert_eq!(
            metadata.template_schema_version.as_deref(),
            Some("2026-03-03.r1")
        );
        assert_eq!(
            metadata.cargo_ai_build_target.as_deref(),
            Some("aarch64-apple-darwin")
        );
        assert_eq!(metadata.cargo_ai_binary_sha256.as_deref(), Some("abc123"));
        assert!(Uuid::parse_str(
            metadata
                .cargo_ai_install_id
                .as_deref()
                .expect("install id should be set")
        )
        .is_ok());
    }

    #[test]
    fn persist_metadata_preserves_existing_install_id_and_refreshes_hash() {
        let mut cfg = default_test_config();

        persist_metadata_in_config(
            &mut cfg,
            CURRENT_CARGO_AI_VERSION,
            "2026-03-03.r1",
            "aarch64-apple-darwin",
            "abc123",
        );
        let install_id = cfg
            .cargo_ai_metadata
            .as_ref()
            .and_then(|metadata| metadata.cargo_ai_install_id.clone())
            .expect("install id should exist after initial persistence");

        persist_metadata_in_config(
            &mut cfg,
            DIFFERENT_CARGO_AI_VERSION,
            "2026-03-03.r2",
            "aarch64-apple-darwin",
            "def456",
        );
        let metadata = cfg
            .cargo_ai_metadata
            .as_ref()
            .expect("cargo-ai metadata should still be initialized");

        assert_eq!(
            metadata.cargo_ai_install_id.as_deref(),
            Some(install_id.as_str())
        );
        assert_eq!(metadata.cargo_ai_binary_sha256.as_deref(), Some("def456"));
        assert_eq!(
            metadata.cargo_ai_version.as_deref(),
            Some(DIFFERENT_CARGO_AI_VERSION)
        );
        assert_eq!(
            metadata.template_schema_version.as_deref(),
            Some("2026-03-03.r2")
        );
    }

    #[test]
    fn binary_sha256_changes_when_file_contents_change() {
        let path = temp_file_path("sha256");
        fs::write(&path, b"first build").expect("temp binary should be written");
        let first_hash = binary_sha256_for_path(&path).expect("hash should be computed");

        fs::write(&path, b"second build").expect("temp binary should be updated");
        let second_hash = binary_sha256_for_path(&path).expect("updated hash should be computed");

        assert_eq!(first_hash.len(), 64);
        assert_eq!(second_hash.len(), 64);
        assert_ne!(first_hash, second_hash);

        let _ = fs::remove_file(path);
    }

    #[test]
    fn metadata_writer_persists_owned_section() {
        let path = temp_file_path("write");
        fs::write(&path, "profile = []\nunknown = \"preserve\"\n")
            .expect("test config should be written");

        persist_metadata_values_at(
            &path,
            CURRENT_CARGO_AI_VERSION,
            "2026-03-03.r1",
            "aarch64-apple-darwin",
            "abc123",
        )
        .expect("metadata should be persisted");

        let written = fs::read_to_string(&path).expect("written config should be readable");
        assert!(written.contains("cargo_ai_metadata"));
        assert!(written.contains(&format!(
            "cargo_ai_version = \"{CURRENT_CARGO_AI_VERSION}\""
        )));
        assert!(written.contains("template_schema_version = \"2026-03-03.r1\""));
        assert!(written.contains("cargo_ai_build_target = \"aarch64-apple-darwin\""));
        assert!(written.contains("cargo_ai_binary_sha256 = \"abc123\""));
        assert!(written.contains("unknown = \"preserve\""));

        let _ = fs::remove_file(path.with_file_name(format!(
            "{}.bak",
            path.file_name()
                .and_then(|name| name.to_str())
                .expect("test path should have a file name")
        )));
        let _ = fs::remove_file(path);
    }

    #[test]
    fn metadata_writer_leaves_malformed_config_byte_identical() {
        let path = temp_file_path("malformed");
        let original = "profile = [\n";
        fs::write(&path, original).expect("test config should be written");

        let error = persist_metadata_values_at(
            &path,
            CURRENT_CARGO_AI_VERSION,
            "2026-03-03.r1",
            "aarch64-apple-darwin",
            "abc123",
        )
        .expect_err("malformed config must block metadata persistence");

        assert!(error.contains(&path.display().to_string()));
        assert_eq!(
            fs::read_to_string(&path).expect("config should remain readable"),
            original
        );
        assert!(!path
            .with_file_name(format!(
                "{}.bak",
                path.file_name()
                    .and_then(|name| name.to_str())
                    .expect("test path should have a file name")
            ))
            .exists());

        let _ = fs::remove_file(path);
    }

    #[test]
    fn normalized_metadata_trims_values_and_omits_empty_fields() {
        let metadata = CargoAiMetadata {
            cargo_ai_version: Some(format!(" {CURRENT_CARGO_AI_VERSION} ")),
            template_schema_version: Some(" ".to_string()),
            cargo_ai_build_target: Some(" aarch64-apple-darwin ".to_string()),
            cargo_ai_install_id: Some(String::new()),
            cargo_ai_binary_sha256: Some(" abc123 ".to_string()),
        };

        let normalized = normalized_metadata(metadata).expect("metadata should remain present");

        assert_eq!(
            normalized.cargo_ai_version.as_deref(),
            Some(CURRENT_CARGO_AI_VERSION)
        );
        assert!(normalized.template_schema_version.is_none());
        assert_eq!(
            normalized.cargo_ai_build_target.as_deref(),
            Some("aarch64-apple-darwin")
        );
        assert!(normalized.cargo_ai_install_id.is_none());
        assert_eq!(normalized.cargo_ai_binary_sha256.as_deref(), Some("abc123"));
    }

    #[test]
    fn normalized_metadata_returns_none_when_all_fields_are_empty() {
        let metadata = CargoAiMetadata {
            cargo_ai_version: Some(" ".to_string()),
            template_schema_version: None,
            cargo_ai_build_target: Some(String::new()),
            cargo_ai_install_id: None,
            cargo_ai_binary_sha256: Some("\n".to_string()),
        };

        assert!(normalized_metadata(metadata).is_none());
    }
}
