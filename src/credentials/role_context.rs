//! Private, explicitly refreshed connection identity. Passive validation never repairs state.
use crate::config::schema::{Profile, SecretStoreMode};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

const UNAVAILABLE: &str =
    "Profile context is unavailable; explicitly refresh the selected profile context.";
const STALE: &str = "Profile context changed; refresh and review the new connection generation.";

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
    pub credential: ValidatedCredential,
}

#[derive(PartialEq, Eq)]
pub enum ValidatedCredential {
    None,
    ApiKey(String),
    OpenaiAccount { token: String, account_id: String },
}
impl ValidatedCredential {
    pub fn token(&self) -> Option<&str> {
        match self {
            Self::None => None,
            Self::ApiKey(token) | Self::OpenaiAccount { token, .. } => Some(token),
        }
    }
    pub fn account_id(&self) -> Option<&str> {
        match self {
            Self::OpenaiAccount { account_id, .. } => Some(account_id),
            _ => None,
        }
    }
    pub fn matches(&self, token: &str, account_id: Option<&str>) -> bool {
        self.token().unwrap_or_default() == token && self.account_id() == account_id
    }
}
impl ValidatedProfileContext {
    pub fn matches(&self, token: &str, account_id: Option<&str>) -> bool {
        self.credential.matches(token, account_id)
    }
}

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct ContextRecord {
    #[serde(default)]
    pub(crate) version: u32,
    pub(crate) reference: ProfileContextRef,
    pub(crate) transaction: String,
    pub(crate) state: String,
    pub(crate) comparison: String,
    pub(crate) refreshed_at: u64,
}

#[derive(Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct ContextRecords {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) keys: Option<super::access_continuity::KeyCache>,
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

