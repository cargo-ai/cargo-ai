//! Recovery-safe persistence for automatic `config.toml` updates.
//!
//! Automatic writers patch only the fields they own in the raw TOML document.
//! This preserves fields introduced by newer Cargo AI versions while strict
//! loading prevents a malformed or unreadable file from becoming a default
//! config. Credential migration uses the dedicated no-backup scrub path so a
//! secret-bearing legacy document is never copied to `config.toml.bak`.

use crate::config::loader::{
    config_path, load_config_from_path, validate_config_path_safety, ConfigLoad, LoadedConfig,
};
use crate::config::schema::Config;
#[cfg(unix)]
use std::fs::File;
use std::fs::{self, OpenOptions};
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static STAGED_FILE_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[cfg(test)]
thread_local! {
    static TEST_REPLACEMENT_FAILURE_PATH: std::cell::RefCell<Option<PathBuf>> =
        const { std::cell::RefCell::new(None) };
    static TEST_NEW_DESTINATION_CONTENTS: std::cell::RefCell<Option<(PathBuf, String)>> =
        const { std::cell::RefCell::new(None) };
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ConfigWriteOutcome {
    Unchanged,
    Written,
}

pub(crate) fn persist_section_fields(
    section_name: &str,
    fields: &[(&str, Option<toml::Value>)],
) -> Result<ConfigWriteOutcome, String> {
    persist_section_fields_at(&config_path(), section_name, fields)
}

pub(crate) fn persist_section_fields_at(
    path: &Path,
    section_name: &str,
    fields: &[(&str, Option<toml::Value>)],
) -> Result<ConfigWriteOutcome, String> {
    validate_key_name("section", section_name)?;
    for (field_name, _) in fields {
        validate_key_name("field", field_name)?;
    }

    match load_config_from_path(path).map_err(|error| error.to_string())? {
        ConfigLoad::Missing => {
            let mut document = new_config_document();
            let changed = patch_section_fields(&mut document, section_name, fields)?;
            if !changed {
                return Ok(ConfigWriteOutcome::Unchanged);
            }
            persist_new_document(path, &document)
        }
        ConfigLoad::Loaded(loaded) => persist_loaded_section_fields(&loaded, section_name, fields),
    }
}

pub(crate) fn persist_loaded_section_fields(
    loaded: &LoadedConfig,
    section_name: &str,
    fields: &[(&str, Option<toml::Value>)],
) -> Result<ConfigWriteOutcome, String> {
    validate_key_name("section", section_name)?;
    for (field_name, _) in fields {
        validate_key_name("field", field_name)?;
    }
    if contains_legacy_credentials(loaded.document()) {
        return Err(format!(
            "refusing automatic config update for '{}' while legacy credential fields remain; run credential migration first. Cargo AI left the file unchanged",
            loaded.path().display()
        ));
    }

    let mut updated_document = loaded.document().clone();
    if !patch_section_fields(&mut updated_document, section_name, fields)? {
        return Ok(ConfigWriteOutcome::Unchanged);
    }

    let backup_mode = if backup_is_provably_secret_free(loaded.document()) {
        BackupMode::SanitizedRaw
    } else {
        // Unknown fields must remain in the active raw document, but Cargo AI
        // cannot prove that their values are non-secret. The optional managed
        // backup is suppressed instead of guessing.
        BackupMode::None
    };
    persist_loaded_document(loaded, &updated_document, backup_mode)
}

/// Removes legacy credential keys from a strictly loaded raw document and
/// recovery-safely replaces it without creating a pre-scrub backup.
///
/// Callers must persist the extracted credentials before invoking this helper.
pub(crate) fn persist_legacy_credential_scrub(
    loaded: &LoadedConfig,
) -> Result<ConfigWriteOutcome, String> {
    let mut scrubbed = loaded.document().clone();
    if !remove_legacy_credentials(&mut scrubbed) {
        return Ok(ConfigWriteOutcome::Unchanged);
    }

    persist_loaded_document(loaded, &scrubbed, BackupMode::None)
}

/// Reconciles the exact managed `config.toml.bak` before a legacy credential
/// scrub. A strictly valid, fully known, secret-free backup is preserved;
/// anything else is removed without inspecting or traversing sibling recovery
/// directories.
pub(crate) fn reconcile_managed_backup_before_credential_scrub(
    loaded: &LoadedConfig,
) -> Result<(), String> {
    ensure_source_unchanged(loaded)?;
    let path = backup_path(loaded.path())?;
    let metadata = match fs::symlink_metadata(&path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(format!(
                "failed to inspect managed Cargo AI config backup '{}': {error}; active config was left unchanged",
                path.display()
            ));
        }
    };

    let preserve = if metadata.file_type().is_file() {
        fs::read_to_string(&path)
            .ok()
            .is_some_and(|contents| managed_backup_contents_are_provably_safe(&contents))
    } else {
        false
    };

    if !preserve {
        fs::remove_file(&path).map_err(|error| {
            format!(
                "failed to remove unsafe managed Cargo AI config backup '{}': {error}; active config was left unchanged",
                path.display()
            )
        })?;
        sync_parent_directory(&path)?;
    }

    ensure_source_unchanged(loaded)
}

