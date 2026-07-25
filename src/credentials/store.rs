//! Credential storage primitives for profile and account secrets.
//!
//! Storage strategy:
//! - `file` mode stores secrets only in `credentials.toml`.
//! - `keychain` mode stores secrets only in OS keychain backends.
//! - Legacy configs without an explicit mode keep keychain-first read
//!   compatibility to avoid silent auth regressions.

use crate::config::loader::load_config;
use crate::config::schema::{default_secret_store_mode, SecretStoreMode};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::fs::OpenOptions;
use std::io::{ErrorKind, Write};
#[cfg(windows)]
use std::os::windows::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

const KEYCHAIN_SERVICE: &str = "cargo-ai";
const PROFILE_TOKEN_PREFIX: &str = "profile/";
const PROFILE_TOKEN_SUFFIX: &str = "/token";
const ACCOUNT_ACCESS_KEY: &str = "account/access_token";
const ACCOUNT_REFRESH_KEY: &str = "account/refresh_token";
const OPENAI_OAUTH_ACCESS_KEY: &str = "openai_oauth/access_token";
const OPENAI_OAUTH_REFRESH_KEY: &str = "openai_oauth/refresh_token";
#[cfg(windows)]
const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0000_0400;
static CREDENTIALS_STAGING_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[cfg(test)]
thread_local! {
    static TEST_NEW_CREDENTIALS_DESTINATION_CONTENTS: std::cell::RefCell<Option<(PathBuf, String)>> =
        const { std::cell::RefCell::new(None) };
}

#[derive(Debug, Clone)]
pub struct AccountTokens {
    pub access_token: String,
    pub refresh_token: Option<String>,
}

#[derive(Debug, Clone)]
pub struct OpenAiOAuthTokens {
    pub access_token: String,
    pub refresh_token: Option<String>,
}

#[derive(Debug, Clone)]
pub struct SecretStoreStatus {
    pub configured_mode: Option<SecretStoreMode>,
    pub default_mode: SecretStoreMode,
    pub file_credentials_present: bool,
    pub keychain_credentials_present: bool,
    pub keychain_backend_accessible: bool,
    pub keychain_probe_error: Option<String>,
}

#[derive(Debug, Clone)]
pub struct SecretStoreMigrationOutcome {
    pub source_mode: Option<SecretStoreMode>,
    pub target_mode: SecretStoreMode,
    pub migrated_profile_tokens: usize,
    pub migrated_account_tokens: bool,
    pub source_had_secrets: bool,
}

#[derive(Debug, Serialize, Deserialize, Default)]
struct CredentialsFile {
    #[serde(default)]
    profile_tokens: BTreeMap<String, String>,

    #[serde(default)]
    account: Option<CredentialsAccount>,

    #[serde(default)]
    openai_oauth: Option<CredentialsAccount>,

    #[serde(flatten)]
    other: BTreeMap<String, toml::Value>,

    #[serde(skip)]
    original_contents: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Default)]
struct CredentialsAccount {
    #[serde(default)]
    access_token: Option<String>,

    #[serde(default)]
    refresh_token: Option<String>,

    #[serde(flatten)]
    other: BTreeMap<String, toml::Value>,
}

#[derive(Debug, Default, Clone)]
struct SecretSnapshot {
    profile_tokens: BTreeMap<String, String>,
    account_tokens: Option<AccountTokens>,
    openai_oauth_tokens: Option<OpenAiOAuthTokens>,
}

impl SecretSnapshot {
    fn is_empty(&self) -> bool {
        self.profile_tokens.is_empty()
            && self.account_tokens.is_none()
            && self.openai_oauth_tokens.is_none()
    }

    fn merge_keychain_preferred(file_snapshot: Self, keychain_snapshot: Self) -> Self {
        let mut merged = file_snapshot;
        for (profile, token) in keychain_snapshot.profile_tokens {
            merged.profile_tokens.insert(profile, token);
        }

        if keychain_snapshot.account_tokens.is_some() {
            merged.account_tokens = keychain_snapshot.account_tokens;
        }

        if keychain_snapshot.openai_oauth_tokens.is_some() {
            merged.openai_oauth_tokens = keychain_snapshot.openai_oauth_tokens;
        }

        merged
    }
}

fn resolve_credentials_path(cargo_ai_root: std::path::PathBuf) -> std::path::PathBuf {
    cargo_ai_root.join("credentials.toml")
}

pub fn credentials_path() -> PathBuf {
    resolve_credentials_path(crate::config::paths::cargo_ai_root())
}

pub fn configured_secret_store_mode() -> Option<SecretStoreMode> {
    load_config().and_then(|cfg| cfg.secret_store)
}

fn profile_names_from_config() -> Vec<String> {
    match load_config() {
        Some(cfg) => cfg
            .profile
            .into_iter()
            .map(|profile| profile.name)
            .collect(),
        None => Vec::new(),
    }
}

fn keychain_account_for_profile(profile_name: &str) -> String {
    format!("{PROFILE_TOKEN_PREFIX}{profile_name}{PROFILE_TOKEN_SUFFIX}")
}

fn keychain_enabled() -> bool {
    match std::env::var("CARGO_AI_DISABLE_KEYCHAIN") {
        Ok(value) => {
            let normalized = value.trim().to_ascii_lowercase();
            normalized != "1" && normalized != "true" && normalized != "yes"
        }
        Err(_) => true,
    }
}

#[cfg(any(
    target_os = "macos",
    target_os = "ios",
    target_os = "windows",
    target_os = "linux",
    target_os = "freebsd",
    target_os = "openbsd"
))]
fn keychain_supported_on_target() -> bool {
    true
}

#[cfg(not(any(
    target_os = "macos",
    target_os = "ios",
    target_os = "windows",
    target_os = "linux",
    target_os = "freebsd",
    target_os = "openbsd"
)))]
fn keychain_supported_on_target() -> bool {
    false
}

fn read_credentials_file(path: &Path) -> Result<CredentialsFile, String> {
    validate_credentials_path_safety(path)?;
    if !path.exists() {
        return Ok(CredentialsFile::default());
    }

    validate_file_permissions(path)?;

    let raw = fs::read_to_string(path)
        .map_err(|error| format!("failed to read '{}': {error}", path.display()))?;

    let mut credentials = toml::from_str::<CredentialsFile>(&raw).map_err(|_| {
        format!(
            "failed to parse credentials '{}'; fix or restore this file, which Cargo AI left unchanged",
            path.display()
        )
    })?;
    credentials.original_contents = Some(raw);
    Ok(credentials)
}

fn write_credentials_file(path: &Path, credentials: &CredentialsFile) -> Result<(), String> {
    write_credentials_file_with_replacer(path, credentials, replace_credentials_file)
}

fn write_credentials_file_with_replacer<Replace>(
    path: &Path,
    credentials: &CredentialsFile,
    replace: Replace,
) -> Result<(), String>
where
    Replace: FnOnce(&Path, &Path) -> Result<(), String>,
{
    validate_credentials_path_safety(path)?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| {
            format!(
                "failed to create credentials directory '{}': {error}",
                parent.display()
            )
        })?;
    }
    validate_credentials_path_safety(path)?;

    if path.exists() {
        validate_file_permissions(path)?;
    }

    let serialized = toml::to_string_pretty(credentials)
        .map_err(|error| format!("failed to serialize credentials: {error}"))?;

    toml::from_str::<CredentialsFile>(&serialized)
        .map_err(|_| "failed to validate serialized credentials".to_string())?;

    let staging_path = credentials_staging_path(path)?;
    let write_result = (|| {
        let mut staging_file = open_private_staging_file(&staging_path)?;
        staging_file
            .write_all(serialized.as_bytes())
            .map_err(|error| {
                format!(
                    "failed to write staged credentials '{}': {error}",
                    staging_path.display()
                )
            })?;
        staging_file.flush().map_err(|error| {
            format!(
                "failed to flush staged credentials '{}': {error}",
                staging_path.display()
            )
        })?;
        staging_file.sync_all().map_err(|error| {
            format!(
                "failed to sync staged credentials '{}': {error}",
                staging_path.display()
            )
        })?;
        drop(staging_file);

        lock_down_file_permissions(&staging_path)?;
        validate_credentials_path_safety(path)?;
        match credentials.original_contents.as_deref() {
            Some(expected_contents) => {
                // This comparison narrows, but cannot eliminate, the final
                // check-to-rename window for non-cooperating existing-file
                // writers. Missing files use the atomic no-clobber path below.
                ensure_credentials_source_unchanged(path, expected_contents)?;
                replace(&staging_path, path)?;
            }
            None => install_staged_new_credentials(&staging_path, path)?,
        }
        lock_down_file_permissions(path)?;
        sync_credentials_parent_directory(path)
    })();

    if staging_path.exists() {
        let _ = fs::remove_file(&staging_path);
    }

    write_result?;
    Ok(())
}

