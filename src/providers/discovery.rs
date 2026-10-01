//! Read-only, bounded catalog adapters. Catalog presence is not invocation proof.
use super::ProviderKind;
use crate::config::schema::ProfileAuthMode;
use reqwest::{Client, Url};
use serde::Serialize;
use serde_json::{json, Value};
use std::{collections::BTreeSet, time::Duration};

pub(crate) const MAX_PAGE_BYTES: usize = 1024 * 1024;
pub(crate) const MAX_PAGES: u32 = 20;
pub(crate) const TOTAL_SECONDS: u64 = 30;

#[derive(Debug)]
pub struct DiscoveryError {
    pub code: &'static str,
    pub message: &'static str,
    pub retryable: bool,
}
impl DiscoveryError {
    pub(crate) fn new(code: &'static str, message: &'static str) -> Self {
        Self {
            code,
            message,
            retryable: false,
        }
    }
    pub(crate) fn malformed() -> Self {
        Self::new(
            "invalid_provider_response",
            "The provider returned an invalid model catalog.",
        )
    }
}

pub(crate) struct Connection {
    pub provider: ProviderKind,
    pub auth: ProfileAuthMode,
    pub endpoint: Url,
    pub request_endpoint: String,
    pub token: String,
    pub profile: Option<String>,
    pub account: Option<super::account_discovery::Snapshot>,
}

#[derive(Debug, Serialize)]
struct Model {
    id: String,
    name: Option<String>,
    metadata: Value,
    metadata_source: &'static str,
    invocation_access: &'static str,
}
struct Page {
    models: Vec<Model>,
    next: Option<String>,
}

pub(crate) fn provider_name(provider: ProviderKind) -> &'static str {
    match provider {
        ProviderKind::OpenAi => "openai",
        ProviderKind::Anthropic => "anthropic",
        ProviderKind::Gemini => "gemini",
        ProviderKind::Mistral => "mistral",
        ProviderKind::Xai => "xai",
        ProviderKind::Ollama => "ollama",
        ProviderKind::TypeSafe => "typesafe",
    }
}

pub(crate) fn thinking_value(
    connection: &Connection,
    model: &str,
    support: super::thinking::ThinkingSupport,
) -> Value {
    json!({"schema_version":1,"provider":provider_name(connection.provider),"auth":connection.auth,"model":model,
        "connection":{"endpoint":super::provider_url_origin(connection.endpoint.as_str()),"profile":connection.profile},
        "fetched_at":time::OffsetDateTime::now_utc().format(&time::format_description::well_known::Rfc3339).unwrap_or_default(),
        "thinking":support,"invocation_access":"unverified"})
}

pub(crate) async fn thinking(connection: Connection, model: &str) -> Result<Value, DiscoveryError> {
    if model.is_empty() || model.len() > 4096 || model.chars().any(char::is_control) {
        return Err(DiscoveryError::new(
            "invalid_request",
            "Select a bounded nonempty model identifier.",
        ));
    }
    if connection.auth == ProfileAuthMode::OpenaiAccount {
        return super::account_discovery::thinking(connection, model).await;
    }
    let support = tokio::select! {
        support = super::thinking_metadata::query_support(connection.provider,&connection.request_endpoint,model,&connection.token,None,super::thinking_metadata::ThinkingRequestKind::Text) => support.map_err(thinking_metadata_error)?,
        result = tokio::signal::ctrl_c() => { let _ = result; return Err(DiscoveryError::new("cancelled","Thinking discovery was cancelled.")); },
    };
    Ok(thinking_value(&connection, model, support))
}