#[derive(Debug, Clone, Copy)]
enum BackupMode {
    None,
    SanitizedRaw,
}

fn validate_key_name(kind: &str, value: &str) -> Result<(), String> {
    if value.trim().is_empty() {
        Err(format!("config {kind} name cannot be empty"))
    } else {
        Ok(())
    }
}

fn new_config_document() -> toml::Value {
    let mut root = toml::Table::new();
    root.insert("profile".to_string(), toml::Value::Array(Vec::new()));
    root.insert(
        "secret_store".to_string(),
        toml::Value::String("file".to_string()),
    );
    toml::Value::Table(root)
}

fn patch_section_fields(
    document: &mut toml::Value,
    section_name: &str,
    fields: &[(&str, Option<toml::Value>)],
) -> Result<bool, String> {
    let root = document
        .as_table_mut()
        .ok_or_else(|| "Cargo AI config must be a top-level TOML table".to_string())?;

    let section_created = if !root.contains_key(section_name) {
        if fields.iter().all(|(_, value)| value.is_none()) {
            return Ok(false);
        }
        root.insert(
            section_name.to_string(),
            toml::Value::Table(toml::Table::new()),
        );
        true
    } else {
        false
    };
    let section = root
        .get_mut(section_name)
        .and_then(toml::Value::as_table_mut)
        .ok_or_else(|| format!("Cargo AI config section '{section_name}' must be a TOML table"))?;

    let mut changed = section_created;
    for (field_name, value) in fields {
        match value {
            Some(value) if section.get(*field_name) != Some(value) => {
                section.insert((*field_name).to_string(), value.clone());
                changed = true;
            }
            Some(_) => {}
            None if section.remove(*field_name).is_some() => changed = true,
            None => {}
        }
    }

    Ok(changed)
}

fn persist_new_document(path: &Path, document: &toml::Value) -> Result<ConfigWriteOutcome, String> {
    validate_config_path_safety(path).map_err(|error| error.to_string())?;
    let serialized = serialize_and_validate(path, document)?;
    let staged = stage_file(path, &serialized)?;
    if let Err(error) = install_staged_new_file(&staged, path) {
        let _ = fs::remove_file(&staged);
        return Err(error);
    }
    sync_parent_directory(path)?;
    Ok(ConfigWriteOutcome::Written)
}

fn persist_loaded_document(
    loaded: &LoadedConfig,
    document: &toml::Value,
    backup_mode: BackupMode,
) -> Result<ConfigWriteOutcome, String> {
    let path = loaded.path();
    validate_config_path_safety(path).map_err(|error| error.to_string())?;
    ensure_source_unchanged(loaded)?;
    let serialized = serialize_and_validate(path, document)?;
    let staged = stage_file(path, &serialized)?;

    if matches!(backup_mode, BackupMode::SanitizedRaw) {
        let backup_path = match backup_path(path) {
            Ok(backup_path) => backup_path,
            Err(error) => {
                let _ = fs::remove_file(&staged);
                return Err(error);
            }
        };
        let backup_document = sanitized_backup_document(loaded.document());
        let backup_serialized = match serialize_and_validate(&backup_path, &backup_document) {
            Ok(serialized) => serialized,
            Err(error) => {
                let _ = fs::remove_file(&staged);
                return Err(error);
            }
        };
        let staged_backup = match stage_file(&backup_path, &backup_serialized) {
            Ok(staged_backup) => staged_backup,
            Err(error) => {
                let _ = fs::remove_file(&staged);
                return Err(error);
            }
        };
        if let Err(error) = replace_staged_file(&staged_backup, &backup_path) {
            let _ = fs::remove_file(&staged_backup);
            let _ = fs::remove_file(&staged);
            return Err(error);
        }
        if let Err(error) = sync_parent_directory(&backup_path) {
            let _ = fs::remove_file(&staged);
            return Err(error);
        }
    }

    // Recheck after preparing the staged config and backup so edits observed
    // before final replacement are not overwritten. This narrows, but cannot
    // eliminate, the final check-to-rename window for non-cooperating writers.
    if let Err(error) = ensure_source_unchanged(loaded) {
        let _ = fs::remove_file(&staged);
        return Err(error);
    }

    if let Err(error) = replace_staged_file(&staged, path) {
        let _ = fs::remove_file(&staged);
        return Err(error);
    }
    sync_parent_directory(path)?;
    Ok(ConfigWriteOutcome::Written)
}

