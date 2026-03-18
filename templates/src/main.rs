mod args;
mod web_resources;
mod config;
mod credentials;
mod providers;

use jsonlogic::apply;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use config::loader::{find_profile, load_config};
use config::schema::{Profile, ProfileAuthMode, SecretStoreMode};
use providers::{
    provider_error_messages, validate_provider_content_parts, validate_provider_request,
    ProviderKind,
};

include!(concat!(env!("OUT_DIR"), "/agent_model.rs"));

const INFRA_BASE_URL: &str = "https://api.cargo-ai.org";
const OPENAI_ACCOUNT_RESPONSES_URL: &str = "https://chatgpt.com/backend-api/codex/responses";
const OPENAI_REFRESH_BUFFER_SEC: i64 = 30;
const KEYCHAIN_SERVICE: &str = "cargo-ai";
const ACCOUNT_ACCESS_TOKEN_STORAGE_KEY: &str = "account/access_token";
const ACCOUNT_REFRESH_TOKEN_STORAGE_KEY: &str = "account/refresh_token";
const AGENT_ACTION_DEPTH_ENV: &str = "CARGO_AI_AGENT_ACTION_DEPTH";
const AGENT_ACTION_MAX_DEPTH_ENV: &str = "CARGO_AI_AGENT_ACTION_MAX_DEPTH";
const AGENT_ACTION_MAX_RUNTIME_SECS_ENV: &str = "CARGO_AI_AGENT_MAX_RUNTIME_SECS";
const AGENT_ACTION_RUNTIME_STARTED_AT_MS_ENV: &str = "CARGO_AI_AGENT_RUNTIME_STARTED_AT_MS";
const AGENT_ACTION_RUNTIME_DEADLINE_MS_ENV: &str = "CARGO_AI_AGENT_RUNTIME_DEADLINE_MS";
const DEFAULT_AGENT_ACTION_MAX_DEPTH: u32 = 5;
const DEFAULT_AGENT_ACTION_MAX_RUNTIME_SECS: u64 = 600;
const SUPPORTED_FILE_EXTENSIONS_MESSAGE: &str =
    "pdf, docx, csv, xla, xlb, xlc, xlm, xls, xlsx, xlt, xlw, tsv, iif, doc, dot, odt, rtf, pot, ppa, pps, ppt, pptx, pwz, wiz";

fn unknown_server_messages(server: &str) -> Vec<String> {
    let display_server = if server.trim().is_empty() {
        "(not set)"
    } else {
        server
    };

    vec![
        format!("❌ Unknown AI server '{}'.", display_server),
        "Use `--server ollama` or `--server openai`.".to_string(),
        "Hint: Set `--server` explicitly or configure a default profile with a supported server."
            .to_string(),
        "Example: cargo ai preflight --server ollama --model mistral --input-text \"What is 2 + 2?\""
            .to_string(),
    ]
}

#[derive(Debug, Clone)]
struct SelectedProfile {
    name: String,
    auth_mode: ProfileAuthMode,
    legacy_token: Option<String>,
}

#[derive(Debug, Clone)]
struct ResolvedOpenAiToken {
    token: String,
    uses_account_session: bool,
}

#[derive(Debug, Clone, Copy)]
struct InvocationRuntimeBudget {
    max_runtime_secs: u64,
    started_at_ms: u64,
    deadline_ms: u64,
}

#[derive(Debug, Clone)]
struct ActionProviderContext {
    provider: ProviderKind,
    url: String,
    token: String,
    inference_timeout_in_sec: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StepExecutionOutcome {
    Completed,
    SoftFailureLogged,
    SuccessAlreadyPrinted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RuntimeInputMode {
    Replace,
    Append,
    Prepend,
}

#[derive(Debug, Clone)]
struct AccountAuth {
    access_token: String,
    refresh_token: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Default)]
struct CredentialsFile {
    #[serde(default)]
    profile_tokens: BTreeMap<String, String>,

    #[serde(default)]
    account: Option<CredentialsAccount>,

    #[serde(default)]
    openai_oauth: Option<CredentialsAccount>,
}

#[derive(Debug, Serialize, Deserialize, Default)]
struct CredentialsAccount {
    #[serde(default)]
    access_token: Option<String>,
    #[serde(default)]
    refresh_token: Option<String>,
}

#[derive(Debug, Clone, Copy)]
enum LoadedProfileKind {
    Explicit,
    Default,
}

fn profile_selection_messages(
    kind: LoadedProfileKind,
    profile_name: &str,
    overrides: &[String],
) -> Vec<String> {
    let base_message = match kind {
        LoadedProfileKind::Explicit => format!("Using profile '{}'", profile_name),
        LoadedProfileKind::Default => format!("Using default profile '{}'", profile_name),
    };

    if overrides.is_empty() {
        vec![base_message]
    } else {
        vec![
            format!("{base_message} as fallback."),
            format!("CLI overrides: {}", overrides.join(", ")),
        ]
    }
}

fn cli_override_descriptions(
    matches: &clap::ArgMatches,
    include_token_override: bool,
) -> Vec<String> {
    let mut overrides = Vec::new();

    if let Some(server) = matches.get_one::<String>("server") {
        overrides.push(format!("server={}", server.to_lowercase()));
    }

    if let Some(model) = matches.get_one::<String>("model") {
        overrides.push(format!("model={model}"));
    }

    if let Some(url) = matches.get_one::<String>("url") {
        overrides.push(format!("url={url}"));
    }

    if let Some(timeout) = matches.get_one::<u64>("inference_timeout_in_sec") {
        overrides.push(format!("inference_timeout_in_sec={timeout}"));
    }

    if let Some(max_depth) = matches.get_one::<u32>("max_agent_depth") {
        overrides.push(format!("max_agent_depth={max_depth}"));
    }

    if let Some(max_runtime) = matches.get_one::<u64>("max_runtime_in_sec") {
        overrides.push(format!("max_runtime_in_sec={max_runtime}"));
    }

    if include_token_override {
        overrides.push("token=(explicit)".to_string());
    }

    overrides
}

fn resolve_profile_api_token(profile: &SelectedProfile) -> Result<String, String> {
    match credentials::store::load_profile_token(&profile.name) {
        Ok(Some(token)) if !token.trim().is_empty() => Ok(token),
        Ok(Some(_)) | Ok(None) => profile
            .legacy_token
            .as_deref()
            .map(str::trim)
            .filter(|token| !token.is_empty())
            .map(str::to_string)
            .ok_or_else(|| {
                format!(
                    "Missing API token for profile '{}'. Use `cargo ai profile set {} --token <TOKEN> --auth api_key`.",
                    profile.name, profile.name
                )
            }),
        Err(error) => Err(format!(
            "Failed to load profile token for '{}': {error}",
            profile.name
        )),
    }
}

fn now_unix_seconds() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or(0)
}

#[derive(Debug, Clone)]
struct CodexSession {
    access_token: String,
    access_token_expires_at_unix: Option<i64>,
}

fn parse_unix_timestamp(value: &serde_json::Value) -> Option<i64> {
    if let Some(seconds) = value.as_i64() {
        return Some(if seconds > 1_000_000_000_000 {
            seconds / 1000
        } else {
            seconds
        });
    }

    value
        .as_str()
        .map(str::trim)
        .filter(|raw| !raw.is_empty())
        .and_then(|raw| raw.parse::<i64>().ok())
        .map(|seconds| {
            if seconds > 1_000_000_000_000 {
                seconds / 1000
            } else {
                seconds
            }
        })
}

fn parse_non_empty_token(container: &serde_json::Value, key: &str) -> Option<String> {
    container
        .get(key)
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|token| !token.is_empty())
        .map(str::to_string)
}

fn parse_access_token_expires_at_unix(tokens: &serde_json::Value) -> Option<i64> {
    let keys = [
        "access_token_expires_at_unix",
        "access_token_expires_at",
        "expires_at",
        "expiresAt",
    ];

    for key in keys {
        if let Some(value) = tokens.get(key) {
            if let Some(parsed) = parse_unix_timestamp(value) {
                return Some(parsed);
            }
        }
    }

    None
}

fn codex_auth_path() -> Result<PathBuf, String> {
    if let Ok(codex_home) = std::env::var("CODEX_HOME") {
        let trimmed = codex_home.trim();
        if !trimmed.is_empty() {
            return Ok(PathBuf::from(trimmed).join("auth.json"));
        }
    }

    let home_dir = dirs::home_dir()
        .ok_or_else(|| "failed to resolve home directory for Codex auth lookup".to_string())?;
    Ok(home_dir.join(".codex").join("auth.json"))
}

fn load_codex_session() -> Result<Option<CodexSession>, String> {
    let path = codex_auth_path()?;
    if !path.exists() {
        return Ok(None);
    }

    let raw = fs::read_to_string(&path)
        .map_err(|error| format!("failed to read '{}': {error}", path.display()))?;
    let parsed = serde_json::from_str::<serde_json::Value>(&raw)
        .map_err(|error| format!("failed to parse Codex auth JSON: {error}"))?;

    let tokens = parsed.get("tokens").ok_or_else(|| {
        "Codex auth payload did not include a `tokens` object. Re-run `codex login`.".to_string()
    })?;

    let access_token = parse_non_empty_token(tokens, "access_token").ok_or_else(|| {
        "Codex auth payload did not include a non-empty access token. Re-run `codex login`."
            .to_string()
    })?;

    Ok(Some(CodexSession {
        access_token,
        access_token_expires_at_unix: parse_access_token_expires_at_unix(tokens),
    }))
}