fn thinking_metadata_error(
    error: super::thinking_metadata::ThinkingMetadataError,
) -> DiscoveryError {
    use super::thinking_metadata::ThinkingMetadataError;
    match error {
        ThinkingMetadataError::Unauthorized => DiscoveryError::new(
            "unauthorized",
            "The provider rejected the catalog credential.",
        ),
        ThinkingMetadataError::Forbidden => {
            DiscoveryError::new("forbidden", "The provider denied catalog access.")
        }
        ThinkingMetadataError::RateLimited => {
            DiscoveryError::new("rate_limited", "The provider rate limited catalog access.")
        }
        ThinkingMetadataError::RedirectRefused => DiscoveryError::new(
            "redirect_refused",
            "Model catalog redirects are not followed.",
        ),
        ThinkingMetadataError::ProviderError => DiscoveryError::new(
            "provider_error",
            "The provider could not return a model catalog.",
        ),
        ThinkingMetadataError::Timeout => {
            DiscoveryError::new("timeout", "Model discovery timed out.")
        }
        ThinkingMetadataError::Connectivity => {
            DiscoveryError::new("connectivity", "Could not reach the model catalog.")
        }
        ThinkingMetadataError::InvalidProviderResponse => DiscoveryError::malformed(),
        ThinkingMetadataError::ResponseTooLarge => DiscoveryError::new(
            "response_too_large",
            "The model catalog exceeds the page byte limit.",
        ),
    }
}

pub(crate) fn thinking_endpoint(
    provider: ProviderKind,
    raw: Option<&str>,
    auth: ProfileAuthMode,
) -> Result<Url, DiscoveryError> {
    if provider != ProviderKind::TypeSafe {
        return endpoint(provider, raw, auth);
    }
    if auth != ProfileAuthMode::ApiKey {
        return Err(DiscoveryError::new(
            "unsupported_connection",
            "This connection requires API-key authentication.",
        ));
    }
    let url = Url::parse(raw.unwrap_or(provider.default_url()))
        .map_err(|_| DiscoveryError::new("invalid_connection", "The connection URL is invalid."))?;
    if url.scheme() != "https"
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(DiscoveryError::new(
            "invalid_connection",
            "Authenticated remote catalogs require a clean HTTPS URL.",
        ));
    }
    Ok(url)
}

pub(crate) fn endpoint(
    provider: ProviderKind,
    raw: Option<&str>,
    auth: ProfileAuthMode,
) -> Result<Url, DiscoveryError> {
    if auth == ProfileAuthMode::OpenaiAccount {
        if provider != ProviderKind::OpenAi {
            return Err(DiscoveryError::new(
                "unsupported_connection",
                "OpenAI account discovery requires the OpenAI provider.",
            ));
        }
        return super::account_discovery::endpoint(raw);
    }
    if provider == ProviderKind::TypeSafe {
        return Err(DiscoveryError::new(
            "unsupported_connection",
            "Model discovery is unsupported for this provider or authentication mode.",
        ));
    }
    if auth == ProfileAuthMode::None && provider != ProviderKind::Ollama {
        return Err(DiscoveryError::new(
            "authentication_required",
            "This catalog requires an explicit API key.",
        ));
    }
    let mut url = Url::parse(raw.unwrap_or(provider.default_url()))
        .map_err(|_| DiscoveryError::new("invalid_connection", "The connection URL is invalid."))?;
    if !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(DiscoveryError::new(
            "invalid_connection",
            "Connection URLs must not contain userinfo, queries or fragments.",
        ));
    }
    let loopback = match url.host_str() {
        Some(host) => {
            host == "localhost"
                || host
                    .trim_matches(['[', ']'])
                    .parse::<std::net::IpAddr>()
                    .is_ok_and(|ip| ip.is_loopback())
        }
        None => false,
    };
    if url.scheme() != "https"
        && !(url.scheme() == "http"
            && loopback
            && (provider == ProviderKind::Ollama || auth == ProfileAuthMode::None))
    {
        return Err(DiscoveryError::new(
            "invalid_connection",
            "Authenticated remote catalogs require HTTPS; local Ollama may use loopback HTTP.",
        ));
    }
    let path = url.path().trim_end_matches('/');
    let replacements: &[(&str, &str)] = match provider {
        ProviderKind::Anthropic => &[("/v1/messages", "/v1/models"), ("/v1/models", "/v1/models")],
        ProviderKind::Gemini => &[
            ("/v1beta/interactions", "/v1beta/models"),
            ("/v1beta/models", "/v1beta/models"),
            ("/v1/models", "/v1/models"),
        ],
        ProviderKind::Ollama => &[
            ("/v1/chat/completions", "/api/tags"),
            ("/api/chat", "/api/tags"),
            ("/api/generate", "/api/tags"),
            ("/api/tags", "/api/tags"),
        ],
        ProviderKind::OpenAi => &[
            ("/v1/chat/completions", "/v1/models"),
            ("/v1/responses", "/v1/models"),
            ("/v1/models", "/v1/models"),
        ],
        ProviderKind::Mistral => &[
            ("/v1/chat/completions", "/v1/models"),
            ("/v1/models", "/v1/models"),
        ],
        ProviderKind::Xai => &[
            ("/v1/responses", "/v1/models"),
            ("/v1/models", "/v1/models"),
        ],
        ProviderKind::TypeSafe => unreachable!(),
    };
    let replacement = replacements.iter().find_map(|(from, to)| {
        path.strip_suffix(from)
            .map(|prefix| format!("{prefix}{to}"))
    });
    let Some(replacement) = replacement else {
        return Err(DiscoveryError::new(
            "unsupported_endpoint",
            "This URL has no supported discovery route for the selected provider.",
        ));
    };
    url.set_path(&replacement);
    Ok(url)
}