fn serialize_and_validate(path: &Path, document: &toml::Value) -> Result<String, String> {
    let serialized = toml::to_string_pretty(document)
        .map_err(|_| format!("failed to serialize Cargo AI config '{}'", path.display()))?;
    toml::from_str::<toml::Value>(&serialized).map_err(|_| {
        format!(
            "failed to validate staged Cargo AI TOML '{}': invalid TOML syntax",
            path.display()
        )
    })?;
    toml::from_str::<Config>(&serialized).map_err(|_| {
        format!(
            "failed to validate staged Cargo AI config '{}': config does not match the supported schema",
            path.display()
        )
    })?;
    Ok(serialized)
}

fn ensure_source_unchanged(loaded: &LoadedConfig) -> Result<(), String> {
    validate_config_path_safety(loaded.path()).map_err(|error| error.to_string())?;
    let current = fs::read_to_string(loaded.path()).map_err(|error| {
        format!(
            "failed to re-read Cargo AI config '{}' before replacement: {error}. Cargo AI left it unchanged",
            loaded.path().display()
        )
    })?;
    if current != loaded.original_contents() {
        return Err(format!(
            "Cargo AI config '{}' changed while an automatic update was being prepared; retry the command. Cargo AI left the newer file unchanged",
            loaded.path().display()
        ));
    }
    Ok(())
}

fn backup_path(config_path: &Path) -> Result<PathBuf, String> {
    let file_name = config_path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| {
            format!(
                "Cargo AI config path '{}' has no valid file name",
                config_path.display()
            )
        })?;
    Ok(config_path.with_file_name(format!("{file_name}.bak")))
}

fn stage_file(destination: &Path, contents: &str) -> Result<PathBuf, String> {
    validate_config_path_safety(destination).map_err(|error| error.to_string())?;
    let parent = destination.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent).map_err(|error| {
        format!(
            "failed to create Cargo AI config directory '{}': {error}",
            parent.display()
        )
    })?;
    validate_config_path_safety(destination).map_err(|error| error.to_string())?;
    let file_name = destination
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| format!("invalid config destination '{}'", destination.display()))?;

    for _ in 0..16 {
        let sequence = STAGED_FILE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let staged = parent.join(format!(
            ".{file_name}.cargo-ai-{}-{sequence}.tmp",
            std::process::id()
        ));
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }

        match options.open(&staged) {
            Ok(mut file) => {
                let result = (|| {
                    file.write_all(contents.as_bytes()).map_err(|error| {
                        format!(
                            "failed to write staged config '{}': {error}",
                            staged.display()
                        )
                    })?;
                    file.flush().map_err(|error| {
                        format!(
                            "failed to flush staged config '{}': {error}",
                            staged.display()
                        )
                    })?;
                    file.sync_all().map_err(|error| {
                        format!(
                            "failed to sync staged config '{}': {error}",
                            staged.display()
                        )
                    })?;
                    lock_down_file_permissions(&staged)?;
                    validate_staged_file(&staged)?;
                    Ok(())
                })();
                if let Err(error) = result {
                    drop(file);
                    let _ = fs::remove_file(&staged);
                    return Err(error);
                }
                return Ok(staged);
            }
            Err(error) if error.kind() == ErrorKind::AlreadyExists => continue,
            Err(error) => {
                return Err(format!(
                    "failed to create staged config beside '{}': {error}",
                    destination.display()
                ));
            }
        }
    }

    Err(format!(
        "failed to allocate a unique staged config beside '{}'",
        destination.display()
    ))
}

fn install_staged_new_file(staged: &Path, destination: &Path) -> Result<(), String> {
    maybe_create_destination_for_test(destination)?;
    fs::hard_link(staged, destination).map_err(|error| {
        if error.kind() == ErrorKind::AlreadyExists {
            format!(
                "Cargo AI config '{}' was created concurrently; Cargo AI left the newer file unchanged",
                destination.display()
            )
        } else {
            format!(
                "failed to install new Cargo AI config '{}' without replacing an existing file: {error}",
                destination.display()
            )
        }
    })?;
    fs::remove_file(staged).map_err(|error| {
        format!(
            "installed new Cargo AI config '{}' but failed to remove staged link '{}': {error}",
            destination.display(),
            staged.display()
        )
    })
}