fn codex_access_token_expired_or_near(expires_at_unix: Option<i64>) -> bool {
    match expires_at_unix {
        Some(expires_at) => expires_at.saturating_sub(OPENAI_REFRESH_BUFFER_SEC) <= now_unix_seconds(),
        None => false,
    }
}

fn openai_account_locally_disabled(config: Option<&config::schema::Config>) -> bool {
    config
        .and_then(|cfg| cfg.openai_auth.as_ref())
        .and_then(|auth| auth.locally_disabled)
        .unwrap_or(false)
}

fn resolve_credentials_path(cargo_home: Option<PathBuf>, home_dir: Option<PathBuf>) -> PathBuf {
    if let Some(cargo_home) = cargo_home {
        return cargo_home.join(".cargo-ai/credentials.toml");
    }

    if let Some(home_dir) = home_dir {
        return home_dir.join(".cargo/.cargo-ai/credentials.toml");
    }

    PathBuf::from(".cargo/.cargo-ai/credentials.toml")
}

fn credentials_path() -> PathBuf {
    resolve_credentials_path(
        std::env::var_os("CARGO_HOME").map(PathBuf::from),
        dirs::home_dir(),
    )
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
fn load_account_tokens_from_keychain() -> Result<Option<AccountAuth>, String> {
    if !keychain_enabled() {
        return Err("keychain usage is disabled by CARGO_AI_DISABLE_KEYCHAIN".to_string());
    }

    let access_entry = keyring::Entry::new(KEYCHAIN_SERVICE, ACCOUNT_ACCESS_TOKEN_STORAGE_KEY)
        .map_err(|error| format!("failed to initialize account access-token keyring entry: {error}"))?;
    let refresh_entry = keyring::Entry::new(KEYCHAIN_SERVICE, ACCOUNT_REFRESH_TOKEN_STORAGE_KEY)
        .map_err(|error| format!("failed to initialize account refresh-token keyring entry: {error}"))?;

    let access_token = match access_entry.get_password() {
        Ok(token) if !token.trim().is_empty() => token,
        Ok(_) | Err(keyring::Error::NoEntry) => return Ok(None),
        Err(error) => {
            return Err(format!(
                "keyring lookup failed for account access token: {error}"
            ))
        }
    };

    let refresh_token = match refresh_entry.get_password() {
        Ok(token) if !token.trim().is_empty() => Some(token),
        Ok(_) | Err(keyring::Error::NoEntry) => None,
        Err(error) => {
            return Err(format!(
                "keyring lookup failed for account refresh token: {error}"
            ))
        }
    };

    Ok(Some(AccountAuth {
        access_token,
        refresh_token,
    }))
}

#[cfg(not(any(
    target_os = "macos",
    target_os = "ios",
    target_os = "windows",
    target_os = "linux",
    target_os = "freebsd",
    target_os = "openbsd"
)))]
fn load_account_tokens_from_keychain() -> Result<Option<AccountAuth>, String> {
    Err("keychain backend is unavailable on this platform".to_string())
}

fn load_account_tokens_from_file() -> Result<Option<AccountAuth>, String> {
    let path = credentials_path();
    if !path.exists() {
        return Ok(None);
    }

    let raw = fs::read_to_string(&path)
        .map_err(|error| format!("failed to read '{}': {error}", path.display()))?;
    let parsed = toml::from_str::<CredentialsFile>(&raw)
        .map_err(|error| format!("failed to parse '{}': {error}", path.display()))?;

    let Some(account) = parsed.account else {
        return Ok(None);
    };

    let access_token = account
        .access_token
        .as_deref()
        .map(str::trim)
        .filter(|token| !token.is_empty())
        .map(str::to_string);

    let refresh_token = account
        .refresh_token
        .as_deref()
        .map(str::trim)
        .filter(|token| !token.is_empty())
        .map(str::to_string);

    Ok(access_token.map(|access_token| AccountAuth {
        access_token,
        refresh_token,
    }))
}

fn read_credentials_file(path: &Path) -> Result<CredentialsFile, String> {
    if !path.exists() {
        return Ok(CredentialsFile::default());
    }

    let raw = fs::read_to_string(path)
        .map_err(|error| format!("failed to read '{}': {error}", path.display()))?;
    toml::from_str::<CredentialsFile>(&raw)
        .map_err(|error| format!("failed to parse '{}': {error}", path.display()))
}

fn write_credentials_file(path: &Path, credentials: &CredentialsFile) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| {
            format!(
                "failed to create credentials directory '{}': {error}",
                parent.display()
            )
        })?;
    }

    let serialized = toml::to_string_pretty(credentials)
        .map_err(|error| format!("failed to serialize credentials: {error}"))?;
    fs::write(path, serialized)
        .map_err(|error| format!("failed to write '{}': {error}", path.display()))?;
    lock_down_credentials_permissions(path)
}

#[cfg(unix)]
fn lock_down_credentials_permissions(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;

    let mut permissions = fs::metadata(path)
        .map_err(|error| format!("failed to read metadata for '{}': {error}", path.display()))?
        .permissions();
    permissions.set_mode(0o600);
    fs::set_permissions(path, permissions)
        .map_err(|error| format!("failed to set permissions on '{}': {error}", path.display()))
}

#[cfg(not(unix))]
fn lock_down_credentials_permissions(_path: &Path) -> Result<(), String> {
    Ok(())
}

fn persist_account_tokens_to_file(
    access_token: &str,
    refresh_token: Option<&str>,
) -> Result<(), String> {
    let path = credentials_path();
    let mut credentials = read_credentials_file(&path)?;
    credentials.account = Some(CredentialsAccount {
        access_token: Some(access_token.to_string()),
        refresh_token: refresh_token
            .map(str::trim)
            .filter(|token| !token.is_empty())
            .map(str::to_string),
    });
    write_credentials_file(&path, &credentials)
}

#[cfg(any(
    target_os = "macos",
    target_os = "ios",
    target_os = "windows",
    target_os = "linux",
    target_os = "freebsd",
    target_os = "openbsd"
))]
fn persist_account_tokens_to_keychain(
    access_token: &str,
    refresh_token: Option<&str>,
) -> Result<(), String> {
    if !keychain_enabled() {
        return Err("keychain usage is disabled by CARGO_AI_DISABLE_KEYCHAIN".to_string());
    }

    let access_entry = keyring::Entry::new(KEYCHAIN_SERVICE, ACCOUNT_ACCESS_TOKEN_STORAGE_KEY)
        .map_err(|error| format!("failed to initialize account access-token keyring entry: {error}"))?;
    access_entry
        .set_password(access_token)
        .map_err(|error| format!("failed to update account access token in keychain: {error}"))?;

    let refresh_entry = keyring::Entry::new(KEYCHAIN_SERVICE, ACCOUNT_REFRESH_TOKEN_STORAGE_KEY)
        .map_err(|error| format!("failed to initialize account refresh-token keyring entry: {error}"))?;
    match refresh_token.map(str::trim).filter(|token| !token.is_empty()) {
        Some(token) => refresh_entry
            .set_password(token)
            .map_err(|error| format!("failed to update account refresh token in keychain: {error}"))?,
        None => match refresh_entry.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => {}
            Err(error) => {
                return Err(format!(
                    "failed to clear account refresh token from keychain: {error}"
                ))
            }
        },
    }

    Ok(())
}

#[cfg(not(any(
    target_os = "macos",
    target_os = "ios",
    target_os = "windows",
    target_os = "linux",
    target_os = "freebsd",
    target_os = "openbsd"
)))]
fn persist_account_tokens_to_keychain(
    _access_token: &str,
    _refresh_token: Option<&str>,
) -> Result<(), String> {
    Err("keychain backend is unavailable on this platform".to_string())
}

fn persist_refreshed_account_tokens(
    access_token: &str,
    refresh_token: Option<&str>,
    config: Option<&config::schema::Config>,
) -> Result<(), String> {
    match config.and_then(|cfg| cfg.secret_store) {
        Some(SecretStoreMode::File) => persist_account_tokens_to_file(access_token, refresh_token),
        Some(SecretStoreMode::Keychain) => {
            persist_account_tokens_to_keychain(access_token, refresh_token)
        }
        None => match persist_account_tokens_to_keychain(access_token, refresh_token) {
            Ok(()) => Ok(()),
            Err(_) => persist_account_tokens_to_file(access_token, refresh_token),
        },
    }
}