fn id(value: &Value, field: &str) -> Result<String, DiscoveryError> {
    let value = value
        .get(field)
        .and_then(Value::as_str)
        .ok_or_else(DiscoveryError::malformed)?;
    if value.is_empty() || value.len() > 4096 || value.chars().any(char::is_control) {
        return Err(DiscoveryError::malformed());
    }
    Ok(value.to_owned())
}
fn optional_text(value: &Value, field: &str) -> Result<Option<String>, DiscoveryError> {
    match value.get(field) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(text)) if text.len() <= 4096 && !text.chars().any(char::is_control) => {
            Ok(Some(text.clone()))
        }
        _ => Err(DiscoveryError::malformed()),
    }
}
fn parse_page(provider: ProviderKind, body: &[u8]) -> Result<Page, DiscoveryError> {
    let value: Value = serde_json::from_slice(body).map_err(|_| DiscoveryError::malformed())?;
    let field = match provider {
        ProviderKind::Gemini | ProviderKind::Ollama => "models",
        _ => "data",
    };
    let records = value
        .get(field)
        .and_then(Value::as_array)
        .ok_or_else(DiscoveryError::malformed)?;
    let mut models = Vec::new();
    for record in records {
        let (model_id, name, metadata) = match provider {
            ProviderKind::Gemini => {
                let resource = id(record, "name")?;
                let model_id = resource
                    .strip_prefix("models/")
                    .filter(|id| !id.is_empty())
                    .ok_or_else(DiscoveryError::malformed)?
                    .to_owned();
                (
                    model_id,
                    optional_text(record, "displayName")?,
                    json!({"resource_name": resource}),
                )
            }
            ProviderKind::Ollama => (id(record, "name")?, None, json!({})),
            _ => (
                id(record, "id")?,
                optional_text(record, "display_name")?,
                json!({}),
            ),
        };
        models.push(Model {
            id: model_id,
            name,
            metadata,
            metadata_source: "provider",
            invocation_access: "unverified",
        });
    }
    let next = match provider {
        ProviderKind::Gemini => optional_text(&value, "nextPageToken")?.filter(|v| !v.is_empty()),
        ProviderKind::Anthropic => match value.get("has_more").and_then(Value::as_bool) {
            Some(true) => Some(id(&value, "last_id")?),
            Some(false) => None,
            _ => return Err(DiscoveryError::malformed()),
        },
        _ => {
            // Unsupported pagination conventions must never imply complete coverage.
            if value.get("has_more").and_then(Value::as_bool) == Some(true)
                || value.get("next").is_some_and(|v| !v.is_null())
            {
                return Err(DiscoveryError::new(
                    "unsupported_pagination",
                    "The provider uses an unsupported model catalog pagination format.",
                ));
            }
            None
        }
    };
    Ok(Page { models, next })
}