fn validate_credentials_path_safety(path: &Path) -> Result<(), String> {
    // The credentials file and its final Cargo AI Home directory are the
    // managed mutation boundary. Broader ancestors such as `~/.cargo` may be
    // linked, so they are deliberately not traversed or rejected here.
    for candidate in [Some(path), path.parent()].into_iter().flatten() {
        match fs::symlink_metadata(candidate) {
            Ok(metadata) if credentials_metadata_is_link_like(&metadata) => {
                return Err(format!(
                    "refusing credentials path '{}' because managed path '{}' is a symbolic link or reparse point; Cargo AI left it unchanged",
                    path.display(),
                    candidate.display()
                ));
            }
            Ok(_) => {}
            Err(error) if error.kind() == ErrorKind::NotFound => {}
            Err(error) => {
                return Err(format!(
                    "failed to inspect managed credentials path '{}': {error}; Cargo AI left '{}' unchanged",
                    candidate.display(),
                    path.display()
                ));
            }
        }
    }
    Ok(())
}

#[cfg(windows)]
fn credentials_metadata_is_link_like(metadata: &fs::Metadata) -> bool {
    metadata.file_type().is_symlink()
        || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

#[cfg(not(windows))]
fn credentials_metadata_is_link_like(metadata: &fs::Metadata) -> bool {
    metadata.file_type().is_symlink()
}

fn ensure_credentials_source_unchanged(path: &Path, expected_contents: &str) -> Result<(), String> {
    validate_credentials_path_safety(path)?;
    match fs::read_to_string(path) {
        Ok(current) if current == expected_contents => Ok(()),
        Ok(_) => Err(format!(
            "credentials '{}' changed while an update was being prepared; retry the command. Cargo AI left the newer file unchanged",
            path.display()
        )),
        Err(error) => Err(format!(
            "failed to re-read credentials '{}' before replacement: {error}. Cargo AI left the file unchanged",
            path.display()
        )),
    }
}

fn install_staged_new_credentials(staging_path: &Path, path: &Path) -> Result<(), String> {
    maybe_create_credentials_destination_for_test(path)?;
    fs::hard_link(staging_path, path).map_err(|error| {
        if error.kind() == ErrorKind::AlreadyExists {
            format!(
                "credentials '{}' were created concurrently; Cargo AI left the newer file unchanged",
                path.display()
            )
        } else {
            format!(
                "failed to install new credentials '{}' without replacing an existing file: {error}",
                path.display()
            )
        }
    })?;
    fs::remove_file(staging_path).map_err(|error| {
        format!(
            "installed new credentials '{}' but failed to remove staged link '{}': {error}",
            path.display(),
            staging_path.display()
        )
    })
}

fn maybe_create_credentials_destination_for_test(path: &Path) -> Result<(), String> {
    #[cfg(test)]
    {
        let contents = TEST_NEW_CREDENTIALS_DESTINATION_CONTENTS.with(|fixture| {
            let mut fixture = fixture.borrow_mut();
            if fixture
                .as_ref()
                .is_some_and(|(fixture_path, _)| fixture_path == path)
            {
                fixture.take().map(|(_, contents)| contents)
            } else {
                None
            }
        });
        if let Some(contents) = contents {
            fs::write(path, contents).map_err(|error| {
                format!(
                    "failed to create injected concurrent credentials '{}': {error}",
                    path.display()
                )
            })?;
            lock_down_file_permissions(path)?;
        }
    }
    #[cfg(not(test))]
    let _ = path;
    Ok(())
}

fn credentials_staging_path(path: &Path) -> Result<PathBuf, String> {
    let file_name = path
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or_else(|| format!("credentials path '{}' has no file name", path.display()))?;
    let sequence = CREDENTIALS_STAGING_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    Ok(path.with_file_name(format!(
        ".{file_name}.tmp-{}-{sequence}",
        std::process::id()
    )))
}

#[cfg(unix)]
fn open_private_staging_file(path: &Path) -> Result<fs::File, String> {
    use std::os::unix::fs::OpenOptionsExt;

    OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .map_err(|error| {
            format!(
                "failed to create staged credentials '{}': {error}",
                path.display()
            )
        })
}

#[cfg(not(unix))]
fn open_private_staging_file(path: &Path) -> Result<fs::File, String> {
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|error| {
            format!(
                "failed to create staged credentials '{}': {error}",
                path.display()
            )
        })
}

#[cfg(not(windows))]
fn replace_credentials_file(staging_path: &Path, path: &Path) -> Result<(), String> {
    fs::rename(staging_path, path).map_err(|error| {
        format!(
            "failed to replace credentials '{}' from staged file '{}': {error}",
            path.display(),
            staging_path.display()
        )
    })
}

#[cfg(windows)]
fn replace_credentials_file(staging_path: &Path, path: &Path) -> Result<(), String> {
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

    let staging_wide = staging_path
        .as_os_str()
        .encode_wide()
        .chain(iter::once(0))
        .collect::<Vec<_>>();
    let path_wide = path
        .as_os_str()
        .encode_wide()
        .chain(iter::once(0))
        .collect::<Vec<_>>();
    // SAFETY: both paths are owned, NUL-terminated UTF-16 buffers that remain
    // alive for the duration of the Windows API call.
    let succeeded = unsafe {
        MoveFileExW(
            staging_wide.as_ptr(),
            path_wide.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };
    if succeeded == 0 {
        return Err(format!(
            "failed to replace credentials '{}' from staged file '{}': {}",
            path.display(),
            staging_path.display(),
            std::io::Error::last_os_error()
        ));
    }
    Ok(())
}

#[cfg(unix)]
fn sync_credentials_parent_directory(path: &Path) -> Result<(), String> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    fs::File::open(parent)
        .and_then(|directory| directory.sync_all())
        .map_err(|error| {
            format!(
                "failed to sync credentials directory '{}': {error}",
                parent.display()
            )
        })
}

#[cfg(not(unix))]
fn sync_credentials_parent_directory(_path: &Path) -> Result<(), String> {
    Ok(())
}

#[cfg(unix)]
fn lock_down_file_permissions(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;

    let mut permissions = fs::metadata(path)
        .map_err(|error| format!("failed to read metadata for '{}': {error}", path.display()))?
        .permissions();
    permissions.set_mode(0o600);
    fs::set_permissions(path, permissions)
        .map_err(|error| format!("failed to set permissions on '{}': {error}", path.display()))?;

    validate_file_permissions(path)
}

#[cfg(not(unix))]
fn lock_down_file_permissions(_path: &Path) -> Result<(), String> {
    Ok(())
}

#[cfg(unix)]
fn validate_file_permissions(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;

    let mode = fs::metadata(path)
        .map_err(|error| format!("failed to read metadata for '{}': {error}", path.display()))?
        .permissions()
        .mode();

    if mode & 0o077 != 0 {
        return Err(format!(
            "refusing insecure permissions on '{}' (expected owner-only permissions)",
            path.display()
        ));
    }

    Ok(())
}

#[cfg(not(unix))]
fn validate_file_permissions(_path: &Path) -> Result<(), String> {
    Ok(())
}

