//! Private, explicitly refreshed connection identity. Passive validation never repairs state.
use crate::config::schema::{Profile, SecretStoreMode};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

const UNAVAILABLE: &str =
    "Profile context is unavailable; explicitly refresh the selected profile context.";
const STALE: &str = "Profile context changed; refresh and review the new connection generation.";
const MAX_CONTEXT_AGE_SECONDS: u64 = 24 * 60 * 60;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ProfileContextRef {
    pub profile_uuid: String,
    pub connection_generation: String,
}

// Deliberately not Debug/Serialize: the private selection is not a public inventory.
pub struct ValidatedProfileContext {
    pub context: ProfileContextRef,
    pub profile_name: String,
    pub profile: Profile,
}

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct ContextRecord {
    pub(crate) reference: ProfileContextRef,
    pub(crate) transaction: String,
    pub(crate) state: String,
    pub(crate) comparison: String,
    pub(crate) refreshed_at: u64,
}

#[derive(Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct ContextRecords {
    #[serde(default)]
    pub(crate) profiles: BTreeMap<String, ContextRecord>,
}

pub(crate) fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|v| v.as_secs())
        .unwrap_or(0)
}

pub(crate) fn root() -> Result<PathBuf, String> {
    crate::config::loader::config_path()
        .parent()
        .map(Path::to_path_buf)
        .ok_or_else(|| UNAVAILABLE.to_owned())
}

pub(crate) fn metadata_key(root: &Path) -> String {
    format!(
        "role_contexts/{}",
        hex_hash(root.to_string_lossy().as_bytes())
    )
}

fn hex_hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

pub(crate) fn config_at(root: &Path) -> Result<(toml::Value, SecretStoreMode), String> {
    let path = root.join("config.toml");
    for candidate in [root, path.as_path()] {
        let metadata = std::fs::symlink_metadata(candidate).map_err(|_| UNAVAILABLE.to_owned())?;
        if metadata.file_type().is_symlink() {
            return Err(UNAVAILABLE.to_owned());
        }
        #[cfg(windows)]
        {
            use std::os::windows::fs::MetadataExt;
            if metadata.file_attributes() & 0x400 != 0 {
                return Err(UNAVAILABLE.to_owned());
            }
        }
    }
    let raw = std::fs::read_to_string(&path).map_err(|_| UNAVAILABLE.to_owned())?;
    if raw.len() > 4 * 1024 * 1024 {
        return Err(UNAVAILABLE.to_owned());
    }
    let config: toml::Value = toml::from_str(&raw).map_err(|_| UNAVAILABLE.to_owned())?;
    let mode = match config.get("secret_store").and_then(toml::Value::as_str) {
        Some("file") => SecretStoreMode::File,
        Some("keychain") => SecretStoreMode::Keychain,
        _ => return Err(UNAVAILABLE.to_owned()),
    };
    Ok((config, mode))
}

fn selected_profile(config: &toml::Value, name: &str) -> Result<toml::Value, String> {
    let profiles = config
        .get("profile")
        .and_then(toml::Value::as_array)
        .ok_or(UNAVAILABLE)?;
    let mut matches = profiles
        .iter()
        .filter(|p| p.get("name").and_then(toml::Value::as_str) == Some(name));
    let selected = matches.next().ok_or(UNAVAILABLE)?.clone();
    if matches.next().is_some() || selected.get("token").is_some() {
        return Err(UNAVAILABLE.to_owned());
    }
    Ok(selected)
}

