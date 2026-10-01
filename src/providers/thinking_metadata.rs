//! Bounded thinking metadata for an already resolved connection.
use super::{
    thinking::{ThinkingChoice, ThinkingSupport},
    ProviderKind,
};
use reqwest::{Client, Url};
use serde_json::Value;
use std::{collections::BTreeSet, time::Duration};

const MAX_BYTES: usize = 1024 * 1024;
const BUDGET: Duration = Duration::from_secs(30);
pub(crate) const ACCOUNT_CATALOG_VERSION: &str = "0.159.2";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ThinkingRequestKind {
    Text,
    Image,
    Speech,
    Transcription,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ThinkingMetadataError {
    Unauthorized,
    Forbidden,
    RateLimited,
    RedirectRefused,
    ProviderError,
    Timeout,
    Connectivity,
    InvalidProviderResponse,
    ResponseTooLarge,
}

pub(crate) fn unknown(reason: &str) -> ThinkingSupport {
    ThinkingSupport::Unknown {
        reason: reason.into(),
        evidence: None,
    }
}
fn unsupported(reason: &str, evidence: &str) -> ThinkingSupport {
    ThinkingSupport::Unsupported {
        reason: reason.into(),
        evidence: Some(evidence.into()),
    }
}
fn choices(values: &[&str], default: Option<&str>, evidence: &str) -> ThinkingSupport {
    ThinkingSupport::Configurable {
        choices: values
            .iter()
            .map(|value| ThinkingChoice {
                value: (*value).into(),
                description: None,
            })
            .collect(),
        default: default.map(str::to_owned),
        evidence: Some(evidence.into()),
    }
}
fn bounded_text(value: &Value) -> Option<&str> {
    value.as_str().filter(|text| {
        !text.is_empty() && text.len() <= 4096 && !text.chars().any(char::is_control)
    })
}

pub(crate) fn account_record_support(record: &Value) -> ThinkingSupport {
    let Some(values) = record
        .get("supported_reasoning_levels")
        .and_then(Value::as_array)
        .filter(|values| values.len() <= 64)
    else {
        return unknown("missing or malformed account thinking metadata");
    };
    let mut choices = Vec::new();
    let mut seen = BTreeSet::new();
    for entry in values {
        let Some(value) = entry.get("effort").and_then(bounded_text) else {
            return unknown("malformed account thinking choice");
        };
        if !seen.insert(value) {
            return unknown("duplicate account thinking choices");
        }
        let description = match entry.get("description") {
            None | Some(Value::Null) => None,
            Some(value) => match bounded_text(value) {
                Some(value) => Some(value.to_owned()),
                None => return unknown("malformed account thinking description"),
            },
        };
        choices.push(ThinkingChoice {
            value: value.to_owned(),
            description,
        });
    }
    let default = match record.get("default_reasoning_level") {
        None | Some(Value::Null) => None,
        Some(value) => match bounded_text(value) {
            Some(value) if seen.contains(value) => Some(value.to_owned()),
            _ => return unknown("account thinking default is inconsistent with its choices"),
        },
    };
    let evidence = "https://chatgpt.com/backend-api/codex/models?client_version=0.159.2";
    if choices.is_empty() {
        unsupported(
            "the selected account model advertises no named choices",
            evidence,
        )
    } else {
        ThinkingSupport::Configurable {
            choices,
            default,
            evidence: Some(evidence.into()),
        }
    }
}

fn account_catalog_support(catalog: &Value, model: &str) -> ThinkingSupport {
    let Some(records) = catalog.get("models").and_then(Value::as_array) else {
        return unknown("malformed account model catalog");
    };
    let mut selected = records
        .iter()
        .filter(|record| record.get("slug").and_then(Value::as_str) == Some(model));
    let Some(record) = selected.next() else {
        return unknown("the selected model is absent from current account metadata");
    };
    if selected.next().is_some() {
        return unknown("ambiguous account model metadata");
    }
    account_record_support(record)
}

pub(crate) fn ollama_support(value: &Value, model: &str) -> ThinkingSupport {
    for field in ["model", "name"] {
        if value
            .get(field)
            .is_some_and(|value| value.as_str() != Some(model))
        {
            return unknown("Ollama metadata identifies a different model");
        }
    }
    let Some(thinking) = value.get("thinking").and_then(Value::as_object) else {
        return unknown("missing or malformed Ollama thinking metadata");
    };
    let Some(values) = thinking
        .get("values")
        .and_then(Value::as_array)
        .filter(|values| !values.is_empty() && values.len() <= 64)
    else {
        return unknown("malformed Ollama thinking choices");
    };
    let evidence = "https://docs.ollama.com/capabilities/thinking";
    if values.iter().all(Value::is_boolean) {
        if thinking
            .get("default")
            .is_some_and(|default| !values.contains(default))
        {
            return unknown("Ollama thinking default is inconsistent with its choices");
        }
        return unsupported(
            "the model exposes only a boolean thinking control",
            evidence,
        );
    }
    let mut seen = BTreeSet::new();
    let mut retained = Vec::new();
    for value in values {
        let Some(value) = bounded_text(value) else {
            return unknown("malformed Ollama named thinking choice");
        };
        if !seen.insert(value) {
            return unknown("duplicate Ollama thinking choices");
        }
        retained.push(ThinkingChoice {
            value: value.to_owned(),
            description: None,
        });
    }
    let default = match thinking.get("default") {
        None | Some(Value::Null) => None,
        Some(value) => match bounded_text(value) {
            Some(value) if seen.contains(value) => Some(value.to_owned()),
            _ => return unknown("Ollama thinking default is inconsistent with its choices"),
        },
    };
    ThinkingSupport::Configurable {
        choices: retained,
        default,
        evidence: Some(evidence.into()),
    }
}

fn valid_url(raw: &str) -> Option<Url> {
    let url = Url::parse(raw).ok()?;
    let local = url.host_str().is_some_and(|host| {
        host == "localhost"
            || host
                .trim_matches(['[', ']'])
                .parse::<std::net::IpAddr>()
                .is_ok_and(|ip| ip.is_loopback())
    });
    (url.username().is_empty()
        && url.password().is_none()
        && url.query().is_none()
        && url.fragment().is_none()
        && (url.scheme() == "https" || (url.scheme() == "http" && local)))
        .then_some(url)
}
fn account_endpoint(url: &Url) -> bool {
    url.scheme() == "https"
        && url.host_str() == Some("chatgpt.com")
        && url.port_or_known_default() == Some(443)
        && matches!(
            url.path().trim_end_matches('/'),
            "/backend-api/codex" | "/backend-api/codex/responses"
        )
}

/// Exact documented adapters are valid only for their known provider origin and route.
pub(crate) fn adapter_support(
    provider: ProviderKind,
    raw: &str,
    model: &str,
    kind: ThinkingRequestKind,
) -> ThinkingSupport {
    let Some(url) = valid_url(raw) else {
        return unknown("the endpoint cannot be qualified");
    };
    if provider == ProviderKind::TypeSafe {
        return unsupported(
            "SystemOne has no named thinking request field",
            "https://api.typesafe.ai/openapi.json",
        );
    }
    if matches!(
        kind,
        ThinkingRequestKind::Speech | ThinkingRequestKind::Transcription
    ) {
        return match provider {
            ProviderKind::OpenAi => unsupported(
                "the audio endpoint has no named thinking request field",
                "https://developers.openai.com/api/docs/guides/audio",
            ),
            ProviderKind::Xai => unsupported(
                "the audio endpoint has no named thinking request field",
                "https://docs.x.ai/developers/model-capabilities/audio/text-to-speech",
            ),
            _ => unknown("named thinking support is not qualified for this audio endpoint"),
        };
    }
    if kind == ThinkingRequestKind::Image {
        if provider == ProviderKind::OpenAi && url.host_str() == Some("api.openai.com") {
            return unsupported(
                "the image endpoint has no named thinking request field",
                "https://developers.openai.com/api/docs/guides/image-generation",
            );
        }
        if provider == ProviderKind::Gemini {
            return super::thinking_image::gemini_support(raw, model);
        }
        return unknown("named thinking support is not qualified for this image endpoint");
    }
    let qualified = url.scheme() == "https" && url.port_or_known_default() == Some(443);
    if !qualified {
        return unknown("no authoritative thinking metadata for this custom endpoint");
    }
    match (provider, url.host_str(), url.path()) {
        (ProviderKind::OpenAi, Some("api.openai.com"), "/v1/chat/completions") => match model {
            "gpt-5" | "gpt-5-2025-08-07" => choices(
                &["minimal", "low", "medium", "high"],
                None,
                "https://developers.openai.com/api/docs/models/gpt-5",
            ),
            "gpt-5.2" | "gpt-5.2-2025-12-11" => choices(
                &["none", "low", "medium", "high", "xhigh"],
                Some("none"),
                "https://developers.openai.com/api/docs/models/gpt-5.2",
            ),
            "gpt-5.4" => choices(
                &["none", "low", "medium", "high", "xhigh"],
                Some("none"),
                "https://developers.openai.com/api/docs/models/gpt-5.4",
            ),
            "gpt-6.1-sol" => choices(
                &["low", "medium", "high", "xhigh", "max"],
                Some("medium"),
                "https://developers.openai.com/api/docs/models/gpt-6.1-sol",
            ),
            "gpt-5.6-sol" | "gpt-5.6-terra" | "gpt-5.6-luna" | "gpt-5.6" => choices(
                &["none", "low", "medium", "high", "xhigh", "max"],
                Some("medium"),
                match model {
                    "gpt-5.6-terra" => {
                        "https://developers.openai.com/api/docs/models/gpt-5.6-terra"
                    }
                    "gpt-5.6-luna" => "https://developers.openai.com/api/docs/models/gpt-5.6-luna",
                    _ => "https://developers.openai.com/api/docs/models/gpt-5.6-sol",
                },
            ),
            _ => unknown("this exact API model has no qualified named thinking enumeration"),
        },
        (ProviderKind::Anthropic, Some("api.anthropic.com"), "/v1/messages") => match model {
            "claude-opus-4-5" | "claude-opus-4-5-20251101" => choices(
                &["low", "medium", "high"],
                Some("high"),
                "https://platform.claude.com/docs/en/build-with-claude/effort",
            ),
            "claude-opus-4-6" | "claude-sonnet-4-6" => choices(
                &["low", "medium", "high", "max"],
                Some("high"),
                "https://platform.claude.com/docs/en/build-with-claude/effort",
            ),
            _ => unknown("this exact Claude model has no qualified named effort enumeration"),
        },
        (
            ProviderKind::Gemini,
            Some("generativelanguage.googleapis.com"),
            "/v1beta/interactions",
        ) => match model {
            "gemini-3-pro-preview" => choices(
                &["low", "high"],
                Some("high"),
                "https://ai.google.dev/gemini-api/docs/thinking",
            ),
            "gemini-3.1-pro-preview" => choices(
                &["low", "medium", "high"],
                Some("high"),
                "https://ai.google.dev/gemini-api/docs/thinking",
            ),
            "gemini-3-flash-preview" => choices(
                &["minimal", "low", "medium", "high"],
                Some("high"),
                "https://ai.google.dev/gemini-api/docs/thinking",
            ),
            "gemini-2.5-pro" | "gemini-2.5-flash" | "gemini-2.5-flash-lite" => choices(
                &["low", "medium", "high"],
                None,
                "https://ai.google.dev/gemini-api/docs/thinking",
            ),
            _ => unknown("this exact Gemini model has no qualified thinking-level enumeration"),
        },
        (ProviderKind::Mistral, Some("api.mistral.ai"), "/v1/chat/completions")
            if matches!(model, "mistral-small-latest" | "mistral-medium-3-5") =>
        {
            choices(
                &["none", "high"],
                None,
                "https://docs.mistral.ai/studio/conversations/reasoning",
            )
        }
        (ProviderKind::Xai, Some("api.x.ai"), "/v1/responses") => match model {
            "grok-4.5" => choices(
                &["low", "medium", "high"],
                Some("high"),
                "https://docs.x.ai/developers/model-capabilities/text/reasoning",
            ),
            "grok-4.6" | "grok-4.7" => choices(
                &["low", "medium", "high", "xhigh"],
                Some("high"),
                "https://docs.x.ai/developers/model-capabilities/text/reasoning",
            ),
            "grok-4.20-multi-agent" => choices(
                &["low", "medium", "high", "xhigh"],
                None,
                "https://docs.x.ai/developers/model-capabilities/text/reasoning",
            ),
            _ => unknown("this exact xAI model has no qualified named effort enumeration"),
        },
        _ => unknown("no authoritative named thinking metadata for this model and endpoint"),
    }
}

pub(crate) async fn request_support(
    provider: ProviderKind,
    raw: &str,
    model: &str,
    token: &str,
    account_id: Option<&str>,
    kind: ThinkingRequestKind,
) -> ThinkingSupport {
    query_support(provider, raw, model, token, account_id, kind)
        .await
        .unwrap_or_else(|_| unknown("fresh bounded thinking metadata is unavailable"))
}

pub(crate) async fn query_support(
    provider: ProviderKind,
    raw: &str,
    model: &str,
    token: &str,
    account_id: Option<&str>,
    kind: ThinkingRequestKind,
) -> Result<ThinkingSupport, ThinkingMetadataError> {
    query_support_with_budget(provider, raw, model, token, account_id, kind, BUDGET).await
}

async fn query_support_with_budget(
    provider: ProviderKind,
    raw: &str,
    model: &str,
    token: &str,
    account_id: Option<&str>,
    kind: ThinkingRequestKind,
    budget: Duration,
) -> Result<ThinkingSupport, ThinkingMetadataError> {
    let Some(mut url) = valid_url(raw) else {
        return Ok(unknown("the endpoint cannot be qualified"));
    };
    let account = provider == ProviderKind::OpenAi
        && account_endpoint(&url)
        && matches!(kind, ThinkingRequestKind::Text | ThinkingRequestKind::Image);
    if !account && !(provider == ProviderKind::Ollama && kind == ThinkingRequestKind::Text) {
        return Ok(adapter_support(provider, raw, model, kind));
    }
    if model.is_empty() || model.len() > 4096 || model.chars().any(char::is_control) {
        return Ok(unknown("the model identifier cannot be qualified"));
    }
    let client = Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .retry(reqwest::retry::never())
        .timeout(budget)
        .build()
        .map_err(|_| ThinkingMetadataError::Connectivity)?;
    let request = if account {
        let Some(id) = account_id
            .filter(|id| !id.is_empty() && id.len() <= 4096 && !id.chars().any(char::is_control))
        else {
            return Ok(unknown("the selected account context is unavailable"));
        };
        if token.is_empty() {
            return Ok(unknown("the selected account credential is unavailable"));
        }
        url.set_path("/backend-api/codex/models");
        client
            .get(url)
            .query(&[("client_version", ACCOUNT_CATALOG_VERSION)])
            .bearer_auth(token)
            .header("ChatGPT-Account-ID", id)
            .header("originator", "cargo-ai")
            .header(
                "User-Agent",
                concat!("cargo-ai/", env!("CARGO_PKG_VERSION")),
            )
    } else {
        let path = url.path().trim_end_matches('/');
        let Some(prefix) = [
            "/v1/chat/completions",
            "/api/tags",
            "/api/chat",
            "/api/generate",
        ]
        .iter()
        .find_map(|suffix| path.strip_suffix(suffix)) else {
            return Ok(unknown(
                "the Ollama endpoint has no qualified metadata route",
            ));
        };
        url.set_path(&format!("{prefix}/api/show"));
        let request = client.post(url).json(&serde_json::json!({"model":model}));
        if token.is_empty() {
            request
        } else {
            request.bearer_auth(token)
        }
    };
    let operation = async {
        let mut response = request.send().await.map_err(|error| {
            if error.is_timeout() {
                ThinkingMetadataError::Timeout
            } else {
                ThinkingMetadataError::Connectivity
            }
        })?;
        let status = response.status().as_u16();
        if !(200..300).contains(&status) {
            return Err(match status {
                401 => ThinkingMetadataError::Unauthorized,
                403 => ThinkingMetadataError::Forbidden,
                429 => ThinkingMetadataError::RateLimited,
                300..=399 => ThinkingMetadataError::RedirectRefused,
                _ => ThinkingMetadataError::ProviderError,
            });
        }
        if response
            .content_length()
            .is_some_and(|length| length > MAX_BYTES as u64)
        {
            return Err(ThinkingMetadataError::ResponseTooLarge);
        }
        let mut body = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|error| {
            if error.is_timeout() {
                ThinkingMetadataError::Timeout
            } else {
                ThinkingMetadataError::InvalidProviderResponse
            }
        })? {
            if chunk.len() > MAX_BYTES.saturating_sub(body.len()) {
                return Err(ThinkingMetadataError::ResponseTooLarge);
            }
            body.extend_from_slice(&chunk);
        }
        Ok(serde_json::from_slice::<Value>(&body).ok())
    };
    let Some(value) = tokio::time::timeout(budget, operation)
        .await
        .map_err(|_| ThinkingMetadataError::Timeout)??
    else {
        return Ok(unknown("fresh bounded thinking metadata is unavailable"));
    };
    let support = if account {
        account_catalog_support(&value, model)
    } else {
        ollama_support(&value, model)
    };
    let serialized = serde_json::to_string(&support).unwrap_or_default();
    if [Some(token), account_id]
        .into_iter()
        .flatten()
        .any(|secret| !secret.is_empty() && serialized.contains(secret))
    {
        return Ok(unknown(
            "thinking metadata failed credential redaction checks",
        ));
    }
    Ok(support)
}