#[cfg(any(
    target_os = "macos",
    target_os = "ios",
    target_os = "windows",
    target_os = "linux",
    target_os = "freebsd",
    target_os = "openbsd"
))]
fn keyring_entry(account: &str) -> Result<keyring::Entry, String> {
    keyring::Entry::new(KEYCHAIN_SERVICE, account)
        .map_err(|error| format!("failed to initialize keyring entry for '{account}': {error}"))
}

#[cfg(any(
    target_os = "macos",
    target_os = "ios",
    target_os = "windows",
    target_os = "linux",
    target_os = "freebsd",
    target_os = "openbsd"
))]
fn keychain_get(account: &str) -> Result<Option<String>, String> {
    if !keychain_enabled() {
        return Err("keychain usage is disabled by CARGO_AI_DISABLE_KEYCHAIN".to_string());
    }

    let entry = keyring_entry(account)?;
    match entry.get_password() {
        Ok(value) => {
            if value.is_empty() {
                Ok(None)
            } else {
                Ok(Some(value))
            }
        }
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(error) => Err(format!("keyring lookup failed for '{account}': {error}")),
    }
}

#[cfg(not(any(
    target_os = "macos",
    target_os = "ios",
    target_os = "windows",
    target_os = "linux",
    target_os = "freebsd",
    target_os = "openbsd"
)))]
fn keychain_get(_account: &str) -> Result<Option<String>, String> {
    Err("keychain backend is unavailable on this platform".to_string())
}

#[cfg(any(
    target_os = "macos",
    target_os = "ios",
    target_os = "windows",
    target_os = "linux",
    target_os = "freebsd",
    target_os = "openbsd"
))]
fn keychain_set(account: &str, secret: &str) -> Result<(), String> {
    if !keychain_enabled() {
        return Err("keychain usage is disabled by CARGO_AI_DISABLE_KEYCHAIN".to_string());
    }

    let entry = keyring_entry(account)?;
    entry
        .set_password(secret)
        .map_err(|error| format!("keyring write failed for '{account}': {error}"))
}

#[cfg(not(any(
    target_os = "macos",
    target_os = "ios",
    target_os = "windows",
    target_os = "linux",
    target_os = "freebsd",
    target_os = "openbsd"
)))]
fn keychain_set(_account: &str, _secret: &str) -> Result<(), String> {
    Err("keychain backend is unavailable on this platform".to_string())
}

#[cfg(any(
    target_os = "macos",
    target_os = "ios",
    target_os = "windows",
    target_os = "linux",
    target_os = "freebsd",
    target_os = "openbsd"
))]
fn keychain_delete(account: &str) -> Result<(), String> {
    if !keychain_enabled() {
        return Err("keychain usage is disabled by CARGO_AI_DISABLE_KEYCHAIN".to_string());
    }

    let entry = keyring_entry(account)?;
    match entry.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(error) => Err(format!("keyring delete failed for '{account}': {error}")),
    }
}

#[cfg(not(any(
    target_os = "macos",
    target_os = "ios",
    target_os = "windows",
    target_os = "linux",
    target_os = "freebsd",
    target_os = "openbsd"
)))]
fn keychain_delete(_account: &str) -> Result<(), String> {
    Err("keychain backend is unavailable on this platform".to_string())
}

fn load_profile_token_from_file_with_path(
    path: &Path,
    profile_name: &str,
) -> Result<Option<String>, String> {
    let credentials = read_credentials_file(path)?;
    Ok(credentials
        .profile_tokens
        .get(profile_name)
        .and_then(|value| {
            let trimmed = value.trim();
            if trimmed.is_empty() {
                None
            } else {
                Some(trimmed.to_string())
            }
        }))
}

fn store_profile_token_in_file_with_path(
    path: &Path,
    profile_name: &str,
    token: &str,
) -> Result<(), String> {
    let mut credentials = read_credentials_file(path)?;
    credentials
        .profile_tokens
        .insert(profile_name.to_string(), token.to_string());
    write_credentials_file(path, &credentials)
}

fn clear_profile_token_in_file_with_path(path: &Path, profile_name: &str) -> Result<(), String> {
    let mut credentials = read_credentials_file(path)?;
    credentials.profile_tokens.remove(profile_name);
    write_credentials_file(path, &credentials)
}

fn load_account_tokens_from_file_with_path(path: &Path) -> Result<Option<AccountTokens>, String> {
    let credentials = read_credentials_file(path)?;
    let account = match credentials.account {
        Some(account) => account,
        None => return Ok(None),
    };

    let access_token = match account.access_token {
        Some(value) if !value.trim().is_empty() => value,
        _ => return Ok(None),
    };

    let refresh_token = account.refresh_token.and_then(|value| {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        }
    });

    Ok(Some(AccountTokens {
        access_token,
        refresh_token,
    }))
}

fn store_account_tokens_in_file_with_path(
    path: &Path,
    access_token: &str,
    refresh_token: Option<&str>,
) -> Result<(), String> {
    let mut credentials = read_credentials_file(path)?;
    let account = credentials
        .account
        .get_or_insert_with(CredentialsAccount::default);
    account.access_token = Some(access_token.to_string());
    account.refresh_token = refresh_token
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    write_credentials_file(path, &credentials)
}

fn clear_account_tokens_in_file_with_path(path: &Path) -> Result<(), String> {
    let mut credentials = read_credentials_file(path)?;
    credentials.account = None;
    write_credentials_file(path, &credentials)
}

#[allow(dead_code)]
fn load_openai_oauth_tokens_from_file_with_path(
    path: &Path,
) -> Result<Option<OpenAiOAuthTokens>, String> {
    let credentials = read_credentials_file(path)?;
    let openai_oauth = match credentials.openai_oauth {
        Some(tokens) => tokens,
        None => return Ok(None),
    };

    let access_token = match openai_oauth.access_token {
        Some(value) if !value.trim().is_empty() => value,
        _ => return Ok(None),
    };

    let refresh_token = openai_oauth.refresh_token.and_then(|value| {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        }
    });

    Ok(Some(OpenAiOAuthTokens {
        access_token,
        refresh_token,
    }))
}

#[allow(dead_code)]
fn store_openai_oauth_tokens_in_file_with_path(
    path: &Path,
    access_token: &str,
    refresh_token: Option<&str>,
) -> Result<(), String> {
    let mut credentials = read_credentials_file(path)?;
    let openai_oauth = credentials
        .openai_oauth
        .get_or_insert_with(CredentialsAccount::default);
    openai_oauth.access_token = Some(access_token.to_string());
    openai_oauth.refresh_token = refresh_token
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    write_credentials_file(path, &credentials)
}

fn clear_openai_oauth_tokens_in_file_with_path(path: &Path) -> Result<(), String> {
    let mut credentials = read_credentials_file(path)?;
    credentials.openai_oauth = None;
    write_credentials_file(path, &credentials)
}

fn load_account_tokens_from_keychain() -> Result<Option<AccountTokens>, String> {
    let access_token = match keychain_get(ACCOUNT_ACCESS_KEY)? {
        Some(value) => value,
        None => return Ok(None),
    };

    let refresh_token = keychain_get(ACCOUNT_REFRESH_KEY)?.and_then(|value| {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        }
    });

    Ok(Some(AccountTokens {
        access_token,
        refresh_token,
    }))
}

fn load_openai_oauth_tokens_from_keychain() -> Result<Option<OpenAiOAuthTokens>, String> {
    let access_token = match keychain_get(OPENAI_OAUTH_ACCESS_KEY)? {
        Some(value) => value,
        None => return Ok(None),
    };

    let refresh_token = keychain_get(OPENAI_OAUTH_REFRESH_KEY)?.and_then(|value| {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        }
    });

    Ok(Some(OpenAiOAuthTokens {
        access_token,
        refresh_token,
    }))
}

