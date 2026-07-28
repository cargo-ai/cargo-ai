use crate::config::schema::{Config, Profile};
use std::fmt;
use std::fs;
use std::io::ErrorKind;
#[cfg(windows)]
use std::os::windows::fs::MetadataExt;
use std::path::{Path, PathBuf};

#[cfg(windows)]
const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0000_0400;

fn resolve_config_path(cargo_ai_root: PathBuf) -> PathBuf {
    cargo_ai_root.join("config.toml")
}

pub fn config_path() -> PathBuf {
    resolve_config_path(crate::config::paths::cargo_ai_root())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ConfigLoadErrorKind {
    Read,
    Parse,
}

#[derive(Debug)]
pub(crate) struct ConfigLoadError {
    path: PathBuf,
    kind: ConfigLoadErrorKind,
    detail: String,
}

impl ConfigLoadError {
    fn read(path: &Path, error: impl fmt::Display) -> Self {
        Self {
            path: path.to_path_buf(),
            kind: ConfigLoadErrorKind::Read,
            detail: error.to_string(),
        }
    }

    fn parse(path: &Path, stage: &str) -> Self {
        Self {
            path: path.to_path_buf(),
            kind: ConfigLoadErrorKind::Parse,
            // `toml::de::Error` Display output includes a source-line excerpt.
            // Config files may still contain legacy credentials, so diagnostics
            // intentionally retain only the generic validation stage.
            detail: stage.to_string(),
        }
    }

    #[cfg(test)]
    pub(crate) fn kind(&self) -> ConfigLoadErrorKind {
        self.kind
    }
}

impl fmt::Display for ConfigLoadError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let operation = match self.kind {
            ConfigLoadErrorKind::Read => "read",
            ConfigLoadErrorKind::Parse => "parse",
        };
        write!(
            formatter,
            "failed to {operation} Cargo AI config '{}': {}. Fix or restore this file before retrying; Cargo AI left it unchanged",
            self.path.display(),
            self.detail
        )
    }
}

impl std::error::Error for ConfigLoadError {}

#[derive(Debug)]
pub(crate) struct LoadedConfig {
    path: PathBuf,
    config: Config,
    document: toml::Value,
    original_contents: String,
}

impl LoadedConfig {
    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    pub(crate) fn config(&self) -> &Config {
        &self.config
    }

    pub(crate) fn document(&self) -> &toml::Value {
        &self.document
    }

    pub(crate) fn original_contents(&self) -> &str {
        &self.original_contents
    }
}

#[derive(Debug)]
pub(crate) enum ConfigLoad {
    Missing,
    Loaded(LoadedConfig),
}

pub(crate) fn load_config_from_path(path: &Path) -> Result<ConfigLoad, ConfigLoadError> {
    validate_config_path_safety(path)?;
    let contents = match fs::read_to_string(path) {
        Ok(contents) => contents,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(ConfigLoad::Missing),
        Err(error) => return Err(ConfigLoadError::read(path, error)),
    };

    let document = toml::from_str::<toml::Value>(&contents)
        .map_err(|_| ConfigLoadError::parse(path, "invalid TOML syntax"))?;
    let config = toml::from_str::<Config>(&contents)
        .map_err(|_| ConfigLoadError::parse(path, "config does not match the supported schema"))?;

    Ok(ConfigLoad::Loaded(LoadedConfig {
        path: path.to_path_buf(),
        config,
        document,
        original_contents: contents,
    }))
}

pub(crate) fn validate_config_path_safety(path: &Path) -> Result<(), ConfigLoadError> {
    // `config.toml` and the Cargo AI Home directory are the mutation boundary.
    // A broader ancestor such as `~/.cargo` may legitimately be symlinked, so
    // only these two managed paths are refused when link-like.
    for candidate in [Some(path), path.parent()].into_iter().flatten() {
        match fs::symlink_metadata(candidate) {
            Ok(metadata) if metadata_is_link_like(&metadata) => {
                return Err(ConfigLoadError::read(
                    path,
                    format!(
                        "refusing symbolic link or reparse point at managed path '{}'",
                        candidate.display()
                    ),
                ));
            }
            Ok(_) => {}
            Err(error) if error.kind() == ErrorKind::NotFound => {}
            Err(error) => return Err(ConfigLoadError::read(path, error)),
        }
    }
    Ok(())
}

#[cfg(windows)]
fn metadata_is_link_like(metadata: &fs::Metadata) -> bool {
    metadata.file_type().is_symlink()
        || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

#[cfg(not(windows))]
fn metadata_is_link_like(metadata: &fs::Metadata) -> bool {
    metadata.file_type().is_symlink()
}

pub(crate) fn load_config_strict() -> Result<ConfigLoad, ConfigLoadError> {
    load_config_from_path(&config_path())
}

/// Compatibility loader for read-only and explicit mutation paths that have not
/// adopted strict error handling yet. Automatic startup writers must use
/// `load_config_strict` so an existing unreadable or malformed file is never
/// mistaken for a missing file.
pub fn load_config() -> Option<Config> {
    match load_config_strict() {
        Ok(ConfigLoad::Missing) => None,
        Ok(ConfigLoad::Loaded(loaded)) => Some(loaded.config),
        Err(_) => None,
    }
}

pub fn find_profile<'a>(config: &'a Config, name: &str) -> Option<&'a Profile> {
    config.profile.iter().find(|p| p.name == name)
}