async fn fetch_page(
    client: &Client,
    connection: &Connection,
    cursor: Option<&str>,
) -> Result<Page, DiscoveryError> {
    let mut url = connection.endpoint.clone();
    if let Some(cursor) = cursor {
        let key = match connection.provider {
            ProviderKind::Anthropic => "after_id",
            ProviderKind::Gemini => "pageToken",
            _ => return Err(DiscoveryError::malformed()),
        };
        url.query_pairs_mut().append_pair(key, cursor);
    }
    let mut request = client.get(url);
    if connection.auth == ProfileAuthMode::ApiKey {
        request = match connection.provider {
            ProviderKind::Anthropic => request
                .header("x-api-key", &connection.token)
                .header("anthropic-version", "2023-06-01"),
            ProviderKind::Gemini => request.header("x-goog-api-key", &connection.token),
            _ => request.bearer_auth(&connection.token),
        };
    }
    let mut response = request.send().await.map_err(|error| {
        if error.is_timeout() {
            DiscoveryError::new("timeout", "Model discovery timed out.")
        } else {
            DiscoveryError::new("connectivity", "Could not reach the model catalog.")
        }
    })?;
    let status = response.status().as_u16();
    if !(200..300).contains(&status) {
        return Err(match status {
            401 => DiscoveryError::new(
                "unauthorized",
                "The provider rejected the catalog credential.",
            ),
            403 => DiscoveryError::new("forbidden", "The provider denied catalog access."),
            429 => DiscoveryError::new("rate_limited", "The provider rate limited catalog access."),
            300..=399 => DiscoveryError::new(
                "redirect_refused",
                "Model catalog redirects are not followed.",
            ),
            _ => DiscoveryError::new(
                "provider_error",
                "The provider could not return a model catalog.",
            ),
        });
    }
    if response
        .content_length()
        .is_some_and(|len| len > MAX_PAGE_BYTES as u64)
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
    parse_page(connection.provider, &bytes)
}

pub(crate) async fn list(connection: Connection, page_limit: u32) -> Result<Value, DiscoveryError> {
    if connection.auth == ProfileAuthMode::OpenaiAccount {
        if page_limit == 0 || page_limit > MAX_PAGES {
            return Err(DiscoveryError::new(
                "invalid_request",
                "Page limit must be between 1 and 20.",
            ));
        }
        return super::account_discovery::list(connection).await;
    }
    list_with_budget(connection, page_limit, Duration::from_secs(TOTAL_SECONDS)).await
}