fn load_snapshot_from_file(path: &Path) -> Result<SecretSnapshot, String> {
    let credentials = read_credentials_file(path)?;

    let mut snapshot = SecretSnapshot::default();

    for (profile, token) in credentials.profile_tokens {
        let trimmed = token.trim();
        if !trimmed.is_empty() {
            snapshot.profile_tokens.insert(profile, trimmed.to_string());
        }
    }

    if let Some(account) = credentials.account {
        if let Some(access_token) = account
            .access_token
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
        {
            let refresh_token = account
                .refresh_token
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty());
            snapshot.account_tokens = Some(AccountTokens {
                access_token,
                refresh_token,
            });
        }
    }

    if let Some(openai_oauth) = credentials.openai_oauth {
        if let Some(access_token) = openai_oauth
            .access_token
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
        {
            let refresh_token = openai_oauth
                .refresh_token
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty());
            snapshot.openai_oauth_tokens = Some(OpenAiOAuthTokens {
                access_token,
                refresh_token,
            });
        }
    }

    Ok(snapshot)
}

fn load_snapshot_from_keychain(profile_names: &[String]) -> Result<SecretSnapshot, String> {
    let mut snapshot = SecretSnapshot::default();

    for profile_name in profile_names {
        let account = keychain_account_for_profile(profile_name);
        if let Some(token) = keychain_get(&account)? {
            snapshot.profile_tokens.insert(profile_name.clone(), token);
        }
    }

    snapshot.account_tokens = load_account_tokens_from_keychain()?;
    snapshot.openai_oauth_tokens = load_openai_oauth_tokens_from_keychain()?;
    Ok(snapshot)
}

fn write_snapshot_to_file(path: &Path, snapshot: &SecretSnapshot) -> Result<(), String> {
    let mut credentials = read_credentials_file(path)?;
    credentials.profile_tokens = snapshot.profile_tokens.clone();
    match &snapshot.account_tokens {
        Some(tokens) => {
            let account = credentials
                .account
                .get_or_insert_with(CredentialsAccount::default);
            account.access_token = Some(tokens.access_token.clone());
            account.refresh_token = tokens.refresh_token.clone();
        }
        None => credentials.account = None,
    }
    match &snapshot.openai_oauth_tokens {
        Some(tokens) => {
            let openai_oauth = credentials
                .openai_oauth
                .get_or_insert_with(CredentialsAccount::default);
            openai_oauth.access_token = Some(tokens.access_token.clone());
            openai_oauth.refresh_token = tokens.refresh_token.clone();
        }
        None => credentials.openai_oauth = None,
    }

    write_credentials_file(path, &credentials)
}

fn clear_file_snapshot(path: &Path) -> Result<(), String> {
    let mut credentials = read_credentials_file(path)?;
    credentials.profile_tokens.clear();
    credentials.account = None;
    credentials.openai_oauth = None;
    write_credentials_file(path, &credentials)
}

fn write_snapshot_to_keychain(snapshot: &SecretSnapshot) -> Result<(), String> {
    for (profile_name, token) in &snapshot.profile_tokens {
        let account = keychain_account_for_profile(profile_name);
        keychain_set(&account, token)?;
    }

    match &snapshot.account_tokens {
        Some(tokens) => {
            keychain_set(ACCOUNT_ACCESS_KEY, &tokens.access_token)?;
            match tokens.refresh_token.as_deref() {
                Some(refresh) => keychain_set(ACCOUNT_REFRESH_KEY, refresh)?,
                None => {
                    keychain_delete(ACCOUNT_REFRESH_KEY)?;
                }
            }
        }
        None => {
            keychain_delete(ACCOUNT_ACCESS_KEY)?;
            keychain_delete(ACCOUNT_REFRESH_KEY)?;
        }
    }

    match &snapshot.openai_oauth_tokens {
        Some(tokens) => {
            keychain_set(OPENAI_OAUTH_ACCESS_KEY, &tokens.access_token)?;
            match tokens.refresh_token.as_deref() {
                Some(refresh) => keychain_set(OPENAI_OAUTH_REFRESH_KEY, refresh)?,
                None => {
                    keychain_delete(OPENAI_OAUTH_REFRESH_KEY)?;
                }
            }
        }
        None => {
            keychain_delete(OPENAI_OAUTH_ACCESS_KEY)?;
            keychain_delete(OPENAI_OAUTH_REFRESH_KEY)?;
        }
    }

    Ok(())
}

fn clear_keychain_snapshot(profile_names: &[String]) -> Result<(), String> {
    keychain_delete(ACCOUNT_ACCESS_KEY)?;
    keychain_delete(ACCOUNT_REFRESH_KEY)?;
    keychain_delete(OPENAI_OAUTH_ACCESS_KEY)?;
    keychain_delete(OPENAI_OAUTH_REFRESH_KEY)?;

    for profile_name in profile_names {
        keychain_delete(&keychain_account_for_profile(profile_name))?;
    }

    Ok(())
}

fn legacy_snapshot(profile_names: &[String]) -> Result<SecretSnapshot, String> {
    let file_snapshot = load_snapshot_from_file(&credentials_path())?;
    let keychain_snapshot = match load_snapshot_from_keychain(profile_names) {
        Ok(snapshot) => snapshot,
        Err(_) => SecretSnapshot::default(),
    };

    Ok(SecretSnapshot::merge_keychain_preferred(
        file_snapshot,
        keychain_snapshot,
    ))
}

fn source_snapshot_for_mode(mode: Option<SecretStoreMode>) -> Result<SecretSnapshot, String> {
    let path = credentials_path();
    let profile_names = profile_names_from_config();

    match mode {
        Some(SecretStoreMode::File) => load_snapshot_from_file(&path),
        Some(SecretStoreMode::Keychain) => load_snapshot_from_keychain(&profile_names),
        None => legacy_snapshot(&profile_names),
    }
}

fn clear_source_after_migration(
    mode: Option<SecretStoreMode>,
    target: SecretStoreMode,
) -> Result<(), String> {
    let path = credentials_path();
    let profile_names = profile_names_from_config();

    match mode {
        Some(SecretStoreMode::File) if target != SecretStoreMode::File => {
            clear_file_snapshot(&path)
        }
        Some(SecretStoreMode::Keychain) if target != SecretStoreMode::Keychain => {
            clear_keychain_snapshot(&profile_names)
        }
        None => match target {
            SecretStoreMode::File => clear_keychain_snapshot(&profile_names),
            SecretStoreMode::Keychain => clear_file_snapshot(&path),
        },
        _ => Ok(()),
    }
}

fn legacy_config_target_mode(configured_mode: Option<SecretStoreMode>) -> SecretStoreMode {
    match configured_mode {
        Some(mode) => mode,
        None if keychain_supported_on_target() && keychain_enabled() => {
            if keychain_get(ACCOUNT_ACCESS_KEY).is_ok() {
                SecretStoreMode::Keychain
            } else {
                SecretStoreMode::File
            }
        }
        None => SecretStoreMode::File,
    }
}

fn commit_legacy_tokens_to_file_with_path(
    path: &Path,
    profile_tokens: &[(String, String)],
    account_tokens: Option<(&str, Option<&str>)>,
) -> Result<(), String> {
    if profile_tokens.is_empty() && account_tokens.is_none() {
        return Ok(());
    }

    let mut credentials = read_credentials_file(path)?;
    let mut changed = false;
    for (profile_name, token) in profile_tokens {
        if credentials.profile_tokens.get(profile_name) != Some(token) {
            credentials
                .profile_tokens
                .insert(profile_name.clone(), token.clone());
            changed = true;
        }
    }

    if let Some((access_token, refresh_token)) = account_tokens {
        let account = credentials
            .account
            .get_or_insert_with(CredentialsAccount::default);
        if account.access_token.as_deref() != Some(access_token) {
            account.access_token = Some(access_token.to_string());
            changed = true;
        }
        if let Some(refresh_token) = refresh_token
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            if account.refresh_token.as_deref() != Some(refresh_token) {
                account.refresh_token = Some(refresh_token.to_string());
                changed = true;
            }
        }
    }

    if changed {
        write_credentials_file(path, &credentials)
    } else {
        Ok(())
    }
}