#[cfg(test)]
mod tests {
    use super::{load_config_from_path, resolve_config_path, ConfigLoad, ConfigLoadErrorKind};
    use std::fs;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_path(stem: &str) -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock should be valid")
            .as_nanos();
        std::env::temp_dir().join(format!("cargo-ai-config-loader-{stem}-{unique}"))
    }

    #[test]
    fn joins_config_file_under_resolved_root() {
        let path = resolve_config_path(PathBuf::from("/tmp/cargo-ai-home"));

        assert_eq!(path, PathBuf::from("/tmp/cargo-ai-home/config.toml"));
    }

    #[test]
    fn strict_loader_distinguishes_missing_file() {
        let path = temp_path("missing");

        assert!(matches!(
            load_config_from_path(&path).expect("missing config is not an error"),
            ConfigLoad::Missing
        ));
    }

    #[test]
    fn strict_loader_returns_typed_and_raw_config() {
        let path = temp_path("valid");
        fs::write(&path, "profile = []\nunknown_top_level = \"preserve-me\"\n")
            .expect("test config should be written");

        let ConfigLoad::Loaded(loaded) =
            load_config_from_path(&path).expect("valid config should load")
        else {
            panic!("config should exist");
        };

        assert!(loaded.config().profile.is_empty());
        assert_eq!(
            loaded
                .document()
                .get("unknown_top_level")
                .and_then(toml::Value::as_str),
            Some("preserve-me")
        );
        assert_eq!(
            loaded.original_contents(),
            "profile = []\nunknown_top_level = \"preserve-me\"\n"
        );

        let _ = fs::remove_file(path);
    }

    #[test]
    fn strict_loader_reports_parse_failure_with_path() {
        let path = temp_path("malformed");
        let sentinel_secret = "must-not-appear-in-error";
        fs::write(
            &path,
            format!("profile = [{{ token = \"{sentinel_secret}\" }}"),
        )
        .expect("test config should be written");

        let error = load_config_from_path(&path).expect_err("malformed config must fail");

        assert_eq!(error.kind(), ConfigLoadErrorKind::Parse);
        assert!(error.to_string().contains(&path.display().to_string()));
        assert!(error.to_string().contains("left it unchanged"));
        assert!(!error.to_string().contains(sentinel_secret));

        let _ = fs::remove_file(path);
    }

    #[cfg(unix)]
    #[test]
    fn strict_loader_reports_read_failure_with_path() {
        use std::os::unix::fs::PermissionsExt;

        let path = temp_path("unreadable-directory");
        fs::create_dir(&path).expect("test directory should be created");
        let mut permissions = fs::metadata(&path)
            .expect("directory metadata should be readable")
            .permissions();
        permissions.set_mode(0o000);
        fs::set_permissions(&path, permissions).expect("permissions should be updated");

        let error = load_config_from_path(&path).expect_err("directory cannot be read as config");

        assert_eq!(error.kind(), ConfigLoadErrorKind::Read);
        assert!(error.to_string().contains(&path.display().to_string()));

        let _ = fs::set_permissions(&path, fs::Permissions::from_mode(0o700));
        let _ = fs::remove_dir(path);
    }

    #[cfg(unix)]
    #[test]
    fn strict_loader_refuses_linked_config_and_linked_home() {
        use std::os::unix::fs::symlink;

        let root = temp_path("links");
        let actual_home = root.join("actual-home");
        fs::create_dir_all(&actual_home).expect("actual home should be created");
        let actual_config = actual_home.join("config.toml");
        fs::write(&actual_config, "profile = []\n").expect("actual config should be written");

        let linked_config = root.join("linked-config.toml");
        symlink(&actual_config, &linked_config).expect("config symlink should be created");
        let config_error =
            load_config_from_path(&linked_config).expect_err("linked config must be refused");
        assert_eq!(config_error.kind(), ConfigLoadErrorKind::Read);
        assert!(config_error.to_string().contains("symbolic link"));

        let linked_home = root.join("linked-home");
        symlink(&actual_home, &linked_home).expect("home symlink should be created");
        let home_error = load_config_from_path(&linked_home.join("config.toml"))
            .expect_err("linked Cargo AI Home must be refused");
        assert_eq!(home_error.kind(), ConfigLoadErrorKind::Read);
        assert!(home_error.to_string().contains("symbolic link"));

        assert_eq!(
            fs::read_to_string(&actual_config).expect("target should remain readable"),
            "profile = []\n"
        );
        let _ = fs::remove_dir_all(root);
    }
}