fn load_account_auth(config: Option<&config::schema::Config>) -> Result<AccountAuth, String> {
    let configured_mode = config.and_then(|cfg| cfg.secret_store);
    let auth = match configured_mode {
        Some(config::schema::SecretStoreMode::File) => load_account_tokens_from_file()?,
        Some(config::schema::SecretStoreMode::Keychain) => load_account_tokens_from_keychain()?,
        None => match load_account_tokens_from_keychain() {
            Ok(Some(tokens)) => Some(tokens),
            Ok(None) | Err(_) => load_account_tokens_from_file()?,
        },
    };

    auth.ok_or_else(|| {
        format!(
            "No account access token found in '{}'. Run `cargo ai account confirm <code>` or `cargo ai account status` from Cargo AI first.",
            credentials_path().display()
        )
    })
}

async fn fetch_account_status(
    access_token: &str,
    refresh_token: Option<&str>,
) -> Result<serde_json::Value, String> {
    let url = format!("{}/account", INFRA_BASE_URL.trim_end_matches('/'));
    let mut credentials = serde_json::json!({
        "access_token": access_token
    });

    if let Some(refresh_token) = refresh_token {
        credentials["refresh_token"] = serde_json::json!(refresh_token);
    }

    let mut body = serde_json::json!({
        "action": "status",
        "credentials": credentials
    });

    if refresh_token.is_some() {
        body["session_policy"] = serde_json::json!({
            "allow_refresh": true
        });
    }

    reqwest::Client::new()
        .post(url)
        .json(&body)
        .send()
        .await
        .map_err(|error| format!("{error:?}"))?
        .json::<serde_json::Value>()
        .await
        .map_err(|error| format!("{error:?}"))
}

async fn send_account_mail_request(
    access_token: &str,
    subject: &str,
    text: &str,
) -> Result<serde_json::Value, String> {
    let url = format!("{}/account", INFRA_BASE_URL.trim_end_matches('/'));
    let body = serde_json::json!({
        "action": "send_mail",
        "credentials": {
            "access_token": access_token
        },
        "send_mail": {
            "subject": subject,
            "text": text
        }
    });

    reqwest::Client::new()
        .post(url)
        .json(&body)
        .send()
        .await
        .map_err(|error| format!("{error:?}"))?
        .json::<serde_json::Value>()
        .await
        .map_err(|error| format!("{error:?}"))
}

async fn run_email_me_action(
    subject: &str,
    text: &str,
    config: Option<&config::schema::Config>,
) -> Result<(), String> {
    let auth = load_account_auth(config)?;
    let access_token = auth.access_token;
    let refresh_token = auth.refresh_token;

    let mut response = send_account_mail_request(access_token.as_str(), subject, text).await?;

    let is_expired_error = response
        .get("type")
        .and_then(|value| value.as_str())
        .map(|value| value == "access_token_expired")
        .unwrap_or(false);

    if is_expired_error {
        let refresh_token = refresh_token.as_deref().ok_or_else(|| {
            "Access token expired, and no refresh token exists in credentials storage. Run `cargo ai account status` from Cargo AI first."
                .to_string()
        })?;

        let refresh_response =
            fetch_account_status(access_token.as_str(), Some(refresh_token)).await?;
        let refreshed_access_token = refresh_response
            .get("session")
            .and_then(|session| session.get("access_token"))
            .and_then(|value| value.as_str())
            .filter(|value| !value.is_empty())
            .map(str::to_string)
            .ok_or_else(|| {
                format_backend_error_message(&refresh_response).unwrap_or_else(|| {
                    "Session refresh did not return a new access token. Cannot retry email_me action."
                        .to_string()
                })
            })?;

        if let Err(error) = persist_refreshed_account_tokens(
            refreshed_access_token.as_str(),
            Some(refresh_token),
            config,
        ) {
            eprintln!("⚠️ Failed to update account tokens in credential store: {error}");
        }

        response =
            send_account_mail_request(refreshed_access_token.as_str(), subject, text).await?;
    }

    let succeeded = response
        .get("status")
        .and_then(|value| value.as_str())
        .map(|status| status.eq_ignore_ascii_case("success"))
        .unwrap_or(false);

    if succeeded {
        render_account_response(&response);
        Ok(())
    } else {
        Err(format_backend_error_message(&response)
            .unwrap_or_else(|| format!("email_me request failed.\n{}", pretty_backend_json(&response))))
    }
}

async fn resolve_openai_oauth_access_token(
    config: Option<&config::schema::Config>,
) -> Result<String, String> {
    if openai_account_locally_disabled(config) {
        return Err(
            "OpenAI account auth is logged out for Cargo AI locally. Run `cargo ai auth login openai` to re-enable, or pass `--token`."
                .to_string(),
        );
    }

    let Some(session) = load_codex_session()? else {
        return Err(
            "OpenAI authentication is missing. Install Codex and run `codex login`, or pass `--token`."
                .to_string(),
        );
    };

    if codex_access_token_expired_or_near(session.access_token_expires_at_unix) {
        return Err(
            "OpenAI account session in Codex cache is expired or near expiry. Re-run `codex login`."
                .to_string(),
        );
    }

    Ok(session.access_token)
}

async fn resolve_openai_token_for_request(
    selected_profile: Option<&SelectedProfile>,
    config: Option<&config::schema::Config>,
) -> Result<ResolvedOpenAiToken, String> {
    match selected_profile {
        Some(profile) => match profile.auth_mode {
            ProfileAuthMode::ApiKey => Ok(ResolvedOpenAiToken {
                token: resolve_profile_api_token(profile)?,
                uses_account_session: false,
            }),
            ProfileAuthMode::OpenaiAccount => Ok(ResolvedOpenAiToken {
                token: resolve_openai_oauth_access_token(config).await?,
                uses_account_session: true,
            }),
            ProfileAuthMode::None => Err(format!(
                "Profile '{}' auth mode is '{}'. Set it to '{}' or '{}' before using OpenAI without `--token`.",
                profile.name,
                ProfileAuthMode::None.as_str(),
                ProfileAuthMode::ApiKey.as_str(),
                ProfileAuthMode::OpenaiAccount.as_str()
            )),
        },
        None => Ok(ResolvedOpenAiToken {
            token: resolve_openai_oauth_access_token(config).await?,
            uses_account_session: true,
        }),
    }
}

fn apply_profile(profile: &Profile, server: &mut String, model: &mut String, timeout_in_sec: &mut u64, url: &mut String) -> SelectedProfile {
    *server = profile.server.clone().to_lowercase();
    *model = profile.model.clone();
    *timeout_in_sec = profile.timeout_in_sec;
    *url = profile.url.clone().unwrap_or_default();

    SelectedProfile {
        name: profile.name.clone(),
        auth_mode: profile.auth_mode,
        legacy_token: profile.token.clone(),
    }
}

fn runtime_input_overrides(cmd_args: &clap::ArgMatches) -> Vec<Input> {
    let mut ordered = Vec::new();

    collect_flagged_inputs(cmd_args, "input_text")
        .into_iter()
        .for_each(|(index, value)| ordered.push((index, Input::Text { text: value })));
    collect_flagged_inputs(cmd_args, "input_url")
        .into_iter()
        .for_each(|(index, value)| ordered.push((index, Input::Url { url: value })));
    collect_flagged_inputs(cmd_args, "input_image")
        .into_iter()
        .for_each(|(index, value)| ordered.push((index, Input::Image { path: value })));
    collect_flagged_inputs(cmd_args, "input_file")
        .into_iter()
        .for_each(|(index, value)| ordered.push((index, Input::File { path: value })));

    ordered.sort_by_key(|(index, _)| *index);
    ordered.into_iter().map(|(_, input)| input).collect()
}

fn runtime_input_mode(cmd_args: &clap::ArgMatches) -> Result<RuntimeInputMode, String> {
    match cmd_args.get_one::<String>("input_mode").map(String::as_str) {
        None | Some("replace") => Ok(RuntimeInputMode::Replace),
        Some("append") => Ok(RuntimeInputMode::Append),
        Some("prepend") => Ok(RuntimeInputMode::Prepend),
        Some(other) => Err(format!(
            "Unsupported --input-mode '{other}'. Expected replace, append, or prepend."
        )),
    }
}

fn collect_flagged_inputs(cmd_args: &clap::ArgMatches, id: &str) -> Vec<(usize, String)> {
    match (cmd_args.indices_of(id), cmd_args.get_many::<String>(id)) {
        (Some(indices), Some(values)) => indices
            .zip(values)
            .map(|(index, value)| (index, value.to_string()))
            .collect(),
        _ => Vec::new(),
    }
}

fn resolved_inputs_for_run(cmd_args: &clap::ArgMatches) -> Result<Vec<Input>, String> {
    let runtime_inputs = runtime_input_overrides(cmd_args);

    if runtime_inputs.is_empty() {
        if cmd_args.get_one::<String>("input_mode").is_some() {
            return Err(
                "--input-mode requires at least one runtime input flag such as --input-text, --input-url, --input-image, or --input-file."
                    .to_string(),
            );
        }
        return Ok(inputs());
    }

    let input_mode = runtime_input_mode(cmd_args)?;
    Ok(match input_mode {
        RuntimeInputMode::Replace => runtime_inputs,
        RuntimeInputMode::Append => {
            let mut selected_inputs = inputs();
            selected_inputs.extend(runtime_inputs);
            selected_inputs
        }
        RuntimeInputMode::Prepend => {
            let mut selected_inputs = runtime_inputs;
            selected_inputs.extend(inputs());
            selected_inputs
        }
    })
}