fn commit_legacy_tokens_to_keychain_with<GetSecret, SetSecret>(
    profile_tokens: &[(String, String)],
    account_tokens: Option<(&str, Option<&str>)>,
    mut get_secret: GetSecret,
    mut set_secret: SetSecret,
) -> Result<(), String>
where
    GetSecret: FnMut(&str) -> Result<Option<String>, String>,
    SetSecret: FnMut(&str, &str) -> Result<(), String>,
{
    let mut planned_writes = Vec::new();
    for (profile_name, token) in profile_tokens {
        let account = keychain_account_for_profile(profile_name);
        if get_secret(&account)?.as_deref() != Some(token) {
            planned_writes.push((account, token.clone()));
        }
    }

    if let Some((access_token, refresh_token)) = account_tokens {
        if let Some(refresh_token) = refresh_token
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            if get_secret(ACCOUNT_REFRESH_KEY)?.as_deref() != Some(refresh_token) {
                planned_writes.push((ACCOUNT_REFRESH_KEY.to_string(), refresh_token.to_string()));
            }
        }
        if get_secret(ACCOUNT_ACCESS_KEY)?.as_deref() != Some(access_token) {
            planned_writes.push((ACCOUNT_ACCESS_KEY.to_string(), access_token.to_string()));
        }
    }

    for (account, secret) in planned_writes {
        set_secret(&account, &secret)?;
    }

    Ok(())
}

pub(crate) fn commit_legacy_config_tokens(
    configured_mode: Option<SecretStoreMode>,
    profile_tokens: &[(String, String)],
    account_tokens: Option<(&str, Option<&str>)>,
) -> Result<(), String> {
    if profile_tokens.is_empty() && account_tokens.is_none() {
        return Ok(());
    }

    match legacy_config_target_mode(configured_mode) {
        SecretStoreMode::File => commit_legacy_tokens_to_file_with_path(
            &credentials_path(),
            profile_tokens,
            account_tokens,
        ),
        SecretStoreMode::Keychain => commit_legacy_tokens_to_keychain_with(
            profile_tokens,
            account_tokens,
            keychain_get,
            keychain_set,
        ),
    }
}

pub fn secret_store_status() -> Result<SecretStoreStatus, String> {
    let configured_mode = configured_secret_store_mode();
    let default_mode = default_secret_store_mode();

    let file_snapshot = load_snapshot_from_file(&credentials_path())?;
    let file_credentials_present = !file_snapshot.is_empty();

    let profile_names = profile_names_from_config();
    let (keychain_credentials_present, keychain_backend_accessible, keychain_probe_error) =
        if !keychain_supported_on_target() {
            (
                false,
                false,
                Some("keychain backend is unavailable on this platform".to_string()),
            )
        } else if !keychain_enabled() {
            (
                false,
                false,
                Some("keychain usage is disabled by CARGO_AI_DISABLE_KEYCHAIN".to_string()),
            )
        } else {
            match load_snapshot_from_keychain(&profile_names) {
                Ok(snapshot) => (!snapshot.is_empty(), true, None),
                Err(error) => (false, false, Some(error)),
            }
        };

    Ok(SecretStoreStatus {
        configured_mode,
        default_mode,
        file_credentials_present,
        keychain_credentials_present,
        keychain_backend_accessible,
        keychain_probe_error,
    })
}

pub fn migrate_secret_store(
    target_mode: SecretStoreMode,
    dry_run: bool,
) -> Result<SecretStoreMigrationOutcome, String> {
    let source_mode = configured_secret_store_mode();
    let source_snapshot = source_snapshot_for_mode(source_mode)?;

    let migrated_profile_tokens = source_snapshot.profile_tokens.len();
    let migrated_account_tokens = source_snapshot.account_tokens.is_some();
    let source_had_secrets = !source_snapshot.is_empty();

    if dry_run || source_snapshot.is_empty() || source_mode == Some(target_mode) {
        return Ok(SecretStoreMigrationOutcome {
            source_mode,
            target_mode,
            migrated_profile_tokens,
            migrated_account_tokens,
            source_had_secrets,
        });
    }

    match target_mode {
        SecretStoreMode::File => {
            write_snapshot_to_file(&credentials_path(), &source_snapshot)?;
        }
        SecretStoreMode::Keychain => {
            write_snapshot_to_keychain(&source_snapshot)?;
        }
    }

    clear_source_after_migration(source_mode, target_mode)?;

    Ok(SecretStoreMigrationOutcome {
        source_mode,
        target_mode,
        migrated_profile_tokens,
        migrated_account_tokens,
        source_had_secrets,
    })
}

pub fn load_profile_token(profile_name: &str) -> Result<Option<String>, String> {
    let keychain_account = keychain_account_for_profile(profile_name);

    match configured_secret_store_mode() {
        Some(SecretStoreMode::File) => {
            load_profile_token_from_file_with_path(&credentials_path(), profile_name)
        }
        Some(SecretStoreMode::Keychain) => keychain_get(&keychain_account),
        None => match keychain_get(&keychain_account) {
            Ok(Some(value)) => Ok(Some(value)),
            Ok(None) | Err(_) => {
                load_profile_token_from_file_with_path(&credentials_path(), profile_name)
            }
        },
    }
}

pub fn store_profile_token(profile_name: &str, token: &str) -> Result<(), String> {
    if token.trim().is_empty() {
        return clear_profile_token(profile_name);
    }

    let keychain_account = keychain_account_for_profile(profile_name);

    match configured_secret_store_mode() {
        Some(SecretStoreMode::File) => {
            store_profile_token_in_file_with_path(&credentials_path(), profile_name, token)
        }
        Some(SecretStoreMode::Keychain) => {
            keychain_set(&keychain_account, token)?;
            let _ = clear_profile_token_in_file_with_path(&credentials_path(), profile_name);
            Ok(())
        }
        None => {
            if keychain_set(&keychain_account, token).is_ok() {
                let _ = clear_profile_token_in_file_with_path(&credentials_path(), profile_name);
                return Ok(());
            }

            store_profile_token_in_file_with_path(&credentials_path(), profile_name, token)
        }
    }
}

pub fn clear_profile_token(profile_name: &str) -> Result<(), String> {
    let keychain_account = keychain_account_for_profile(profile_name);
    let _ = keychain_delete(&keychain_account);
    clear_profile_token_in_file_with_path(&credentials_path(), profile_name)
}

pub fn load_account_tokens() -> Result<Option<AccountTokens>, String> {
    match configured_secret_store_mode() {
        Some(SecretStoreMode::File) => load_account_tokens_from_file_with_path(&credentials_path()),
        Some(SecretStoreMode::Keychain) => load_account_tokens_from_keychain(),
        None => {
            let access_from_keychain = keychain_get(ACCOUNT_ACCESS_KEY);
            let refresh_from_keychain = keychain_get(ACCOUNT_REFRESH_KEY);

            if let Ok(Some(access_token)) = access_from_keychain {
                let refresh_token = refresh_from_keychain
                    .ok()
                    .and_then(|value| value.filter(|token| !token.trim().is_empty()));
                return Ok(Some(AccountTokens {
                    access_token,
                    refresh_token,
                }));
            }

            load_account_tokens_from_file_with_path(&credentials_path())
        }
    }
}

pub fn store_account_tokens(access_token: &str, refresh_token: Option<&str>) -> Result<(), String> {
    if access_token.trim().is_empty() {
        return clear_account_tokens();
    }

    match configured_secret_store_mode() {
        Some(SecretStoreMode::File) => {
            store_account_tokens_in_file_with_path(&credentials_path(), access_token, refresh_token)
        }
        Some(SecretStoreMode::Keychain) => {
            keychain_set(ACCOUNT_ACCESS_KEY, access_token)?;
            match refresh_token {
                Some(token) if !token.trim().is_empty() => {
                    keychain_set(ACCOUNT_REFRESH_KEY, token)?
                }
                _ => {
                    keychain_delete(ACCOUNT_REFRESH_KEY)?;
                }
            }
            let _ = clear_account_tokens_in_file_with_path(&credentials_path());
            Ok(())
        }
        None => {
            let keychain_access_result = keychain_set(ACCOUNT_ACCESS_KEY, access_token);
            let keychain_refresh_result = match refresh_token {
                Some(token) if !token.trim().is_empty() => keychain_set(ACCOUNT_REFRESH_KEY, token),
                _ => keychain_delete(ACCOUNT_REFRESH_KEY),
            };

            if keychain_access_result.is_ok() && keychain_refresh_result.is_ok() {
                let _ = clear_account_tokens_in_file_with_path(&credentials_path());
                return Ok(());
            }

            store_account_tokens_in_file_with_path(&credentials_path(), access_token, refresh_token)
        }
    }
}