pub(crate) fn comparison(root: &Path, name: &str, salt: &str) -> Result<(String, Profile), String> {
    let (config, mode) = config_at(root)?;
    let selected = selected_profile(&config, name)?;
    let profile: Profile = selected
        .clone()
        .try_into()
        .map_err(|_| UNAVAILABLE.to_owned())?;
    let mut hash = Sha256::new();
    for bytes in [
        salt.as_bytes(),
        root.to_string_lossy().as_bytes(),
        mode.as_str().as_bytes(),
        selected.to_string().as_bytes(),
    ] {
        hash.update((bytes.len() as u64).to_le_bytes());
        hash.update(bytes);
    }
    // A same-byte config replacement is still an out-of-band lifecycle change.
    let modified = std::fs::metadata(root.join("config.toml"))
        .and_then(|m| m.modified())
        .map_err(|_| UNAVAILABLE.to_owned())?;
    hash.update(format!("{modified:?}"));
    match profile.auth_mode {
        crate::config::schema::ProfileAuthMode::ApiKey => {
            let token =
                super::store::context_profile_token(root, mode, name)?.ok_or(UNAVAILABLE)?;
            if token.trim().is_empty() {
                return Err(UNAVAILABLE.to_owned());
            }
            hash.update(token.as_bytes());
        }
        crate::config::schema::ProfileAuthMode::OpenaiAccount => {
            // Use the existing bounded session parser; never import its token into Cargo AI.
            if config
                .get("openai_auth")
                .and_then(|v| v.get("locally_disabled"))
                .and_then(toml::Value::as_bool)
                == Some(true)
            {
                return Err(UNAVAILABLE.to_owned());
            }
            let session = super::store::context_account_comparison()?;
            hash.update(session.as_bytes());
            hash.update(
                config
                    .get("openai_auth")
                    .map(ToString::to_string)
                    .unwrap_or_default(),
            );
        }
        crate::config::schema::ProfileAuthMode::None => {}
    }
    Ok((format!("{:x}", hash.finalize()), profile))
}

pub fn resolve_profile_context(
    profile_uuid: &str,
    connection_generation: &str,
) -> Result<ValidatedProfileContext, String> {
    resolve_at(&root()?, profile_uuid, connection_generation)
}

/// Resolve an explicitly selected private name through existing active protected metadata.
/// Missing or uncertain state stays unavailable; this lookup never creates or repairs identity.
pub(crate) fn resolve_named_at(root: &Path, name: &str) -> Result<ValidatedProfileContext, String> {
    let (_, mode) = config_at(root)?;
    let records = super::store::read_context_records(root, mode)?.ok_or(UNAVAILABLE)?;
    let record = records.profiles.get(name).ok_or(UNAVAILABLE)?;
    let validated = resolve_at(
        root,
        &record.reference.profile_uuid,
        &record.reference.connection_generation,
    )?;
    if validated.profile_name != name {
        return Err(UNAVAILABLE.to_owned());
    }
    Ok(validated)
}

#[cfg(all(test, cargo_ai_cli))]
pub fn revalidate_profile_context(context: &ProfileContextRef) -> Result<(), String> {
    resolve_profile_context(&context.profile_uuid, &context.connection_generation).map(|_| ())
}

pub(crate) fn resolve_at(
    root: &Path,
    uuid: &str,
    generation: &str,
) -> Result<ValidatedProfileContext, String> {
    let (_, mode) = config_at(root)?;
    let records = super::store::read_context_records(root, mode)?.ok_or(UNAVAILABLE)?;
    let mut matches = records
        .profiles
        .iter()
        .filter(|(_, record)| record.reference.profile_uuid == uuid);
    let (name, record) = matches.next().ok_or(UNAVAILABLE)?;
    if matches.next().is_some() || record.state != "active" {
        return Err(UNAVAILABLE.to_owned());
    }
    if record.reference.connection_generation != generation
        || now() < record.refreshed_at
        || now() - record.refreshed_at > MAX_CONTEXT_AGE_SECONDS
    {
        return Err(STALE.to_owned());
    }
    let (current, profile) = comparison(root, name, &record.transaction)?;
    if current != record.comparison {
        return Err(STALE.to_owned());
    }
    // Catch a cooperating writer between credential inspection and admission.
    if super::store::read_context_records(root, mode)?.as_ref() != Some(&records)
        || comparison(root, name, &record.transaction)?.0 != current
    {
        return Err(STALE.to_owned());
    }
    Ok(ValidatedProfileContext {
        context: record.reference.clone(),
        profile_name: name.clone(),
        profile,
    })
}

/// Serialize every credential-store interaction while a noninteractive read
/// temporarily changes the process-wide macOS interaction setting.
static KEYCHAIN_INTERACTION: std::sync::Mutex<()> = std::sync::Mutex::new(());
static KEYCHAIN_UI_UNCERTAIN: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);
thread_local! { static KEYCHAIN_DEPTH: std::cell::Cell<usize> = const { std::cell::Cell::new(0) }; }
pub(crate) struct KeychainInteractionGuard {
    _lock: Option<std::sync::MutexGuard<'static, ()>>,
}
impl Drop for KeychainInteractionGuard {
    fn drop(&mut self) {
        KEYCHAIN_DEPTH.with(|depth| depth.set(depth.get() - 1));
    }
}
pub(crate) fn keychain_interaction_lock() -> Result<KeychainInteractionGuard, String> {
    if KEYCHAIN_UI_UNCERTAIN.load(std::sync::atomic::Ordering::Acquire) {
        return Err(UNAVAILABLE.to_owned());
    }
    let nested = KEYCHAIN_DEPTH.with(|depth| depth.get() != 0);
    let lock = if nested {
        None
    } else {
        Some(
            KEYCHAIN_INTERACTION
                .lock()
                .map_err(|_| UNAVAILABLE.to_owned())?,
        )
    };
    if KEYCHAIN_UI_UNCERTAIN.load(std::sync::atomic::Ordering::Acquire) {
        return Err(UNAVAILABLE.to_owned());
    }
    KEYCHAIN_DEPTH.with(|depth| depth.set(depth.get() + 1));
    Ok(KeychainInteractionGuard { _lock: lock })
}