// Initialize Tokio runtime macro
#[tokio::main]
async fn main() {
    let cmd_args = args::build_cli();
    let config = load_config();

    let mut server = String::new();
    let mut model = String::new();
    let mut url = String::new();
    let mut token = String::new();
    let mut inference_timeout_in_sec: u64 = 60;
    let mut selected_profile: Option<SelectedProfile> = None;
    let mut loaded_profile_message: Option<(LoadedProfileKind, String)> = None;
    let mut use_openai_account_transport = false;

    if let Some(profile_name) = cmd_args.get_one::<String>("profile") {
        if let Some(profile) = config
            .as_ref()
            .and_then(|cfg| find_profile(cfg, profile_name))
        {
            selected_profile = Some(apply_profile(
                profile,
                &mut server,
                &mut model,
                &mut inference_timeout_in_sec,
                &mut url,
            ));
            loaded_profile_message = Some((LoadedProfileKind::Explicit, profile_name.to_string()));
        } else if config.is_some() {
            eprintln!("Profile '{}' not found.", profile_name);
        } else {
            eprintln!("No config file found.");
        }
    }

    if server.is_empty() {
        if let Some(profile) = config
            .as_ref()
            .and_then(|cfg| cfg.default_profile.as_deref().and_then(|name| find_profile(cfg, name)))
        {
            selected_profile = Some(apply_profile(
                profile,
                &mut server,
                &mut model,
                &mut inference_timeout_in_sec,
                &mut url,
            ));
            loaded_profile_message = Some((LoadedProfileKind::Default, profile.name.clone()));
        }
    }

    if let Some(server_arg) = cmd_args.get_one::<String>("server") {
        server = server_arg.to_lowercase();
    }

    if let Some(model_arg) = cmd_args.get_one::<String>("model") {
        model = model_arg.to_string();
    }

    if let Some(url_arg) = cmd_args.get_one::<String>("url") {
        url = url_arg.to_string();
    }

    if let Some(timeout_arg) = cmd_args.get_one::<u64>("inference_timeout_in_sec").copied() {
        inference_timeout_in_sec = timeout_arg;
    }

    let max_agent_depth =
        configured_agent_action_max_depth(cmd_args.get_one::<u32>("max_agent_depth").copied());
    let runtime_budget =
        configured_agent_action_runtime_budget(cmd_args.get_one::<u64>("max_runtime_in_sec").copied());

    let provider = match ProviderKind::from_server_value(&server) {
        Some(provider) => provider,
        None => {
            for line in unknown_server_messages(&server) {
                eprintln!("{line}");
            }
            return;
        }
    };

    let explicit_token_override = cmd_args.get_one::<String>("token").map(|token| token.to_string());
    if let Some((kind, profile_name)) = loaded_profile_message.as_ref() {
        for line in profile_selection_messages(
            *kind,
            profile_name,
            &cli_override_descriptions(
                &cmd_args,
                explicit_token_override.is_some() && provider == ProviderKind::OpenAi,
            ),
        ) {
            println!("{line}");
        }
    }

    if let Some(cmd_token) = explicit_token_override {
        if provider == ProviderKind::OpenAi {
            println!("Using explicit --token override; bypassing profile auth-mode resolution.");
        }
        token = cmd_token;
    } else if provider == ProviderKind::OpenAi {
        token = match resolve_openai_token_for_request(selected_profile.as_ref(), config.as_ref()).await {
            Ok(resolved) => {
                use_openai_account_transport = resolved.uses_account_session;
                resolved.token
            }
            Err(error) => {
                eprintln!("❌ {error}");
                return;
            }
        };
    }

    if url.is_empty() {
        if provider == ProviderKind::OpenAi && use_openai_account_transport {
            url = OPENAI_ACCOUNT_RESPONSES_URL.to_string();
        } else {
            url = provider.default_url().to_string();
        }
    }

    if let Err(validation_issues) = validate_provider_request(provider, &model, &url, &token) {
        for issue in validation_issues {
            eprintln!("{issue}");
        }
        return;
    }

    let selected_inputs = match resolved_inputs_for_run(&cmd_args) {
        Ok(selected_inputs) => selected_inputs,
        Err(error) => {
            eprintln!("❌ {error}");
            return;
        }
    };
    let resolved_inputs = match crate::providers::resolve_provider_inputs(&selected_inputs).await {
        Ok(resolved_inputs) => resolved_inputs,
        Err(error) => {
            eprintln!("❌ Failed to resolve runtime inputs.");
            eprintln!("Reason: {error}");
            return;
        }
    };

    if let Err(validation_issues) =
        validate_provider_content_parts(provider, &url, &resolved_inputs)
    {
        for issue in validation_issues {
            eprintln!("{issue}");
        }
        return;
    }

    let static_context =
        "A question will be asked and you will need to return the answer in the specified JSON format.";
    let mut ai_cargo =
        crate::providers::AgentCargo::<Output>::new(resolved_inputs, static_context.to_string());

    let content_parts = ai_cargo.content_parts();
    let mut response = String::new();

    if provider == ProviderKind::Ollama {
        let remaining = match remaining_runtime_duration(runtime_budget, "before starting inference") {
            Ok(remaining) => remaining,
            Err(error) => {
                eprintln!(
                    "❌ {}",
                    current_agent_runtime_timeout_message(runtime_budget, error.as_str())
                );
                return;
            }
        };

        match tokio::time::timeout(
            remaining,
            crate::providers::send_ollama_request(
                &url,
                &model,
                &content_parts,
                inference_timeout_in_sec,
                json_schema_value(),
            ),
        )
        .await
        {
            Ok(Ok(r)) => response.push_str(&r),
            Ok(Err(error)) => {
                for line in provider_error_messages(&error) {
                    eprintln!("{line}");
                }
                return;
            }
            Err(_) => {
                eprintln!(
                    "❌ {}",
                    current_agent_runtime_timeout_message(
                        runtime_budget,
                        "while waiting for the model response"
                    )
                );
                return;
            }
        }
    } else if provider == ProviderKind::OpenAi {
        let mut schema = json_schema_value();
        if let Some(obj) = schema.as_object_mut() {
            obj.insert("additionalProperties".into(), serde_json::Value::Bool(false));
        }

        let fmt = serde_json::json!({
            "type": "json_schema",
            "json_schema": {
                "name": "Output",
                "schema": schema,
                "strict": true
            }
        });

        let remaining = match remaining_runtime_duration(runtime_budget, "before starting inference") {
            Ok(remaining) => remaining,
            Err(error) => {
                eprintln!(
                    "❌ {}",
                    current_agent_runtime_timeout_message(runtime_budget, error.as_str())
                );
                return;
            }
        };

        match tokio::time::timeout(
            remaining,
            crate::providers::send_openai_request(
                &url,
                &model,
                &content_parts,
                inference_timeout_in_sec,
                &token,
                fmt,
            ),
        )
        .await
        {
            Ok(Ok(r)) => response.push_str(&r),
            Ok(Err(error)) => {
                for line in provider_error_messages(&error) {
                    eprintln!("{line}");
                }
                return;
            }
            Err(_) => {
                eprintln!(
                    "❌ {}",
                    current_agent_runtime_timeout_message(
                        runtime_budget,
                        "while waiting for the model response"
                    )
                );
                return;
            }
        };
    }

    if !ai_cargo.set_response(response.clone()) {
        eprintln!("❌ LLM output did NOT conform to the required JSON schema.");
        eprintln!("Raw output received from server:\n{}\n", response);
        return;
    }

    let output = match ai_cargo.get_response() {
        Some(o) => o,
        None => {
            eprintln!("❌ Internal error: response was expected but missing.");
            eprintln!("Raw output received from server:\n{}\n", response);
            return;
        }
    };

    let actions = actions();
    let action_provider_context = ActionProviderContext {
        provider,
        url: url.clone(),
        token: token.clone(),
        inference_timeout_in_sec,
    };
    if let Err(error) =
        apply_actions(
            &output,
            &actions,
            config.as_ref(),
            &action_provider_context,
            max_agent_depth,
            runtime_budget,
        )
        .await
    {
        eprintln!("❌ {error}");
        std::process::exit(1);
    }
}