fn maybe_create_destination_for_test(destination: &Path) -> Result<(), String> {
    #[cfg(test)]
    {
        let contents = TEST_NEW_DESTINATION_CONTENTS.with(|fixture| {
            let mut fixture = fixture.borrow_mut();
            if fixture
                .as_ref()
                .is_some_and(|(path, _)| path == destination)
            {
                fixture.take().map(|(_, contents)| contents)
            } else {
                None
            }
        });
        if let Some(contents) = contents {
            fs::write(destination, contents).map_err(|error| {
                format!(
                    "failed to create injected concurrent config '{}': {error}",
                    destination.display()
                )
            })?;
        }
    }
    #[cfg(not(test))]
    let _ = destination;
    Ok(())
}

fn validate_staged_file(path: &Path) -> Result<(), String> {
    let staged = fs::read_to_string(path)
        .map_err(|error| format!("failed to read staged config '{}': {error}", path.display()))?;
    toml::from_str::<toml::Value>(&staged).map_err(|_| {
        format!(
            "failed to parse staged TOML '{}': invalid TOML syntax",
            path.display()
        )
    })?;
    toml::from_str::<Config>(&staged).map_err(|_| {
        format!(
            "failed to parse staged config '{}': config does not match the supported schema",
            path.display()
        )
    })?;
    Ok(())
}

#[cfg(unix)]
fn lock_down_file_permissions(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;

    let permissions = fs::Permissions::from_mode(0o600);
    fs::set_permissions(path, permissions).map_err(|error| {
        format!(
            "failed to set owner-only permissions on '{}': {error}",
            path.display()
        )
    })
}

#[cfg(not(unix))]
fn lock_down_file_permissions(_path: &Path) -> Result<(), String> {
    Ok(())
}

fn replace_staged_file(staged: &Path, destination: &Path) -> Result<(), String> {
    maybe_fail_replacement(destination)?;
    replace_staged_file_platform(staged, destination)
}

#[cfg(not(windows))]
fn replace_staged_file_platform(staged: &Path, destination: &Path) -> Result<(), String> {
    fs::rename(staged, destination).map_err(|error| {
        format!(
            "failed to replace Cargo AI config '{}' with staged file '{}': {error}",
            destination.display(),
            staged.display()
        )
    })
}

#[cfg(windows)]
fn replace_staged_file_platform(staged: &Path, destination: &Path) -> Result<(), String> {
    use std::iter;
    use std::os::windows::ffi::OsStrExt;

    const MOVEFILE_REPLACE_EXISTING: u32 = 0x1;
    const MOVEFILE_WRITE_THROUGH: u32 = 0x8;

    #[link(name = "Kernel32")]
    extern "system" {
        fn MoveFileExW(
            existing_file_name: *const u16,
            new_file_name: *const u16,
            flags: u32,
        ) -> i32;
    }

    let staged_wide = staged
        .as_os_str()
        .encode_wide()
        .chain(iter::once(0))
        .collect::<Vec<_>>();
    let destination_wide = destination
        .as_os_str()
        .encode_wide()
        .chain(iter::once(0))
        .collect::<Vec<_>>();
    // SAFETY: both paths are encoded as owned, NUL-terminated UTF-16 buffers
    // that remain alive for the duration of the Windows API call.
    let succeeded = unsafe {
        MoveFileExW(
            staged_wide.as_ptr(),
            destination_wide.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };
    if succeeded == 0 {
        return Err(format!(
            "failed to replace Cargo AI config '{}' with staged file '{}': {}",
            destination.display(),
            staged.display(),
            std::io::Error::last_os_error()
        ));
    }
    Ok(())
}

fn maybe_fail_replacement(destination: &Path) -> Result<(), String> {
    #[cfg(test)]
    {
        let should_fail = TEST_REPLACEMENT_FAILURE_PATH.with(|failure_path| {
            let mut failure_path = failure_path.borrow_mut();
            if failure_path.as_deref() == Some(destination) {
                failure_path.take();
                true
            } else {
                false
            }
        });
        if should_fail {
            return Err(format!(
                "injected failure replacing active Cargo AI config '{}'",
                destination.display()
            ));
        }
    }
    #[cfg(not(test))]
    let _ = destination;
    Ok(())
}

#[cfg(unix)]
fn sync_parent_directory(path: &Path) -> Result<(), String> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    File::open(parent)
        .and_then(|directory| directory.sync_all())
        .map_err(|error| {
            format!(
                "failed to sync Cargo AI config directory '{}': {error}",
                parent.display()
            )
        })
}

