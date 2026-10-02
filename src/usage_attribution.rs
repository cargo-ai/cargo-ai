//! Invocation-scoped, local usage attribution shared by the CLI and generated agents.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs::{self, OpenOptions};
use std::io::{ErrorKind, Read, Write};
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};
use uuid::Uuid;

pub(crate) const WORKSPACE_ENV: &str = "CARGO_AI_USAGE_CALLER_WORKSPACE";
pub(crate) const ATTRIBUTION_SCHEMA_VERSION: u32 = 1;

#[allow(dead_code)] // The shared source is compiled by both CLI and generated-agent crates.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) enum RuntimeKind {
    #[default]
    Cli,
    Generated,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "status", content = "value", rename_all = "snake_case")]
pub(crate) enum Workspace {
    Known(PathBuf),
    None,
    Unknown(String),
}

#[derive(Clone, Debug, Default)]
pub(crate) struct AttributionInput {
    pub(crate) authored_package_id: Option<String>,
    pub(crate) hosted_source_id: Option<String>,
    pub(crate) package_version: Option<String>,
    pub(crate) package_content_digest: Option<String>,
    pub(crate) hosted_version_id: Option<String>,
    pub(crate) package_root: Option<PathBuf>,
    pub(crate) agent_key: Option<String>,
    pub(crate) definition_hash: Option<String>,
    pub(crate) workspace: Option<Workspace>,
    pub(crate) runtime_kind: RuntimeKind,
    pub(crate) runtime_version: Option<String>,
    pub(crate) generator_version: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct Identity {
    pub(crate) id: Option<String>,
    pub(crate) source: Option<&'static str>,
    pub(crate) unknown_reason: Option<&'static str>,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct Package {
    pub(crate) id: Option<String>,
    pub(crate) source: Option<&'static str>,
    pub(crate) unknown_reason: Option<&'static str>,
    pub(crate) authored_id_unknown_reason: Option<&'static str>,
    pub(crate) hosted_source_id: Option<String>,
    pub(crate) version: Option<String>,
    pub(crate) content_digest: Option<String>,
    pub(crate) hosted_version_id: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct Location {
    pub(crate) path: Option<String>,
    pub(crate) source: Option<&'static str>,
    pub(crate) unknown_reason: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct Agent {
    pub(crate) id: Option<String>,
    pub(crate) source: Option<&'static str>,
    pub(crate) unknown_reason: Option<&'static str>,
    pub(crate) key: Option<String>,
    pub(crate) definition_hash: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct WorkspaceAttribution {
    pub(crate) path: Option<String>,
    pub(crate) source: Option<&'static str>,
    pub(crate) unknown_reason: Option<String>,
    pub(crate) environment_id: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct Runtime {
    pub(crate) kind: &'static str,
    pub(crate) version: String,
    pub(crate) build_target: String,
    pub(crate) executable_path: Option<String>,
    pub(crate) executable_sha256: Option<String>,
    pub(crate) executable_unknown_reason: Option<&'static str>,
    pub(crate) digest_unknown_reason: Option<&'static str>,
    pub(crate) generator_version: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct AttributionSnapshot {
    pub(crate) schema_version: u32,
    pub(crate) environment: Identity,
    pub(crate) package: Package,
    pub(crate) package_location: Location,
    pub(crate) agent: Agent,
    pub(crate) workspace: WorkspaceAttribution,
    pub(crate) runtime: Runtime,
}

#[cfg(cargo_ai_cli)]
#[derive(Serialize)]
pub(crate) struct UsageContextInspection {
    pub(crate) effective_home: String,
    pub(crate) environment: Identity,
    pub(crate) runtime: Runtime,
    pub(crate) attribution_schema_version: u32,
    pub(crate) query_schema_versions: [u32; 2],
    pub(crate) grouping_dimensions: &'static [&'static str],
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct InheritedWorkspace {
    workspace: Workspace,
    environment_id: Option<String>,
}

pub(crate) fn capture_workspace() -> Workspace {
    let cwd = match std::env::current_dir() {
        Ok(path) => path,
        Err(_) => return Workspace::Unknown("current_directory_unavailable".to_string()),
    };
    for ancestor in cwd.ancestors() {
        let marker = ancestor.join(".cargo-ai/project.toml");
        match fs::metadata(&marker) {
            Ok(metadata) if metadata.is_file() => {
                return match fs::canonicalize(ancestor) {
                    Ok(path) => Workspace::Known(path),
                    Err(_) => Workspace::Unknown("canonicalization_failed".to_string()),
                };
            }
            Ok(_) => return Workspace::Unknown("project_marker_invalid".to_string()),
            Err(error) if error.kind() == ErrorKind::NotFound => {}
            Err(_) => return Workspace::Unknown("project_marker_unreadable".to_string()),
        }
    }
    Workspace::None
}

pub(crate) fn snapshot(input: AttributionInput, allow_initialize_id: bool) -> AttributionSnapshot {
    let environment = environment_identity(allow_initialize_id);
    let inherited_raw = std::env::var_os(WORKSPACE_ENV);
    let old_child_without_workspace = inherited_raw.is_none()
        && std::env::var_os(crate::usage_log::USAGE_ROOT_RUN_ID_ENV).is_some();
    let (workspace_capture, origin_environment) = resolve_workspace_context(
        input.workspace,
        inherited_raw.as_ref().map(|raw| raw.to_str().unwrap_or("")),
        environment.id.clone(),
        old_child_without_workspace,
    );
    let workspace = workspace_attribution(&workspace_capture, origin_environment);

    let (package_id, package_source, package_unknown_reason) = package_identity(
        input.authored_package_id.as_deref(),
        input.hosted_source_id.as_deref(),
    );
    let authored_id_unknown_reason = authored_id_issue(input.authored_package_id.as_deref());
    let package_location = match input.package_root.as_ref() {
        Some(root) => match fs::canonicalize(root) {
            Ok(path) => match path.to_str() {
                Some(path) => Location {
                    path: Some(path.to_string()),
                    source: Some("canonical_package_root"),
                    unknown_reason: None,
                },
                None => Location {
                    path: None,
                    source: None,
                    unknown_reason: Some("non_unicode_path".to_string()),
                },
            },
            Err(_) => Location {
                path: None,
                source: None,
                unknown_reason: Some("canonicalization_failed".to_string()),
            },
        },
        None => Location {
            path: None,
            source: None,
            unknown_reason: Some("package_root_unavailable".to_string()),
        },
    };
    let key = input
        .agent_key
        .as_deref()
        .and_then(non_empty)
        .map(str::to_string);
    let (agent_id, agent_source) = match (
        package_id.as_deref(),
        package_location.path.as_deref(),
        key.as_deref(),
    ) {
        (Some(package_id), _, Some(key)) => (
            Some(serde_json::json!(["package", package_id, key]).to_string()),
            Some("package_key"),
        ),
        (None, Some(location), Some(key)) if environment.id.is_some() => (
            Some(serde_json::json!(["location", environment.id, location, key]).to_string()),
            Some("location_key"),
        ),
        _ => (None, None),
    };
    let agent_unknown_reason = if agent_id.is_some() {
        None
    } else if key.is_none() {
        Some("agent_key_unavailable")
    } else if package_id.is_none() && environment.id.is_none() {
        Some("environment_unavailable")
    } else {
        Some("package_and_location_unavailable")
    };
    AttributionSnapshot {
        schema_version: ATTRIBUTION_SCHEMA_VERSION,
        environment,
        package: Package {
            id: package_id,
            source: package_source,
            unknown_reason: package_unknown_reason,
            authored_id_unknown_reason,
            hosted_source_id: input.hosted_source_id,
            version: input.package_version,
            content_digest: input.package_content_digest,
            hosted_version_id: input.hosted_version_id,
        },
        package_location,
        agent: Agent {
            id: agent_id,
            source: agent_source,
            unknown_reason: agent_unknown_reason,
            key,
            definition_hash: input.definition_hash,
        },
        workspace,
        runtime: runtime(
            input.runtime_kind,
            input.runtime_version,
            input.generator_version,
        ),
    }
}

fn package_identity(
    authored: Option<&str>,
    hosted: Option<&str>,
) -> (Option<String>, Option<&'static str>, Option<&'static str>) {
    if let Some(id) = authored.and_then(|id| Uuid::parse_str(id).ok()) {
        return (Some(id.to_string()), Some("authored_project_id"), None);
    }
    if let Some(id) = hosted.and_then(non_empty) {
        return (Some(format!("hosted:{id}")), Some("hosted_source_id"), None);
    }
    (
        None,
        None,
        Some(if authored.is_some() {
            "invalid_authored_project_id"
        } else {
            "missing_package_identity"
        }),
    )
}

fn authored_id_issue(id: Option<&str>) -> Option<&'static str> {
    id.filter(|id| Uuid::parse_str(id).is_err())
        .map(|_| "invalid_authored_project_id")
}

fn resolve_workspace_context(
    input: Option<Workspace>,
    inherited_raw: Option<&str>,
    environment_id: Option<String>,
    old_child_without_workspace: bool,
) -> (Workspace, Option<String>) {
    if let Some(raw) = inherited_raw {
        return match serde_json::from_str::<InheritedWorkspace>(raw) {
            Ok(inherited) => (inherited.workspace, inherited.environment_id),
            Err(_) => (
                Workspace::Unknown("inherited_context_invalid".to_string()),
                None,
            ),
        };
    }
    if old_child_without_workspace {
        return (
            Workspace::Unknown("inherited_context_unavailable".to_string()),
            None,
        );
    }
    (input.unwrap_or_else(capture_workspace), environment_id)
}

impl AttributionSnapshot {
    pub(crate) fn inherited_workspace_env(&self) -> String {
        let workspace = if let Some(path) = self.workspace.path.as_ref() {
            Workspace::Known(PathBuf::from(path))
        } else if self.workspace.source == Some("none") {
            Workspace::None
        } else {
            Workspace::Unknown(
                self.workspace
                    .unknown_reason
                    .clone()
                    .unwrap_or_else(|| "unavailable".to_string()),
            )
        };
        serde_json::to_string(&InheritedWorkspace {
            workspace,
            environment_id: self.workspace.environment_id.clone(),
        })
        .unwrap_or_default()
    }
}

#[cfg(cargo_ai_cli)]
pub(crate) fn inspect_context() -> UsageContextInspection {
    let home = crate::config::loader::config_path()
        .parent()
        .unwrap_or(Path::new("."))
        .to_path_buf();
    UsageContextInspection {
        effective_home: home.to_string_lossy().into_owned(),
        environment: environment_identity(false),
        runtime: runtime(RuntimeKind::Cli, None, None),
        attribution_schema_version: ATTRIBUTION_SCHEMA_VERSION,
        query_schema_versions: [1, 2],
        grouping_dimensions: &[
            "day",
            "profile",
            "provider",
            "requested_model",
            "resolved_model",
            "environment",
            "package",
            "package_location",
            "agent",
            "workspace",
            "runtime_version",
            "package_revision",
            "agent_revision",
            "runtime_digest",
        ],
    }
}

fn workspace_attribution(
    workspace: &Workspace,
    environment_id: Option<String>,
) -> WorkspaceAttribution {
    match workspace {
        Workspace::Known(path)
            if path.is_absolute()
                && !path.components().any(|component| {
                    matches!(
                        component,
                        std::path::Component::ParentDir | std::path::Component::CurDir
                    )
                }) =>
        {
            match path.to_str() {
                Some(path) => WorkspaceAttribution {
                    path: Some(path.to_string()),
                    source: Some("canonical_project_root"),
                    unknown_reason: None,
                    environment_id,
                },
                None => WorkspaceAttribution {
                    path: None,
                    source: None,
                    unknown_reason: Some("non_unicode_path".to_string()),
                    environment_id,
                },
            }
        }
        Workspace::Known(_) => WorkspaceAttribution {
            path: None,
            source: None,
            unknown_reason: Some("captured_path_invalid".to_string()),
            environment_id,
        },
        Workspace::None => WorkspaceAttribution {
            path: None,
            source: Some("none"),
            unknown_reason: None,
            environment_id,
        },
        Workspace::Unknown(reason) => WorkspaceAttribution {
            path: None,
            source: None,
            unknown_reason: Some(reason.clone()),
            environment_id,
        },
    }
}

fn runtime(
    kind: RuntimeKind,
    version: Option<String>,
    generator_version: Option<String>,
) -> Runtime {
    let executable = std::env::current_exe().ok();
    let digest = executable
        .as_ref()
        .and_then(|path| binary_digest(path).ok());
    let executable_unknown_reason = executable
        .is_none()
        .then_some("current_executable_unavailable");
    let digest_unknown_reason = digest.is_none().then_some(if executable.is_none() {
        "current_executable_unavailable"
    } else {
        "executable_unreadable"
    });
    Runtime {
        kind: match kind {
            RuntimeKind::Cli => "cli",
            RuntimeKind::Generated => "generated",
        },
        version: version.unwrap_or_else(|| env!("CARGO_PKG_VERSION").to_string()),
        build_target: build_target(),
        executable_path: executable.map(|path| path.to_string_lossy().into_owned()),
        executable_sha256: digest,
        executable_unknown_reason,
        digest_unknown_reason,
        generator_version,
    }
}

fn binary_digest(path: &Path) -> Result<String, std::io::Error> {
    let mut file = fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 8192];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

fn build_target() -> String {
    let arch = std::env::consts::ARCH;
    let vendor = if cfg!(target_vendor = "apple") {
        "apple"
    } else if cfg!(target_vendor = "pc") {
        "pc"
    } else {
        "unknown"
    };
    let os = if cfg!(target_os = "macos") {
        "darwin"
    } else {
        std::env::consts::OS
    };
    let abi = if cfg!(target_env = "msvc") {
        "-msvc"
    } else if cfg!(target_env = "gnu") {
        "-gnu"
    } else if cfg!(target_env = "musl") {
        "-musl"
    } else {
        ""
    };
    format!("{arch}-{vendor}-{os}{abi}")
}

fn environment_identity(initialize: bool) -> Identity {
    let path = crate::config::loader::config_path();
    let id = if initialize {
        ensure_environment_id_at(&path)
    } else {
        read_environment_id_at(&path)
    };
    match id {
        Ok(Some(id)) => Identity {
            id: Some(id),
            source: Some("cargo_ai_install_id"),
            unknown_reason: None,
        },
        Ok(None) => Identity {
            id: None,
            source: None,
            unknown_reason: Some("missing_install_id"),
        },
        Err(_) => Identity {
            id: None,
            source: None,
            unknown_reason: Some("config_unavailable"),
        },
    }
}

struct EnvironmentLock(PathBuf);
struct StagedConfig(PathBuf);

impl Drop for EnvironmentLock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

impl Drop for StagedConfig {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

fn read_environment_id_at(path: &Path) -> Result<Option<String>, String> {
    reject_link_like(path)?;
    let raw = match fs::read_to_string(path) {
        Ok(raw) => raw,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.to_string()),
    };
    let doc: toml::Value = toml::from_str(&raw).map_err(|_| "invalid config TOML".to_string())?;
    toml::from_str::<crate::config::schema::Config>(&raw)
        .map_err(|_| "invalid config schema".to_string())?;
    Ok(doc
        .get("cargo_ai_metadata")
        .and_then(|metadata| metadata.get("cargo_ai_install_id"))
        .and_then(toml::Value::as_str)
        .and_then(non_empty)
        .map(str::to_string))
}

pub(crate) fn ensure_environment_id_at(path: &Path) -> Result<Option<String>, String> {
    if let Some(id) = read_environment_id_at(path)? {
        return Ok(Some(id));
    }
    with_environment_lock(path, || {
        if let Some(id) = read_environment_id_at(path)? {
            return Ok(Some(id));
        }
        reject_link_like(path)?;
        let original = match fs::read_to_string(path) {
            Ok(raw) => Some(raw),
            Err(error) if error.kind() == ErrorKind::NotFound => None,
            Err(error) => return Err(error.to_string()),
        };
        let mut doc: toml::Value = if let Some(raw) = original.as_deref() {
            toml::from_str::<crate::config::schema::Config>(raw)
                .map_err(|_| "invalid config schema".to_string())?;
            toml::from_str(raw).map_err(|_| "invalid config TOML".to_string())?
        } else {
            let mut table = toml::map::Map::new();
            table.insert("profile".to_string(), toml::Value::Array(Vec::new()));
            toml::Value::Table(table)
        };
        if contains_legacy_credentials(&doc) {
            return Err(
                "refusing attribution identity initialization while legacy credentials remain"
                    .to_string(),
            );
        }
        let table = doc.as_table_mut().ok_or("config must be a TOML table")?;
        let metadata = table
            .entry("cargo_ai_metadata".to_string())
            .or_insert_with(|| toml::Value::Table(toml::map::Map::new()))
            .as_table_mut()
            .ok_or("cargo_ai_metadata must be a TOML table")?;
        let id = Uuid::new_v4().to_string();
        metadata.insert(
            "cargo_ai_install_id".to_string(),
            toml::Value::String(id.clone()),
        );
        let rendered = toml::to_string_pretty(&doc).map_err(|error| error.to_string())?;
        toml::from_str::<crate::config::schema::Config>(&rendered)
            .map_err(|_| "updated config schema invalid".to_string())?;
        let staged = path.with_file_name(format!(".config.toml.{}.tmp", Uuid::new_v4()));
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&staged).map_err(|error| error.to_string())?;
        let staged_guard = StagedConfig(staged.clone());
        file.write_all(rendered.as_bytes())
            .map_err(|error| error.to_string())?;
        file.sync_all().map_err(|error| error.to_string())?;
        drop(file);
        reject_link_like(path)?;
        match (&original, fs::read_to_string(path)) {
            (Some(before), Ok(current)) if before == &current => {}
            (None, Err(error)) if error.kind() == ErrorKind::NotFound => {}
            _ => {
                return Err(
                    "Cargo AI config changed before attribution identity replacement".to_string(),
                )
            }
        }
        if original.is_some() {
            replace_config(&staged, path)?;
        } else {
            fs::hard_link(&staged, path).map_err(|error| {
                format!(
                    "Cargo AI config was created concurrently or could not be installed: {error}"
                )
            })?;
        }
        drop(staged_guard);
        sync_parent_directory(path)?;
        Ok(Some(id))
    })
}

pub(crate) fn with_environment_lock<T>(
    path: &Path,
    action: impl FnOnce() -> Result<T, String>,
) -> Result<T, String> {
    with_environment_lock_inner(path, action, |_| {})
}

fn lock_creation_is_contended(error: &std::io::Error) -> bool {
    // Windows can deny creation while the previous lock is pending deletion.
    error.kind() == ErrorKind::AlreadyExists || (cfg!(windows) && error.raw_os_error() == Some(5))
}

fn with_environment_lock_inner<T>(
    path: &Path,
    action: impl FnOnce() -> Result<T, String>,
    mut on_contention: impl FnMut(&std::io::Error),
) -> Result<T, String> {
    reject_link_like(path)?;
    let parent = path.parent().ok_or("config path has no parent")?;
    fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    reject_link_like(path)?;
    let lock = path.with_extension("toml.attribution.lock");
    let start = Instant::now();
    loop {
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        match options.open(&lock) {
            Ok(file) => {
                let _lock = EnvironmentLock(lock.clone());
                let result = action();
                drop(file);
                return result;
            }
            Err(error)
                if lock_creation_is_contended(&error)
                    && start.elapsed() < Duration::from_secs(3) =>
            {
                on_contention(&error);
                thread::sleep(Duration::from_millis(10))
            }
            Err(error) => return Err(format!("attribution config lock unavailable: {error}")),
        }
    }
}

#[cfg(not(windows))]
fn replace_config(staged: &Path, path: &Path) -> Result<(), String> {
    fs::rename(staged, path).map_err(|error| error.to_string())
}

#[cfg(windows)]
fn replace_config(staged: &Path, path: &Path) -> Result<(), String> {
    use std::iter;
    use std::os::windows::ffi::OsStrExt;
    const MOVEFILE_REPLACE_EXISTING: u32 = 0x1;
    const MOVEFILE_WRITE_THROUGH: u32 = 0x8;
    #[link(name = "Kernel32")]
    unsafe extern "system" {
        fn MoveFileExW(source: *const u16, destination: *const u16, flags: u32) -> i32;
    }
    let source = staged
        .as_os_str()
        .encode_wide()
        .chain(iter::once(0))
        .collect::<Vec<_>>();
    let destination = path
        .as_os_str()
        .encode_wide()
        .chain(iter::once(0))
        .collect::<Vec<_>>();
    // SAFETY: both owned UTF-16 buffers are NUL-terminated and live through this call.
    let succeeded = unsafe {
        MoveFileExW(
            source.as_ptr(),
            destination.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };
    if succeeded == 0 {
        Err(std::io::Error::last_os_error().to_string())
    } else {
        Ok(())
    }
}

fn reject_link_like(path: &Path) -> Result<(), String> {
    for candidate in [path.parent(), Some(path)].into_iter().flatten() {
        match fs::symlink_metadata(candidate) {
            Ok(metadata) if metadata_is_link_like(&metadata) => {
                return Err("managed config path is a symbolic link or reparse point".to_string())
            }
            Ok(_) => {}
            Err(error) if error.kind() == ErrorKind::NotFound => {}
            Err(error) => return Err(error.to_string()),
        }
    }
    Ok(())
}

#[cfg(not(windows))]
fn metadata_is_link_like(metadata: &fs::Metadata) -> bool {
    metadata.file_type().is_symlink()
}

#[cfg(windows)]
fn metadata_is_link_like(metadata: &fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0000_0400;
    metadata.file_type().is_symlink()
        || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

fn contains_legacy_credentials(document: &toml::Value) -> bool {
    let Some(root) = document.as_table() else {
        return false;
    };
    if root
        .get("profile")
        .and_then(toml::Value::as_array)
        .is_some_and(|profiles| {
            profiles.iter().any(|profile| {
                profile
                    .as_table()
                    .is_some_and(|profile| profile.contains_key("token"))
            })
        })
    {
        return true;
    }
    root.get("account")
        .and_then(toml::Value::as_table)
        .is_some_and(|account| {
            account.contains_key("access_token") || account.contains_key("refresh_token")
        })
        || root.contains_key("cargo_ai_token")
}

#[cfg(unix)]
fn sync_parent_directory(path: &Path) -> Result<(), String> {
    let parent = path.parent().ok_or("config path has no parent")?;
    fs::File::open(parent)
        .and_then(|directory| directory.sync_all())
        .map_err(|error| error.to_string())
}

#[cfg(not(unix))]
fn sync_parent_directory(_path: &Path) -> Result<(), String> {
    Ok(())
}

fn non_empty(value: &str) -> Option<&str> {
    let value = value.trim();
    (!value.is_empty()).then_some(value)
}

#[cfg(test)]
mod tests {
    use super::{
        authored_id_issue, ensure_environment_id_at, lock_creation_is_contended, package_identity,
        read_environment_id_at, resolve_workspace_context, workspace_attribution,
        InheritedWorkspace, Workspace,
    };
    use std::fs;
    use std::sync::Arc;
    use std::thread;
    use uuid::Uuid;

    #[test]
    fn lock_creation_retries_only_platform_contention() {
        use std::io::{Error, ErrorKind};
        assert!(lock_creation_is_contended(&ErrorKind::AlreadyExists.into()));
        assert_eq!(
            lock_creation_is_contended(&Error::from_raw_os_error(5)),
            cfg!(windows)
        );
        for kind in [
            ErrorKind::NotFound,
            ErrorKind::PermissionDenied,
            ErrorKind::InvalidInput,
        ] {
            assert!(!lock_creation_is_contended(&kind.into()));
        }
    }

    #[cfg(windows)]
    #[test]
    fn transient_lock_access_denial_waits_for_observed_retry_then_acquires() {
        let dir = std::env::temp_dir().join(format!("cargo-ai-attribution-{}", Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");
        let lock = path.with_extension("toml.attribution.lock");
        fs::create_dir(&lock).unwrap();
        let denied = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&lock)
            .unwrap_err();
        assert_eq!(denied.raw_os_error(), Some(5));
        let mut actions = 0;
        let mut retries = 0;
        super::with_environment_lock_inner(
            &path,
            || {
                assert!(lock.is_file());
                actions += 1;
                Ok(())
            },
            |error| {
                if retries == 0 {
                    assert_eq!(error.raw_os_error(), Some(5));
                    assert!(lock.is_dir());
                    fs::remove_dir(&lock).unwrap();
                }
                retries += 1;
            },
        )
        .unwrap();
        assert!(retries >= 1);
        assert_eq!(actions, 1);
        assert!(!lock.exists());
        fs::remove_dir_all(dir).unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn persistent_lock_access_denial_does_not_run_action_or_extend_budget() {
        use std::time::{Duration, Instant};
        let dir = std::env::temp_dir().join(format!("cargo-ai-attribution-{}", Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");
        let lock = path.with_extension("toml.attribution.lock");
        fs::create_dir(&lock).unwrap();
        let denied = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&lock)
            .unwrap_err();
        assert_eq!(denied.raw_os_error(), Some(5));
        let started = Instant::now();
        let mut actions = 0;
        let error = super::with_environment_lock(&path, || {
            actions += 1;
            Ok(())
        })
        .unwrap_err();
        assert!(error.contains("os error 5"), "{error}");
        assert_eq!(actions, 0);
        assert!(started.elapsed() >= Duration::from_secs(3));
        assert!(started.elapsed() < Duration::from_secs(5));
        assert!(lock.is_dir());
        assert!(!path.exists());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn concurrent_initializers_reuse_one_persisted_identity() {
        let dir = std::env::temp_dir().join(format!("cargo-ai-attribution-{}", Uuid::new_v4()));
        let path = Arc::new(dir.join("config.toml"));
        let workers = (0..8)
            .map(|_| {
                let path = Arc::clone(&path);
                thread::spawn(move || ensure_environment_id_at(&path).unwrap().unwrap())
            })
            .collect::<Vec<_>>();
        let ids = workers
            .into_iter()
            .map(|worker| worker.join().unwrap())
            .collect::<Vec<_>>();
        assert!(ids.iter().all(|id| id == &ids[0]));
        assert_eq!(
            read_environment_id_at(&path).unwrap().as_deref(),
            Some(ids[0].as_str())
        );
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn malformed_config_is_not_rewritten() {
        let dir = std::env::temp_dir().join(format!("cargo-ai-attribution-{}", Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");
        fs::write(&path, "profile = [\n").unwrap();
        assert!(ensure_environment_id_at(&path).is_err());
        assert_eq!(fs::read_to_string(&path).unwrap(), "profile = [\n");
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn inspection_of_missing_config_creates_nothing() {
        let dir = std::env::temp_dir().join(format!("cargo-ai-attribution-{}", Uuid::new_v4()));
        let path = dir.join("config.toml");
        assert_eq!(read_environment_id_at(&path).unwrap(), None);
        assert!(!dir.exists());
    }

    #[test]
    fn existing_install_identity_is_reused_without_rewriting_config() {
        let dir = std::env::temp_dir().join(format!("cargo-ai-attribution-{}", Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");
        let original =
            "profile = []\n[cargo_ai_metadata]\ncargo_ai_install_id = \"preserved-id\"\n";
        fs::write(&path, original).unwrap();
        assert_eq!(
            ensure_environment_id_at(&path).unwrap().as_deref(),
            Some("preserved-id")
        );
        assert_eq!(fs::read_to_string(&path).unwrap(), original);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn moved_and_copied_home_keep_existing_identity() {
        let parent = std::env::temp_dir().join(format!("cargo-ai-attribution-{}", Uuid::new_v4()));
        let original = parent.join("original");
        let moved = parent.join("moved");
        let copied = parent.join("copied");
        let original_config = original.join("config.toml");
        let id = ensure_environment_id_at(&original_config).unwrap().unwrap();
        fs::create_dir_all(original.join("usage")).unwrap();
        fs::write(original.join("usage/sentinel"), "history").unwrap();

        fs::rename(&original, &moved).unwrap();
        assert_eq!(
            ensure_environment_id_at(&moved.join("config.toml")).unwrap(),
            Some(id.clone())
        );

        fs::create_dir_all(copied.join("usage")).unwrap();
        fs::copy(moved.join("config.toml"), copied.join("config.toml")).unwrap();
        fs::copy(moved.join("usage/sentinel"), copied.join("usage/sentinel")).unwrap();
        let before = fs::read(copied.join("config.toml")).unwrap();
        assert_eq!(
            ensure_environment_id_at(&copied.join("config.toml")).unwrap(),
            Some(id)
        );
        assert_eq!(fs::read(copied.join("config.toml")).unwrap(), before);
        assert_eq!(
            fs::read_to_string(copied.join("usage/sentinel")).unwrap(),
            "history"
        );
        let _ = fs::remove_dir_all(parent);
    }

    #[test]
    fn legacy_secret_config_is_not_rewritten() {
        let dir = std::env::temp_dir().join(format!("cargo-ai-attribution-{}", Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");
        let original = "profile = []\n[account]\naccess_token = \"sentinel\"\n";
        fs::write(&path, original).unwrap();
        assert!(ensure_environment_id_at(&path).is_err());
        assert_eq!(fs::read_to_string(&path).unwrap(), original);
        let _ = fs::remove_dir_all(dir);
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_config_is_refused_without_changing_target() {
        use std::os::unix::fs::symlink;
        let dir = std::env::temp_dir().join(format!("cargo-ai-attribution-{}", Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        let target = dir.join("target.toml");
        fs::write(&target, "profile = []\n").unwrap();
        let path = dir.join("config.toml");
        symlink(&target, &path).unwrap();
        assert!(ensure_environment_id_at(&path).is_err());
        assert_eq!(fs::read_to_string(&target).unwrap(), "profile = []\n");
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn inherited_workspace_keeps_explicit_none_and_missing_origin() {
        let inherited = InheritedWorkspace {
            workspace: Workspace::None,
            environment_id: None,
        };
        let encoded = serde_json::to_string(&inherited).unwrap();
        let (workspace, origin) = resolve_workspace_context(
            Some(Workspace::Known("/unrelated/child".into())),
            Some(&encoded),
            Some("child-home".to_string()),
            false,
        );
        assert!(matches!(workspace, Workspace::None));
        assert!(origin.is_none());
    }

    #[test]
    fn inherited_workspace_keeps_captured_path_and_parent_origin() {
        let caller = std::env::temp_dir().join("previous-caller");
        let child = std::env::temp_dir().join("unrelated-child");
        let inherited = InheritedWorkspace {
            workspace: Workspace::Known(caller.clone()),
            environment_id: Some("parent-home".to_string()),
        };
        let encoded = serde_json::to_string(&inherited).unwrap();
        let (workspace, origin) = resolve_workspace_context(
            Some(Workspace::Known(child)),
            Some(&encoded),
            Some("child-home".to_string()),
            false,
        );
        let attributed = workspace_attribution(&workspace, origin);
        assert_eq!(attributed.path.as_deref(), caller.to_str());
        assert_eq!(attributed.environment_id.as_deref(), Some("parent-home"));
    }

    #[test]
    fn malformed_inherited_workspace_does_not_capture_child_cwd() {
        let (workspace, origin) = resolve_workspace_context(
            Some(Workspace::Known("/unrelated/child".into())),
            Some("invalid"),
            Some("child-home".to_string()),
            false,
        );
        assert!(
            matches!(workspace, Workspace::Unknown(ref reason) if reason == "inherited_context_invalid")
        );
        assert!(origin.is_none());
    }

    #[test]
    fn old_child_without_workspace_reports_unknown() {
        let (workspace, origin) = resolve_workspace_context(
            Some(Workspace::Known("/unrelated/child".into())),
            None,
            Some("child-home".to_string()),
            true,
        );
        assert!(
            matches!(workspace, Workspace::Unknown(ref reason) if reason == "inherited_context_unavailable")
        );
        assert!(origin.is_none());
    }

    #[test]
    fn equivalent_authored_uuid_spellings_share_package_identity() {
        let lower = "de305d54-75b4-431b-adb2-eb6b9e546014";
        let upper = "DE305D54-75B4-431B-ADB2-EB6B9E546014";
        assert_eq!(
            package_identity(Some(lower), None),
            package_identity(Some(upper), None)
        );
    }

    #[test]
    fn malformed_authored_id_retains_hosted_identity_and_warning() {
        assert_eq!(
            package_identity(Some("invalid"), Some("source-123")),
            (
                Some("hosted:source-123".to_string()),
                Some("hosted_source_id"),
                None
            )
        );
        assert_eq!(
            authored_id_issue(Some("invalid")),
            Some("invalid_authored_project_id")
        );
    }
}