async fn apply_actions(
    output: &Output,
    actions: &[Action],
    config: Option<&config::schema::Config>,
    provider_context: &ActionProviderContext,
    max_agent_depth: u32,
    runtime_budget: InvocationRuntimeBudget,
) -> Result<(), String> {
    let data = serde_json::to_value(output).unwrap();
    let current_platform = current_action_platform();

    for action in actions {
        if let Ok(result) = apply(&action.logic, &data) {
            if result.as_bool() == Some(true) {
                let matching_steps = matching_run_steps(&action.run, current_platform);
                if matching_steps.is_empty() {
                    println!(
                        "⚠️ No run steps matched the current platform for action '{}' (current platform: {}).",
                        action.name,
                        current_platform.unwrap_or("unsupported")
                        );
                    continue;
                }

                print_action_start(&action.name);
                let single_step_action = matching_steps.len() == 1;
                let mut outcomes = Vec::with_capacity(matching_steps.len());
                let mut action_data = data.clone();

                for step in matching_steps {
                    if !should_run_step(step, &action_data, &action.name)? {
                        continue;
                    }

                    let step_result = if step.kind.eq_ignore_ascii_case("exec") {
                        run_exec_step(step, &action_data, &action.name, runtime_budget)
                            .await
                            .map(|captured_output| {
                                (StepExecutionOutcome::Completed, captured_output)
                            })
                    } else if step.kind.eq_ignore_ascii_case("email_me") {
                        run_email_me_step(
                            step,
                            &action_data,
                            &action.name,
                            config,
                            runtime_budget,
                            single_step_action,
                        )
                        .await
                        .map(|outcome| (outcome, None))
                    } else if step.kind.eq_ignore_ascii_case("agent") {
                        run_agent_step(
                            step,
                            &action_data,
                            &action.name,
                            max_agent_depth,
                            runtime_budget,
                        )
                        .await
                        .map(|outcome| (outcome, None))
                    } else if step.kind.eq_ignore_ascii_case("generate_image") {
                        run_generate_image_step(
                            step,
                            &action_data,
                            &action.name,
                            provider_context,
                            runtime_budget,
                        )
                        .await
                        .map(|outcome| (outcome, None))
                    } else {
                        println!(
                            "⚠️ Skipping action '{}' with unsupported step kind '{}'.",
                            action.name, step.kind
                        );
                        outcomes.push(StepExecutionOutcome::SoftFailureLogged);
                        continue;
                    };

                    match step_result {
                        Ok((outcome, captured_output)) => {
                            if let Some((name, value)) = captured_output {
                                insert_action_output_variable(
                                    &mut action_data,
                                    name.as_str(),
                                    value,
                                    action.name.as_str(),
                                )?;
                            }
                            insert_step_status_variable(
                                &mut action_data,
                                step,
                                "succeeded",
                                action.name.as_str(),
                            )?;
                            outcomes.push(outcome);
                        }
                        Err(error) => {
                            insert_step_status_variable(
                                &mut action_data,
                                step,
                                "failed",
                                action.name.as_str(),
                            )?;
                            insert_step_error_variable(
                                &mut action_data,
                                step,
                                error.as_str(),
                                action.name.as_str(),
                            )?;

                            if matches!(step_failure_mode(step), FailureMode::Continue) {
                                println!("{error}");
                                outcomes.push(StepExecutionOutcome::SoftFailureLogged);
                            } else {
                                return Err(error);
                            }
                        }
                    }
                }

                if let Some(summary) = action_completion_summary(&outcomes) {
                    print_action_success(&action.name, summary);
                }
            }
        } else {
            println!("Failed to evaluate logic for action: {}", action.name);
        }
    }

    if let Some(message) = root_run_completion_message() {
        println!("{message}");
    }

    Ok(())
}

async fn run_exec_step(
    step: &RunStep,
    data: &serde_json::Value,
    action_name: &str,
    runtime_budget: InvocationRuntimeBudget,
) -> Result<Option<(String, String)>, String> {
    let program = step.program.as_deref().ok_or_else(|| {
        format!(
            "Action '{}' exec step is missing required `program`.",
            action_name
        )
    })?;

    let resolved_args = resolve_run_args(&step.args, data, action_name)
        .map_err(|error| format!("Action '{}': {error}", action_name))?;

    let remaining = remaining_runtime_duration(
        runtime_budget,
        &format!("before starting command '{}'", program),
    )
    .map_err(|context| {
        action_runtime_timeout_message(action_name, runtime_budget, context.as_str())
    })?;

    if let Some(output_variable) = step.output_variable.as_deref() {
        let child = tokio::process::Command::new(program)
            .args(&resolved_args)
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .map_err(|error| format!("{action_name}: failed to execute command: {error}."))?;

        match tokio::time::timeout(remaining, child.wait_with_output()).await {
            Ok(Ok(output)) if output.status.success() => {
                let captured_output = String::from_utf8_lossy(&output.stdout)
                    .trim_end_matches(['\r', '\n'])
                    .to_string();
                println!(
                    "ℹ️ Action '{}' stored exec output in variable '{}'.",
                    action_name, output_variable
                );
                Ok(Some((output_variable.to_string(), captured_output)))
            }
            Ok(Ok(output)) => Err(format!(
                "Action '{}' exec step command '{}' exited with status {}.",
                action_name, program, output.status
            )),
            Ok(Err(error)) => Err(format!(
                "Action '{}' exec step failed while waiting for command '{}': {}",
                action_name, program, error
            )),
            Err(_) => Err(action_runtime_timeout_message(
                action_name,
                runtime_budget,
                &format!("while waiting for command '{}'", program),
            )),
        }
    } else {
        let mut child = tokio::process::Command::new(program)
            .args(&resolved_args)
            .spawn()
            .map_err(|error| format!("{action_name}: failed to execute command: {error}."))?;

        match tokio::time::timeout(remaining, child.wait()).await {
            Ok(Ok(status)) if status.success() => Ok(None),
            Ok(Ok(status)) => Err(format!(
                "Action '{}' exec step command '{}' exited with status {}.",
                action_name, program, status
            )),
            Ok(Err(error)) => Err(format!(
                "Action '{}' exec step failed while waiting for command '{}': {}",
                action_name, program, error
            )),
            Err(_) => {
                let _ = child.kill().await;
                Err(action_runtime_timeout_message(
                    action_name,
                    runtime_budget,
                    &format!("while waiting for command '{}'", program),
                ))
            }
        }
    }
}

fn insert_action_output_variable(
    data: &mut serde_json::Value,
    name: &str,
    value: String,
    action_name: &str,
) -> Result<(), String> {
    insert_action_string_variable(data, name, value, action_name)
}

fn insert_action_string_variable(
    data: &mut serde_json::Value,
    name: &str,
    value: String,
    action_name: &str,
) -> Result<(), String> {
    let Some(object) = data.as_object_mut() else {
        return Err(format!(
            "Action '{}' could not store captured variable '{}' because the action data context is not an object.",
            action_name, name
        ));
    };

    object.insert(name.to_string(), serde_json::Value::String(value));
    Ok(())
}

fn insert_step_status_variable(
    data: &mut serde_json::Value,
    step: &RunStep,
    status: &str,
    action_name: &str,
) -> Result<(), String> {
    let Some(name) = step.status_variable.as_deref() else {
        return Ok(());
    };

    insert_action_string_variable(data, name, status.to_string(), action_name)
}

fn insert_step_error_variable(
    data: &mut serde_json::Value,
    step: &RunStep,
    error: &str,
    action_name: &str,
) -> Result<(), String> {
    let Some(name) = step.error_variable.as_deref() else {
        return Ok(());
    };

    insert_action_string_variable(data, name, error.to_string(), action_name)
}

fn step_failure_mode(step: &RunStep) -> FailureMode {
    step.failure_mode.clone().unwrap_or(FailureMode::Stop)
}

fn should_run_step(
    step: &RunStep,
    data: &serde_json::Value,
    action_name: &str,
) -> Result<bool, String> {
    let Some(condition) = step.when.as_ref() else {
        return Ok(true);
    };

    apply(condition, data)
        .map(|result| result.as_bool() == Some(true))
        .map_err(|error| {
            format!(
                "Action '{}' failed to evaluate step `when` for kind '{}': {}",
                action_name, step.kind, error
            )
        })
}

async fn run_email_me_step(
    step: &RunStep,
    data: &serde_json::Value,
    action_name: &str,
    config: Option<&config::schema::Config>,
    runtime_budget: InvocationRuntimeBudget,
    single_step_action: bool,
) -> Result<StepExecutionOutcome, String> {
    let subject_parts = step.subject.as_deref().ok_or_else(|| {
        format!(
            "Action '{}' email_me step is missing required `subject`.",
            action_name
        )
    })?;
    let text_parts = step.text.as_deref().ok_or_else(|| {
        format!(
            "Action '{}' email_me step is missing required `text`.",
            action_name
        )
    })?;

    let subject = resolve_string_parts(subject_parts, data, action_name, "subject")?;
    let text = resolve_string_parts(text_parts, data, action_name, "text")?;

    let remaining =
        remaining_runtime_duration(runtime_budget, "before sending email").map_err(|context| {
            action_runtime_timeout_message(action_name, runtime_budget, context.as_str())
        })?;

    match tokio::time::timeout(
        remaining,
        run_email_me_action(subject.as_str(), text.as_str(), config),
    )
    .await
    {
        Ok(Ok(())) => {}
        Ok(Err(error)) => return Err(format!("Action '{}': {error}", action_name)),
        Err(_) => {
            return Err(action_runtime_timeout_message(
                action_name,
                runtime_budget,
                "while sending email",
            ));
        }
    }

    if single_step_action {
        print_action_success(action_name, "email sent");
    }
    Ok(if single_step_action {
        StepExecutionOutcome::SuccessAlreadyPrinted
    } else {
        StepExecutionOutcome::Completed
    })
}