#[cfg(not(unix))]
fn sync_parent_directory(_path: &Path) -> Result<(), String> {
    Ok(())
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
}

fn backup_is_provably_secret_free(document: &toml::Value) -> bool {
    const ROOT_KEYS: &[&str] = &[
        "profile",
        "default_profile",
        "secret_store",
        "account",
        "openai_auth",
        "web_resources",
        "update_check",
        "cargo_ai_metadata",
    ];
    const PROFILE_KEYS: &[&str] = &[
        "name",
        "server",
        "model",
        "url",
        "timeout_in_sec",
        "description",
        "auth_mode",
    ];
    const ACCOUNT_KEYS: &[&str] = &["email", "access_token_expires_in", "access_token_issued_at"];
    const OPENAI_AUTH_KEYS: &[&str] = &[
        "access_token_expires_in",
        "access_token_issued_at",
        "locally_disabled",
    ];
    const WEB_RESOURCE_KEYS: &[&str] = &["max_attempts", "base_backoff_ms", "retry_on_empty_body"];
    const UPDATE_CHECK_KEYS: &[&str] = &["mode", "last_checked_unix_seconds", "latest_version"];
    const METADATA_KEYS: &[&str] = &[
        "cargo_ai_version",
        "template_schema_version",
        "cargo_ai_build_target",
        "cargo_ai_install_id",
        "cargo_ai_binary_sha256",
    ];

    let Some(root) = document.as_table() else {
        return false;
    };
    if !table_has_only_keys(root, ROOT_KEYS) {
        return false;
    }
    let profiles_are_known = root
        .get("profile")
        .and_then(toml::Value::as_array)
        .is_some_and(|profiles| {
            profiles.iter().all(|profile| {
                profile.as_table().is_some_and(|profile| {
                    table_has_only_keys(profile, PROFILE_KEYS) && !profile.contains_key("url")
                })
            })
        });
    if !profiles_are_known {
        return false;
    }

    optional_table_has_only_keys(root, "account", ACCOUNT_KEYS)
        && optional_table_has_only_keys(root, "openai_auth", OPENAI_AUTH_KEYS)
        && optional_table_has_only_keys(root, "web_resources", WEB_RESOURCE_KEYS)
        && optional_table_has_only_keys(root, "update_check", UPDATE_CHECK_KEYS)
        && optional_table_has_only_keys(root, "cargo_ai_metadata", METADATA_KEYS)
}

fn managed_backup_contents_are_provably_safe(contents: &str) -> bool {
    let Ok(document) = toml::from_str::<toml::Value>(contents) else {
        return false;
    };
    if toml::from_str::<Config>(contents).is_err() {
        return false;
    }
    backup_is_provably_secret_free(&document)
}

fn table_has_only_keys(table: &toml::Table, allowed: &[&str]) -> bool {
    table.keys().all(|key| allowed.contains(&key.as_str()))
}

fn optional_table_has_only_keys(root: &toml::Table, key: &str, allowed: &[&str]) -> bool {
    match root.get(key) {
        None => true,
        Some(value) => value
            .as_table()
            .is_some_and(|table| table_has_only_keys(table, allowed)),
    }
}

fn remove_legacy_credentials(document: &mut toml::Value) -> bool {
    let Some(root) = document.as_table_mut() else {
        return false;
    };
    let mut changed = false;
    if let Some(profiles) = root.get_mut("profile").and_then(toml::Value::as_array_mut) {
        for profile in profiles {
            if let Some(profile) = profile.as_table_mut() {
                changed |= profile.remove("token").is_some();
            }
        }
    }
    if let Some(account) = root.get_mut("account").and_then(toml::Value::as_table_mut) {
        changed |= account.remove("access_token").is_some();
        changed |= account.remove("refresh_token").is_some();
    }
    changed
}

fn sanitized_backup_document(document: &toml::Value) -> toml::Value {
    let mut sanitized = document.clone();
    remove_legacy_credentials(&mut sanitized);
    if let Some(openai_auth) = sanitized
        .as_table_mut()
        .and_then(|root| root.get_mut("openai_auth"))
        .and_then(toml::Value::as_table_mut)
    {
        openai_auth.remove("access_token");
        openai_auth.remove("refresh_token");
    }
    sanitized
}