async fn list_with_budget(
    connection: Connection,
    page_limit: u32,
    budget: Duration,
) -> Result<Value, DiscoveryError> {
    if page_limit == 0 || page_limit > MAX_PAGES {
        return Err(DiscoveryError::new(
            "invalid_request",
            "Page limit must be between 1 and 20.",
        ));
    }
    let client = Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .retry(reqwest::retry::never())
        .timeout(budget)
        .build()
        .map_err(|_| {
            DiscoveryError::new("connectivity", "Could not initialize catalog transport.")
        })?;
    let mut next: Option<String> = None;
    let mut models = Vec::new();
    let mut pages = 0;
    let operation = async {
        let mut seen_pages = BTreeSet::new();
        let mut seen_models = BTreeSet::new();
        loop {
            if !seen_pages.insert(next.clone()) {
                return Err(DiscoveryError::malformed());
            }
            let page = fetch_page(&client, &connection, next.as_deref()).await?;
            pages += 1;
            for model in page.models {
                if !connection.token.is_empty()
                    && (model.id.contains(&connection.token)
                        || model
                            .name
                            .as_ref()
                            .is_some_and(|name| name.contains(&connection.token))
                        || model
                            .metadata
                            .get("resource_name")
                            .and_then(Value::as_str)
                            .is_some_and(|name| name.contains(&connection.token)))
                {
                    return Err(DiscoveryError::malformed());
                }
                if seen_models.insert(model.id.clone()) {
                    models.push(model);
                }
            }
            next = page.next;
            if !connection.token.is_empty()
                && next
                    .as_ref()
                    .is_some_and(|cursor| cursor.contains(&connection.token))
            {
                return Err(DiscoveryError::malformed());
            }
            if next.is_none() || pages >= page_limit {
                break;
            }
        }
        Ok(())
    };
    let result = tokio::select! {
        result = tokio::time::timeout(budget, operation) => result.unwrap_or_else(|_| Err(DiscoveryError::new("timeout", "Model discovery timed out."))),
        result = tokio::signal::ctrl_c() => { let _ = result; Err(DiscoveryError::new("cancelled", "Model discovery was cancelled.")) },
    };
    let reason = match result {
        Ok(()) if next.is_some() => Some("page_budget_exhausted"),
        Ok(()) => None,
        Err(error) if error.code == "timeout" && pages > 0 => Some("time_budget_exhausted"),
        Err(error) => return Err(error),
    };
    Ok(
        json!({"schema_version": 1, "provider": provider_name(connection.provider), "auth": connection.auth.as_str(),
        "connection": {"endpoint": connection.endpoint.origin().ascii_serialization(), "profile": connection.profile},
        "fetched_at": time::OffsetDateTime::now_utc().format(&time::format_description::well_known::Rfc3339).unwrap_or_default(),
        "source": "live", "cache": null, "complete": reason.is_none(),
        "continuation": reason.map(|reason| json!({"available":false,"reason":reason})),
        "pages_fetched": pages, "models": models, "invocation_access": "unverified"}),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    fn connection(provider: ProviderKind, url: String) -> Connection {
        Connection {
            request_endpoint: url.clone(),
            provider,
            auth: ProfileAuthMode::ApiKey,
            endpoint: Url::parse(&url).unwrap(),
            token: "synthetic-key".to_owned(),
            profile: None,
            account: None,
        }
    }
    #[test]
    fn discovery_routes_and_unsafe_urls() {
        for provider in [
            ProviderKind::OpenAi,
            ProviderKind::Anthropic,
            ProviderKind::Gemini,
            ProviderKind::Mistral,
            ProviderKind::Xai,
            ProviderKind::Ollama,
        ] {
            let url = endpoint(provider, None, ProfileAuthMode::ApiKey).unwrap();
            assert!(url.path().ends_with("/models") || url.path() == "/api/tags");
            for raw in [
                "https://user:secret@fixture.test/v1/models",
                "https://fixture.test/v1/models?key=secret",
                "https://fixture.test/v1/models#secret",
                "http://fixture.test/v1/models",
                "ftp://fixture.test/v1/models",
                "https://fixture.test/unknown",
            ] {
                assert!(endpoint(provider, Some(raw), ProfileAuthMode::ApiKey).is_err());
            }
        }
        assert!(endpoint(ProviderKind::OpenAi, None, ProfileAuthMode::OpenaiAccount).is_ok());
        assert!(endpoint(ProviderKind::TypeSafe, None, ProfileAuthMode::ApiKey).is_err());
        assert!(endpoint(
            ProviderKind::OpenAi,
            Some("https://chatgpt.com/backend-api/codex"),
            ProfileAuthMode::ApiKey
        )
        .is_err());
        assert!(endpoint(
            ProviderKind::OpenAi,
            Some("http://127.0.0.1:1234/v1/models"),
            ProfileAuthMode::ApiKey
        )
        .is_err());
    }
    #[test]
    fn discovery_normalizes_exact_ids_and_rejects_malformed_payloads() {
        for provider in [
            ProviderKind::OpenAi,
            ProviderKind::Mistral,
            ProviderKind::Xai,
        ] {
            let page = parse_page(
                provider,
                br#"{"data":[{"id":"custom/opaque:model","capabilities":{"reasoning":true}}]}"#,
            )
            .unwrap();
            assert_eq!(page.models[0].id, "custom/opaque:model");
            assert_eq!(page.models[0].metadata, json!({}));
        }
        let page = parse_page(
            ProviderKind::Gemini,
            br#"{"models":[{"name":"models/gemini-custom","displayName":"Custom"}]}"#,
        )
        .unwrap();
        assert_eq!(page.models[0].id, "gemini-custom");
        assert_eq!(
            page.models[0].metadata["resource_name"],
            "models/gemini-custom"
        );
        assert!(parse_page(
            ProviderKind::Gemini,
            br#"{"models":[{"name":"unexpected"}]}"#
        )
        .is_err());
        assert_eq!(
            parse_page(
                ProviderKind::Ollama,
                br#"{"models":[{"name":"local:latest"}]}"#
            )
            .unwrap()
            .models[0]
                .id,
            "local:latest"
        );
        assert!(parse_page(ProviderKind::Anthropic, br#"{"data":[],"has_more":true}"#).is_err());
        assert!(parse_page(ProviderKind::OpenAi, br#"{"data":[{"id":""}]}"#).is_err());
        assert!(parse_page(ProviderKind::OpenAi, br#"{"error":"synthetic-key"}"#).is_err());
        assert_eq!(
            parse_page(ProviderKind::OpenAi, br#"{"data":[]}"#)
                .unwrap()
                .models
                .len(),
            0
        );
    }
    #[tokio::test]
    async fn discovery_six_provider_gets_use_native_headers_and_no_inference() {
        for provider in [
            ProviderKind::OpenAi,
            ProviderKind::Anthropic,
            ProviderKind::Gemini,
            ProviderKind::Mistral,
            ProviderKind::Xai,
            ProviderKind::Ollama,
        ] {
            let mut server = mockito::Server::new_async().await;
            let body = match provider {
                ProviderKind::Gemini => r#"{"models":[{"name":"models/exact-id"}]}"#,
                ProviderKind::Ollama => r#"{"models":[{"name":"exact-id"}]}"#,
                ProviderKind::Anthropic => r#"{"data":[{"id":"exact-id"}],"has_more":false}"#,
                _ => r#"{"data":[{"id":"exact-id"}]}"#,
            };
            let header = match provider {
                ProviderKind::Anthropic => "x-api-key",
                ProviderKind::Gemini => "x-goog-api-key",
                _ => "authorization",
            };
            let value = if header == "authorization" {
                "Bearer synthetic-key"
            } else {
                "synthetic-key"
            };
            let mut mock_builder = server.mock("GET", "/models").match_header(header, value);
            if provider == ProviderKind::Anthropic {
                mock_builder = mock_builder.match_header("anthropic-version", "2023-06-01");
            }
            let mock = mock_builder
                .with_status(200)
                .with_body(body)
                .expect(1)
                .create_async()
                .await;
            let result = list(connection(provider, format!("{}/models", server.url())), 20)
                .await
                .unwrap();
            assert_eq!(result["models"][0]["id"], "exact-id");
            assert_eq!(result["models"][0]["invocation_access"], "unverified");
            assert_eq!(result["complete"], true);
            assert!(!result.to_string().contains("synthetic-key"));
            mock.assert_async().await;
        }
    }
    #[tokio::test]
    async fn discovery_pagination_empty_partial_and_repeated_page() {
        let mut server = mockito::Server::new_async().await;
        let first = server
            .mock("GET", "/models")
            .match_query(mockito::Matcher::Missing)
            .with_status(200)
            .with_body(r#"{"data":[{"id":"a"}],"has_more":true,"last_id":"a"}"#)
            .expect(3)
            .create_async()
            .await;
        let second = server
            .mock("GET", "/models")
            .match_query(mockito::Matcher::UrlEncoded(
                "after_id".to_owned(),
                "a".to_owned(),
            ))
            .with_status(200)
            .with_body(r#"{"data":[],"has_more":false}"#)
            .create_async()
            .await;
        let url = format!("{}/models", server.url());
        let result = list(connection(ProviderKind::Anthropic, url.clone()), 20)
            .await
            .unwrap();
        assert_eq!(result["pages_fetched"], 2);
        assert_eq!(result["complete"], true);
        second.assert_async().await;
        let partial = list(connection(ProviderKind::Anthropic, url.clone()), 1)
            .await
            .unwrap();
        assert_eq!(partial["complete"], false);
        assert!(!partial["continuation"].is_null());
        second.remove_async().await;
        let repeat = server
            .mock("GET", "/models")
            .match_query(mockito::Matcher::UrlEncoded(
                "after_id".to_owned(),
                "a".to_owned(),
            ))
            .with_status(200)
            .with_body(r#"{"data":[],"has_more":true,"last_id":"a"}"#)
            .create_async()
            .await;
        assert_eq!(
            list(connection(ProviderKind::Anthropic, url), 20)
                .await
                .unwrap_err()
                .code,
            "invalid_provider_response"
        );
        repeat.assert_async().await;
        first.assert_async().await;
    }
    #[tokio::test]
    async fn discovery_errors_are_secret_safe_and_redirects_never_followed() {
        for (status, code) in [
            (401, "unauthorized"),
            (403, "forbidden"),
            (429, "rate_limited"),
            (302, "redirect_refused"),
            (500, "provider_error"),
        ] {
            let mut server = mockito::Server::new_async().await;
            let mock = server
                .mock("GET", "/models")
                .with_status(status)
                .with_header("location", "/credential-target")
                .with_body("synthetic-key")
                .expect(1)
                .create_async()
                .await;
            let forbidden = server
                .mock("GET", "/credential-target")
                .expect(0)
                .create_async()
                .await;
            let error = list(
                connection(ProviderKind::OpenAi, format!("{}/models", server.url())),
                20,
            )
            .await
            .unwrap_err();
            assert_eq!(error.code, code);
            assert!(!error.message.contains("synthetic-key"));
            assert!(!error.retryable);
            mock.assert_async().await;
            forbidden.assert_async().await;
        }
    }
    #[tokio::test]
    async fn discovery_oversize_and_invalid_budget_stop_safely() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("GET", "/models")
            .with_status(200)
            .with_body(vec![b'x'; MAX_PAGE_BYTES + 1])
            .expect(1)
            .create_async()
            .await;
        let url = format!("{}/models", server.url());
        assert_eq!(
            list(connection(ProviderKind::OpenAi, url.clone()), 0)
                .await
                .unwrap_err()
                .code,
            "invalid_request"
        );
        assert_eq!(
            list(connection(ProviderKind::OpenAi, url), 20)
                .await
                .unwrap_err()
                .code,
            "response_too_large"
        );
        mock.assert_async().await;
    }
    #[tokio::test]
    async fn discovery_total_budget_stops_pending_response() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = [0u8; 4096];
            let count = socket.read(&mut request).await.unwrap();
            assert!(request[..count].starts_with(b"GET /models HTTP/1.1"));
            socket
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 1000\r\n\r\n{")
                .await
                .unwrap();
            tokio::time::sleep(Duration::from_secs(1)).await;
        });
        let error = list_with_budget(
            connection(ProviderKind::OpenAi, format!("http://{address}/models")),
            20,
            Duration::from_millis(50),
        )
        .await
        .unwrap_err();
        assert_eq!(error.code, "timeout");
        server.abort();
    }
    #[tokio::test]
    async fn discovery_total_budget_retains_only_completed_valid_pages() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut first, _) = listener.accept().await.unwrap();
            let mut request = [0u8; 4096];
            let count = first.read(&mut request).await.unwrap();
            assert!(request[..count].starts_with(b"GET /models HTTP/1.1"));
            let body = r#"{"data":[{"id":"exact-first"}],"has_more":true,"last_id":"exact-first"}"#;
            first.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len()).as_bytes()).await.unwrap();
            drop(first);
            let (mut second, _) = listener.accept().await.unwrap();
            let count = second.read(&mut request).await.unwrap();
            assert!(request[..count].starts_with(b"GET /models?after_id=exact-first HTTP/1.1"));
            second
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 1000\r\n\r\n{")
                .await
                .unwrap();
            tokio::time::sleep(Duration::from_secs(3)).await;
        });
        let started = std::time::Instant::now();
        let result = list_with_budget(
            connection(ProviderKind::Anthropic, format!("http://{address}/models")),
            20,
            Duration::from_millis(300),
        )
        .await
        .unwrap();
        assert!(started.elapsed() < Duration::from_secs(1));
        assert_eq!(result["complete"], false);
        assert_eq!(result["pages_fetched"], 1);
        assert_eq!(result["models"].as_array().unwrap().len(), 1);
        assert_eq!(result["models"][0]["id"], "exact-first");
        assert_eq!(result["models"][0]["invocation_access"], "unverified");
        assert_eq!(
            result["continuation"],
            json!({"available":false,"reason":"time_budget_exhausted"})
        );
        assert!(!result.to_string().contains("synthetic-key"));
        server.abort();
    }
    #[tokio::test]
    async fn discovery_rejects_provider_echoed_credential() {
        for token in ["synthetic-key", "synthetic-\"escaped-key"] {
            let mut server = mockito::Server::new_async().await;
            let mock = server
                .mock("GET", "/models")
                .with_status(200)
                .with_body(json!({"data":[{"id":token}]}).to_string())
                .create_async()
                .await;
            let mut selected = connection(ProviderKind::OpenAi, format!("{}/models", server.url()));
            selected.token = token.to_owned();
            let error = list(selected, 20).await.unwrap_err();
            assert_eq!(error.code, "invalid_provider_response");
            assert!(!error.message.contains(token));
            mock.assert_async().await;
        }
    }
}