async fn run_generate_image_step(
    step: &RunStep,
    data: &serde_json::Value,
    action_name: &str,
    provider_context: &ActionProviderContext,
    runtime_budget: InvocationRuntimeBudget,
) -> Result<StepExecutionOutcome, String> {
    if provider_context.provider != ProviderKind::OpenAi {
        return Err(format!(
            "Action '{}' generate_image step requires `--server openai`; current server is {}.",
            action_name,
            provider_context.provider.display_name()
        ));
    }

    let model = step.model.as_ref().ok_or_else(|| {
        format!(
            "Action '{}' generate_image step is missing required `model`.",
            action_name
        )
    })?;

    if provider_context.url.contains("chatgpt.com/backend-api/codex")
        && model.starts_with("gpt-image")
    {
        return Err(format!(
            "Action '{}' generate_image step uses OpenAI account transport, so `model` must be a tool-capable mainline model such as `gpt-5.2`, not '{}'.",
            action_name, model
        ));
    }
    let prompt_parts = step.prompt.as_deref().ok_or_else(|| {
        format!(
            "Action '{}' generate_image step is missing required `prompt`.",
            action_name
        )
    })?;
    let path_parts = step.path.as_deref().ok_or_else(|| {
        format!(
            "Action '{}' generate_image step is missing required `path`.",
            action_name
        )
    })?;

    let prompt = resolve_string_parts(prompt_parts, data, action_name, "prompt")
        .map_err(|error| format!("Action '{}': {error}", action_name))?;
    let output_path = resolve_string_parts(path_parts, data, action_name, "path")
        .map_err(|error| format!("Action '{}': {error}", action_name))?;
    let output_format = generated_image_output_format(output_path.as_str(), action_name)?;

    let remaining = remaining_runtime_duration(
        runtime_budget,
        &format!("before starting image generation with model '{}'", model),
    )
    .map_err(|context| {
        action_runtime_timeout_message(
            action_name,
            runtime_budget,
            context.as_str(),
        )
    })?;

    let image_bytes = match tokio::time::timeout(
        remaining,
        crate::providers::send_openai_image_request(
            &provider_context.url,
            model,
            prompt.as_str(),
            provider_context.inference_timeout_in_sec,
            &provider_context.token,
            output_format,
        ),
    )
    .await
    {
        Ok(Ok(bytes)) => bytes,
        Ok(Err(error)) => {
            let mut lines =
                vec![format!("Action '{}' generate_image step failed.", action_name)];
            lines.extend(provider_error_messages(&error));
            return Err(lines.join("\n"));
        }
        Err(_) => {
            return Err(action_runtime_timeout_message(
                action_name,
                runtime_budget,
                "while waiting for image generation",
            ));
        }
    };

    let output_path_ref = Path::new(output_path.as_str());
    validate_generated_image_output_path(output_path_ref, action_name)?;
    if let Some(parent) = output_path_ref.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent).map_err(|error| {
                format!(
                    "Action '{}' failed to create image output directory '{}': {}",
                    action_name,
                    parent.display(),
                    error
                )
            })?;
        }
    }

    std::fs::write(output_path_ref, image_bytes).map_err(|error| {
        format!(
            "Action '{}' failed to write generated image '{}': {}",
            action_name,
            output_path_ref.display(),
            error
        )
    })?;

    println!(
        "ℹ️ Action '{}' wrote generated image to '{}'.",
        action_name,
        output_path_ref.display()
    );
    Ok(StepExecutionOutcome::Completed)
}

async fn run_agent_step(
    step: &RunStep,
    data: &serde_json::Value,
    action_name: &str,
    max_agent_depth: u32,
    runtime_budget: InvocationRuntimeBudget,
) -> Result<StepExecutionOutcome, String> {
    let agent = step.agent.as_deref().ok_or_else(|| {
        format!(
            "Action '{}' agent step is missing required `agent`.",
            action_name
        )
    })?;

    let current_depth = current_agent_action_depth();
    validate_agent_action_depth(current_depth, max_agent_depth, action_name)?;

    validate_agent_step_target(agent, action_name)?;
    let agent_path = Path::new(agent);
    if !agent_path.exists() {
        return Err(format!(
            "Action '{}' agent step target '{}' was not found relative to the current working directory.",
            action_name, agent
        ));
    }

    let mut command = tokio::process::Command::new(agent_path);
    let (child_args, resolution_notes) =
        child_input_args(step.input_mode, step.inputs.as_deref(), data, action_name)?;
    for note in resolution_notes {
        println!("ℹ️ {note}");
    }
    for argument in child_args {
        command.arg(argument);
    }
    command.env(AGENT_ACTION_DEPTH_ENV, (current_depth + 1).to_string());
    command.env(AGENT_ACTION_MAX_DEPTH_ENV, max_agent_depth.to_string());
    command.env(
        AGENT_ACTION_MAX_RUNTIME_SECS_ENV,
        runtime_budget.max_runtime_secs.to_string(),
    );
    command.env(
        AGENT_ACTION_RUNTIME_STARTED_AT_MS_ENV,
        runtime_budget.started_at_ms.to_string(),
    );
    command.env(
        AGENT_ACTION_RUNTIME_DEADLINE_MS_ENV,
        runtime_budget.deadline_ms.to_string(),
    );

    let remaining = remaining_runtime_duration(
        runtime_budget,
        &format!("before starting child agent '{}'", agent),
    )
    .map_err(|context| {
        action_runtime_timeout_message(
            action_name,
            runtime_budget,
            context.as_str(),
        )
    })?;

    let mut child = command.spawn().map_err(|error| {
        format!(
            "Action '{}' failed to start child agent '{}': {}",
            action_name, agent, error
        )
    })?;

    match tokio::time::timeout(remaining, child.wait()).await {
        Ok(Ok(status)) if status.success() => Ok(StepExecutionOutcome::Completed),
        Ok(Ok(status)) => Err(format!(
            "Action '{}' child agent '{}' exited with status {} at depth {}.",
            action_name,
            agent,
            status,
            current_depth + 1
        )),
        Ok(Err(error)) => Err(format!(
            "Action '{}' failed while waiting for child agent '{}' at depth {}: {}",
            action_name,
            agent,
            current_depth + 1,
            error
        )),
        Err(_) => {
            let _ = child.kill().await;
            Err(action_runtime_timeout_message(
                action_name,
                runtime_budget,
                &format!("while waiting for child agent '{}' at depth {}", agent, current_depth + 1),
            ))
        }
    }
}

fn action_completion_summary(outcomes: &[StepExecutionOutcome]) -> Option<&'static str> {
    if outcomes.is_empty()
        || outcomes.iter().any(|outcome| {
            matches!(
                outcome,
                StepExecutionOutcome::SoftFailureLogged | StepExecutionOutcome::SuccessAlreadyPrinted
            )
        })
    {
        None
    } else {
        Some("completed")
    }
}

fn run_completion_message_for_depth(depth: u32) -> Option<&'static str> {
    if depth == 0 {
        Some("✅ Run complete.")
    } else {
        None
    }
}

fn root_run_completion_message() -> Option<&'static str> {
    run_completion_message_for_depth(current_agent_action_depth())
}

fn print_action_start(action_name: &str) {
    println!("Running action: {}", action_name);
}

fn print_action_success(action_name: &str, summary: &str) {
    println!("{action_name}: {summary}.");
}

fn render_account_response(response: &serde_json::Value) {
    if let Some(message) = format_backend_ui_message(response, true) {
        println!("{message}");
    } else {
        println!("{}", pretty_backend_json(response));
    }
}

fn format_backend_error_message(response: &serde_json::Value) -> Option<String> {
    format_backend_ui_message(response, false)
}

fn format_backend_ui_message(
    response: &serde_json::Value,
    include_kind_prefix: bool,
) -> Option<String> {
    let ui = response.get("ui")?;
    if ui.get("schema").and_then(|value| value.as_str()) != Some("1.0") {
        return None;
    }

    let kind = ui.get("kind").and_then(|value| value.as_str()).unwrap_or("info");
    let title = ui
        .get("title")
        .and_then(|value| value.as_str())
        .unwrap_or("Status");
    let summary = ui
        .get("summary")
        .and_then(|value| value.as_str())
        .unwrap_or("Status response received.");

    let mut lines = Vec::new();
    if include_kind_prefix {
        let kind_prefix = match kind {
            "success" => "✅",
            "error" => "⚠️",
            "failure" => "❌",
            _ => "ℹ️",
        };
        lines.push(format!("{kind_prefix} {title}"));
    } else {
        lines.push(title.to_string());
    }
    lines.push(summary.to_string());

    if let Some(variant) = ui.get("variant").and_then(|value| value.as_str()) {
        if !variant.trim().is_empty() {
            lines.push(format!("Variant: {variant}"));
        }
    }

    if let Some(sections) = ui.get("sections").and_then(|value| value.as_array()) {
        for section in sections {
            append_backend_section_lines(section, &mut lines);
        }
    }

    if let Some(actions) = ui.get("actions").and_then(|value| value.as_array()) {
        let action_lines = actions
            .iter()
            .filter_map(|action| {
                let label = action.get("label").and_then(|value| value.as_str()).unwrap_or("");
                let command = action
                    .get("command")
                    .and_then(|value| value.as_str())
                    .unwrap_or("");

                if label.is_empty() && command.is_empty() {
                    None
                } else if !label.is_empty() && !command.is_empty() {
                    Some(format!("- {}: {}", label, command))
                } else if !label.is_empty() {
                    Some(format!("- {}", label))
                } else {
                    Some(format!("- {}", command))
                }
            })
            .collect::<Vec<_>>();

        if !action_lines.is_empty() {
            lines.push(String::new());
            lines.push("Actions:".to_string());
            lines.extend(action_lines);
        }
    }

    if let Some(next_steps) = ui.get("next_steps").and_then(|value| value.as_array()) {
        let step_lines = next_steps
            .iter()
            .filter_map(|step| step.as_str())
            .map(str::trim)
            .filter(|step| !step.is_empty())
            .map(|step| format!("- {}", step))
            .collect::<Vec<_>>();

        if !step_lines.is_empty() {
            lines.push(String::new());
            lines.push("Next steps:".to_string());
            lines.extend(step_lines);
        }
    }

    Some(lines.join("\n"))
}