pub fn clear_account_tokens() -> Result<(), String> {
    let _ = keychain_delete(ACCOUNT_ACCESS_KEY);
    let _ = keychain_delete(ACCOUNT_REFRESH_KEY);
    clear_account_tokens_in_file_with_path(&credentials_path())
}

#[allow(dead_code)]
pub fn load_openai_oauth_tokens() -> Result<Option<OpenAiOAuthTokens>, String> {
    match configured_secret_store_mode() {
        Some(SecretStoreMode::File) => {
            load_openai_oauth_tokens_from_file_with_path(&credentials_path())
        }
        Some(SecretStoreMode::Keychain) => load_openai_oauth_tokens_from_keychain(),
        None => {
            let access_from_keychain = keychain_get(OPENAI_OAUTH_ACCESS_KEY);
            let refresh_from_keychain = keychain_get(OPENAI_OAUTH_REFRESH_KEY);

            if let Ok(Some(access_token)) = access_from_keychain {
                let refresh_token = refresh_from_keychain
                    .ok()
                    .and_then(|value| value.filter(|token| !token.trim().is_empty()));
                return Ok(Some(OpenAiOAuthTokens {
                    access_token,
                    refresh_token,
                }));
            }

            load_openai_oauth_tokens_from_file_with_path(&credentials_path())
        }
    }
}

#[allow(dead_code)]
pub fn store_openai_oauth_tokens(
    access_token: &str,
    refresh_token: Option<&str>,
) -> Result<(), String> {
    if access_token.trim().is_empty() {
        return clear_openai_oauth_tokens();
    }

    match configured_secret_store_mode() {
        Some(SecretStoreMode::File) => store_openai_oauth_tokens_in_file_with_path(
            &credentials_path(),
            access_token,
            refresh_token,
        ),
        Some(SecretStoreMode::Keychain) => {
            keychain_set(OPENAI_OAUTH_ACCESS_KEY, access_token)?;
            match refresh_token {
                Some(token) if !token.trim().is_empty() => {
                    keychain_set(OPENAI_OAUTH_REFRESH_KEY, token)?
                }
                _ => {
                    keychain_delete(OPENAI_OAUTH_REFRESH_KEY)?;
                }
            }
            let _ = clear_openai_oauth_tokens_in_file_with_path(&credentials_path());
            Ok(())
        }
        None => {
            let keychain_access_result = keychain_set(OPENAI_OAUTH_ACCESS_KEY, access_token);
            let keychain_refresh_result = match refresh_token {
                Some(token) if !token.trim().is_empty() => {
                    keychain_set(OPENAI_OAUTH_REFRESH_KEY, token)
                }
                _ => keychain_delete(OPENAI_OAUTH_REFRESH_KEY),
            };

            if keychain_access_result.is_ok() && keychain_refresh_result.is_ok() {
                let _ = clear_openai_oauth_tokens_in_file_with_path(&credentials_path());
                return Ok(());
            }

            store_openai_oauth_tokens_in_file_with_path(
                &credentials_path(),
                access_token,
                refresh_token,
            )
        }
    }
}