#[cfg(test)]
mod tests {
    use super::super::thinking::{ThinkingFallback, ThinkingSetting};
    use super::*;
    use serde_json::json;

    fn assert_runtime_unknown_fallback(support: ThinkingSupport) {
        assert!(matches!(&support, ThinkingSupport::Unknown { .. }));
        let selection = ThinkingSetting::Choice {
            value: "high".into(),
        };
        let outcome = super::super::thinking::resolve(Some(&selection), "run", support);
        assert_eq!(outcome.effective, ThinkingSetting::ProviderDefault);
        assert_eq!(outcome.fallback, Some(ThinkingFallback::UnknownSupport));
        assert!(outcome.applied_choice().is_none());
    }

    #[tokio::test]
    async fn query_errors_are_categorical_while_runtime_uses_unknown_fallback() {
        let mut server = mockito::Server::new_async().await;
        let raw = format!("{}/v1/chat/completions", server.url());
        for (status, error) in [
            (401, ThinkingMetadataError::Unauthorized),
            (403, ThinkingMetadataError::Forbidden),
            (429, ThinkingMetadataError::RateLimited),
            (302, ThinkingMetadataError::RedirectRefused),
            (500, ThinkingMetadataError::ProviderError),
        ] {
            let mock = server
                .mock("POST", "/api/show")
                .with_status(status)
                .with_header("location", "/must-not-follow")
                .with_body("synthetic-private-token provider failure")
                .expect(2)
                .create_async()
                .await;
            let redirected = server
                .mock("POST", "/must-not-follow")
                .expect(0)
                .create_async()
                .await;
            assert_eq!(
                query_support(
                    ProviderKind::Ollama,
                    &raw,
                    "fixture",
                    "synthetic-private-token",
                    None,
                    ThinkingRequestKind::Text
                )
                .await
                .unwrap_err(),
                error,
            );
            assert_runtime_unknown_fallback(
                request_support(
                    ProviderKind::Ollama,
                    &raw,
                    "fixture",
                    "synthetic-private-token",
                    None,
                    ThinkingRequestKind::Text,
                )
                .await,
            );
            mock.assert_async().await;
            redirected.assert_async().await;
            mock.remove_async().await;
            redirected.remove_async().await;
        }
        let oversized = server
            .mock("POST", "/api/show")
            .with_body(vec![b'x'; MAX_BYTES + 1])
            .expect(2)
            .create_async()
            .await;
        assert_eq!(
            query_support(
                ProviderKind::Ollama,
                &raw,
                "fixture",
                "",
                None,
                ThinkingRequestKind::Text
            )
            .await
            .unwrap_err(),
            ThinkingMetadataError::ResponseTooLarge,
        );
        assert_runtime_unknown_fallback(
            request_support(
                ProviderKind::Ollama,
                &raw,
                "fixture",
                "",
                None,
                ThinkingRequestKind::Text,
            )
            .await,
        );
        oversized.assert_async().await;
    }