#[cfg(any(target_os = "macos", test))]
fn with_interaction_disabled<T>(
    mut get: impl FnMut() -> Result<bool, String>,
    mut set: impl FnMut(bool) -> Result<(), String>,
    read: impl FnOnce() -> Result<T, String>,
) -> Result<T, String> {
    struct Restore<'a, F: FnMut(bool) -> Result<(), String>> {
        set: &'a mut F,
        original: bool,
        armed: bool,
    }
    impl<F: FnMut(bool) -> Result<(), String>> Drop for Restore<'_, F> {
        fn drop(&mut self) {
            if self.armed {
                let _ = (self.set)(self.original);
            }
        }
    }
    let original = get()?;
    // Arm restoration before disabling: a setter may change state then report an error.
    let mut guard = Restore {
        set: &mut set,
        original,
        armed: true,
    };
    (guard.set)(false)?;
    if get()? {
        return Err(UNAVAILABLE.to_owned());
    }
    let result = read();
    (guard.set)(original)?;
    if get()? != original {
        return Err(UNAVAILABLE.to_owned());
    }
    guard.armed = false;
    result
}

pub(crate) fn noninteractive_keychain_read<T>(
    read: impl FnOnce() -> Result<T, String>,
) -> Result<T, String> {
    let _interaction = keychain_interaction_lock()?;
    #[cfg(target_os = "macos")]
    {
        #[link(name = "Security", kind = "framework")]
        unsafe extern "C" {
            fn SecKeychainGetUserInteractionAllowed(state: *mut u8) -> i32;
            fn SecKeychainSetUserInteractionAllowed(state: u8) -> i32;
        }
        with_interaction_disabled(
            || {
                let mut state = 0u8;
                // SAFETY: Security.framework writes one Boolean to this live stack value.
                let result = unsafe { SecKeychainGetUserInteractionAllowed(&mut state) };
                if result == 0 {
                    Ok(state != 0)
                } else {
                    Err(UNAVAILABLE.to_owned())
                }
            },
            |state| {
                // SAFETY: the ABI takes an unsigned-byte Boolean. All Cargo AI
                // keychain interactions share the guard held above.
                let result = unsafe { SecKeychainSetUserInteractionAllowed(u8::from(state)) };
                if result == 0 {
                    Ok(())
                } else {
                    // An ambiguous process-wide setting blocks subsequent OS
                    // keychain operations until the process exits.
                    KEYCHAIN_UI_UNCERTAIN.store(true, std::sync::atomic::Ordering::Release);
                    Err(UNAVAILABLE.to_owned())
                }
            },
            read,
        )
    }
    #[cfg(any(target_os = "linux", target_os = "windows"))]
    {
        read()
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    {
        // No proven noninteractive lookup is available for the configured iOS
        // or Secret Service backend. Never probe it during guided resolution.
        let _ = read;
        Err(UNAVAILABLE.to_owned())
    }
}

/// Only a bounded digest is returned; session tokens remain in their existing source.
pub(crate) fn session_file_comparison(path: &Path) -> Result<String, String> {
    use std::io::Read;
    let mut bytes = Vec::new();
    std::fs::File::open(path)
        .map_err(|_| UNAVAILABLE)?
        .take(65537)
        .read_to_end(&mut bytes)
        .map_err(|_| UNAVAILABLE)?;
    if bytes.len() > 65536 {
        return Err(UNAVAILABLE.to_owned());
    }
    let modified = std::fs::metadata(path)
        .and_then(|m| m.modified())
        .map_err(|_| UNAVAILABLE)?;
    Ok(format!("{}:{modified:?}", hex_hash(&bytes)))
}