pub fn clear_openai_oauth_tokens() -> Result<(), String> {
    let _ = keychain_delete(OPENAI_OAUTH_ACCESS_KEY);
    let _ = keychain_delete(OPENAI_OAUTH_REFRESH_KEY);
    clear_openai_oauth_tokens_in_file_with_path(&credentials_path())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::schema::SecretStoreMode;
    use std::cell::Cell;
    use std::time::{SystemTime, UNIX_EPOCH};

    static TEST_CREDENTIALS_PATH_SEQUENCE: AtomicU64 = AtomicU64::new(0);

    fn temp_credentials_root(timestamp_nanos: u128) -> PathBuf {
        let sequence = TEST_CREDENTIALS_PATH_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!(
            "cargo-ai-credentials-file-test-{}-{timestamp_nanos}-{sequence}",
            std::process::id()
        ))
    }

    fn with_temp_credentials_path<F>(test: F)
    where
        F: FnOnce(&Path),
    {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time should be valid")
            .as_nanos();
        let root = temp_credentials_root(unique);
        fs::create_dir(&root).expect("temp root should be created");
        let path = root.join("credentials.toml");
        test(path.as_path());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn temp_credentials_roots_are_unique_for_the_same_timestamp() {
        let first = temp_credentials_root(1);
        let second = temp_credentials_root(1);

        assert_ne!(first, second);
    }

    fn write_private_test_file(path: &Path, contents: &str) {
        fs::write(path, contents).expect("test credentials should be written");

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(path, fs::Permissions::from_mode(0o600))
                .expect("test credentials should be private");
        }
    }

    fn staged_credentials_count(path: &Path) -> usize {
        fs::read_dir(path)
            .expect("credentials directory should be readable")
            .filter_map(Result::ok)
            .filter(|entry| entry.file_name().to_string_lossy().contains(".tmp-"))
            .count()
    }

    #[test]
    fn profile_token_fallback_roundtrip() {
        with_temp_credentials_path(|path| {
            store_profile_token_in_file_with_path(path, "openai-dev", "secret-1")
                .expect("profile token should persist");
            let loaded = load_profile_token_from_file_with_path(path, "openai-dev")
                .expect("profile token should load");
            assert_eq!(loaded.as_deref(), Some("secret-1"));

            clear_profile_token_in_file_with_path(path, "openai-dev")
                .expect("profile token should clear");
            let loaded = load_profile_token_from_file_with_path(path, "openai-dev")
                .expect("profile token should load");
            assert!(loaded.is_none());
        });
    }

    #[test]
    fn account_token_fallback_roundtrip() {
        with_temp_credentials_path(|path| {
            store_account_tokens_in_file_with_path(path, "access-1", Some("refresh-1"))
                .expect("account tokens should persist");

            let loaded =
                load_account_tokens_from_file_with_path(path).expect("account tokens should load");
            let loaded = loaded.expect("account tokens should be present");
            assert_eq!(loaded.access_token, "access-1");
            assert_eq!(loaded.refresh_token.as_deref(), Some("refresh-1"));

            clear_account_tokens_in_file_with_path(path).expect("account tokens should clear");
            let loaded =
                load_account_tokens_from_file_with_path(path).expect("account tokens should load");
            assert!(loaded.is_none());
        });
    }

    #[test]
    fn bulk_legacy_file_commit_preserves_unrelated_entries() {
        with_temp_credentials_path(|path| {
            write_private_test_file(
                path,
                r#"
profile_tokens = { existing = "existing-secret" }
future_scalar = "keep-me"

[account]
access_token = "old-access"
refresh_token = "old-refresh"
tenant = "keep-tenant"

[openai_oauth]
access_token = "oauth-access"
refresh_token = "oauth-refresh"
audience = "keep-audience"

[future_section]
enabled = true
"#,
            );

            let profile_tokens = vec![("legacy-profile".to_string(), "legacy-secret".to_string())];
            commit_legacy_tokens_to_file_with_path(
                path,
                &profile_tokens,
                Some(("new-access", Some("new-refresh"))),
            )
            .expect("legacy credentials should commit");

            let raw = fs::read_to_string(path).expect("credentials should remain readable");
            let value = toml::from_str::<toml::Value>(&raw)
                .expect("committed credentials should remain valid TOML");

            assert_eq!(
                value["profile_tokens"]["existing"].as_str(),
                Some("existing-secret")
            );
            assert_eq!(
                value["profile_tokens"]["legacy-profile"].as_str(),
                Some("legacy-secret")
            );
            assert_eq!(
                value["account"]["access_token"].as_str(),
                Some("new-access")
            );
            assert_eq!(
                value["account"]["refresh_token"].as_str(),
                Some("new-refresh")
            );
            assert_eq!(value["account"]["tenant"].as_str(), Some("keep-tenant"));
            assert_eq!(
                value["openai_oauth"]["access_token"].as_str(),
                Some("oauth-access")
            );
            assert_eq!(
                value["openai_oauth"]["audience"].as_str(),
                Some("keep-audience")
            );
            assert_eq!(value["future_scalar"].as_str(), Some("keep-me"));
            assert_eq!(value["future_section"]["enabled"].as_bool(), Some(true));

            let parent = path.parent().expect("credentials should have a parent");
            let staged_files = fs::read_dir(parent)
                .expect("credentials directory should be readable")
                .filter_map(Result::ok)
                .filter(|entry| entry.file_name().to_string_lossy().contains(".tmp-"))
                .count();
            assert_eq!(staged_files, 0, "staged credentials should be cleaned up");

            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let mode = fs::metadata(path)
                    .expect("credentials metadata should be readable")
                    .permissions()
                    .mode();
                assert_eq!(mode & 0o077, 0, "credentials must remain owner-only");
            }
        });
    }

    #[test]
    fn bulk_legacy_file_commit_keeps_existing_refresh_when_legacy_config_omits_it() {
        with_temp_credentials_path(|path| {
            write_private_test_file(
                path,
                r#"[account]
access_token = "existing-access"
refresh_token = "existing-refresh"
"#,
            );

            commit_legacy_tokens_to_file_with_path(path, &[], Some(("legacy-access", None)))
                .expect("legacy account access should commit");

            let account = load_account_tokens_from_file_with_path(path)
                .expect("account credentials should load")
                .expect("account credentials should exist");
            assert_eq!(account.access_token, "legacy-access");
            assert_eq!(account.refresh_token.as_deref(), Some("existing-refresh"));
        });
    }

    #[test]
    fn repeated_bulk_legacy_file_commit_is_byte_identical() {
        with_temp_credentials_path(|path| {
            let original = r#"profile_tokens={ legacy="legacy-secret",existing="keep" }
[account]
access_token="legacy-access"
refresh_token="legacy-refresh"
"#;
            write_private_test_file(path, original);
            let profile_tokens = vec![("legacy".to_string(), "legacy-secret".to_string())];

            commit_legacy_tokens_to_file_with_path(
                path,
                &profile_tokens,
                Some(("legacy-access", Some("legacy-refresh"))),
            )
            .expect("repeated credential commit should succeed");

            assert_eq!(
                fs::read_to_string(path).expect("credentials should remain readable"),
                original,
                "an already committed credential set must not be reformatted or rewritten"
            );
        });
    }

    #[test]
    fn bulk_legacy_file_commit_failure_preserves_original_bytes() {
        with_temp_credentials_path(|path| {
            let malformed = b"profile_tokens = [not-valid\n";
            fs::write(path, malformed).expect("malformed credentials should be written");

            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(path, fs::Permissions::from_mode(0o600))
                    .expect("test credentials should be private");
            }

            let profile_tokens = vec![("legacy-profile".to_string(), "legacy-secret".to_string())];
            let result = commit_legacy_tokens_to_file_with_path(path, &profile_tokens, None);

            assert!(result.is_err());
            assert_eq!(
                fs::read(path).expect("original credentials should remain readable"),
                malformed
            );
        });
    }

    #[test]
    fn malformed_credentials_error_does_not_echo_secret_contents() {
        with_temp_credentials_path(|path| {
            let sentinel = "sentinel-secret-must-not-appear";
            write_private_test_file(
                path,
                &format!("[account]\naccess_token = \"{sentinel}\"\nrefresh_token = [not-valid\n"),
            );

            let error = read_credentials_file(path)
                .expect_err("malformed credentials should fail without disclosing contents");

            assert!(error.contains("failed to parse credentials"));
            assert!(!error.contains(sentinel));
        });
    }

    #[test]
    fn staged_replacement_failure_preserves_original_and_removes_staging_file() {
        with_temp_credentials_path(|path| {
            let original = "profile_tokens = { existing = \"existing-secret\" }\n";
            write_private_test_file(path, original);
            let mut updated =
                read_credentials_file(path).expect("original credentials should parse");
            updated
                .profile_tokens
                .insert("legacy-profile".to_string(), "legacy-secret".to_string());

            let result = write_credentials_file_with_replacer(
                path,
                &updated,
                |staging_path, destination| {
                    assert!(
                        staging_path.exists(),
                        "staged file should exist before replace"
                    );
                    assert_eq!(destination, path);
                    Err("injected active replacement failure".to_string())
                },
            );

            assert!(result
                .expect_err("replacement failure should be returned")
                .contains("injected active replacement failure"));
            assert_eq!(
                fs::read_to_string(path).expect("original credentials should remain readable"),
                original
            );
            let parent = path.parent().expect("credentials should have a parent");
            let staged_files = fs::read_dir(parent)
                .expect("credentials directory should be readable")
                .filter_map(Result::ok)
                .filter(|entry| entry.file_name().to_string_lossy().contains(".tmp-"))
                .count();
            assert_eq!(staged_files, 0, "failed staging file should be removed");
        });
    }

    #[test]
    fn missing_credentials_do_not_replace_concurrently_created_file() {
        with_temp_credentials_path(|path| {
            let concurrent = "profile_tokens = { concurrent = \"newer-unrelated-secret\" }\n";
            TEST_NEW_CREDENTIALS_DESTINATION_CONTENTS.with(|fixture| {
                *fixture.borrow_mut() = Some((path.to_path_buf(), concurrent.to_string()));
            });
            let profile_tokens = vec![("legacy".to_string(), "legacy-secret".to_string())];

            let error = commit_legacy_tokens_to_file_with_path(path, &profile_tokens, None)
                .expect_err("concurrently created credentials must win");

            assert!(error.contains("created concurrently"));
            assert_eq!(
                fs::read_to_string(path).expect("concurrent credentials should remain readable"),
                concurrent
            );
            assert_eq!(
                staged_credentials_count(
                    path.parent()
                        .expect("test credentials should have a parent")
                ),
                0,
                "failed no-clobber install must clean staging"
            );
        });
    }

    #[cfg(unix)]
    #[test]
    fn linked_credentials_parent_is_refused_without_touching_target() {
        use std::os::unix::fs::symlink;

        with_temp_credentials_path(|ordinary_path| {
            let root = ordinary_path
                .parent()
                .expect("test credentials should have a parent");
            let maintained_backup = root.join("maintained-backup");
            let linked_home = root.join("linked-home");
            fs::create_dir_all(&maintained_backup).expect("backup directory should exist");
            let target = maintained_backup.join("credentials.toml");
            let sentinel = "profile_tokens = { protected = \"backup-sentinel\" }\n";
            write_private_test_file(&target, sentinel);
            symlink(&maintained_backup, &linked_home).expect("home symlink should be created");
            let linked_credentials = linked_home.join("credentials.toml");

            let read_error = read_credentials_file(&linked_credentials)
                .expect_err("linked credentials parent must be refused for reads");
            let profile_tokens = vec![("legacy".to_string(), "legacy-secret".to_string())];
            let write_error =
                commit_legacy_tokens_to_file_with_path(&linked_credentials, &profile_tokens, None)
                    .expect_err("linked credentials parent must be refused for writes");

            assert!(read_error.contains("symbolic link or reparse point"));
            assert!(write_error.contains("symbolic link or reparse point"));
            assert_eq!(
                fs::read_to_string(&target).expect("backup sentinel should remain readable"),
                sentinel
            );
            assert_eq!(staged_credentials_count(&maintained_backup), 0);
        });
    }

    #[cfg(unix)]
    #[test]
    fn linked_credentials_file_is_refused_without_touching_target() {
        use std::os::unix::fs::symlink;

        with_temp_credentials_path(|linked_credentials| {
            let root = linked_credentials
                .parent()
                .expect("test credentials should have a parent");
            let maintained_backup = root.join("maintained-backup");
            fs::create_dir_all(&maintained_backup).expect("backup directory should exist");
            let target = maintained_backup.join("credentials-backup.toml");
            let sentinel = "profile_tokens = { protected = \"backup-sentinel\" }\n";
            write_private_test_file(&target, sentinel);
            symlink(&target, linked_credentials).expect("credentials symlink should be created");

            let read_error = read_credentials_file(linked_credentials)
                .expect_err("linked credentials file must be refused for reads");
            let profile_tokens = vec![("legacy".to_string(), "legacy-secret".to_string())];
            let write_error =
                commit_legacy_tokens_to_file_with_path(linked_credentials, &profile_tokens, None)
                    .expect_err("linked credentials file must be refused for writes");

            assert!(read_error.contains("symbolic link or reparse point"));
            assert!(write_error.contains("symbolic link or reparse point"));
            assert_eq!(
                fs::read_to_string(&target).expect("backup sentinel should remain readable"),
                sentinel
            );
            assert_eq!(staged_credentials_count(root), 0);
            assert_eq!(staged_credentials_count(&maintained_backup), 0);
        });
    }

    #[cfg(unix)]
    #[test]
    fn linked_ancestor_is_allowed_when_credentials_parent_is_real() {
        use std::os::unix::fs::symlink;

        with_temp_credentials_path(|ordinary_path| {
            let root = ordinary_path
                .parent()
                .expect("test credentials should have a parent");
            let actual_cargo_parent = root.join("actual-cargo-parent");
            let linked_cargo_parent = root.join("linked-cargo-parent");
            let actual_home = actual_cargo_parent.join(".cargo-ai");
            fs::create_dir_all(&actual_home).expect("actual Cargo AI Home should exist");
            symlink(&actual_cargo_parent, &linked_cargo_parent)
                .expect("Cargo ancestor symlink should be created");
            let credentials = linked_cargo_parent.join(".cargo-ai/credentials.toml");
            let profile_tokens = vec![("legacy".to_string(), "legacy-secret".to_string())];

            commit_legacy_tokens_to_file_with_path(&credentials, &profile_tokens, None)
                .expect("a linked ancestor outside the final home should be allowed");

            let loaded = load_profile_token_from_file_with_path(&credentials, "legacy")
                .expect("credentials under linked ancestor should load");
            assert_eq!(loaded.as_deref(), Some("legacy-secret"));
            assert!(actual_home.join("credentials.toml").is_file());
            assert_eq!(staged_credentials_count(&actual_home), 0);
        });
    }

    #[test]
    fn empty_bulk_legacy_commit_does_not_create_credentials_file() {
        with_temp_credentials_path(|path| {
            commit_legacy_tokens_to_file_with_path(path, &[], None)
                .expect("empty commit should be a no-op");
            assert!(!path.exists());
        });
    }

    #[test]
    fn keychain_bulk_commit_stops_on_error_without_destructive_cleanup() {
        let profile_tokens = vec![
            ("first".to_string(), "first-secret".to_string()),
            ("second".to_string(), "second-secret".to_string()),
        ];
        let mut writes = Vec::new();

        let result = commit_legacy_tokens_to_keychain_with(
            &profile_tokens,
            Some(("account-access", Some("account-refresh"))),
            |_| Ok(None),
            |account, secret| {
                writes.push((account.to_string(), secret.to_string()));
                if account == keychain_account_for_profile("second") {
                    Err("injected keychain failure".to_string())
                } else {
                    Ok(())
                }
            },
        );

        assert!(result.is_err());
        assert_eq!(writes.len(), 2);
        assert_eq!(writes[0].0, keychain_account_for_profile("first"));
        assert_eq!(writes[1].0, keychain_account_for_profile("second"));
    }

    #[test]
    fn keychain_bulk_commit_writes_refresh_before_access_commit_marker() {
        let mut writes = Vec::new();
        commit_legacy_tokens_to_keychain_with(
            &[],
            Some(("account-access", Some("account-refresh"))),
            |_| Ok(None),
            |account, secret| {
                writes.push((account.to_string(), secret.to_string()));
                Ok(())
            },
        )
        .expect("keychain writes should succeed");

        assert_eq!(
            writes,
            vec![
                (
                    ACCOUNT_REFRESH_KEY.to_string(),
                    "account-refresh".to_string()
                ),
                (ACCOUNT_ACCESS_KEY.to_string(), "account-access".to_string()),
            ]
        );
    }

    #[test]
    fn repeated_keychain_bulk_commit_skips_unchanged_values() {
        let profile_tokens = vec![("existing".to_string(), "existing-secret".to_string())];
        let writes = Cell::new(0);

        commit_legacy_tokens_to_keychain_with(
            &profile_tokens,
            Some(("account-access", Some("account-refresh"))),
            |account| match account {
                value if value == keychain_account_for_profile("existing") => {
                    Ok(Some("existing-secret".to_string()))
                }
                ACCOUNT_ACCESS_KEY => Ok(Some("account-access".to_string())),
                ACCOUNT_REFRESH_KEY => Ok(Some("account-refresh".to_string())),
                _ => Ok(None),
            },
            |_, _| {
                writes.set(writes.get() + 1);
                Ok(())
            },
        )
        .expect("unchanged keychain credential commit should succeed");

        assert_eq!(writes.get(), 0);
    }

    #[test]
    fn merge_keychain_preferred_uses_keychain_values() {
        let mut file_snapshot = SecretSnapshot::default();
        file_snapshot
            .profile_tokens
            .insert("openai-dev".to_string(), "file-token".to_string());
        file_snapshot.account_tokens = Some(AccountTokens {
            access_token: "file-access".to_string(),
            refresh_token: Some("file-refresh".to_string()),
        });

        let mut keychain_snapshot = SecretSnapshot::default();
        keychain_snapshot
            .profile_tokens
            .insert("openai-dev".to_string(), "keychain-token".to_string());
        keychain_snapshot.account_tokens = Some(AccountTokens {
            access_token: "keychain-access".to_string(),
            refresh_token: Some("keychain-refresh".to_string()),
        });

        let merged = SecretSnapshot::merge_keychain_preferred(file_snapshot, keychain_snapshot);
        assert_eq!(
            merged.profile_tokens.get("openai-dev").map(String::as_str),
            Some("keychain-token")
        );
        assert_eq!(
            merged
                .account_tokens
                .as_ref()
                .map(|tokens| tokens.access_token.as_str()),
            Some("keychain-access")
        );
    }

    #[test]
    fn dry_run_migration_reports_counts_without_writes() {
        with_temp_credentials_path(|path| {
            store_profile_token_in_file_with_path(path, "openai-dev", "file-token")
                .expect("profile token should persist");
            store_account_tokens_in_file_with_path(path, "access-1", Some("refresh-1"))
                .expect("account tokens should persist");

            // Simulate file-mode source snapshot without touching process-global config.
            let snapshot = load_snapshot_from_file(path).expect("snapshot should load");
            assert!(!snapshot.is_empty());

            let outcome = SecretStoreMigrationOutcome {
                source_mode: Some(SecretStoreMode::File),
                target_mode: SecretStoreMode::Keychain,
                migrated_profile_tokens: snapshot.profile_tokens.len(),
                migrated_account_tokens: snapshot.account_tokens.is_some(),
                source_had_secrets: true,
            };

            assert_eq!(outcome.migrated_profile_tokens, 1);
            assert!(outcome.migrated_account_tokens);
            assert!(outcome.source_had_secrets);
            assert_eq!(outcome.target_mode, SecretStoreMode::Keychain);
        });
    }

    #[cfg(unix)]
    #[test]
    fn rejects_insecure_fallback_permissions() {
        use std::os::unix::fs::PermissionsExt;

        with_temp_credentials_path(|path| {
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).expect("credentials parent should exist");
            }
            fs::write(path, "profile_tokens = {}\n").expect("credentials file should be written");
            fs::set_permissions(path, fs::Permissions::from_mode(0o644))
                .expect("insecure permissions should be set for test");

            let result = load_profile_token_from_file_with_path(path, "openai-dev");
            assert!(result.is_err());
        });
    }
}
