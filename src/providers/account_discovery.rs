//! Fresh picker-visible catalogs from the selected file-backed Codex session.
use super::discovery::{Connection, DiscoveryError, MAX_PAGE_BYTES, TOTAL_SECONDS};
use crate::config::loader::{load_config_from_path, ConfigLoad};
use crate::credentials::openai_oauth::{self, CodexSession};
use base64::Engine;
use reqwest::{Client, Request, Url};
use serde_json::{json, Map, Value};
use std::{
    collections::BTreeSet,
    future::Future,
    path::{Path, PathBuf},
    time::Duration,
};

// Wire contract inspected at openai/codex rust-v0.159.2 (ff6aec96948b).
// Maintain with the provider compatibility policy; do not infer it from PATH.
const CATALOG_COMPATIBILITY_VERSION: &str = super::thinking_metadata::ACCOUNT_CATALOG_VERSION;
const CATALOG_URL: &str = "https://chatgpt.com/backend-api/codex/models";
const AUTH_NAMESPACE: &str = "https://api.openai.com/auth";

pub(crate) struct Snapshot {
    auth_path: PathBuf,
    auth_raw: String,
    config_path: PathBuf,
    config_raw: Option<String>,
    session: CodexSession,
    secrets: Vec<String>,
}
impl std::fmt::Debug for Snapshot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Snapshot")
            .field("session", &self.session)
            .finish_non_exhaustive()
    }
}
fn invalid() -> DiscoveryError {
    DiscoveryError::new(
        "invalid_credentials",
        "The selected Codex session is invalid; run `codex login`.",
    )
}
fn unsupported() -> DiscoveryError {
    DiscoveryError::new("unsupported_connection", "Account discovery requires a supported file-backed commercial Codex session and known selected account context.")
}
fn changed() -> DiscoveryError {
    DiscoveryError::new(
        "changed_connection",
        "The selected connection changed during model discovery; run a new listing.",
    )
}

pub(crate) fn endpoint(raw: Option<&str>) -> Result<Url, DiscoveryError> {
    let native = Url::parse(openai_oauth::OPENAI_ACCOUNT_RESPONSES_URL).unwrap();
    let supplied = Url::parse(
        raw.filter(|value| !value.is_empty())
            .unwrap_or(openai_oauth::OPENAI_ACCOUNT_RESPONSES_URL),
    )
    .map_err(|_| unsupported())?;
    if supplied != native {
        return Err(unsupported());
    }
    Ok(Url::parse(CATALOG_URL).unwrap())
}

fn claims(token: &str) -> Result<Value, DiscoveryError> {
    // This reads routing/expiry metadata; decoding does not verify a JWT signature.
    if !openai_oauth::bounded_header(token, openai_oauth::MAX_TOKEN_BYTES) {
        return Err(invalid());
    }
    let parts: Vec<_> = token.split('.').collect();
    if parts.len() != 3 || parts.iter().any(|part| part.is_empty()) {
        return Err(invalid());
    }
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(parts[1])
        .map_err(|_| invalid())?;
    let value: Value = serde_json::from_slice(&bytes).map_err(|_| invalid())?;
    if !value.is_object() {
        return Err(invalid());
    }
    Ok(value)
}
fn validate_claims(value: &Value, selected: &str) -> Result<(), DiscoveryError> {
    if let Some(auth) = value.get(AUTH_NAMESPACE).filter(|value| !value.is_null()) {
        let auth = auth.as_object().ok_or_else(invalid)?;
        if let Some(account) = auth.get("chatgpt_account_id") {
            if !account.is_null() && account.as_str() != Some(selected) {
                return Err(invalid());
            }
        }
        match auth.get("chatgpt_account_is_fedramp") {
            None | Some(Value::Bool(false)) => {}
            Some(Value::Bool(true)) => return Err(unsupported()),
            _ => return Err(invalid()),
        }
    }
    Ok(())
}
fn validate_session(raw: &str, now: i64) -> Result<CodexSession, DiscoveryError> {
    let value: Value = serde_json::from_str(raw).map_err(|_| invalid())?;
    if !value.is_object() {
        return Err(invalid());
    }
    match value.get("auth_mode") {
        None | Some(Value::String(_))
            if value.get("auth_mode").is_none() || value["auth_mode"] == "chatgpt" => {}
        _ => return Err(unsupported()),
    }
    if value
        .get("OPENAI_API_KEY")
        .is_some_and(|key| !key.is_null())
    {
        return Err(unsupported());
    }
    let session = openai_oauth::parse_codex_auth_payload(raw).map_err(|_| invalid())?;
    if session
        .refresh_token
        .as_ref()
        .is_some_and(|token| !openai_oauth::bounded_header(token, openai_oauth::MAX_TOKEN_BYTES))
    {
        return Err(invalid());
    }
    let selected = session.account_id.as_deref().ok_or_else(unsupported)?;
    let access = claims(&session.access_token)?;
    let exp = access
        .get("exp")
        .and_then(Value::as_i64)
        .filter(|exp| *exp > 0)
        .ok_or_else(invalid)?;
    if openai_oauth::access_token_expired_or_near(Some(exp), now)
        || openai_oauth::access_token_expired_or_near(session.access_token_expires_at_unix, now)
    {
        return Err(DiscoveryError::new(
            "expired_credentials",
            "The selected Codex session is expired or near expiry; run `codex login`.",
        ));
    }
    let id = value
        .get("tokens")
        .and_then(|tokens| tokens.get("id_token"))
        .and_then(Value::as_str)
        .ok_or_else(unsupported)?;
    let identity = claims(id)?;
    validate_claims(&access, selected)?;
    validate_claims(&identity, selected)?;
    Ok(session)
}

