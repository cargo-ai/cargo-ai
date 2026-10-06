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

/// A per-home OS lock, released even if a process exits during persistence.
pub(crate) struct ContextMutationGuard {
    file: Option<std::fs::File>,
    root: PathBuf,
}
thread_local! { static HELD: std::cell::RefCell<BTreeMap<PathBuf, usize>> = const { std::cell::RefCell::new(BTreeMap::new()) }; }
impl Drop for ContextMutationGuard {
    fn drop(&mut self) {
        HELD.with(|held| {
            let mut held = held.borrow_mut();
            if let Some(depth) = held.get_mut(&self.root) {
                *depth -= 1;
                if *depth == 0 {
                    held.remove(&self.root);
                }
            }
        });
        if let Some(file) = &self.file {
            let _ = file.unlock();
        }
    }
}

pub(crate) fn lock_at(root: &Path) -> Result<ContextMutationGuard, String> {
    if HELD.with(|held| held.borrow().contains_key(root)) {
        HELD.with(|held| *held.borrow_mut().get_mut(root).unwrap() += 1);
        return Ok(ContextMutationGuard {
            file: None,
            root: root.to_owned(),
        });
    }
    let lock_path = root.join(".profile-context.lock");
    for path in [root, lock_path.as_path()] {
        match std::fs::symlink_metadata(path) {
            Ok(m) => {
                if m.file_type().is_symlink() {
                    return Err(UNAVAILABLE.to_owned());
                }
                #[cfg(windows)]
                {
                    use std::os::windows::fs::MetadataExt;
                    if m.file_attributes() & 0x400 != 0 {
                        return Err(UNAVAILABLE.to_owned());
                    }
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => return Err(UNAVAILABLE.to_owned()),
        }
    }
    std::fs::create_dir_all(root).map_err(|_| UNAVAILABLE.to_owned())?;
    let mut options = std::fs::OpenOptions::new();
    options.read(true).write(true).create(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let file = options
        .open(lock_path)
        .map_err(|_| UNAVAILABLE.to_owned())?;
    file.try_lock().map_err(|_| {
        "A profile context mutation is already running; retry when it completes.".to_owned()
    })?;
    HELD.with(|held| held.borrow_mut().insert(root.to_owned(), 1));
    Ok(ContextMutationGuard {
        file: Some(file),
        root: root.to_owned(),
    })
}

fn write_verified(
    root: &Path,
    mode: SecretStoreMode,
    records: &ContextRecords,
) -> Result<(), String> {
    super::store::write_context_records(root, mode, records)?;
    if super::store::read_context_records(root, mode)?.as_ref() != Some(records) {
        return Err(UNAVAILABLE.to_owned());
    }
    Ok(())
}

/// Explicit refresh is also the recovery action for pending metadata. It always
/// issues a new generation; it never restores trust in an earlier generation.
pub fn refresh_profile_context(name: &str) -> Result<ProfileContextRef, String> {
    refresh_at(&root()?, name)
}

pub(crate) fn refresh_at(root: &Path, name: &str) -> Result<ProfileContextRef, String> {
    let _lock = lock_at(root)?;
    let (_, mode) = config_at(root)?;
    let mut records = super::store::read_context_records(root, mode)?.unwrap_or_default();
    let transaction = uuid::Uuid::new_v4().to_string();
    let profile_uuid = records
        .profiles
        .get(name)
        .filter(|r| {
            r.comparison.is_empty()
                || comparison(root, name, &r.transaction).is_ok_and(|v| v.0 == r.comparison)
        })
        .map(|r| r.reference.profile_uuid.clone())
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
    let reference = ProfileContextRef {
        profile_uuid,
        connection_generation: uuid::Uuid::new_v4().to_string(),
    };
    let (expected, _) = comparison(root, name, &transaction)?;
    records.profiles.insert(
        name.to_owned(),
        ContextRecord {
            reference: reference.clone(),
            transaction: transaction.clone(),
            state: "pending".to_owned(),
            comparison: expected.clone(),
            refreshed_at: now(),
        },
    );
    write_verified(root, mode, &records)?;
    if config_at(root)?.1 != mode || comparison(root, name, &transaction)?.0 != expected {
        return Err(STALE.to_owned());
    }
    records.profiles.get_mut(name).unwrap().state = "active".to_owned();
    write_verified(root, mode, &records)?;
    resolve_at(
        root,
        &reference.profile_uuid,
        &reference.connection_generation,
    )?;
    Ok(reference)
}

/// Invalidate before a cooperating effect; failure to persist/read back prevents
/// the effect. No metadata means there is no guided trust to invalidate.
pub(crate) fn invalidate_at(
    root: &Path,
    mode: SecretStoreMode,
    name: Option<&str>,
    remove: bool,
) -> Result<(), String> {
    let Some(mut records) = super::store::read_context_records(root, mode)? else {
        return Ok(());
    };
    if remove {
        if let Some(name) = name {
            records.profiles.remove(name);
        } else {
            records.profiles.clear();
        }
    } else {
        for (profile_name, record) in &mut records.profiles {
            if name.is_none_or(|name| name == profile_name) {
                record.state = "pending".to_owned();
                record.transaction = uuid::Uuid::new_v4().to_string();
                record.reference.connection_generation = uuid::Uuid::new_v4().to_string();
                record.comparison.clear();
            }
        }
    }
    write_verified(root, mode, &records)
}

pub(crate) fn before_mutation(
    name: Option<&str>,
    remove: bool,
) -> Result<ContextMutationGuard, String> {
    let root = root()?;
    let lock = lock_at(&root)?;
    // Legacy mode never has guided identity, and must retain existing migration semantics.
    if let Some(mode) = super::store::configured_secret_store_mode() {
        invalidate_at(&root, mode, name, remove)?;
    }
    Ok(lock)
}

/// Invalidate both stores before moving any secret. A interrupted migration can
/// leave pending records, but cannot reactivate the old generation in either store.
pub(crate) fn before_migration(target: SecretStoreMode) -> Result<ContextMutationGuard, String> {
    let guard = before_mutation(None, false)?;
    let root = root()?;
    if super::store::configured_secret_store_mode() != Some(target) {
        invalidate_at(&root, target, None, true)?;
    }
    Ok(guard)
}

pub(crate) fn before_config_change(
    root: &Path,
    old: &toml::Value,
    new: &toml::Value,
) -> Result<ContextMutationGuard, String> {
    let guard = lock_at(root)?;
    let parse_mode = |v: &toml::Value| match v.get("secret_store").and_then(toml::Value::as_str) {
        Some("file") => Some(SecretStoreMode::File),
        Some("keychain") => Some(SecretStoreMode::Keychain),
        _ => None,
    };
    if let Some(mode) = parse_mode(old) {
        // Any context-relevant config write breaks continuity, even if a later
        // write restores the old bytes. Unrelated config fields do not change it.
        if old.get("profile") != new.get("profile")
            || old.get("openai_auth") != new.get("openai_auth")
            || parse_mode(new) != Some(mode)
        {
            invalidate_at(root, mode, None, false)?;
        }
    }
    if parse_mode(old) != parse_mode(new) {
        if let Some(mode) = parse_mode(new) {
            invalidate_at(root, mode, None, true)?;
        }
    }
    Ok(guard)
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::credentials::store::{self, ContextMockKeychain, CONTEXT_MOCK_KEYCHAIN};
    struct Fixture(PathBuf);
    impl Fixture {
        fn new(mode: &str) -> Self {
            let fixture = Self(
                std::env::temp_dir().join(format!("cargo-ai-context-{}", uuid::Uuid::new_v4())),
            );
            std::fs::create_dir(&fixture.0).unwrap();
            std::fs::write(fixture.0.join("config.toml"), format!("secret_store = '{mode}'\n[[profile]]\nname = 'example'\nserver = 'openai'\nmodel = 'gpt-5.2'\nauth_mode = 'api_key'\n")).unwrap();
            if mode == "file" {
                std::fs::write(fixture.0.join("credentials.toml"), "[profile_tokens]\nexample = 'synthetic-secret'\nother = 'preserved-secret'\n[future]\nvalue = 'preserved'\n").unwrap();
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    std::fs::set_permissions(
                        fixture.0.join("credentials.toml"),
                        std::fs::Permissions::from_mode(0o600),
                    )
                    .unwrap();
                }
            } else {
                let mut mock = ContextMockKeychain::default();
                mock.values.insert(
                    "profile/example/token".to_owned(),
                    "synthetic-secret".to_owned(),
                );
                mock.values.insert(
                    "unrelated/account".to_owned(),
                    "preserved-secret".to_owned(),
                );
                CONTEXT_MOCK_KEYCHAIN.with(|m| *m.borrow_mut() = Some(mock));
            }
            fixture
        }
        fn resolve(
            &self,
            reference: &ProfileContextRef,
        ) -> Result<ValidatedProfileContext, String> {
            resolve_at(
                &self.0,
                &reference.profile_uuid,
                &reference.connection_generation,
            )
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
            CONTEXT_MOCK_KEYCHAIN.with(|m| *m.borrow_mut() = None);
        }
    }
    #[test]
    fn explicit_refresh_stable_uuid_restart_and_read_only_lookup() {
        let f = Fixture::new("file");
        let before = std::fs::read(f.0.join("credentials.toml")).unwrap();
        assert!(resolve_at(&f.0, "missing", "missing").is_err());
        assert!(resolve_named_at(&f.0, "example").is_err());
        assert_eq!(before, std::fs::read(f.0.join("credentials.toml")).unwrap());
        let a = refresh_at(&f.0, "example").unwrap();
        let saved = std::fs::read(f.0.join("credentials.toml")).unwrap();
        assert_eq!(f.resolve(&a).unwrap().profile_name, "example");
        assert_eq!(resolve_named_at(&f.0, "example").unwrap().context, a);
        assert!(resolve_named_at(&f.0, "missing").is_err());
        assert_eq!(saved, std::fs::read(f.0.join("credentials.toml")).unwrap());
        let b = refresh_at(&f.0, "example").unwrap();
        assert_eq!(a.profile_uuid, b.profile_uuid);
        assert_ne!(a.connection_generation, b.connection_generation);
        assert!(f.resolve(&a).is_err());
        assert!(f.resolve(&b).is_ok());
        let raw = std::fs::read_to_string(f.0.join("credentials.toml")).unwrap();
        assert!(raw.contains("preserved-secret") && raw.contains("preserved"));
        let records = store::read_context_records(&f.0, SecretStoreMode::File)
            .unwrap()
            .unwrap();
        let journal = serde_json::to_string(&records).unwrap();
        assert!(!journal.contains("synthetic-secret") && !journal.contains("preserved-secret"));
    }
    #[test]
    fn removal_recreation_and_explicit_mutation_do_not_restore_trust() {
        let f = Fixture::new("file");
        let a = refresh_at(&f.0, "example").unwrap();
        invalidate_at(&f.0, SecretStoreMode::File, Some("example"), false).unwrap();
        assert!(f.resolve(&a).is_err());
        let b = refresh_at(&f.0, "example").unwrap();
        assert_eq!(a.profile_uuid, b.profile_uuid);
        invalidate_at(&f.0, SecretStoreMode::File, Some("example"), true).unwrap();
        let c = refresh_at(&f.0, "example").unwrap();
        assert_ne!(c.profile_uuid, b.profile_uuid);
        assert!(f.resolve(&a).is_err() && f.resolve(&b).is_err());
    }
    #[test]
    fn out_of_band_config_token_and_expiry_fail_without_repair() {
        let f = Fixture::new("file");
        let a = refresh_at(&f.0, "example").unwrap();
        let path = f.0.join("credentials.toml");
        let old = std::fs::read_to_string(&path).unwrap();
        std::fs::write(&path, old.replace("synthetic-secret", "changed-secret")).unwrap();
        let changed = std::fs::read(&path).unwrap();
        assert!(f.resolve(&a).is_err());
        assert_eq!(changed, std::fs::read(&path).unwrap());
        let b = refresh_at(&f.0, "example").unwrap();
        let mut records = store::read_context_records(&f.0, SecretStoreMode::File)
            .unwrap()
            .unwrap();
        records.profiles.get_mut("example").unwrap().refreshed_at = 0;
        store::write_context_records(&f.0, SecretStoreMode::File, &records).unwrap();
        assert!(f.resolve(&b).is_err());
        let c = refresh_at(&f.0, "example").unwrap();
        let config = f.0.join("config.toml");
        std::fs::write(
            &config,
            std::fs::read_to_string(&config)
                .unwrap()
                .replace("gpt-5.2", "other-model"),
        )
        .unwrap();
        assert!(f.resolve(&c).is_err());
    }
    #[test]
    fn keychain_pending_and_active_write_faults_require_explicit_recovery() {
        for write in [1, 2] {
            for write_before_error in [false, true] {
                let f = Fixture::new("keychain");
                CONTEXT_MOCK_KEYCHAIN.with(|m| {
                    let mut m = m.borrow_mut();
                    let m = m.as_mut().unwrap();
                    m.fail_write = Some(write);
                    m.write_before_error = write_before_error;
                });
                assert!(refresh_at(&f.0, "example").is_err());
                let saved = store::read_context_records(&f.0, SecretStoreMode::Keychain).unwrap();
                if let Some(records) = &saved {
                    for r in records.profiles.values() {
                        if r.state == "pending" {
                            assert!(f.resolve(&r.reference).is_err());
                        } else {
                            assert!(f.resolve(&r.reference).is_ok());
                        } // Exact new state, never stale old state.
                    }
                }
                CONTEXT_MOCK_KEYCHAIN.with(|m| {
                    m.borrow_mut().as_mut().unwrap().fail_write = None;
                });
                let repaired = refresh_at(&f.0, "example").unwrap();
                assert!(f.resolve(&repaired).is_ok());
                if let Some(records) = saved {
                    for r in records.profiles.values() {
                        assert!(f.resolve(&r.reference).is_err());
                    }
                }
                CONTEXT_MOCK_KEYCHAIN.with(|m| {
                    assert_eq!(
                        m.borrow().as_ref().unwrap().values["unrelated/account"],
                        "preserved-secret"
                    )
                });
            }
        }
    }
    #[test]
    fn unavailable_readback_never_returns_a_reference() {
        // Refresh reads: metadata, token, pending readback, token, active readback,
        // then resolution metadata/token/readback/token.
        for read in 1..=9 {
            let f = Fixture::new("keychain");
            CONTEXT_MOCK_KEYCHAIN.with(|m| m.borrow_mut().as_mut().unwrap().fail_read = Some(read));
            assert!(refresh_at(&f.0, "example").is_err(), "read {read}");
            CONTEXT_MOCK_KEYCHAIN.with(|m| m.borrow_mut().as_mut().unwrap().fail_read = None);
            let repaired = refresh_at(&f.0, "example").unwrap();
            assert!(f.resolve(&repaired).is_ok());
        }
    }
    #[test]
    fn interrupted_migration_invalidates_source_and_destination_metadata() {
        let f = Fixture::new("file");
        let a = refresh_at(&f.0, "example").unwrap();
        let records = store::read_context_records(&f.0, SecretStoreMode::File)
            .unwrap()
            .unwrap();
        let mut mock = ContextMockKeychain::default();
        mock.values
            .insert(metadata_key(&f.0), serde_json::to_string(&records).unwrap());
        mock.values.insert(
            "profile/example/token".to_owned(),
            "synthetic-secret".to_owned(),
        );
        CONTEXT_MOCK_KEYCHAIN.with(|m| *m.borrow_mut() = Some(mock));
        invalidate_at(&f.0, SecretStoreMode::File, None, false).unwrap();
        invalidate_at(&f.0, SecretStoreMode::Keychain, None, true).unwrap();
        assert!(f.resolve(&a).is_err());
        let path = f.0.join("config.toml");
        std::fs::write(
            &path,
            std::fs::read_to_string(&path)
                .unwrap()
                .replace("'file'", "'keychain'"),
        )
        .unwrap();
        assert!(f.resolve(&a).is_err());
        let b = refresh_at(&f.0, "example").unwrap();
        assert!(f.resolve(&b).is_ok());
        assert_ne!(a.profile_uuid, b.profile_uuid);
        assert!(std::fs::read_to_string(f.0.join("credentials.toml"))
            .unwrap()
            .contains("preserved-secret"));
    }
    #[test]
    fn competing_threads_cannot_mutate_under_an_active_lock() {
        let f = Fixture::new("file");
        let guard = lock_at(&f.0).unwrap();
        let root = f.0.clone();
        assert!(std::thread::spawn(move || lock_at(&root).is_err())
            .join()
            .unwrap());
        drop(guard);
        assert!(lock_at(&f.0).is_ok());
    }
    #[test]
    fn bounded_session_comparison_detects_changed_session_without_copying_it() {
        let f = Fixture::new("file");
        let path = f.0.join("synthetic-auth.json");
        std::fs::write(&path, "synthetic-session-a").unwrap();
        let a = session_file_comparison(&path).unwrap();
        std::fs::write(&path, "synthetic-session-b").unwrap();
        let b = session_file_comparison(&path).unwrap();
        assert_ne!(a, b);
        assert!(!a.contains("synthetic"));
        std::fs::write(&path, vec![b'x'; 65537]).unwrap();
        assert!(session_file_comparison(&path).is_err());
    }
}

#[cfg(test)]
mod process_lock_tests {
    use super::*;
    #[test]
    fn isolated_lock_child() {
        let Some(root) = std::env::var_os("CARGO_AI_CONTEXT_LOCK_CHILD_ROOT") else {
            return;
        };
        let root = PathBuf::from(root);
        if std::env::var_os("CARGO_AI_CONTEXT_LOCK_EXPECT_BLOCKED").is_some() {
            assert!(lock_at(&root).is_err());
            return;
        }
        let _guard = lock_at(&root).unwrap();
        std::fs::write(root.join("child-ready"), "ready").unwrap();
        for _ in 0..200 {
            std::thread::sleep(std::time::Duration::from_millis(25));
        }
        panic!("Parent did not terminate the isolated lock holder");
    }
    #[test]
    fn operating_system_lock_serializes_processes_and_recovers_after_termination() {
        let root =
            std::env::temp_dir().join(format!("cargo-ai-context-lock-{}", uuid::Uuid::new_v4()));
        let guard = lock_at(&root).unwrap();
        let executable = std::env::current_exe().unwrap();
        let mut child = std::process::Command::new(&executable);
        child
            .args([
                "--exact",
                "credentials::role_context::process_lock_tests::isolated_lock_child",
            ])
            .env("CARGO_AI_CONTEXT_LOCK_CHILD_ROOT", &root)
            .env("CARGO_AI_CONTEXT_LOCK_EXPECT_BLOCKED", "1");
        assert!(child.status().unwrap().success());
        drop(guard);
        let mut child = std::process::Command::new(&executable)
            .args([
                "--exact",
                "credentials::role_context::process_lock_tests::isolated_lock_child",
            ])
            .env("CARGO_AI_CONTEXT_LOCK_CHILD_ROOT", &root)
            .spawn()
            .unwrap();
        for _ in 0..200 {
            if root.join("child-ready").exists() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let ready = root.join("child-ready").exists();
        if ready {
            assert!(lock_at(&root).is_err());
        }
        child.kill().unwrap();
        let _ = child.wait();
        assert!(ready);
        assert!(lock_at(&root).is_ok());
        std::fs::remove_dir_all(root).unwrap();
    }
}

#[cfg(test)]
mod parity_tests {
    #[test]
    fn emitted_context_validation_matches_native_source() {
        let native = include_str!("role_context.rs");
        let emitted = include_str!("../../templates/src/credentials/role_context.rs");
        let common = native.split("/// A per-home OS lock").next().unwrap();
        assert!(emitted.starts_with(common));
    }
}

#[cfg(test)]
mod migration_transaction_tests {
    use super::*;
    use crate::credentials::store::{self, ContextMockKeychain, CONTEXT_MOCK_KEYCHAIN};
    struct SelectedHome {
        root: PathBuf,
        previous: Option<std::ffi::OsString>,
    }
    impl SelectedHome {
        fn new() -> Self {
            let root = std::env::temp_dir().join(format!(
                "cargo-ai-context-transfer-{}",
                uuid::Uuid::new_v4()
            ));
            std::fs::create_dir(&root).unwrap();
            std::fs::write(root.join("config.toml"), "secret_store = 'file'\n[[profile]]\nname = 'example'\nserver = 'openai'\nmodel = 'gpt-5.2'\nauth_mode = 'api_key'\n").unwrap();
            let previous = std::env::var_os("CARGO_AI_HOME");
            std::env::set_var("CARGO_AI_HOME", &root);
            CONTEXT_MOCK_KEYCHAIN.with(|m| *m.borrow_mut() = Some(ContextMockKeychain::default()));
            store::store_profile_token("example", "synthetic-transfer-token").unwrap();
            Self { root, previous }
        }
        fn select(&self, mode: &str) {
            let path = self.root.join("config.toml");
            let mut document: toml::Value =
                toml::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
            document["secret_store"] = toml::Value::String(mode.to_owned());
            std::fs::write(path, toml::to_string(&document).unwrap()).unwrap();
        }
    }
    impl Drop for SelectedHome {
        fn drop(&mut self) {
            if let Some(value) = &self.previous {
                std::env::set_var("CARGO_AI_HOME", value);
            } else {
                std::env::remove_var("CARGO_AI_HOME");
            }
            CONTEXT_MOCK_KEYCHAIN.with(|m| *m.borrow_mut() = None);
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }
    #[test]
    fn actual_migration_round_trip_never_reuses_source_identity() {
        let f = SelectedHome::new();
        let a = refresh_profile_context("example").unwrap();
        store::migrate_secret_store(SecretStoreMode::Keychain, false).unwrap();
        assert!(resolve_profile_context(&a.profile_uuid, &a.connection_generation).is_err());
        f.select("keychain");
        assert!(resolve_profile_context(&a.profile_uuid, &a.connection_generation).is_err());
        let b = refresh_profile_context("example").unwrap();
        assert_eq!(
            store::load_profile_token("example").unwrap().as_deref(),
            Some("synthetic-transfer-token")
        );
        store::migrate_secret_store(SecretStoreMode::File, false).unwrap();
        f.select("file");
        let c = refresh_profile_context("example").unwrap();
        assert_ne!(a.profile_uuid, b.profile_uuid);
        assert_ne!(b.profile_uuid, c.profile_uuid);
        assert_eq!(
            store::load_profile_token("example").unwrap().as_deref(),
            Some("synthetic-transfer-token")
        );
        assert!(resolve_profile_context(&a.profile_uuid, &a.connection_generation).is_err());
        assert!(resolve_profile_context(&b.profile_uuid, &b.connection_generation).is_err());
    }
    #[test]
    fn migration_write_succeeded_error_and_unreadable_readback_retain_source_pending() {
        for unreadable in [false, true] {
            let f = SelectedHome::new();
            let a = refresh_profile_context("example").unwrap();
            CONTEXT_MOCK_KEYCHAIN.with(|m| {
                let mut m = m.borrow_mut();
                let m = m.as_mut().unwrap();
                m.reads = 0;
                m.writes = 0;
                if unreadable {
                    m.fail_read = Some(2);
                } else {
                    m.fail_write = Some(1);
                    m.write_before_error = true;
                }
            });
            assert!(store::migrate_secret_store(SecretStoreMode::Keychain, false).is_err());
            assert_eq!(
                store::load_profile_token("example").unwrap().as_deref(),
                Some("synthetic-transfer-token")
            );
            assert!(resolve_profile_context(&a.profile_uuid, &a.connection_generation).is_err());
            let records = store::read_context_records(&f.root, SecretStoreMode::File)
                .unwrap()
                .unwrap();
            assert_eq!(records.profiles["example"].state, "pending");
            CONTEXT_MOCK_KEYCHAIN.with(|m| {
                let mut m = m.borrow_mut();
                let m = m.as_mut().unwrap();
                m.fail_read = None;
                m.fail_write = None;
            });
            let b = refresh_profile_context("example").unwrap();
            assert!(resolve_profile_context(&b.profile_uuid, &b.connection_generation).is_ok());
        }
    }
}

#[cfg(test)]
mod noninteractive_tests {
    use super::*;
    use std::cell::Cell;
    #[test]
    fn interaction_guard_restores_original_on_success_read_error_and_panic() {
        for original in [false, true] {
            for fail in [false, true] {
                let state = Cell::new(original);
                let outcome = with_interaction_disabled(
                    || Ok(state.get()),
                    |value| {
                        state.set(value);
                        Ok(())
                    },
                    || {
                        assert!(!state.get());
                        if fail {
                            Err("injected lookup failure".to_owned())
                        } else {
                            Ok(())
                        }
                    },
                );
                assert_eq!(outcome.is_err(), fail);
                assert_eq!(state.get(), original);
            }
        }
        let state = Cell::new(true);
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _: Result<(), String> = with_interaction_disabled(
                || Ok(state.get()),
                |value| {
                    state.set(value);
                    Ok(())
                },
                || panic!("injected panic"),
            );
        }));
        assert!(result.is_err());
        assert!(state.get());
    }
    #[test]
    fn interaction_guard_blocks_read_on_suppression_failure_and_reports_restore_failure() {
        for fail_at in [1, 2] {
            let state = Cell::new(true);
            let writes = Cell::new(0);
            let reads = Cell::new(0);
            let outcome = with_interaction_disabled(
                || Ok(state.get()),
                |value| {
                    writes.set(writes.get() + 1);
                    state.set(value);
                    if writes.get() == fail_at {
                        Err("injected setter error after effect".to_owned())
                    } else {
                        Ok(())
                    }
                },
                || {
                    reads.set(reads.get() + 1);
                    Ok(())
                },
            );
            assert!(outcome.is_err());
            assert!(state.get());
            assert_eq!(reads.get(), if fail_at == 1 { 0 } else { 1 });
        }
    }
    #[test]
    fn all_keychain_interactions_share_one_reentrant_process_lock() {
        let guard = keychain_interaction_lock().unwrap();
        let nested = keychain_interaction_lock().unwrap();
        drop(nested);
        let (send, receive) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            let _other = keychain_interaction_lock().unwrap();
            send.send(()).unwrap();
        });
        assert!(receive
            .recv_timeout(std::time::Duration::from_millis(25))
            .is_err());
        drop(guard);
        receive
            .recv_timeout(std::time::Duration::from_secs(2))
            .unwrap();
        worker.join().unwrap();
    }
}
