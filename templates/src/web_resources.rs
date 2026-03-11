use crate::config::loader::load_config;
use crate::config::schema::WebResources as WebResourcesConfig;
use futures::future::join_all;
use reqwest::header::{ACCEPT, HeaderMap, HeaderValue, USER_AGENT};
use reqwest::{Client, StatusCode};
use tokio::time::{Duration, sleep};

const DEFAULT_MAX_ATTEMPTS: u32 = 3;
const MAX_MAX_ATTEMPTS: u32 = 5;
const DEFAULT_BASE_BACKOFF_MS: u64 = 500;
const MAX_BASE_BACKOFF_MS: u64 = 5_000;
const WEB_RESOURCE_ACCEPT_HEADER: &str =
    "text/html,application/xhtml+xml,application/xml;q=0.9,text/plain;q=0.8,*/*;q=0.5";

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
struct RetryPolicy {
    max_attempts: u32,
    base_backoff_ms: u64,
    retry_on_empty_body: bool,
}

impl RetryPolicy {
    fn from_config(config: Option<&WebResourcesConfig>) -> Self {
        let config = match config {
            Some(config) => config,
            None => {
                return Self {
                    max_attempts: DEFAULT_MAX_ATTEMPTS,
                    base_backoff_ms: DEFAULT_BASE_BACKOFF_MS,
                    retry_on_empty_body: true,
                };
            }
        };

        let max_attempts = config
            .max_attempts
            .unwrap_or(DEFAULT_MAX_ATTEMPTS)
            .clamp(1, MAX_MAX_ATTEMPTS);
        let base_backoff_ms = config
            .base_backoff_ms
            .unwrap_or(DEFAULT_BASE_BACKOFF_MS)
            .clamp(1, MAX_BASE_BACKOFF_MS);

        Self {
            max_attempts,
            base_backoff_ms,
            retry_on_empty_body: config.retry_on_empty_body.unwrap_or(true),
        }
    }
}

fn configured_retry_policy() -> RetryPolicy {
    let config = load_config();
    let web_resources = config.as_ref().and_then(|cfg| cfg.web_resources.as_ref());
    RetryPolicy::from_config(web_resources)
}

fn should_retry_status(status: StatusCode) -> bool {
    status == StatusCode::REQUEST_TIMEOUT
        || status == StatusCode::TOO_MANY_REQUESTS
        || status.is_server_error()
}

fn web_resource_user_agent() -> String {
    format!(
        "cargo-ai/{} (+https://cargo-ai.org)",
        env!("CARGO_PKG_VERSION")
    )
}

fn build_web_resource_client() -> Result<Client, String> {
    let mut headers = HeaderMap::new();
    let user_agent = HeaderValue::from_str(&web_resource_user_agent())
        .map_err(|error| format!("Failed to build web-resource User-Agent header: {error}"))?;
    headers.insert(USER_AGENT, user_agent);
    headers.insert(ACCEPT, HeaderValue::from_static(WEB_RESOURCE_ACCEPT_HEADER));

    Client::builder()
        .default_headers(headers)
        .build()
        .map_err(|error| format!("Failed to build web-resource HTTP client: {error}"))
}

fn truncate_error_body(body: &str) -> String {
    let normalized = body.replace('\n', " ").replace('\r', " ");
    let trimmed = normalized.trim();
    const MAX_LEN: usize = 200;
    if trimmed.len() <= MAX_LEN {
        trimmed.to_string()
    } else {
        format!("{}...", &trimmed[..MAX_LEN])
    }
}

async fn maybe_backoff(attempt: u32, policy: RetryPolicy) {
    let delay = policy.base_backoff_ms.saturating_mul(attempt as u64);
    sleep(Duration::from_millis(delay)).await;
}

fn request_shape_hint(status: StatusCode) -> Option<&'static str> {
    match status {
        StatusCode::FORBIDDEN | StatusCode::NOT_ACCEPTABLE | StatusCode::UNSUPPORTED_MEDIA_TYPE => {
            Some(
                " The server rejected Cargo AI's HTTP request shape. If the URL works in curl or a browser, the endpoint may expect different request headers or browser-only behavior.",
            )
        }
        _ => None,
    }
}

fn format_http_error(status: StatusCode, url: &str, body: &str, retryable: bool) -> String {
    let mut message = if retryable {
        format!(
            "Retryable HTTP error {} for '{}': {}",
            status,
            url,
            truncate_error_body(body)
        )
    } else {
        format!("HTTP error {} for '{}': {}", status, url, truncate_error_body(body))
    };

    if let Some(hint) = request_shape_hint(status) {
        message.push_str(hint);
    }

    message
}

async fn fetch_resource_with_policy(
    client: &Client,
    url: &str,
    policy: RetryPolicy,
) -> Result<String, String> {
    for attempt in 1..=policy.max_attempts {
        let response = match client.get(url).send().await {
            Ok(response) => response,
            Err(error) => {
                if attempt < policy.max_attempts {
                    maybe_backoff(attempt, policy).await;
                    continue;
                }
                return Err(format!(
                    "Failed to fetch URL '{}' after {} attempts: {}",
                    url, policy.max_attempts, error
                ));
            }
        };

        let status = response.status();
        let body = match response.text().await {
            Ok(body) => body,
            Err(error) => {
                if attempt < policy.max_attempts {
                    maybe_backoff(attempt, policy).await;
                    continue;
                }
                return Err(format!(
                    "Failed to read response body from '{}' after {} attempts: {}",
                    url, policy.max_attempts, error
                ));
            }
        };

        if !status.is_success() {
            if should_retry_status(status) && attempt < policy.max_attempts {
                maybe_backoff(attempt, policy).await;
                continue;
            }

            if should_retry_status(status) {
                return Err(format!(
                    "{} after {} attempts.",
                    format_http_error(status, url, &body, true),
                    policy.max_attempts
                ));
            }

            return Err(format_http_error(status, url, &body, false));
        }

        if policy.retry_on_empty_body && body.trim().is_empty() {
            if attempt < policy.max_attempts {
                maybe_backoff(attempt, policy).await;
                continue;
            }

            return Err(format!(
                "Received empty body from '{}' after {} attempts.",
                url, policy.max_attempts
            ));
        }

        return Ok(body);
    }

    Err("Unexpected retry loop exit while fetching web resources.".to_string())
}