    #[tokio::test]
    async fn refused_connection_is_an_error_for_query_and_unknown_for_runtime() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let raw = format!(
            "http://{}/v1/chat/completions",
            listener.local_addr().unwrap()
        );
        drop(listener);
        assert_eq!(
            query_support(
                ProviderKind::Ollama,
                &raw,
                "fixture",
                "",
                None,
                ThinkingRequestKind::Text
            )
            .await
            .unwrap_err(),
            ThinkingMetadataError::Connectivity,
        );
        assert_runtime_unknown_fallback(
            request_support(
                ProviderKind::Ollama,
                &raw,
                "fixture",
                "",
                None,
                ThinkingRequestKind::Text,
            )
            .await,
        );
    }

    #[tokio::test]
    async fn slow_metadata_is_bounded_by_the_query_budget() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let raw = format!(
            "http://{}/v1/chat/completions",
            listener.local_addr().unwrap()
        );
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = [0u8; 4096];
            let count = stream.read(&mut request).await.unwrap();
            assert!(request[..count].starts_with(b"POST /api/show HTTP/1.1"));
            stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 1000\r\n\r\n{")
                .await
                .unwrap();
            tokio::time::sleep(Duration::from_secs(1)).await;
        });
        assert_eq!(
            query_support_with_budget(
                ProviderKind::Ollama,
                &raw,
                "fixture",
                "",
                None,
                ThinkingRequestKind::Text,
                Duration::from_millis(100)
            )
            .await
            .unwrap_err(),
            ThinkingMetadataError::Timeout,
        );
        server.abort();
    }

    #[tokio::test]
    async fn incomplete_and_oversized_streams_fail_query_and_fall_back_at_runtime() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        for oversized in [false, true] {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let raw = format!(
                "http://{}/v1/chat/completions",
                listener.local_addr().unwrap()
            );
            let server = tokio::spawn(async move {
                for _ in 0..2 {
                    let (mut stream, _) = listener.accept().await.unwrap();
                    let mut request = [0u8; 4096];
                    let count = stream.read(&mut request).await.unwrap();
                    assert!(request[..count].starts_with(b"POST /api/show HTTP/1.1"));
                    if oversized {
                        stream
                            .write_all(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n")
                            .await
                            .unwrap();
                        let chunk = vec![b'x'; MAX_BYTES + 1];
                        let _ = stream
                            .write_all(format!("{:x}\r\n", chunk.len()).as_bytes())
                            .await;
                        let _ = stream.write_all(&chunk).await;
                        let _ = stream.write_all(b"\r\n0\r\n\r\n").await;
                    } else {
                        stream
                            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 1000\r\n\r\n{")
                            .await
                            .unwrap();
                    }
                    let _ = stream.shutdown().await;
                }
            });
            let expected = if oversized {
                ThinkingMetadataError::ResponseTooLarge
            } else {
                ThinkingMetadataError::InvalidProviderResponse
            };
            assert_eq!(
                query_support(
                    ProviderKind::Ollama,
                    &raw,
                    "fixture",
                    "",
                    None,
                    ThinkingRequestKind::Text
                )
                .await
                .unwrap_err(),
                expected,
            );
            assert_runtime_unknown_fallback(
                request_support(
                    ProviderKind::Ollama,
                    &raw,
                    "fixture",
                    "",
                    None,
                    ThinkingRequestKind::Text,
                )
                .await,
            );
            server.await.unwrap();
        }
    }

    #[tokio::test]
    async fn malformed_metadata_body_is_honest_unknown_for_query_and_runtime() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("POST", "/api/show")
            .with_body("{malformed metadata")
            .expect(2)
            .create_async()
            .await;
        let raw = format!("{}/v1/chat/completions", server.url());
        assert!(matches!(
            query_support(
                ProviderKind::Ollama,
                &raw,
                "fixture",
                "",
                None,
                ThinkingRequestKind::Text
            )
            .await
            .unwrap(),
            ThinkingSupport::Unknown { .. },
        ));
        assert_runtime_unknown_fallback(
            request_support(
                ProviderKind::Ollama,
                &raw,
                "fixture",
                "",
                None,
                ThinkingRequestKind::Text,
            )
            .await,
        );
        mock.assert_async().await;
    }

    #[test]
    fn exact_claude_models_preserve_documented_effort_distinctions() {
        let setting = super::super::thinking::ThinkingSetting::Choice {
            value: "max".into(),
        };
        for (model, applicable) in [
            ("claude-opus-4-5", false),
            ("claude-opus-4-6", true),
            ("claude-sonnet-4-6", true),
            ("claude-sonnet-4-5", false),
        ] {
            let support = adapter_support(
                ProviderKind::Anthropic,
                "https://api.anthropic.com/v1/messages",
                model,
                ThinkingRequestKind::Text,
            );
            assert_eq!(
                super::super::thinking::resolve(Some(&setting), "run", support)
                    .applied_choice()
                    .is_some(),
                applicable,
                "{model}"
            );
        }
    }
    #[test]
    fn metadata_is_exact_and_malformed_is_unknown() {
        let long_choice = "long".repeat(120);
        let selection = ThinkingSetting::Choice {
            value: long_choice.clone(),
        };
        for support in [
            ollama_support(
                &json!({"thinking":{"values":[long_choice.clone()],"default":long_choice.clone()}}),
                "fixture",
            ),
            account_record_support(&json!({
                "supported_reasoning_levels":[{"effort":long_choice.clone()}],
                "default_reasoning_level":long_choice.clone()
            })),
        ] {
            let ThinkingSupport::Configurable {
                choices, default, ..
            } = &support
            else {
                panic!("bounded exact metadata choice was not retained");
            };
            assert_eq!(choices[0].value, long_choice);
            assert_eq!(default.as_deref(), Some(long_choice.as_str()));
            assert_eq!(
                super::super::thinking::resolve(Some(&selection), "run", support).applied_choice(),
                Some(long_choice.as_str()),
            );
        }
        assert!(matches!(
            ollama_support(
                &json!({"thinking":{"values":["fast","thorough"],"default":"fast"}}),
                "fixture"
            ),
            ThinkingSupport::Configurable { .. }
        ));
        assert!(matches!(
            ollama_support(
                &json!({"thinking":{"values":[true,false],"default":true}}),
                "fixture"
            ),
            ThinkingSupport::Unsupported { .. }
        ));
        for value in [
            json!({}),
            json!({"model":"other","thinking":{"values":["low"]}}),
            json!({"thinking":{"values":["low"],"default":"max"}}),
            json!({"thinking":{"values":["low",true]}}),
        ] {
            assert!(matches!(
                ollama_support(&value, "fixture"),
                ThinkingSupport::Unknown { .. }
            ));
        }
        assert!(matches!(
            account_record_support(&json!({"supported_reasoning_levels":[]})),
            ThinkingSupport::Unsupported { .. }
        ));
        assert!(matches!(
            account_record_support(&json!({})),
            ThinkingSupport::Unknown { .. }
        ));
    }
    #[test]
    fn adapter_does_not_guess_prefixes_or_borrow_text_for_audio() {
        assert!(matches!(
            adapter_support(
                ProviderKind::Anthropic,
                "https://api.anthropic.com/v1/messages",
                "claude-opus-4-6-extra",
                ThinkingRequestKind::Text
            ),
            ThinkingSupport::Unknown { .. }
        ));
        assert!(matches!(
            adapter_support(
                ProviderKind::OpenAi,
                "https://api.openai.com/v1/audio/speech",
                "gpt-6.1-sol",
                ThinkingRequestKind::Speech
            ),
            ThinkingSupport::Unsupported { .. }
        ));
        assert!(matches!(
            adapter_support(
                ProviderKind::OpenAi,
                "https://custom.invalid/v1/chat/completions",
                "gpt-6.1-sol",
                ThinkingRequestKind::Text
            ),
            ThinkingSupport::Unknown { .. }
        ));
    }
    #[tokio::test]
    async fn ollama_show_is_bounded_exact_and_never_follows_redirects() {
        let mut server = mockito::Server::new_async().await;
        let model = "fixture";
        let mock = server
            .mock("POST", "/api/show")
            .match_body(mockito::Matcher::Json(json!({"model":model})))
            .with_body(
                json!({"thinking":{"values":["deliberate","quick"],"default":"quick"}}).to_string(),
            )
            .create_async()
            .await;
        let result = request_support(
            ProviderKind::Ollama,
            &format!("{}/v1/chat/completions", server.url()),
            model,
            "",
            None,
            ThinkingRequestKind::Text,
        )
        .await;
        assert!(matches!(result, ThinkingSupport::Configurable { .. }));
        mock.assert_async().await;
        mock.remove_async().await;
        let mock = server
            .mock("POST", "/api/show")
            .with_status(302)
            .with_header("location", "/redirect")
            .create_async()
            .await;
        let redirected = server
            .mock("POST", "/redirect")
            .expect(0)
            .create_async()
            .await;
        assert!(matches!(
            request_support(
                ProviderKind::Ollama,
                &format!("{}/v1/chat/completions", server.url()),
                model,
                "",
                None,
                ThinkingRequestKind::Text
            )
            .await,
            ThinkingSupport::Unknown { .. }
        ));
        mock.assert_async().await;
        redirected.assert_async().await;
    }
}