#[cfg(cargo_ai_cli)]
pub(crate) fn legacy_comparison(
    root: &Path,
    name: &str,
    salt: &str,
) -> Result<(String, Profile), String> {
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

fn semantic_profile(value: &toml::Value) -> toml::Value {
    // Serialization may materialize defaults during an otherwise benign profile edit.
    // Canonicalize supported fields, while retaining unknown fields conservatively.
    let profile: Result<Profile, _> = value.clone().try_into();
    let mut canonical = profile
        .ok()
        .and_then(|profile| toml::Value::try_from(profile).ok())
        .unwrap_or_else(|| value.clone());
    if let Some(table) = canonical.as_table_mut() {
        table.remove("description");
        if let Some(original) = value.as_table() {
            const KNOWN: &[&str] = &[
                "name",
                "server",
                "model",
                "url",
                "token",
                "timeout_in_sec",
                "max_output_tokens",
                "temperature",
                "thinking",
                "description",
                "auth_mode",
            ];
            for (name, value) in original {
                if !KNOWN.contains(&name.as_str()) {
                    table.insert(name.clone(), value.clone());
                }
            }
        }
    }
    canonical
}

fn comparison_with_snapshot(
    root: &Path,
    name: &str,
    salt: &str,
    keys: Option<&super::access_continuity::KeyCache>,
) -> Result<(String, Profile, ValidatedCredential), String> {
    use crate::config::schema::ProfileAuthMode;
    let (config, mode) = config_at(root)?;
    let selected = selected_profile(&config, name)?;
    let profile: Profile = selected.clone().try_into().map_err(|_| UNAVAILABLE)?;
    let mut hash = Sha256::new();
    let semantic = semantic_profile(&selected).to_string();
    for bytes in [
        salt.as_bytes(),
        root.to_string_lossy().as_bytes(),
        mode.as_str().as_bytes(),
        semantic.as_bytes(),
    ] {
        hash.update((bytes.len() as u64).to_le_bytes());
        hash.update(bytes);
    }
    let credential = match profile.auth_mode {
        ProfileAuthMode::ApiKey => {
            let token = super::store::context_profile_token(root, mode, name)?
                .filter(|s| !s.trim().is_empty())
                .ok_or("credential_unavailable")?;
            hash.update(token.as_bytes());
            ValidatedCredential::ApiKey(token)
        }
        ProfileAuthMode::OpenaiAccount => {
            if config
                .get("openai_auth")
                .and_then(|v| v.get("locally_disabled"))
                .and_then(toml::Value::as_bool)
                == Some(true)
            {
                return Err("credential_disabled".into());
            }
            let snapshot = super::access_continuity::account_snapshot(keys, now())?;
            hash.update(snapshot.identity.as_bytes());
            ValidatedCredential::OpenaiAccount {
                token: snapshot.token,
                account_id: snapshot.account,
            }
        }
        ProfileAuthMode::None => ValidatedCredential::None,
    };
    Ok((format!("{:x}", hash.finalize()), profile, credential))
}

fn require_selected_authority(
    records: &ContextRecords,
    name: &str,
    expected: &ContextRecord,
) -> Result<(), String> {
    let mut matches = records
        .profiles
        .iter()
        .filter(|(_, record)| record.reference.profile_uuid == expected.reference.profile_uuid);
    let (current_name, current) = matches.next().ok_or(UNAVAILABLE)?;
    if matches.next().is_some() || current_name != name {
        return Err(UNAVAILABLE.into());
    }
    // Refresh timestamps and other profiles do not change this connection's authority.
    if current.version != expected.version
        || current.state != expected.state
        || current.reference != expected.reference
        || current.transaction != expected.transaction
        || current.comparison != expected.comparison
    {
        return Err(STALE.into());
    }
    Ok(())
}

#[cfg(all(test, cargo_ai_cli))]
thread_local! {
    static RESOLUTION_READ_HOOK: std::cell::RefCell<Option<Box<dyn FnOnce(&Path)>>> = const { std::cell::RefCell::new(None) };
}

pub(crate) fn resolve_at(
    root: &Path,
    uuid: &str,
    generation: &str,
) -> Result<ValidatedProfileContext, String> {
    const UNSTABLE: &str = "context_verification_unstable";
    let (_, mode) = config_at(root)?;
    let records = super::store::read_context_records(root, mode)?.ok_or(UNAVAILABLE)?;
    let mut matches = records
        .profiles
        .iter()
        .filter(|(_, r)| r.reference.profile_uuid == uuid);
    let (name, record) = matches.next().ok_or(UNAVAILABLE)?;
    if matches.next().is_some() || record.state != "active" {
        return Err(UNAVAILABLE.into());
    }
    if record.reference.connection_generation != generation {
        return Err(STALE.into());
    }
    if record.version != 2 {
        return Err("legacy_context_requires_renewal".into());
    }
    let (current, profile, credential) =
        comparison_with_snapshot(root, name, &record.transaction, records.keys.as_ref())?;
    if current != record.comparison {
        return Err(STALE.into());
    }
    let (after, _, after_credential) =
        comparison_with_snapshot(root, name, &record.transaction, records.keys.as_ref())?;
    if after != current || credential != after_credential {
        return Err(UNSTABLE.into());
    }
    #[cfg(all(test, cargo_ai_cli))]
    {
        let hook = RESOLUTION_READ_HOOK.with(|slot| slot.borrow_mut().take());
        if let Some(hook) = hook {
            hook(root);
        }
    }
    if config_at(root)?.1 != mode {
        return Err(UNSTABLE.into());
    }
    let latest = super::store::read_context_records(root, mode)?.ok_or(UNAVAILABLE)?;
    require_selected_authority(&latest, name, record)?;
    if matches!(&credential, ValidatedCredential::OpenaiAccount { .. })
        && latest.keys != records.keys
    {
        // Evidence renewal is not revocation. Admit only the same credential after
        // verification with the replacement keys, never merely the old cached keys.
        let (latest_comparison, _, latest_credential) =
            comparison_with_snapshot(root, name, &record.transaction, latest.keys.as_ref())?;
        if latest_comparison != current || latest_credential != credential {
            return Err(UNSTABLE.into());
        }
        if config_at(root)?.1 != mode {
            return Err(UNSTABLE.into());
        }
        let final_records = super::store::read_context_records(root, mode)?.ok_or(UNAVAILABLE)?;
        require_selected_authority(&final_records, name, record)?;
        if final_records.keys != latest.keys {
            return Err(UNSTABLE.into());
        }
    }
    Ok(ValidatedProfileContext {
        context: record.reference.clone(),
        profile_name: name.clone(),
        profile,
        credential,
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

#[cfg(any(target_os = "macos", all(test, cargo_ai_cli)))]
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

#[derive(Serialize)]
pub struct ContinuityEffects {
    pub local: String,
    pub remote: String,
    pub context_metadata: String,
    pub public_key_cache: String,
}
#[derive(Serialize)]
pub struct ContinuityReport {
    pub status: String,
    pub review_required: bool,
    pub execution_ready: bool,
    pub context: Option<ProfileContextRef>,
    pub reason: String,
    pub effects: ContinuityEffects,
}
fn report(status: &str, reason: &str, reference: Option<ProfileContextRef>) -> ContinuityReport {
    ContinuityReport {
        status: status.into(),
        review_required: matches!(status, "changed" | "revoked"),
        execution_ready: matches!(status, "unchanged" | "renewed"),
        context: reference,
        reason: reason.into(),
        effects: ContinuityEffects {
            local: "unapplied".into(),
            remote: "unapplied".into(),
            context_metadata: "unapplied".into(),
            public_key_cache: "unapplied".into(),
        },
    }
}
fn classify(reason: &str) -> &'static str {
    if reason.starts_with("unsupported_") {
        "unsupported"
    } else {
        "unavailable"
    }
}
fn is_account(root: &Path, name: &str) -> Result<bool, String> {
    let (config, _) = config_at(root)?;
    let selected = selected_profile(&config, name)?;
    Ok(selected.get("auth_mode").and_then(toml::Value::as_str) == Some("openai_account"))
}

pub fn validate_profile_context(
    name: &str,
    expected: &ProfileContextRef,
    renew: bool,
) -> Result<ContinuityReport, String> {
    validate_at(&root()?, name, expected, renew)
}
fn validate_at(
    root: &Path,
    name: &str,
    expected: &ProfileContextRef,
    renew: bool,
) -> Result<ContinuityReport, String> {
    let _lock = if renew { Some(lock_at(root)?) } else { None };
    let (_, mode) = config_at(root)?;
    let Some(mut records) = super::store::read_context_records(root, mode)? else {
        return Ok(report(
            "unavailable",
            "context_missing",
            Some(expected.clone()),
        ));
    };
    let Some(original) = records.profiles.get(name).cloned() else {
        return Ok(report("revoked", "profile_removed", Some(expected.clone())));
    };
    if original.reference != *expected || original.state != "active" {
        return Ok(report(
            "revoked",
            "generation_not_active",
            Some(expected.clone()),
        ));
    }
    if !matches!(original.version, 0 | 2) {
        return Ok(report(
            "unsupported",
            "unsupported_context_version",
            Some(expected.clone()),
        ));
    }
    if original.version == 0
        && (!renew
            || legacy_comparison(root, name, &original.transaction)
                .map(|v| v.0)
                .ok()
                .as_ref()
                != Some(&original.comparison))
    {
        return Ok(report(
            "unavailable",
            "legacy_context_requires_verified_enrollment",
            Some(expected.clone()),
        ));
    }
    let mut keys_refreshed = false;
    if renew && is_account(root, name)? {
        match super::access_continuity::fetch_keys(now()) {
            Ok(keys) => {
                records.keys = Some(keys);
                keys_refreshed = true;
            }
            Err(reason) => return Ok(report(classify(&reason), &reason, Some(expected.clone()))),
        }
    }
    let (current, _, credential) =
        match comparison_with_snapshot(root, name, &original.transaction, records.keys.as_ref()) {
            Ok(value) => value,
            Err(reason) => return Ok(report(classify(&reason), &reason, Some(expected.clone()))),
        };
    if original.version == 2 && current != original.comparison {
        return Ok(report(
            "changed",
            "connection_or_settings_changed",
            Some(expected.clone()),
        ));
    }
    // A routine renewal never repairs pending metadata or enrolls a different connection.
    let (after, _, after_credential) =
        comparison_with_snapshot(root, name, &original.transaction, records.keys.as_ref())?;
    if current != after || credential != after_credential {
        return Ok(report(
            "unavailable",
            "credential_changed_during_validation",
            Some(expected.clone()),
        ));
    }
    if renew && (keys_refreshed || original.version == 0) {
        let entry = records.profiles.get_mut(name).unwrap();
        entry.version = 2;
        entry.comparison = current;
        entry.refreshed_at = now();
        write_verified(root, mode, &records)?;
    }
    resolve_at(
        root,
        &expected.profile_uuid,
        &expected.connection_generation,
    )?;
    let renewed = renew && (keys_refreshed || original.version == 0);
    let mut result = report(
        if renewed { "renewed" } else { "unchanged" },
        "continuity_verified",
        Some(expected.clone()),
    );
    if renewed {
        result.effects.local = "applied".into();
        result.effects.context_metadata = "applied".into();
    }
    if keys_refreshed {
        result.effects.public_key_cache = "applied".into();
    }
    Ok(result)
}

/// Explicit enrollment is idempotent for an unchanged active connection.
pub fn refresh_profile_context(name: &str) -> Result<ProfileContextRef, String> {
    refresh_at(&root()?, name)
}
pub(crate) fn refresh_at(root: &Path, name: &str) -> Result<ProfileContextRef, String> {
    let _lock = lock_at(root)?;
    let (_, mode) = config_at(root)?;
    let mut records = super::store::read_context_records(root, mode)?.unwrap_or_default();
    let existing = records.profiles.get(name).cloned();
    if existing
        .as_ref()
        .is_some_and(|record| !matches!(record.version, 0 | 2))
    {
        return Err("unsupported_context_version".into());
    }
    if is_account(root, name)? {
        records.keys = Some(super::access_continuity::fetch_keys(now())?);
    }
    let transaction = existing
        .as_ref()
        .map(|r| r.transaction.clone())
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
    let (expected, _, credential) =
        comparison_with_snapshot(root, name, &transaction, records.keys.as_ref())?;
    let reusable = existing.as_ref().is_some_and(|record| {
        record.state == "active"
            && match record.version {
                2 => record.comparison == expected,
                0 => legacy_comparison(root, name, &record.transaction)
                    .is_ok_and(|v| v.0 == record.comparison),
                _ => false,
            }
    });
    let reference = if reusable {
        existing.as_ref().unwrap().reference.clone()
    } else {
        ProfileContextRef {
            profile_uuid: existing
                .as_ref()
                .map(|r| r.reference.profile_uuid.clone())
                .unwrap_or_else(|| uuid::Uuid::new_v4().to_string()),
            connection_generation: uuid::Uuid::new_v4().to_string(),
        }
    };
    if reusable && existing.as_ref().is_some_and(|r| r.version == 2) && !is_account(root, name)? {
        return resolve_at(
            root,
            &reference.profile_uuid,
            &reference.connection_generation,
        )
        .map(|v| v.context);
    }
    records.profiles.insert(
        name.into(),
        ContextRecord {
            version: 2,
            reference: reference.clone(),
            transaction: transaction.clone(),
            state: "pending".into(),
            comparison: expected.clone(),
            refreshed_at: now(),
        },
    );
    write_verified(root, mode, &records)?;
    let (after, _, after_credential) =
        comparison_with_snapshot(root, name, &transaction, records.keys.as_ref())?;
    if config_at(root)?.1 != mode || after != expected || credential != after_credential {
        return Err(STALE.into());
    }
    records.profiles.get_mut(name).unwrap().state = "active".into();
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
        if parse_mode(new) != Some(mode) {
            invalidate_at(root, mode, None, false)?;
        } else if let Some(records) = super::store::read_context_records(root, mode)? {
            for name in records.profiles.keys() {
                let before = selected_profile(old, name);
                let after = selected_profile(new, name);
                let account = before
                    .as_ref()
                    .ok()
                    .and_then(|p| p.get("auth_mode"))
                    .and_then(toml::Value::as_str)
                    == Some("openai_account");
                let disabled = |v: &toml::Value| {
                    v.get("openai_auth")
                        .and_then(|a| a.get("locally_disabled"))
                        .and_then(toml::Value::as_bool)
                        .unwrap_or(false)
                };
                if before.as_ref().ok().map(semantic_profile)
                    != after.as_ref().ok().map(semantic_profile)
                    || (account && disabled(old) != disabled(new))
                {
                    invalidate_at(root, mode, Some(name), after.is_err())?;
                }
            }
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
        assert_eq!(a.connection_generation, b.connection_generation);
        assert!(f.resolve(&a).is_ok());
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
    fn continuity_read_and_renew_preserve_references_and_legacy_age_migration() {
        let f = Fixture::new("file");
        let a = refresh_at(&f.0, "example").unwrap();
        let path = f.0.join("credentials.toml");
        let before = std::fs::read(&path).unwrap();
        for renew in [false, true] {
            let result = validate_at(&f.0, "example", &a, renew).unwrap();
            assert_eq!(result.status, "unchanged");
            assert!(result.execution_ready);
            assert!(!result.review_required);
            assert_eq!(result.context, Some(a.clone()));
            assert_eq!(before, std::fs::read(&path).unwrap());
        }
        let mut records = store::read_context_records(&f.0, SecretStoreMode::File)
            .unwrap()
            .unwrap();
        let record = records.profiles.get_mut("example").unwrap();
        record.version = 0;
        record.refreshed_at = 0;
        record.comparison = legacy_comparison(&f.0, "example", &record.transaction)
            .unwrap()
            .0;
        store::write_context_records(&f.0, SecretStoreMode::File, &records).unwrap();
        assert_eq!(
            validate_at(&f.0, "example", &a, false).unwrap().status,
            "unavailable"
        );
        let migrated = validate_at(&f.0, "example", &a, true).unwrap();
        assert_eq!(migrated.status, "renewed");
        assert_eq!(migrated.context, Some(a.clone()));
        assert!(f.resolve(&a).is_ok());
        let mut records = store::read_context_records(&f.0, SecretStoreMode::File)
            .unwrap()
            .unwrap();
        records.profiles.get_mut("example").unwrap().version = 0;
        store::write_context_records(&f.0, SecretStoreMode::File, &records).unwrap();
        assert_eq!(
            validate_at(&f.0, "example", &a, true).unwrap().status,
            "unavailable"
        );
    }
    #[test]
    fn benign_config_edits_preserve_only_affected_semantic_contexts() {
        let f = Fixture::new("file");
        let a = refresh_at(&f.0, "example").unwrap();
        let path = f.0.join("config.toml");
        let before: toml::Value = toml::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        let mut benign = before.clone();
        benign["profile"][0].as_table_mut().unwrap().insert(
            "description".into(),
            toml::Value::String("New label".into()),
        );
        benign.as_table_mut().unwrap().insert(
            "default_profile".into(),
            toml::Value::String("unrelated".into()),
        );
        {
            let _guard = before_config_change(&f.0, &before, &benign).unwrap();
            std::fs::write(&path, toml::to_string(&benign).unwrap()).unwrap();
        }
        assert_eq!(refresh_at(&f.0, "example").unwrap(), a);
        assert!(f.resolve(&a).is_ok());
        let mut changed = benign.clone();
        changed["profile"][0]["model"] = toml::Value::String("other-model".into());
        {
            let _guard = before_config_change(&f.0, &benign, &changed).unwrap();
            std::fs::write(&path, toml::to_string(&changed).unwrap()).unwrap();
        }
        assert!(f.resolve(&a).is_err());
        {
            let _guard = before_config_change(&f.0, &changed, &benign).unwrap();
            std::fs::write(&path, toml::to_string(&benign).unwrap()).unwrap();
        }
        assert!(f.resolve(&a).is_err());
        assert_ne!(refresh_at(&f.0, "example").unwrap(), a);
    }
    #[test]
    fn signed_account_renewal_preserves_reference_and_pins_replacement_bytes() {
        use super::super::access_continuity::test_support;
        struct Reset(Option<std::ffi::OsString>);
        impl Drop for Reset {
            fn drop(&mut self) {
                test_support::set_keys(None);
                if let Some(value) = &self.0 {
                    std::env::set_var("CODEX_HOME", value)
                } else {
                    std::env::remove_var("CODEX_HOME")
                }
            }
        }
        let f = Fixture::new("file");
        let home = f.0.join("synthetic-codex");
        let _reset = Reset(std::env::var_os("CODEX_HOME"));
        std::env::set_var("CODEX_HOME", &home);
        let path = f.0.join("config.toml");
        std::fs::write(
            &path,
            std::fs::read_to_string(&path)
                .unwrap()
                .replace("api_key", "openai_account"),
        )
        .unwrap();
        let timestamp = now();
        test_support::set_keys(Some(Ok(test_support::keys(timestamp))));
        let mut claims = test_support::claims(timestamp);
        let old = test_support::write_session(&home, &claims);
        let reference = refresh_at(&f.0, "example").unwrap();
        let selected = f.resolve(&reference).unwrap();
        assert!(selected.matches(&old, Some("synthetic-account")));
        claims["exp"] = serde_json::json!(timestamp + 7200);
        claims["jti"] = serde_json::json!("renewed");
        let new = test_support::write_session(&home, &claims);
        let selected = f.resolve(&reference).unwrap();
        assert!(selected.matches(&new, Some("synthetic-account")));
        assert!(!selected.matches(&old, Some("synthetic-account")));
        let renewed = validate_at(&f.0, "example", &reference, true).unwrap();
        assert_eq!(renewed.status, "renewed");
        assert_eq!(renewed.context, Some(reference.clone()));
        let mut records = store::read_context_records(&f.0, SecretStoreMode::File)
            .unwrap()
            .unwrap();
        records.keys = Some(test_support::keys(
            timestamp - super::super::access_continuity::KEY_MAX_AGE - 1,
        ));
        store::write_context_records(&f.0, SecretStoreMode::File, &records).unwrap();
        let retained = std::fs::read(f.0.join("credentials.toml")).unwrap();
        assert_eq!(
            validate_at(&f.0, "example", &reference, false)
                .unwrap()
                .status,
            "unavailable"
        );
        test_support::set_keys(Some(Err("keys_unavailable".into())));
        assert_eq!(
            validate_at(&f.0, "example", &reference, true)
                .unwrap()
                .status,
            "unavailable"
        );
        assert_eq!(
            retained,
            std::fs::read(f.0.join("credentials.toml")).unwrap()
        );
        test_support::set_keys(Some(Ok(test_support::keys(now()))));
        assert_eq!(
            validate_at(&f.0, "example", &reference, true)
                .unwrap()
                .context,
            Some(reference.clone())
        );
        claims["sub"] = serde_json::json!("different-principal");
        test_support::write_session(&home, &claims);
        let changed = validate_at(&f.0, "example", &reference, true).unwrap();
        assert_eq!(changed.status, "changed");
        assert!(changed.review_required);
        assert!(!changed.execution_ready);
        assert!(f.resolve(&reference).is_err());
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
    fn out_of_band_config_and_token_fail_but_elapsed_context_age_does_not() {
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
        assert!(f.resolve(&b).is_ok());
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
    fn selected_context_tolerates_benign_concurrent_metadata_but_checks_latest_authority() {
        use super::super::access_continuity::test_support;
        struct Reset(Option<std::ffi::OsString>);
        impl Drop for Reset {
            fn drop(&mut self) {
                test_support::set_keys(None);
                RESOLUTION_READ_HOOK.with(|slot| *slot.borrow_mut() = None);
                if let Some(value) = &self.0 {
                    std::env::set_var("CODEX_HOME", value);
                } else {
                    std::env::remove_var("CODEX_HOME");
                }
            }
        }
        for case in [
            "unrelated_profile",
            "renewed_keys",
            "revoked",
            "signer_removed",
        ] {
            let f = Fixture::new("file");
            let _reset = Reset(std::env::var_os("CODEX_HOME"));
            let codex = f.0.join("synthetic-codex");
            std::env::set_var("CODEX_HOME", &codex);
            let config = f.0.join("config.toml");
            std::fs::write(
                &config,
                std::fs::read_to_string(&config)
                    .unwrap()
                    .replace("api_key", "openai_account"),
            )
            .unwrap();
            let timestamp = now();
            let token = test_support::write_session(&codex, &test_support::claims(timestamp));
            test_support::set_keys(Some(Ok(test_support::keys(timestamp - 60))));
            let reference = refresh_at(&f.0, "example").unwrap();
            let hook_bytes = std::rc::Rc::new(std::cell::RefCell::new(None));
            let written_bytes = hook_bytes.clone();
            RESOLUTION_READ_HOOK.with(|slot| {
                *slot.borrow_mut() = Some(Box::new(move |root| {
                    let mut records = store::read_context_records(root, SecretStoreMode::File)
                        .unwrap()
                        .unwrap();
                    match case {
                        "unrelated_profile" => {
                            let mut other = records.profiles["example"].clone();
                            other.reference.profile_uuid = "unrelated-profile-uuid".into();
                            records.profiles.insert("other".into(), other);
                            records.profiles.get_mut("example").unwrap().refreshed_at += 1;
                        }
                        "renewed_keys" => records.keys = Some(test_support::keys(timestamp)),
                        "revoked" => {
                            records.profiles.get_mut("example").unwrap().state = "pending".into()
                        }
                        "signer_removed" => {
                            records.keys = Some(test_support::keys_without_signer(timestamp))
                        }
                        _ => unreachable!(),
                    }
                    store::write_context_records(root, SecretStoreMode::File, &records).unwrap();
                    *written_bytes.borrow_mut() =
                        Some(std::fs::read(root.join("credentials.toml")).unwrap());
                }))
            });
            let result = f.resolve(&reference);
            match case {
                "unrelated_profile" | "renewed_keys" => {
                    let selected = result.unwrap();
                    assert_eq!(selected.context, reference);
                    assert!(selected.matches(&token, Some("synthetic-account")));
                }
                "revoked" => assert!(result.is_err()),
                "signer_removed" => assert_eq!(result.err().as_deref(), Some("keys_unavailable")),
                _ => unreachable!(),
            }
            assert_eq!(
                hook_bytes.borrow().as_ref().unwrap(),
                &std::fs::read(f.0.join("credentials.toml")).unwrap()
            );
        }
    }
    #[test]
    fn renewed_account_metadata_write_fault_retains_exact_verified_reference() {
        use super::super::access_continuity::test_support;
        struct Reset(Option<std::ffi::OsString>);
        impl Drop for Reset {
            fn drop(&mut self) {
                test_support::set_keys(None);
                if let Some(value) = &self.0 {
                    std::env::set_var("CODEX_HOME", value);
                } else {
                    std::env::remove_var("CODEX_HOME");
                }
            }
        }
        for write_before_error in [false, true] {
            let f = Fixture::new("keychain");
            let _reset = Reset(std::env::var_os("CODEX_HOME"));
            let codex = f.0.join("synthetic-codex");
            std::env::set_var("CODEX_HOME", &codex);
            let config = f.0.join("config.toml");
            std::fs::write(
                &config,
                std::fs::read_to_string(&config)
                    .unwrap()
                    .replace("api_key", "openai_account"),
            )
            .unwrap();
            let timestamp = now();
            let mut claims = test_support::claims(timestamp);
            test_support::write_session(&codex, &claims);
            test_support::set_keys(Some(Ok(test_support::keys(timestamp - 60))));
            let reference = refresh_at(&f.0, "example").unwrap();
            let before = store::read_context_records(&f.0, SecretStoreMode::Keychain)
                .unwrap()
                .unwrap();
            claims["jti"] = serde_json::json!("routine-renewal");
            let renewed = test_support::write_session(&codex, &claims);
            let keys = test_support::keys(timestamp);
            test_support::set_keys(Some(Ok(keys.clone())));
            CONTEXT_MOCK_KEYCHAIN.with(|mock| {
                let mut mock = mock.borrow_mut();
                let mock = mock.as_mut().unwrap();
                mock.fail_write = Some(mock.writes + 1);
                mock.write_before_error = write_before_error;
            });
            assert!(validate_at(&f.0, "example", &reference, true).is_err());
            CONTEXT_MOCK_KEYCHAIN
                .with(|mock| mock.borrow_mut().as_mut().unwrap().fail_write = None);
            let saved = store::read_context_records(&f.0, SecretStoreMode::Keychain)
                .unwrap()
                .unwrap();
            assert_eq!(saved.profiles["example"].reference, reference);
            assert_eq!(saved.profiles["example"].state, "active");
            assert!(
                saved.keys
                    == if write_before_error {
                        Some(keys)
                    } else {
                        before.keys
                    }
            );
            assert!(f
                .resolve(&reference)
                .unwrap()
                .matches(&renewed, Some("synthetic-account")));
            CONTEXT_MOCK_KEYCHAIN.with(|mock| {
                assert_eq!(
                    mock.borrow().as_ref().unwrap().values["unrelated/account"],
                    "preserved-secret"
                )
            });
            assert_eq!(
                validate_at(&f.0, "example", &reference, true)
                    .unwrap()
                    .context,
                Some(reference)
            );
        }
    }
    #[test]
    fn enrollment_preserves_unsupported_future_metadata() {
        let f = Fixture::new("file");
        let reference = refresh_at(&f.0, "example").unwrap();
        let mut records = store::read_context_records(&f.0, SecretStoreMode::File)
            .unwrap()
            .unwrap();
        records.profiles.get_mut("example").unwrap().version = 99;
        store::write_context_records(&f.0, SecretStoreMode::File, &records).unwrap();
        let saved = std::fs::read(f.0.join("credentials.toml")).unwrap();
        assert!(refresh_at(&f.0, "example").is_err());
        assert_eq!(
            validate_at(&f.0, "example", &reference, true)
                .unwrap()
                .status,
            "unsupported"
        );
        assert_eq!(saved, std::fs::read(f.0.join("credentials.toml")).unwrap());
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
                        if r.state == "pending" {
                            assert!(f.resolve(&r.reference).is_err());
                        } else {
                            assert_eq!(r.reference, repaired);
                        }
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
    fn unchanged_token_and_unrelated_hosted_renewal_preserve_provider_context() {
        let f = SelectedHome::new();
        let reference = refresh_profile_context("example").unwrap();
        store::store_profile_token("example", "synthetic-transfer-token").unwrap();
        assert!(
            resolve_profile_context(&reference.profile_uuid, &reference.connection_generation)
                .is_ok()
        );
        store::store_account_tokens("synthetic-hosted-renewal", Some("synthetic-hosted-refresh"))
            .unwrap();
        store::clear_account_tokens().unwrap();
        assert_eq!(refresh_profile_context("example").unwrap(), reference);
        assert!(std::fs::read_to_string(f.root.join("credentials.toml"))
            .unwrap()
            .contains("synthetic-transfer-token"));
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