async fn fetch_resources_parallel_with_policy(
    urls: &[&str],
    policy: RetryPolicy,
) -> Result<Vec<String>, String> {
    let client = build_web_resource_client()?;
    let futures = urls
        .iter()
        .map(|url| fetch_resource_with_policy(&client, url, policy));
    let results = join_all(futures).await;
    results.into_iter().collect()
}

/// Fetch multiple web resources concurrently.
/// Returns a vector of response bodies as strings.
pub async fn fetch_resources_parallel(urls: &[&str]) -> Result<Vec<String>, String> {
    fetch_resources_parallel_with_policy(urls, configured_retry_policy()).await
}

#[cfg(test)]
mod tests {
    use super::{RetryPolicy, fetch_resources_parallel_with_policy};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;
    use tokio::task::JoinHandle;

    fn policy(max_attempts: u32, base_backoff_ms: u64, retry_on_empty_body: bool) -> RetryPolicy {
        RetryPolicy {
            max_attempts,
            base_backoff_ms,
            retry_on_empty_body,
        }
    }

    fn http_response(status_line: &str, body: &str) -> String {
        format!(
            "HTTP/1.1 {}\r\nContent-Length: {}\r\nContent-Type: text/plain\r\nConnection: close\r\n\r\n{}",
            status_line,
            body.as_bytes().len(),
            body
        )
    }

    async fn read_http_request(socket: &mut tokio::net::TcpStream) -> String {
        let mut request = Vec::new();
        let mut buffer = [0_u8; 1024];

        loop {
            let bytes_read = socket.read(&mut buffer).await.expect("read request");
            if bytes_read == 0 {
                break;
            }
            request.extend_from_slice(&buffer[..bytes_read]);
            if request.windows(4).any(|window| window == b"\r\n\r\n") {
                break;
            }
        }

        String::from_utf8(request).expect("request should be utf8")
    }

    async fn spawn_sequence_server(responses: Vec<String>) -> (String, JoinHandle<Vec<String>>) {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind test server");
        let addr = listener.local_addr().expect("local addr");

        let handle = tokio::spawn(async move {
            let mut requests = Vec::new();
            for response in responses {
                let (mut socket, _) = listener.accept().await.expect("accept connection");
                requests.push(read_http_request(&mut socket).await);
                let _ = socket.write_all(response.as_bytes()).await;
            }
            requests
        });

        (format!("http://{}/resource", addr), handle)
    }

    #[tokio::test]
    async fn retries_transient_status_and_succeeds() {
        let (url, server_task) = spawn_sequence_server(vec![
            http_response("503 Service Unavailable", "warmup"),
            http_response("200 OK", "ready"),
        ])
        .await;

        let result = fetch_resources_parallel_with_policy(&[url.as_str()], policy(3, 1, true))
            .await
            .expect("resource fetch should succeed");

        assert_eq!(result, vec!["ready"]);
        server_task.await.expect("server task");
    }

    #[tokio::test]
    async fn retries_empty_body_when_enabled() {
        let (url, server_task) = spawn_sequence_server(vec![
            http_response("200 OK", "   "),
            http_response("200 OK", "payload"),
        ])
        .await;

        let result = fetch_resources_parallel_with_policy(&[url.as_str()], policy(3, 1, true))
            .await
            .expect("resource fetch should succeed");

        assert_eq!(result, vec!["payload"]);
        server_task.await.expect("server task");
    }

    #[tokio::test]
    async fn fails_fast_on_non_retryable_status() {
        let (url, server_task) =
            spawn_sequence_server(vec![http_response("401 Unauthorized", "auth failed")]).await;

        let error = fetch_resources_parallel_with_policy(&[url.as_str()], policy(3, 1, true))
            .await
            .expect_err("resource fetch should fail");

        assert!(error.contains("HTTP error 401"));
        server_task.await.expect("server task");
    }

    #[tokio::test]
    async fn sends_cargo_ai_web_fetch_headers() {
        let (url, server_task) =
            spawn_sequence_server(vec![http_response("200 OK", "payload")]).await;

        let result = fetch_resources_parallel_with_policy(&[url.as_str()], policy(1, 1, true))
            .await
            .expect("resource fetch should succeed");

        assert_eq!(result, vec!["payload"]);

        let requests = server_task.await.expect("server task");
        let request = requests.first().expect("captured request");
        let request_lower = request.to_ascii_lowercase();

        assert!(request_lower.contains("user-agent: cargo-ai/"));
        assert!(request_lower.contains(
            "accept: text/html,application/xhtml+xml,application/xml;q=0.9,text/plain;q=0.8,*/*;q=0.5"
        ));
    }

    #[tokio::test]
    async fn adds_request_shape_hint_for_header_sensitive_statuses() {
        let (url, server_task) =
            spawn_sequence_server(vec![http_response("403 Forbidden", "blocked")]).await;

        let error = fetch_resources_parallel_with_policy(&[url.as_str()], policy(1, 1, true))
            .await
            .expect_err("resource fetch should fail");

        assert!(error.contains("HTTP error 403"));
        assert!(error.contains("request shape"));
        assert!(error.contains("curl or a browser"));

        server_task.await.expect("server task");
    }
}