fn append_backend_section_lines(section: &serde_json::Value, lines: &mut Vec<String>) {
    let section_type = section
        .get("type")
        .and_then(|value| value.as_str())
        .unwrap_or("");
    let title = section
        .get("title")
        .and_then(|value| value.as_str())
        .unwrap_or("");

    if !title.is_empty() {
        lines.push(String::new());
        lines.push(format!("{title}:"));
    }

    match section_type {
        "kv" => {
            if let Some(items) = section.get("items").and_then(|value| value.as_array()) {
                for item in items {
                    let label = item.get("label").and_then(|value| value.as_str()).unwrap_or("");
                    let value = item
                        .get("value")
                        .map(backend_ui_value_to_string)
                        .unwrap_or_default();

                    if label.is_empty() && value.is_empty() {
                        continue;
                    }

                    if label.is_empty() {
                        lines.push(format!("- {}", value));
                    } else {
                        lines.push(format!("- {}: {}", label, value));
                    }
                }
            }
        }
        "list" => {
            if let Some(items) = section.get("items").and_then(|value| value.as_array()) {
                for item in items {
                    let value = backend_ui_value_to_string(item);
                    if !value.is_empty() {
                        lines.push(format!("- {}", value));
                    }
                }
            }
        }
        "notice" => {
            if let Some(message) = section.get("message").and_then(|value| value.as_str()) {
                if !message.trim().is_empty() {
                    lines.push(message.to_string());
                }
            }
        }
        "json" => {
            if let Some(data) = section.get("data") {
                match serde_json::to_string_pretty(data) {
                    Ok(pretty) => lines.extend(pretty.lines().map(str::to_string)),
                    Err(_) => lines.push(backend_ui_value_to_string(data)),
                }
            }
        }
        _ => {}
    }
}

fn backend_ui_value_to_string(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::Null => "null".to_string(),
        serde_json::Value::Bool(boolean) => boolean.to_string(),
        serde_json::Value::Number(number) => number.to_string(),
        serde_json::Value::String(text) => text.to_string(),
        serde_json::Value::Array(_) | serde_json::Value::Object(_) => {
            serde_json::to_string(value).unwrap_or_default()
        }
    }
}

fn pretty_backend_json(response: &serde_json::Value) -> String {
    serde_json::to_string_pretty(response).unwrap_or_else(|_| format!("{response:?}"))
}

fn current_action_platform() -> Option<&'static str> {
    if cfg!(target_os = "macos") {
        Some("macos")
    } else if cfg!(target_os = "linux") {
        Some("linux")
    } else if cfg!(target_os = "windows") {
        Some("windows")
    } else {
        None
    }
}

fn matching_run_steps<'a>(
    run_steps: &'a [RunStep],
    current_platform: Option<&str>,
) -> Vec<&'a RunStep> {
    run_steps
        .iter()
        .filter(|step| step_matches_platform(step.platforms.as_deref(), current_platform))
        .collect()
}

fn child_input_args(
    input_mode: Option<ActionInputMode>,
    inputs: Option<&[ActionInput]>,
    data: &serde_json::Value,
    action_name: &str,
) -> Result<(Vec<String>, Vec<String>), String> {
    let mut args = Vec::new();
    let mut notes = Vec::new();

    if let Some(input_mode) = input_mode {
        if inputs.is_none() {
            return Err(format!(
                "Action '{}' child-agent `input_mode` requires `inputs`.",
                action_name
            ));
        }

        args.push("--input-mode".to_string());
        args.push(
            match input_mode {
                ActionInputMode::Replace => "replace",
                ActionInputMode::Append => "append",
                ActionInputMode::Prepend => "prepend",
            }
            .to_string(),
        );
    }

    if let Some(inputs) = inputs {
        for (index, input) in inputs.iter().enumerate() {
            match input {
                ActionInput::Text { text } => {
                    let resolved = resolve_string_parts(
                        text,
                        data,
                        action_name,
                        &format!("child-agent text input {}", index + 1),
                    )?;
                    args.push("--input-text".to_string());
                    args.push(resolved);
                    if child_input_uses_dynamic_parts(text) {
                        notes.push(format!(
                            "Action '{}' resolved dynamic child-agent text input {}.",
                            action_name,
                            index + 1
                        ));
                    }
                }
                ActionInput::Url { url } => {
                    let resolved = resolve_string_parts(
                        url,
                        data,
                        action_name,
                        &format!("child-agent url input {}", index + 1),
                    )?;
                    validate_child_input_url(&resolved, action_name, index + 1)?;
                    args.push("--input-url".to_string());
                    args.push(resolved.clone());
                    if child_input_uses_dynamic_parts(url) {
                        notes.push(format!(
                            "Action '{}' resolved dynamic child-agent url input {} -> {}.",
                            action_name,
                            index + 1,
                            resolved
                        ));
                    }
                }
                ActionInput::Image { path } => {
                    let resolved = resolve_string_parts(
                        path,
                        data,
                        action_name,
                        &format!("child-agent image path input {}", index + 1),
                    )?;
                    validate_child_input_path(&resolved, action_name, index + 1, "image")?;
                    args.push("--input-image".to_string());
                    args.push(resolved.clone());
                    if child_input_uses_dynamic_parts(path) {
                        notes.push(format!(
                            "Action '{}' resolved dynamic child-agent image path input {} -> {}.",
                            action_name,
                            index + 1,
                            resolved
                        ));
                    }
                }
                ActionInput::File { path } => {
                    let resolved = resolve_string_parts(
                        path,
                        data,
                        action_name,
                        &format!("child-agent file path input {}", index + 1),
                    )?;
                    validate_child_input_path(&resolved, action_name, index + 1, "file")?;
                    validate_child_file_extension(&resolved, action_name, index + 1)?;
                    args.push("--input-file".to_string());
                    args.push(resolved.clone());
                    if child_input_uses_dynamic_parts(path) {
                        notes.push(format!(
                            "Action '{}' resolved dynamic child-agent file path input {} -> {}.",
                            action_name,
                            index + 1,
                            resolved
                        ));
                    }
                }
            }
        }
    }

    Ok((args, notes))
}

fn child_input_uses_dynamic_parts(parts: &[RunArg]) -> bool {
    parts.iter().any(|part| matches!(part, RunArg::Variable(_)))
}

fn validate_generated_image_output_path(path: &Path, action_name: &str) -> Result<(), String> {
    let raw_path = path.to_string_lossy();
    if raw_path.trim().is_empty() {
        return Err(format!(
            "Action '{}' generate_image `path` must resolve to a non-empty relative path.",
            action_name
        ));
    }
    if path.is_absolute() {
        return Err(format!(
            "Action '{}' generate_image `path` must resolve to a relative path.",
            action_name
        ));
    }
    if path
        .components()
        .any(|component| matches!(component, Component::ParentDir))
    {
        return Err(format!(
            "Action '{}' generate_image `path` must not use parent traversal (`..`).",
            action_name
        ));
    }

    generated_image_output_format(raw_path.as_ref(), action_name).map(|_| ())
}

fn generated_image_output_format(raw_path: &str, action_name: &str) -> Result<&'static str, String> {
    let extension = Path::new(raw_path)
        .extension()
        .and_then(|value| value.to_str())
        .map(|value| value.to_ascii_lowercase());

    match extension.as_deref() {
        Some("png") => Ok("png"),
        Some("jpg") | Some("jpeg") => Ok("jpeg"),
        Some("webp") => Ok("webp"),
        _ => Err(format!(
            "Action '{}' generate_image `path` must use a supported extension: `.png`, `.jpg`, `.jpeg`, `.webp`.",
            action_name
        )),
    }
}

fn validate_child_input_url(url: &str, action_name: &str, input_index: usize) -> Result<(), String> {
    if url.starts_with("http://") || url.starts_with("https://") {
        Ok(())
    } else {
        Err(format!(
            "Action '{}' child-agent url input {} must resolve to an http(s) URL.",
            action_name, input_index
        ))
    }
}