#[cfg(test)]
mod tests {
    use super::{persist_legacy_credential_scrub, persist_section_fields_at, ConfigWriteOutcome};
    use crate::config::loader::{load_config_from_path, ConfigLoad};
    use std::fs;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_config_path(stem: &str) -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock should be valid")
            .as_nanos();
        std::env::temp_dir()
            .join(format!("cargo-ai-config-storage-{stem}-{unique}"))
            .join("config.toml")
    }

    fn write_config(path: &std::path::Path, contents: &str) {
        fs::create_dir_all(path.parent().expect("test config should have a parent"))
            .expect("test config parent should be created");
        fs::write(path, contents).expect("test config should be written");
    }

    fn cleanup(path: &std::path::Path) {
        if let Some(parent) = path.parent() {
            let _ = fs::remove_dir_all(parent);
        }
    }

    #[test]
    fn malformed_config_is_left_byte_identical() {
        let path = temp_config_path("malformed");
        let sentinel_secret = "storage-secret-must-not-leak";
        let original = format!("profile = [{{ token = \"{sentinel_secret}\" }}");
        write_config(&path, &original);

        let error = persist_section_fields_at(
            &path,
            "update_check",
            &[("mode", Some(toml::Value::String("check".to_string())))],
        )
        .expect_err("malformed config must block persistence");

        assert!(error.contains(&path.display().to_string()));
        assert!(!error.contains(sentinel_secret));
        assert_eq!(
            fs::read_to_string(&path).expect("config should remain"),
            original
        );
        assert!(!path.with_file_name("config.toml.bak").exists());
        cleanup(&path);
    }

    #[test]
    fn unreadable_text_config_is_left_byte_identical() {
        let path = temp_config_path("invalid-utf8");
        let original = [0xff, 0xfe, 0xfd];
        fs::create_dir_all(path.parent().expect("test config should have a parent"))
            .expect("test config parent should be created");
        fs::write(&path, original).expect("test config bytes should be written");

        let error = persist_section_fields_at(
            &path,
            "update_check",
            &[("mode", Some(toml::Value::String("check".to_string())))],
        )
        .expect_err("non-UTF-8 config must block persistence");

        assert!(error.contains("failed to read"));
        assert!(error.contains(&path.display().to_string()));
        assert_eq!(fs::read(&path).expect("config should remain"), original);
        assert!(!path.with_file_name("config.toml.bak").exists());
        cleanup(&path);
    }

    #[test]
    fn missing_config_does_not_replace_concurrently_created_file() {
        let path = temp_config_path("missing-race");
        let concurrent = "profile = []\n\n[account]\nemail = \"concurrent@example.com\"\n";
        super::TEST_NEW_DESTINATION_CONTENTS.with(|fixture| {
            *fixture.borrow_mut() = Some((path.clone(), concurrent.to_string()));
        });

        let error = persist_section_fields_at(
            &path,
            "cargo_ai_metadata",
            &[(
                "cargo_ai_version",
                Some(toml::Value::String("1.0.0".to_string())),
            )],
        )
        .expect_err("concurrent config creation must win");

        assert!(error.contains("created concurrently"));
        assert_eq!(
            fs::read_to_string(&path).expect("concurrent config should remain"),
            concurrent
        );
        assert!(!path.with_file_name("config.toml.bak").exists());
        let staged_files = fs::read_dir(path.parent().expect("test path should have a parent"))
            .expect("test directory should be readable")
            .filter_map(Result::ok)
            .filter(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with(".config.toml.cargo-ai-")
            })
            .count();
        assert_eq!(
            staged_files, 0,
            "failed no-clobber install must clean staging"
        );
        cleanup(&path);
    }

    #[test]
    fn automatic_patch_preserves_unknown_fields_without_creating_unproven_backup() {
        let path = temp_config_path("preserve");
        let original = r#"profile = [{ name = "dev", server = "openai", model = "gpt-4o" }]
unknown_top_level = "preserve-me"

[update_check]
mode = "check"

[account]
email = "user@example.com"
"#;
        write_config(&path, original);

        let outcome = persist_section_fields_at(
            &path,
            "update_check",
            &[
                ("mode", Some(toml::Value::String("off".to_string()))),
                ("last_checked_unix_seconds", Some(toml::Value::Integer(42))),
            ],
        )
        .expect("valid config should update");

        assert_eq!(outcome, ConfigWriteOutcome::Written);
        let written = fs::read_to_string(&path).expect("updated config should be readable");
        assert!(written.contains("unknown_top_level = \"preserve-me\""));
        assert!(written.contains("mode = \"off\""));
        assert!(written.contains("last_checked_unix_seconds = 42"));
        assert!(
            !path.with_file_name("config.toml.bak").exists(),
            "unknown top-level fields make backup safety unprovable"
        );
        cleanup(&path);
    }

    #[test]
    fn automatic_patch_preserves_unknown_nested_field_without_creating_backup() {
        let path = temp_config_path("nested-unknown");
        let original = r#"profile = [{ name = "dev", server = "openai", model = "gpt-4o", api_key = "unknown-sensitive-value" }]

[update_check]
mode = "check"
"#;
        write_config(&path, original);

        persist_section_fields_at(
            &path,
            "update_check",
            &[("mode", Some(toml::Value::String("off".to_string())))],
        )
        .expect("unknown nested field should not block active raw update");

        let written = fs::read_to_string(&path).expect("updated config should be readable");
        assert!(written.contains("api_key = \"unknown-sensitive-value\""));
        assert!(written.contains("mode = \"off\""));
        assert!(!path.with_file_name("config.toml.bak").exists());
        cleanup(&path);
    }

    #[test]
    fn automatic_patch_preserves_profile_url_without_copying_it_to_backup() {
        let path = temp_config_path("profile-url");
        let original = r#"profile = [{ name = "dev", server = "custom", model = "model", url = "https://user:secret@example.com/api?key=sensitive" }]

[update_check]
mode = "check"
"#;
        write_config(&path, original);

        persist_section_fields_at(
            &path,
            "update_check",
            &[("mode", Some(toml::Value::String("off".to_string())))],
        )
        .expect("profile URL should remain active while state updates");

        let written = fs::read_to_string(&path).expect("updated config should be readable");
        assert!(written.contains("https://user:secret@example.com/api?key=sensitive"));
        assert!(!path.with_file_name("config.toml.bak").exists());
        cleanup(&path);
    }

    #[test]
    fn automatic_patch_backs_up_fully_known_raw_document() {
        let path = temp_config_path("known-backup");
        let original = r#"profile = [{ name = "dev", server = "openai", model = "gpt-4o", description = "known profile" }]
default_profile = "dev"

[update_check]
mode = "check"

[account]
email = "user@example.com"
"#;
        write_config(&path, original);

        persist_section_fields_at(
            &path,
            "update_check",
            &[("mode", Some(toml::Value::String("off".to_string())))],
        )
        .expect("known config should update");

        let backup = fs::read_to_string(path.with_file_name("config.toml.bak"))
            .expect("known prior config should receive a managed backup");
        assert!(backup.contains("description = \"known profile\""));
        assert!(backup.contains("email = \"user@example.com\""));
        assert!(backup.contains("mode = \"check\""));
        assert!(!backup.contains("mode = \"off\""));
        cleanup(&path);
    }

    #[test]
    fn unchanged_patch_leaves_bytes_and_existing_backup_untouched() {
        let path = temp_config_path("unchanged");
        let original = "profile = []\n\n[update_check]\nmode = \"off\"\n";
        write_config(&path, original);
        let backup_path = path.with_file_name("config.toml.bak");
        fs::write(&backup_path, "existing-backup").expect("fixture backup should be written");

        let outcome = persist_section_fields_at(
            &path,
            "update_check",
            &[("mode", Some(toml::Value::String("off".to_string())))],
        )
        .expect("no-op patch should succeed");

        assert_eq!(outcome, ConfigWriteOutcome::Unchanged);
        assert_eq!(
            fs::read_to_string(&path).expect("config should remain"),
            original
        );
        assert_eq!(
            fs::read_to_string(&backup_path).expect("backup should remain"),
            "existing-backup"
        );
        cleanup(&path);
    }

    #[test]
    fn automatic_patch_refuses_legacy_credentials_without_creating_backup() {
        let path = temp_config_path("legacy");
        let original = r#"profile = [{ name = "dev", server = "openai", model = "gpt-4o", token = "secret" }]
"#;
        write_config(&path, original);

        let error = persist_section_fields_at(
            &path,
            "cargo_ai_metadata",
            &[(
                "cargo_ai_version",
                Some(toml::Value::String("1.0.0".to_string())),
            )],
        )
        .expect_err("legacy credentials must block automatic persistence");

        assert!(error.contains("legacy credential fields remain"));
        assert_eq!(
            fs::read_to_string(&path).expect("config should remain"),
            original
        );
        assert!(!path.with_file_name("config.toml.bak").exists());
        cleanup(&path);
    }

    #[test]
    fn automatic_patch_preserves_reserved_token_without_copying_it_to_backup() {
        let path = temp_config_path("reserved-token");
        let original = "profile = []\ncargo_ai_token = \"reserved-value\"\n";
        write_config(&path, original);

        let outcome = persist_section_fields_at(
            &path,
            "cargo_ai_metadata",
            &[(
                "cargo_ai_version",
                Some(toml::Value::String("1.0.0".to_string())),
            )],
        )
        .expect("reserved token should not block an otherwise safe atomic update");

        assert_eq!(outcome, ConfigWriteOutcome::Written);
        let written = fs::read_to_string(&path).expect("updated config should be readable");
        assert!(written.contains("cargo_ai_token = \"reserved-value\""));
        assert!(written.contains("cargo_ai_version = \"1.0.0\""));
        assert!(
            !path.with_file_name("config.toml.bak").exists(),
            "reserved token must not be duplicated into a managed backup"
        );
        cleanup(&path);
    }

    #[test]
    fn active_replacement_failure_preserves_original_and_recoverable_backup() {
        let path = temp_config_path("replace-failure");
        let original = r#"profile = []

[account]
email = "user@example.com"
"#;
        write_config(&path, original);
        super::TEST_REPLACEMENT_FAILURE_PATH.with(|failure_path| {
            *failure_path.borrow_mut() = Some(path.clone());
        });

        let error = persist_section_fields_at(
            &path,
            "cargo_ai_metadata",
            &[(
                "cargo_ai_version",
                Some(toml::Value::String("1.0.0".to_string())),
            )],
        )
        .expect_err("injected active replacement should fail");

        assert!(error.contains("injected failure"));
        assert_eq!(
            fs::read_to_string(&path).expect("original config should remain"),
            original
        );
        let backup = fs::read_to_string(path.with_file_name("config.toml.bak"))
            .expect("recoverable prior config backup should remain");
        assert!(backup.contains("email = \"user@example.com\""));
        assert!(!backup.contains("access_token"));

        let staged_prefix = ".config.toml.cargo-ai-";
        let staged_files = fs::read_dir(path.parent().expect("test path should have a parent"))
            .expect("test directory should be readable")
            .filter_map(Result::ok)
            .filter(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with(staged_prefix)
            })
            .count();
        assert_eq!(
            staged_files, 0,
            "failed replacement must clean staged files"
        );
        cleanup(&path);
    }

    #[test]
    fn migration_scrub_preserves_unknown_fields_without_pre_scrub_backup() {
        let path = temp_config_path("migration");
        let original = r#"profile = [{ name = "dev", server = "openai", model = "gpt-4o", token = "profile-secret", future = "keep" }]
cargo_ai_token = "reserved-preserve"
unknown_top_level = "keep-too"

[account]
email = "user@example.com"
access_token = "account-secret"
refresh_token = "refresh-secret"
future_account_field = "keep-account"
"#;
        write_config(&path, original);
        let ConfigLoad::Loaded(loaded) =
            load_config_from_path(&path).expect("legacy config should strictly load")
        else {
            panic!("legacy config should exist");
        };

        let outcome = persist_legacy_credential_scrub(&loaded)
            .expect("migration scrub should safely replace config");

        assert_eq!(outcome, ConfigWriteOutcome::Written);
        let written = fs::read_to_string(&path).expect("scrubbed config should be readable");
        assert!(!written.contains("profile-secret"));
        assert!(!written.contains("account-secret"));
        assert!(!written.contains("refresh-secret"));
        assert!(written.contains("cargo_ai_token = \"reserved-preserve\""));
        assert!(written.contains("future = \"keep\""));
        assert!(written.contains("unknown_top_level = \"keep-too\""));
        assert!(written.contains("future_account_field = \"keep-account\""));
        assert!(!path.with_file_name("config.toml.bak").exists());
        cleanup(&path);
    }

    #[cfg(unix)]
    #[test]
    fn written_config_and_backup_use_owner_only_permissions() {
        use std::os::unix::fs::PermissionsExt;

        let path = temp_config_path("permissions");
        write_config(&path, "profile = []\n");

        persist_section_fields_at(
            &path,
            "update_check",
            &[("mode", Some(toml::Value::String("check".to_string())))],
        )
        .expect("config should update");

        let config_mode = fs::metadata(&path)
            .expect("config metadata should exist")
            .permissions()
            .mode()
            & 0o777;
        let backup_mode = fs::metadata(path.with_file_name("config.toml.bak"))
            .expect("backup metadata should exist")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(config_mode, 0o600);
        assert_eq!(backup_mode, 0o600);
        cleanup(&path);
    }
}