pub(crate) fn snapshot(config_path: &Path) -> Result<Snapshot, DiscoveryError> {
    let config_raw = config_snapshot(config_path)?;
    let auth_path = openai_oauth::codex_auth_path().map_err(|_| unsupported())?;
    snapshot_at(config_path, config_raw, auth_path)
}
pub(crate) fn snapshot_saved(
    config_path: &Path,
    expected: &str,
) -> Result<Snapshot, DiscoveryError> {
    let snapshot = snapshot(config_path)?;
    if snapshot.config_raw.as_deref() != Some(expected) {
        return Err(changed());
    }
    Ok(snapshot)
}
fn snapshot_at(
    config_path: &Path,
    config_raw: Option<String>,
    auth_path: PathBuf,
) -> Result<Snapshot, DiscoveryError> {
    let auth_raw = openai_oauth::read_auth_snapshot(&auth_path)
        .map_err(|_| invalid())?
        .ok_or_else(|| {
            DiscoveryError::new(
                "missing_credentials",
                "No file-backed Codex session is available; run `codex login`.",
            )
        })?;
    let session = validate_session(&auth_raw, openai_oauth::now_unix_seconds())?;
    let value: Value = serde_json::from_str(&auth_raw).map_err(|_| invalid())?;
    let id_token = value["tokens"]["id_token"].as_str().ok_or_else(invalid)?;
    let identity = claims(id_token)?;
    let mut secrets = vec![session.access_token.clone(), id_token.to_owned()];
    secrets.extend(session.account_id.iter().cloned());
    secrets.extend(session.refresh_token.iter().cloned());
    for email in [
        identity.get("email"),
        identity
            .get("https://api.openai.com/profile")
            .and_then(|profile| profile.get("email")),
    ]
    .into_iter()
    .flatten()
    {
        if let Some(email) = email.as_str().filter(|email| !email.is_empty()) {
            secrets.push(email.to_owned());
        }
    }
    Ok(Snapshot {
        auth_path,
        auth_raw,
        config_path: config_path.to_owned(),
        config_raw,
        session,
        secrets,
    })
}
fn config_snapshot(path: &Path) -> Result<Option<String>, DiscoveryError> {
    match load_config_from_path(path).map_err(|_| {
        DiscoveryError::new(
            "invalid_configuration",
            "The selected home configuration is unreadable or invalid.",
        )
    })? {
        ConfigLoad::Missing => Ok(None),
        ConfigLoad::Loaded(loaded) => {
            if loaded
                .config()
                .openai_auth
                .as_ref()
                .and_then(|auth| auth.locally_disabled)
                .unwrap_or(false)
            {
                return Err(DiscoveryError::new("locally_disabled", "OpenAI account auth is logged out for this Cargo AI home; run `cargo ai auth login openai`."));
            }
            Ok(Some(loaded.original_contents().to_owned()))
        }
    }
}
impl Snapshot {
    fn revalidate(&self) -> Result<(), DiscoveryError> {
        if openai_oauth::codex_auth_path().map_err(|_| changed())? != self.auth_path
            || config_snapshot(&self.config_path).map_err(|_| changed())? != self.config_raw
            || openai_oauth::read_auth_snapshot(&self.auth_path)
                .map_err(|_| changed())?
                .as_deref()
                != Some(self.auth_raw.as_str())
        {
            return Err(changed());
        }
        validate_session(&self.auth_raw, openai_oauth::now_unix_seconds())?;
        Ok(())
    }
}
fn text(value: &Value) -> Result<&str, DiscoveryError> {
    let text = value.as_str().ok_or_else(DiscoveryError::malformed)?;
    if text.is_empty() || text.len() > 4096 || text.chars().any(char::is_control) {
        return Err(DiscoveryError::malformed());
    }
    Ok(text)
}
fn parse(body: &[u8], snapshot: &Snapshot) -> Result<Vec<Value>, DiscoveryError> {
    parse_selected(body, snapshot, None)
}
fn parse_selected(
    body: &[u8],
    snapshot: &Snapshot,
    selected: Option<&str>,
) -> Result<Vec<Value>, DiscoveryError> {
    if body.len() > MAX_PAGE_BYTES {
        return Err(DiscoveryError::new(
            "response_too_large",
            "The model catalog exceeds the page byte limit.",
        ));
    }
    let value: Value = serde_json::from_slice(body).map_err(|_| DiscoveryError::malformed())?;
    let records = value
        .get("models")
        .and_then(Value::as_array)
        .ok_or_else(DiscoveryError::malformed)?;
    let mut models = Vec::new();
    let mut seen = BTreeSet::new();
    for record in records {
        let slug = text(record.get("slug").ok_or_else(DiscoveryError::malformed)?)?;
        let visibility = record
            .get("visibility")
            .and_then(Value::as_str)
            .ok_or_else(DiscoveryError::malformed)?;
        if !matches!(visibility, "list" | "hide" | "none") {
            return Err(DiscoveryError::malformed());
        }
        let name = match record.get("display_name") {
            None | Some(Value::Null) => None,
            Some(value) => Some(text(value)?),
        };
        let mut metadata = Map::new();
        if let Some(levels) = record.get("supported_reasoning_levels") {
            let levels = levels
                .as_array()
                .filter(|values| values.len() <= 64)
                .ok_or_else(DiscoveryError::malformed)?;
            let mut retained = Vec::new();
            for level in levels {
                let effort = text(level.get("effort").ok_or_else(DiscoveryError::malformed)?)?;
                let mut entry = json!({"effort": effort});
                if let Some(description) = level.get("description") {
                    entry["description"] = Value::String(text(description)?.to_owned());
                }
                retained.push(entry);
            }
            metadata.insert("supported_reasoning_levels".into(), Value::Array(retained));
        }
        if let Some(default) = record
            .get("default_reasoning_level")
            .filter(|value| !value.is_null())
        {
            metadata.insert(
                "default_reasoning_level".into(),
                Value::String(text(default)?.to_owned()),
            );
        }
        if let Some(modalities) = record.get("input_modalities") {
            let values = modalities
                .as_array()
                .filter(|values| values.len() <= 16)
                .ok_or_else(DiscoveryError::malformed)?;
            let mut retained = Vec::new();
            for value in values {
                let value = text(value)?;
                if !matches!(value, "text" | "image" | "audio") {
                    return Err(DiscoveryError::malformed());
                }
                retained.push(Value::String(value.to_owned()));
            }
            metadata.insert("input_modalities".into(), Value::Array(retained));
        }
        let model = json!({"id":slug,"name":name,"metadata":metadata,"metadata_source":"provider","invocation_access":"unverified"});
        for secret in &snapshot.secrets {
            if !secret.is_empty() && contains_string(&model, secret) {
                return Err(DiscoveryError::malformed());
            }
        }
        // Validate every record before applying visibility or duplicate selection.
        if selected.is_some_and(|selected| selected == slug) && seen.contains(slug) {
            return Err(DiscoveryError::malformed());
        }
        if (selected.is_some_and(|selected| selected == slug)
            || (selected.is_none() && visibility == "list"))
            && seen.insert(slug.to_owned())
        {
            models.push(model);
        }
    }
    Ok(models)
}
fn contains_string(value: &Value, secret: &str) -> bool {
    match value {
        Value::String(text) => text.contains(secret),
        Value::Array(values) => values.iter().any(|value| contains_string(value, secret)),
        Value::Object(values) => values.values().any(|value| contains_string(value, secret)),
        _ => false,
    }
}
fn request(
    client: &Client,
    connection: &Connection,
    snapshot: &Snapshot,
) -> Result<Request, DiscoveryError> {
    if connection.endpoint != Url::parse(CATALOG_URL).unwrap() {
        return Err(unsupported());
    }
    client
        .get(connection.endpoint.clone())
        .query(&[("client_version", CATALOG_COMPATIBILITY_VERSION)])
        .bearer_auth(&snapshot.session.access_token)
        .header(
            "ChatGPT-Account-ID",
            snapshot
                .session
                .account_id
                .as_deref()
                .ok_or_else(unsupported)?,
        )
        .header("originator", "cargo-ai")
        .header(
            "User-Agent",
            concat!("cargo-ai/", env!("CARGO_PKG_VERSION")),
        )
        .build()
        .map_err(|_| invalid())
}
async fn fetch(client: Client, request: Request) -> Result<Vec<u8>, DiscoveryError> {
    let mut response = client.execute(request).await.map_err(|error| {
        if error.is_timeout() {
            DiscoveryError::new("timeout", "Model discovery timed out.")
        } else {
            DiscoveryError::new("connectivity", "Could not reach the model catalog.")
        }
    })?;
    match response.status().as_u16() {
        200..=299 => {}
        401 => {
            return Err(DiscoveryError::new(
                "unauthorized",
                "The provider rejected the catalog credential.",
            ))
        }
        403 => {
            return Err(DiscoveryError::new(
                "forbidden",
                "The provider denied catalog access.",
            ))
        }
        429 => {
            return Err(DiscoveryError::new(
                "rate_limited",
                "The provider rate limited catalog access.",
            ))
        }
        300..=399 => {
            return Err(DiscoveryError::new(
                "redirect_refused",
                "Model catalog redirects are not followed.",
            ))
        }
        _ => {
            return Err(DiscoveryError::new(
                "provider_error",
                "The provider could not return a model catalog.",
            ))
        }
    }
    if response
        .content_length()
        .is_some_and(|length| length > MAX_PAGE_BYTES as u64)
    {
        return Err(DiscoveryError::new(
            "response_too_large",
            "The model catalog exceeds the page byte limit.",
        ));
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|error| {
        if error.is_timeout() {
            DiscoveryError::new("timeout", "Model discovery timed out.")
        } else {
            DiscoveryError::malformed()
        }
    })? {
        if chunk.len() > MAX_PAGE_BYTES.saturating_sub(bytes.len()) {
            return Err(DiscoveryError::new(
                "response_too_large",
                "The model catalog exceeds the page byte limit.",
            ));
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}
pub(crate) async fn list(connection: Connection) -> Result<Value, DiscoveryError> {
    let client = Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .retry(reqwest::retry::never())
        .timeout(Duration::from_secs(TOTAL_SECONDS))
        .build()
        .map_err(|_| {
            DiscoveryError::new("connectivity", "Could not initialize catalog transport.")
        })?;
    list_with_fetch(connection, &client, |request| {
        fetch(client.clone(), request)
    })
    .await
}

pub(crate) async fn thinking(connection: Connection, model: &str) -> Result<Value, DiscoveryError> {
    let client = Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .retry(reqwest::retry::never())
        .timeout(Duration::from_secs(TOTAL_SECONDS))
        .build()
        .map_err(|_| {
            DiscoveryError::new("connectivity", "Could not initialize catalog transport.")
        })?;
    thinking_with_fetch(connection, model, &client, |request| {
        fetch(client.clone(), request)
    })
    .await
}
async fn thinking_with_fetch<F, Fut>(
    connection: Connection,
    model: &str,
    client: &Client,
    fetch: F,
) -> Result<Value, DiscoveryError>
where
    F: FnOnce(Request) -> Fut,
    Fut: Future<Output = Result<Vec<u8>, DiscoveryError>>,
{
    let snapshot = connection.account.as_ref().ok_or_else(unsupported)?;
    if model.is_empty()
        || model.len() > 4096
        || model.chars().any(char::is_control)
        || snapshot.secrets.iter().any(|secret| {
            !secret.is_empty()
                && (model.contains(secret)
                    || connection
                        .profile
                        .as_ref()
                        .is_some_and(|profile| profile.contains(secret)))
        })
    {
        return Err(DiscoveryError::malformed());
    }
    snapshot.revalidate()?;
    let operation = async {
        let body = fetch(request(client, &connection, snapshot)?).await?;
        let support = match parse_selected(&body, snapshot, Some(model)) {
            Ok(records) if records.len() == 1 => {
                super::thinking_metadata::account_record_support(&records[0]["metadata"])
            }
            Ok(_) => super::thinking_metadata::unknown(
                "the selected model is absent from current account metadata",
            ),
            Err(_) => super::thinking_metadata::unknown(
                "malformed or mismatched account thinking metadata",
            ),
        };
        snapshot.revalidate()?;
        Ok(super::discovery::thinking_value(
            &connection,
            model,
            support,
        ))
    };
    tokio::select! {
        result = tokio::time::timeout(Duration::from_secs(TOTAL_SECONDS),operation) => result.unwrap_or_else(|_| Err(DiscoveryError::new("timeout","Thinking discovery timed out."))),
        result = tokio::signal::ctrl_c() => { let _ = result; Err(DiscoveryError::new("cancelled","Thinking discovery was cancelled.")) },
    }
}
async fn list_with_fetch<F, Fut>(
    connection: Connection,
    client: &Client,
    fetch: F,
) -> Result<Value, DiscoveryError>
where
    F: FnOnce(Request) -> Fut,
    Fut: Future<Output = Result<Vec<u8>, DiscoveryError>>,
{
    list_with_fetch_budget(
        connection,
        client,
        fetch,
        Duration::from_secs(TOTAL_SECONDS),
    )
    .await
}
async fn list_with_fetch_budget<F, Fut>(
    connection: Connection,
    client: &Client,
    fetch: F,
    budget: Duration,
) -> Result<Value, DiscoveryError>
where
    F: FnOnce(Request) -> Fut,
    Fut: Future<Output = Result<Vec<u8>, DiscoveryError>>,
{
    let snapshot = connection.account.as_ref().ok_or_else(unsupported)?;
    if connection.profile.as_ref().is_some_and(|profile| {
        snapshot
            .secrets
            .iter()
            .any(|secret| !secret.is_empty() && profile.contains(secret))
    }) {
        return Err(DiscoveryError::malformed());
    }
    snapshot.revalidate()?;
    let operation = async {
        let body = fetch(request(client, &connection, snapshot)?).await?;
        let models = parse(&body, snapshot)?;
        snapshot.revalidate()?;
        Ok(
            json!({"schema_version":1,"provider":"openai","auth":"openai_account",
            "connection":{"endpoint":"https://chatgpt.com","profile":connection.profile},
            "fetched_at":time::OffsetDateTime::now_utc().format(&time::format_description::well_known::Rfc3339).unwrap_or_default(),
            "source":"live","cache":null,"complete":true,"continuation":null,"pages_fetched":1,
            "compatibility":{"kind":"codex_backend","client_version":CATALOG_COMPATIBILITY_VERSION},
            "selection":"picker_visible","models":models,"invocation_access":"unverified"}),
        )
    };
    tokio::select! {
        result = tokio::time::timeout(budget, operation) => result.unwrap_or_else(|_| Err(DiscoveryError::new("timeout", "Model discovery timed out."))),
        result = tokio::signal::ctrl_c() => { let _ = result; Err(DiscoveryError::new("cancelled", "Model discovery was cancelled.")) },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::schema::ProfileAuthMode;
    use crate::providers::ProviderKind;
    #[tokio::test]
    async fn thinking_inspects_selected_hidden_record_without_widening_picker() {
        let fixture = Fixture::new("synthetic-thinking-context");
        let body = json!({"models":[
            {"slug":"visible","visibility":"list"},
            {"slug":"hidden","visibility":"hide","supported_reasoning_levels":[{"effort":"deliberate","description":"A provider-specific mode"},{"effort":"quick"}],"default_reasoning_level":"quick"}
        ]}).to_string().into_bytes();
        let connection = fixture.connection();
        assert_eq!(
            parse(&body, connection.account.as_ref().unwrap())
                .unwrap()
                .len(),
            1
        );
        let result =
            thinking_with_fetch(connection, "hidden", &Client::new(), |request| async move {
                assert_eq!(request.url().query(), Some("client_version=0.159.2"));
                Ok(body)
            })
            .await
            .unwrap();
        assert_eq!(result["thinking"]["status"], "configurable");
        assert_eq!(result["thinking"]["choices"][0]["value"], "deliberate");
        assert_eq!(result["thinking"]["default"], "quick");
        assert!(!result.to_string().contains("synthetic-thinking-context"));
    }
    #[tokio::test]
    async fn thinking_malformed_and_absent_metadata_remain_unknown() {
        let fixture = Fixture::new("synthetic-thinking-unknown");
        for body in [
            json!({"models":[{"slug":"selected","visibility":"hide"}]}),
            json!({"models":[{"slug":"selected","visibility":"hide","supported_reasoning_levels":"bad"}]}),
            json!({"models":[]}),
        ] {
            let result = thinking_with_fetch(
                fixture.connection(),
                "selected",
                &Client::new(),
                |_| async { Ok(body.to_string().into_bytes()) },
            )
            .await
            .unwrap();
            assert_eq!(result["thinking"]["status"], "unknown");
        }
    }
    #[tokio::test]
    async fn thinking_revalidates_account_before_returning_metadata() {
        let fixture = Fixture::new("synthetic-thinking-first");
        let error = thinking_with_fetch(fixture.connection(),"selected",&Client::new(),|_| async {
            std::fs::write(fixture.root.join("auth.json"),auth("synthetic-thinking-second")).unwrap();
            Ok(br#"{"models":[{"slug":"selected","visibility":"hide","supported_reasoning_levels":[]}]}"#.to_vec())
        }).await.unwrap_err();
        assert_eq!(error.code, "changed_connection");
    }
    fn jwt(value: Value) -> String {
        format!(
            "e30.{}.synthetic-signature",
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(value.to_string())
        )
    }
    fn auth(account: &str) -> String {
        json!({"auth_mode":"chatgpt","OPENAI_API_KEY":null,"tokens":{
            "access_token":jwt(json!({"exp":openai_oauth::now_unix_seconds()+3600,AUTH_NAMESPACE:{"chatgpt_account_id":account}})),
            "id_token":jwt(json!({"email":"synthetic-person@example.invalid",AUTH_NAMESPACE:{"chatgpt_account_id":account}})),
            "refresh_token":"synthetic-refresh-secret","account_id":account
        }}).to_string()
    }
    struct Fixture {
        root: PathBuf,
        previous: Option<std::ffi::OsString>,
    }
    impl Fixture {
        fn new(account: &str) -> Self {
            let root = std::env::temp_dir()
                .join(format!("cargo-ai-account-catalog-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir(&root).unwrap();
            std::fs::write(root.join("auth.json"), auth(account)).unwrap();
            let previous = std::env::var_os("CODEX_HOME");
            std::env::set_var("CODEX_HOME", &root);
            Self { root, previous }
        }
        fn connection(&self) -> Connection {
            Connection {
                request_endpoint: "https://chatgpt.com/backend-api/codex/responses".into(),
                provider: ProviderKind::OpenAi,
                auth: ProfileAuthMode::OpenaiAccount,
                endpoint: endpoint(None).unwrap(),
                token: String::new(),
                profile: Some("fixture".into()),
                account: Some(snapshot(&self.root.join("absent-home/config.toml")).unwrap()),
            }
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            match &self.previous {
                Some(previous) => std::env::set_var("CODEX_HOME", previous),
                None => std::env::remove_var("CODEX_HOME"),
            }
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }
    #[test]
    fn account_catalog_only_accepts_fixed_native_endpoint() {
        assert_eq!(endpoint(None).unwrap().as_str(), CATALOG_URL);
        assert!(endpoint(Some(openai_oauth::OPENAI_ACCOUNT_RESPONSES_URL)).is_ok());
        assert!(endpoint(Some("")).is_ok());
        for raw in [
            "https://chatgpt.com.evil.invalid/backend-api/codex/responses",
            "http://chatgpt.com/backend-api/codex/responses",
            "https://chatgpt.com:444/backend-api/codex/responses",
            "https://chatgpt.com/backend-api/codex/responses?key=secret",
            "https://chatgpt.com/backend-api/codex/responses#secret",
            "https://user@chatgpt.com/backend-api/codex/responses",
            "https://api.openai.com/v1/responses",
            CATALOG_URL,
        ] {
            assert_eq!(
                endpoint(Some(raw)).unwrap_err().code,
                "unsupported_connection"
            );
        }
    }
    #[test]
    fn account_session_validates_expiry_context_layout_and_routing() {
        let now = openai_oauth::now_unix_seconds();
        let raw = auth("synthetic-personal");
        assert!(validate_session(&raw, now).is_ok());
        let base: Value = serde_json::from_str(&raw).unwrap();
        for (field, value, code) in [
            ("auth_mode", json!("apikey"), "unsupported_connection"),
            (
                "OPENAI_API_KEY",
                json!("synthetic-api-key"),
                "unsupported_connection",
            ),
        ] {
            let mut value_base = base.clone();
            value_base[field] = value;
            assert_eq!(
                validate_session(&value_base.to_string(), now)
                    .unwrap_err()
                    .code,
                code
            );
        }
        for (claim, code) in [
            (json!({"exp":now+20}), "expired_credentials"),
            (json!({}), "invalid_credentials"),
            (json!({"exp":"not-an-expiry"}), "invalid_credentials"),
            (
                json!({"exp":now+3600,AUTH_NAMESPACE:{"chatgpt_account_id":"wrong-context"}}),
                "invalid_credentials",
            ),
            (
                json!({"exp":now+3600,AUTH_NAMESPACE:{"chatgpt_account_is_fedramp":true}}),
                "unsupported_connection",
            ),
            (
                json!({"exp":now+3600,AUTH_NAMESPACE:{"chatgpt_account_is_fedramp":"false"}}),
                "invalid_credentials",
            ),
        ] {
            let mut value = base.clone();
            value["tokens"]["access_token"] = json!(jwt(claim));
            assert_eq!(
                validate_session(&value.to_string(), now).unwrap_err().code,
                code
            );
        }
        for id in [
            json!(null),
            json!(""),
            json!("bad\nheader"),
            json!("x".repeat(257)),
        ] {
            let mut value = base.clone();
            value["tokens"]["account_id"] = id;
            assert!(validate_session(&value.to_string(), now).is_err());
        }
        let mut value = base.clone();
        value["tokens"]["id_token"] = json!(jwt(
            json!({AUTH_NAMESPACE:{"chatgpt_account_id":"other-workspace"}})
        ));
        assert_eq!(
            validate_session(&value.to_string(), now).unwrap_err().code,
            "invalid_credentials"
        );
        let mut value = base;
        value["tokens"]["access_token"] = json!("legacy-opaque-token");
        assert_eq!(
            validate_session(&value.to_string(), now).unwrap_err().code,
            "invalid_credentials"
        );
    }
    #[tokio::test]
    async fn account_catalog_request_is_single_live_read_with_selected_context_and_truthful_identity(
    ) {
        for account in ["synthetic-personal", "synthetic-workspace"] {
            let fixture = Fixture::new(account);
            let before = std::fs::read(fixture.root.join("auth.json")).unwrap();
            let connection = fixture.connection();
            let token = connection
                .account
                .as_ref()
                .unwrap()
                .session
                .access_token
                .clone();
            let result = list_with_fetch(connection, &Client::new(), |request| async move {
                assert_eq!(request.method(), reqwest::Method::GET);
                assert_eq!(request.url().as_str(), format!("{CATALOG_URL}?client_version=0.159.2"));
                assert_eq!(request.headers()["authorization"], format!("Bearer {token}"));
                assert_eq!(request.headers()["ChatGPT-Account-ID"], account);
                assert_eq!(request.headers()["originator"], "cargo-ai");
                assert_eq!(request.headers()["User-Agent"], concat!("cargo-ai/", env!("CARGO_PKG_VERSION")));
                Ok(br#"{"models":[{"slug":"opaque/first:model","visibility":"list","display_name":"First","supported_reasoning_levels":[{"effort":"high","description":"More reasoning"}],"input_modalities":["text","image"],"model_messages":{"instructions_template":"private-provider-instructions"}},{"slug":"hidden-manual","visibility":"hide"},{"slug":"opaque/first:model","visibility":"list"},{"slug":"second-exact","visibility":"list"}]}"#.to_vec())
            }).await.unwrap();
            assert_eq!(result["models"][0]["id"], "opaque/first:model");
            assert_eq!(result["models"][1]["id"], "second-exact");
            assert_eq!(result["models"].as_array().unwrap().len(), 2);
            assert_eq!(
                result["models"][0]["metadata"]["input_modalities"],
                json!(["text", "image"])
            );
            assert_eq!(result["models"][1]["metadata"], json!({}));
            assert_eq!(result["source"], "live");
            assert_eq!(result["selection"], "picker_visible");
            assert_eq!(result["cache"], Value::Null);
            assert_eq!(
                result["compatibility"],
                json!({"kind":"codex_backend","client_version":"0.159.2"})
            );
            assert_eq!(result["invocation_access"], "unverified");
            for secret in [
                account,
                "synthetic-person@example.invalid",
                "private-provider-instructions",
            ] {
                assert!(!result.to_string().contains(secret));
            }
            assert_eq!(
                std::fs::read(fixture.root.join("auth.json")).unwrap(),
                before
            );
            assert!(!fixture.root.join("absent-home").exists());
            assert!(!format!("{:?}", fixture.connection().account).contains(account));
        }
    }
    #[tokio::test]
    async fn supported_session_variants_preserve_dynamic_catalogs_and_state() {
        let fixture = Fixture::new("synthetic-layout-context");
        // The inspected older/current layouts share this representation.
        let common: Value = serde_json::from_str(&auth("synthetic-layout-context")).unwrap();
        let mut legacy = common.clone();
        legacy.as_object_mut().unwrap().remove("auth_mode");
        legacy.as_object_mut().unwrap().remove("OPENAI_API_KEY");
        legacy["tokens"]
            .as_object_mut()
            .unwrap()
            .remove("refresh_token");
        let mut additive = common.clone();
        additive["last_refresh"] = json!("2026-09-30T12:00:00Z");
        additive["future_session_metadata"] = json!({"unknown":true});
        additive["tokens"]["future_token_metadata"] = json!("ignored");
        let ids: Vec<_> = (0..12)
            .map(|index| format!("future/model-{index}:exact"))
            .collect();
        let mut records: Vec<_> = ids
            .iter()
            .map(|id| json!({"slug":id,"visibility":"list","future_capability":{"unknown":true}}))
            .collect();
        records.push(records[0].clone());
        records.push(json!({"slug":"manual-only","visibility":"hide"}));
        let body = json!({"models":records,"future_catalog_field":true}).to_string();
        for payload in [common, legacy, additive] {
            let raw = payload.to_string();
            std::fs::write(fixture.root.join("auth.json"), &raw).unwrap();
            let catalog = list_with_fetch(fixture.connection(), &Client::new(), |_| async {
                Ok(body.as_bytes().to_vec())
            })
            .await
            .unwrap();
            let actual: Vec<_> = catalog["models"]
                .as_array()
                .unwrap()
                .iter()
                .map(|model| model["id"].as_str().unwrap())
                .collect();
            assert_eq!(actual, ids);
            assert!(catalog["models"]
                .as_array()
                .unwrap()
                .iter()
                .all(|model| model["metadata"] == json!({})));
            assert_eq!(
                std::fs::read_to_string(fixture.root.join("auth.json")).unwrap(),
                raw
            );
            assert!(!fixture.root.join("absent-home").exists());
        }
    }
    #[tokio::test]
    async fn catalog_compatibility_is_independent_of_codex_installation() {
        struct RestorePath(Option<std::ffi::OsString>);
        impl Drop for RestorePath {
            fn drop(&mut self) {
                match &self.0 {
                    Some(value) => std::env::set_var("PATH", value),
                    None => std::env::remove_var("PATH"),
                }
            }
        }
        let fixture = Fixture::new("synthetic-install-context");
        let _restore = RestorePath(std::env::var_os("PATH"));
        let mut locations = Vec::new();
        for (location, version) in [
            (
                "Old Application.app/Contents/Resources/codex-cli",
                "0.158.0",
            ),
            ("package manager/bin", "0.159.2"),
            ("standalone tools", "0.159.2"),
        ] {
            let directory = fixture.root.join(location);
            std::fs::create_dir_all(&directory).unwrap();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let binary = directory.join("codex");
                std::fs::write(&binary, format!("#!/bin/sh\nprintf invoked > \"$CODEX_HOME/invoked\"\nprintf '{version}\\n'\n")).unwrap();
                std::fs::set_permissions(binary, std::fs::Permissions::from_mode(0o700)).unwrap();
            }
            #[cfg(windows)]
            std::fs::write(
                directory.join("codex.cmd"),
                format!(
                    "@echo off\r\necho invoked>\"%CODEX_HOME%\\invoked\"\r\necho {version}\r\n"
                ),
            )
            .unwrap();
            locations.push(directory);
        }
        let scenarios = [
            vec![],
            vec![locations[0].clone()],
            vec![locations[1].clone()],
            vec![locations[2].clone()],
            locations.clone(),
        ];
        let before = std::fs::read(fixture.root.join("auth.json")).unwrap();
        let mut expected = None;
        for paths in scenarios {
            std::env::set_var("PATH", std::env::join_paths(paths).unwrap());
            let mut catalog =
                list_with_fetch(fixture.connection(), &Client::new(), |request| async move {
                    assert_eq!(request.url().query(), Some("client_version=0.159.2"));
                    Ok(br#"{"models":[{"slug":"future/exact-id","visibility":"list"}]}"#.to_vec())
                })
                .await
                .unwrap();
            catalog.as_object_mut().unwrap().remove("fetched_at");
            if let Some(expected) = &expected {
                assert_eq!(&catalog, expected);
            } else {
                expected = Some(catalog);
            }
            assert!(!fixture.root.join("invoked").exists());
            assert_eq!(
                std::fs::read(fixture.root.join("auth.json")).unwrap(),
                before
            );
        }
    }
    #[tokio::test]
    async fn account_catalog_empty_hidden_malformed_and_private_echo_are_whole_response_outcomes() {
        let fixture = Fixture::new("synthetic-private-context");
        for body in [
            br#"{"models":[]}"#.as_slice(),
            br#"{"models":[{"slug":"manual-only","visibility":"none"}]}"#.as_slice(),
        ] {
            let result = list_with_fetch(fixture.connection(), &Client::new(), |_| async {
                Ok(body.to_vec())
            })
            .await
            .unwrap();
            assert_eq!(result["models"], json!([]));
            assert_eq!(result["complete"], true);
        }
        for body in [
            json!({"models":[{"slug":"valid","visibility":"list"},{"slug":"invalid"}]}),
            json!({"models":[{"slug":"hidden","visibility":"hide","input_modalities":["unsupported"]}]}),
            json!({"models":[{"slug":"invalid","visibility":"future"}]}),
            json!({"models":[{"slug":"invalid","visibility":"list","supported_reasoning_levels":null}]}),
            json!({"models":[{"slug":"synthetic-private-context","visibility":"list"}]}),
            json!({"models":[{"slug":"valid","visibility":"list","display_name":"synthetic-person@example.invalid"}]}),
        ] {
            let error = list_with_fetch(fixture.connection(), &Client::new(), |_| async {
                Ok(body.to_string().into_bytes())
            })
            .await
            .unwrap_err();
            assert_eq!(error.code, "invalid_provider_response");
            assert!(!error.retryable);
        }
        let error = list_with_fetch(fixture.connection(), &Client::new(), |_| async {
            Ok(vec![b'x'; MAX_PAGE_BYTES + 1])
        })
        .await
        .unwrap_err();
        assert_eq!(error.code, "response_too_large");
    }
    #[tokio::test]
    async fn account_catalog_revalidates_shared_session_and_selected_home_before_success() {
        let fixture = Fixture::new("synthetic-first-account");
        let selected = fixture.root.join("selected-home");
        std::fs::create_dir(&selected).unwrap();
        let config = selected.join("config.toml");
        std::fs::write(&config, "profile=[]\n").unwrap();
        let mut connection = fixture.connection();
        connection.account = Some(snapshot(&config).unwrap());
        let error = list_with_fetch(connection, &Client::new(), |_| async {
            std::fs::write(
                &config,
                "profile=[]\n[openai_auth]\nlocally_disabled=true\n",
            )
            .unwrap();
            Ok(br#"{"models":[]}"#.to_vec())
        })
        .await
        .unwrap_err();
        assert_eq!(error.code, "changed_connection");
        assert_eq!(snapshot(&config).unwrap_err().code, "locally_disabled");
        let connection = fixture.connection();
        let error = list_with_fetch(connection, &Client::new(), |_| async {
            std::fs::write(
                fixture.root.join("auth.json"),
                auth("synthetic-second-account"),
            )
            .unwrap();
            Ok(br#"{"models":[]}"#.to_vec())
        })
        .await
        .unwrap_err();
        assert_eq!(error.code, "changed_connection");
        let next = fixture.connection();
        assert_eq!(
            next.account.as_ref().unwrap().session.account_id.as_deref(),
            Some("synthetic-second-account")
        );
    }
    #[tokio::test]
    async fn account_catalog_preserves_injected_transport_failures_without_fallback() {
        let fixture = Fixture::new("synthetic-transport-context");
        for code in [
            "cancelled",
            "timeout",
            "connectivity",
            "unauthorized",
            "forbidden",
            "rate_limited",
            "redirect_refused",
        ] {
            let error = list_with_fetch(fixture.connection(), &Client::new(), |_| async {
                Err(DiscoveryError::new(code, "Safe catalog failure."))
            })
            .await
            .unwrap_err();
            assert_eq!(error.code, code);
            assert!(!error.retryable);
        }
    }
    #[tokio::test]
    async fn account_catalog_http_errors_and_redirect_targets_are_safe() {
        let fixture = Fixture::new("synthetic-http-context");
        for (status, code) in [
            (401, "unauthorized"),
            (403, "forbidden"),
            (429, "rate_limited"),
            (500, "provider_error"),
            (302, "redirect_refused"),
        ] {
            let mut server = mockito::Server::new_async().await;
            let mock = server
                .mock("GET", "/models")
                .with_status(status)
                .with_header("location", "/private-redirect-target")
                .with_body("synthetic-http-context")
                .expect(1)
                .create_async()
                .await;
            let redirected = server
                .mock("GET", "/private-redirect-target")
                .expect(0)
                .create_async()
                .await;
            let client = Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .retry(reqwest::retry::never())
                .build()
                .unwrap();
            let target = Url::parse(&format!("{}/models", server.url())).unwrap();
            let error = list_with_fetch(fixture.connection(), &client, |mut request| {
                // The injected fixture transport changes only its local destination.
                *request.url_mut() = target;
                fetch(client.clone(), request)
            })
            .await
            .unwrap_err();
            assert_eq!(error.code, code);
            assert!(!error.message.contains("synthetic-http-context"));
            mock.assert_async().await;
            redirected.assert_async().await;
        }
    }
    #[tokio::test]
    async fn account_catalog_budget_drops_pending_transport_without_partial_success() {
        let fixture = Fixture::new("synthetic-budget-context");
        let error = list_with_fetch_budget(
            fixture.connection(),
            &Client::new(),
            |_| async { std::future::pending::<Result<Vec<u8>, DiscoveryError>>().await },
            Duration::from_millis(20),
        )
        .await
        .unwrap_err();
        assert_eq!(error.code, "timeout");
    }
}