fn validate_child_input_path(
    path: &str,
    action_name: &str,
    input_index: usize,
    input_kind: &str,
) -> Result<(), String> {
    if path.trim().is_empty() {
        return Err(format!(
            "Action '{}' child-agent {} input {} must resolve to a non-empty relative path.",
            action_name, input_kind, input_index
        ));
    }

    let candidate = Path::new(path);
    if candidate.is_absolute() {
        return Err(format!(
            "Action '{}' child-agent {} input {} must stay at the current level or below; absolute paths are not allowed.",
            action_name, input_kind, input_index
        ));
    }

    if candidate
        .components()
        .any(|component| matches!(component, Component::ParentDir))
    {
        return Err(format!(
            "Action '{}' child-agent {} input {} must stay at the current level or below; parent traversal (`..`) is not allowed.",
            action_name, input_kind, input_index
        ));
    }

    Ok(())
}

fn validate_child_file_extension(
    path: &str,
    action_name: &str,
    input_index: usize,
) -> Result<(), String> {
    let extension = Path::new(path)
        .extension()
        .and_then(|value| value.to_str())
        .map(|value| value.to_ascii_lowercase());

    match extension.as_deref() {
        Some(
            "pdf" | "docx" | "csv" | "xla" | "xlb" | "xlc" | "xlm" | "xls" | "xlsx" | "xlt"
            | "xlw" | "tsv" | "iif" | "doc" | "dot" | "odt" | "rtf" | "pot" | "ppa" | "pps"
            | "ppt" | "pptx" | "pwz" | "wiz",
        ) => Ok(()),
        _ => Err(format!(
            "Action '{}' child-agent file input {} must use a supported extension: {}.",
            action_name, input_index, SUPPORTED_FILE_EXTENSIONS_MESSAGE
        )),
    }
}

fn current_agent_action_depth() -> u32 {
    std::env::var(AGENT_ACTION_DEPTH_ENV)
        .ok()
        .and_then(|value| value.parse::<u32>().ok())
        .unwrap_or(0)
}

fn current_time_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

fn new_runtime_budget(max_runtime_secs: u64) -> InvocationRuntimeBudget {
    let started_at_ms = current_time_millis();
    InvocationRuntimeBudget {
        max_runtime_secs,
        started_at_ms,
        deadline_ms: started_at_ms.saturating_add(max_runtime_secs.saturating_mul(1000)),
    }
}

fn inherited_agent_action_runtime_budget() -> Option<InvocationRuntimeBudget> {
    let max_runtime_secs = std::env::var(AGENT_ACTION_MAX_RUNTIME_SECS_ENV)
        .ok()
        .and_then(|value| value.parse::<u64>().ok())?;
    let started_at_ms = std::env::var(AGENT_ACTION_RUNTIME_STARTED_AT_MS_ENV)
        .ok()
        .and_then(|value| value.parse::<u64>().ok())?;
    let deadline_ms = std::env::var(AGENT_ACTION_RUNTIME_DEADLINE_MS_ENV)
        .ok()
        .and_then(|value| value.parse::<u64>().ok())?;

    Some(InvocationRuntimeBudget {
        max_runtime_secs,
        started_at_ms,
        deadline_ms,
    })
}

fn configured_agent_action_runtime_budget(cli_override: Option<u64>) -> InvocationRuntimeBudget {
    cli_override
        .map(new_runtime_budget)
        .or_else(inherited_agent_action_runtime_budget)
        .unwrap_or_else(|| new_runtime_budget(DEFAULT_AGENT_ACTION_MAX_RUNTIME_SECS))
}

fn remaining_runtime_duration(
    runtime_budget: InvocationRuntimeBudget,
    exhausted_context: &str,
) -> Result<Duration, String> {
    let now = current_time_millis();
    if now >= runtime_budget.deadline_ms {
        return Err(exhausted_context.to_string());
    }

    Ok(Duration::from_millis(
        runtime_budget.deadline_ms.saturating_sub(now),
    ))
}

fn elapsed_runtime_secs(runtime_budget: InvocationRuntimeBudget) -> u64 {
    current_time_millis()
        .saturating_sub(runtime_budget.started_at_ms)
        .div_ceil(1000)
}

fn current_agent_runtime_timeout_message(
    runtime_budget: InvocationRuntimeBudget,
    context: &str,
) -> String {
    format!(
        "Current agent exceeded max-runtime-in-sec {} after {} seconds {}.",
        runtime_budget.max_runtime_secs,
        elapsed_runtime_secs(runtime_budget),
        context
    )
}

fn action_runtime_timeout_message(
    action_name: &str,
    runtime_budget: InvocationRuntimeBudget,
    context: &str,
) -> String {
    format!(
        "Action '{}' exceeded max-runtime-in-sec {} after {} seconds {}.",
        action_name,
        runtime_budget.max_runtime_secs,
        elapsed_runtime_secs(runtime_budget),
        context
    )
}

fn inherited_agent_action_max_depth() -> Option<u32> {
    std::env::var(AGENT_ACTION_MAX_DEPTH_ENV)
        .ok()
        .and_then(|value| value.parse::<u32>().ok())
}

fn configured_agent_action_max_depth(cli_override: Option<u32>) -> u32 {
    cli_override
        .or_else(inherited_agent_action_max_depth)
        .unwrap_or(DEFAULT_AGENT_ACTION_MAX_DEPTH)
}

fn validate_agent_action_depth(
    current_depth: u32,
    max_agent_depth: u32,
    action_name: &str,
) -> Result<(), String> {
    if current_depth >= max_agent_depth {
        return Err(format!(
            "Action '{}' cannot invoke another agent because current depth {} has reached max-agent-depth {}.",
            action_name, current_depth, max_agent_depth
        ));
    }

    Ok(())
}

fn validate_agent_step_target(agent: &str, action_name: &str) -> Result<(), String> {
    let agent_path = Path::new(agent);
    if agent_path.is_absolute() {
        return Err(format!(
            "Action '{}' agent step target '{}' must be an explicit relative path such as './child_agent'.",
            action_name, agent
        ));
    }

    if agent_path
        .components()
        .any(|component| matches!(component, Component::ParentDir))
    {
        return Err(format!(
            "Action '{}' agent step target '{}' must stay at the current level or below; parent traversal (`..`) is not allowed.",
            action_name, agent
        ));
    }

    if !contains_explicit_path_separator(agent) {
        return Err(format!(
            "Action '{}' agent step target '{}' must be an explicit relative path such as './child_agent'; bare executable names are not allowed because they may resolve through PATH.",
            action_name, agent
        ));
    }

    Ok(())
}

fn contains_explicit_path_separator(path: &str) -> bool {
    path.contains('/') || path.contains('\\')
}

fn step_matches_platform(platforms: Option<&[String]>, current_platform: Option<&str>) -> bool {
    match platforms {
        None => true,
        Some(platforms) => current_platform.is_some_and(|platform| {
            platforms.iter().any(|candidate| candidate == platform)
        }),
    }
}

fn resolve_run_args(
    args: &[RunArg],
    data: &serde_json::Value,
    action_name: &str,
) -> Result<Vec<String>, String> {
    args.iter()
        .enumerate()
        .map(|(index, arg)| resolve_run_arg(arg, data, action_name, index))
        .collect()
}

fn resolve_string_parts(
    parts: &[RunArg],
    data: &serde_json::Value,
    action_name: &str,
    field_name: &str,
) -> Result<String, String> {
    let mut resolved = String::new();

    for (index, part) in parts.iter().enumerate() {
        let value = resolve_run_arg(part, data, action_name, index)?;
        resolved.push_str(&value);
    }

    if resolved.trim().is_empty() {
        return Err(format!(
            "Action '{}' {} resolved to an empty string.",
            action_name, field_name
        ));
    }

    Ok(resolved)
}

fn resolve_run_arg(
    arg: &RunArg,
    data: &serde_json::Value,
    action_name: &str,
    index: usize,
) -> Result<String, String> {
    match arg {
        RunArg::Literal(literal) => Ok(literal.clone()),
        RunArg::Variable(variable) => {
            let Some(value) = data.get(variable) else {
                return Err(format!(
                    "Action '{}' arg {} references missing output field '{}'.",
                    action_name, index, variable
                ));
            };

            match value {
                serde_json::Value::String(text) => Ok(text.clone()),
                serde_json::Value::Bool(boolean) => Ok(boolean.to_string()),
                serde_json::Value::Number(number) => Ok(number.to_string()),
                serde_json::Value::Array(_) => Err(format!(
                    "Action '{}' arg {} references array-valued field '{}', which is unsupported for arg substitution.",
                    action_name, index, variable
                )),
                serde_json::Value::Object(_) => Err(format!(
                    "Action '{}' arg {} references object-valued field '{}', which is unsupported for arg substitution.",
                    action_name, index, variable
                )),
                serde_json::Value::Null => Err(format!(
                    "Action '{}' arg {} references null field '{}', which is unsupported for arg substitution.",
                    action_name, index, variable
                )),
            }
        }
    }
}
