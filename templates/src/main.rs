mod args;
mod web_resources;
mod config;
mod credentials;
mod providers;

use jsonlogic::apply;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

use config::loader::{find_profile, load_config};
use config::schema::{Profile, ProfileAuthMode};
use providers::{
    provider_error_messages, validate_provider_content_parts, validate_provider_request,
    ProviderKind,
};

include!(concat!(env!("OUT_DIR"), "/agent_model.rs"));

const INFRA_BASE_URL: &str = "https://api.cargo-ai.org";
const OPENAI_ACCOUNT_RESPONSES_URL: &str = "https://chatgpt.com/backend-api/codex/responses";
const OPENAI_REFRESH_BUFFER_SEC: i64 = 30;
const KEYCHAIN_SERVICE: &str = "cargo-ai";
const ACCOUNT_ACCESS_KEY: &str = "account/access_token";
const ACCOUNT_REFRESH_KEY: &str = "account/refresh_token";

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

#[derive(Debug, Clone)]
struct AccountAuth {
    access_token: String,
    refresh_token: Option<String>,
}

#[derive(Debug, Deserialize, Default)]
struct CredentialsFile {
    #[serde(default)]
    account: Option<CredentialsAccount>,
}

#[derive(Debug, Deserialize, Default)]
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

    if let Some(timeout) = matches.get_one::<String>("timeout_in_sec") {
        overrides.push(format!("timeout_in_sec={timeout}"));
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

    let access_entry = keyring::Entry::new(KEYCHAIN_SERVICE, ACCOUNT_ACCESS_KEY)
        .map_err(|error| format!("failed to initialize account access-token keyring entry: {error}"))?;
    let refresh_entry = keyring::Entry::new(KEYCHAIN_SERVICE, ACCOUNT_REFRESH_KEY)
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

fn render_account_response(response: &serde_json::Value) {
    match serde_json::to_string_pretty(response) {
        Ok(pretty) => println!("{pretty}"),
        Err(_) => println!("{response:?}"),
    }
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
                render_account_response(&refresh_response);
                "Session refresh did not return a new access token. Cannot retry email_me action."
                    .to_string()
            })?;

        response =
            send_account_mail_request(refreshed_access_token.as_str(), subject, text).await?;
    }

    render_account_response(&response);

    let succeeded = response
        .get("status")
        .and_then(|value| value.as_str())
        .map(|status| status.eq_ignore_ascii_case("success"))
        .unwrap_or(false);

    if succeeded {
        Ok(())
    } else {
        Err("email_me request failed.".to_string())
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

    ordered.sort_by_key(|(index, _)| *index);
    ordered.into_iter().map(|(_, input)| input).collect()
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

fn resolved_inputs_for_run(cmd_args: &clap::ArgMatches) -> Vec<Input> {
    let runtime_inputs = runtime_input_overrides(cmd_args);
    if runtime_inputs.is_empty() {
        inputs()
    } else {
        runtime_inputs
    }
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
    let mut timeout_in_sec: u64 = 60;
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
                &mut timeout_in_sec,
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
                &mut timeout_in_sec,
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

    if let Some(timeout_arg) = cmd_args.get_one::<String>("timeout_in_sec") {
        timeout_in_sec = timeout_arg.parse::<u64>().unwrap_or(60);
    }

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

    let selected_inputs = resolved_inputs_for_run(&cmd_args);
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
        match crate::providers::send_ollama_request(
            &url,
            &model,
            &content_parts,
            timeout_in_sec,
            json_schema_value(),
        )
        .await
        {
            Ok(r) => {
                response.push_str(&r);
            }
            Err(error) => {
                for line in provider_error_messages(&error) {
                    eprintln!("{line}");
                }
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

        match crate::providers::send_openai_request(
            &url,
            &model,
            &content_parts,
            timeout_in_sec,
            &token,
            fmt,
        )
        .await
        {
            Ok(r) => response.push_str(&r),
            Err(error) => {
                for line in provider_error_messages(&error) {
                    eprintln!("{line}");
                }
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
    if let Err(error) = apply_actions(&output, &actions, config.as_ref()).await {
        eprintln!("❌ {error}");
        std::process::exit(1);
    }
}

pub async fn apply_actions(
    output: &Output,
    actions: &[Action],
    config: Option<&config::schema::Config>,
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

                for step in matching_steps {
                    if step.kind.eq_ignore_ascii_case("exec") {
                        run_exec_step(step, &data, &action.name)?;
                    } else if step.kind.eq_ignore_ascii_case("email_me") {
                        run_email_me_step(step, &data, &action.name, config).await?;
                    } else {
                        println!(
                            "⚠️ Skipping action '{}' with unsupported step kind '{}'.",
                            action.name, step.kind
                        );
                    }
                }
            }
        } else {
            println!("Failed to evaluate logic for action: {}", action.name);
        }
    }

    Ok(())
}

fn run_exec_step(
    step: &RunStep,
    data: &serde_json::Value,
    action_name: &str,
) -> Result<(), String> {
    let program = step.program.as_deref().ok_or_else(|| {
        format!(
            "Action '{}' exec step is missing required `program`.",
            action_name
        )
    })?;

    let resolved_args = resolve_run_args(&step.args, data, action_name)?;
    println!("Running '{}': {} {:?}", action_name, program, resolved_args);

    let status = std::process::Command::new(program)
        .args(&resolved_args)
        .status();

    match status {
        Ok(status) if status.success() => {
            println!("Command completed successfully.");
        }
        Ok(status) => {
            println!("Command exited with status: {}", status);
        }
        Err(err) => {
            println!("Failed to execute command: {}", err);
        }
    }

    Ok(())
}

async fn run_email_me_step(
    step: &RunStep,
    data: &serde_json::Value,
    action_name: &str,
    config: Option<&config::schema::Config>,
) -> Result<(), String> {
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
    println!("Running '{}': email_me {:?}", action_name, subject);

    run_email_me_action(subject.as_str(), text.as_str(), config)
        .await
        .map_err(|error| format!("Action '{}': {error}", action_name))
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
