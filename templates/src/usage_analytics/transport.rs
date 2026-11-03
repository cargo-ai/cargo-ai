//! Bounded authenticated account transport; diagnostics never expose response bodies.
use super::elastic::{Request, MAX_ACCOUNT_BYTES};
use serde_json::{json, Value};
use std::time::Duration;

#[derive(Debug)]
pub(crate) struct UploadFailure {
    pub message: String,
    pub retryable: bool,
    pub retry_at_ms: Option<i64>,
    pub invalid_payload: bool,
    pub unknown: bool,
}
impl From<&str> for UploadFailure {
    fn from(message: &str) -> Self {
        Self {
            message: message.into(),
            retryable: false,
            retry_at_ms: None,
            invalid_payload: false,
            unknown: true,
        }
    }
}
impl From<String> for UploadFailure {
    fn from(message: String) -> Self {
        Self::from(message.as_str())
    }
}
impl UploadFailure {
    fn transient(message: &str, retry_at_ms: Option<i64>) -> Self {
        Self {
            retryable: true,
            retry_at_ms,
            ..Self::from(message)
        }
    }
    fn rejected(result: &Value, retry_at_ms: Option<i64>) -> Self {
        let invalid_payload = matches!(
            result["type"].as_str(),
            Some("usage_analytics_invalid_request" | "invalid_request")
        ) || result["outcome"] == "invalid_request";
        let retryable = !invalid_payload
            && (result["retryable"] == true
                || matches!(
                    result["outcome"].as_str(),
                    Some("rate_limited" | "concurrency_limited")
                ));
        Self {
            retryable,
            retry_at_ms,
            invalid_payload,
            unknown: !invalid_payload,
            ..Self::from(
                "Usage cloud operation was rejected; original history and pending payloads remain preserved",
            )
        }
    }
}
fn retry_at(value: &Value, header: Option<i64>) -> Option<i64> {
    [
        header,
        value["retry_at_ms"].as_i64().filter(|n| *n >= 0),
        value["retry_after_ms"]
            .as_u64()
            .map(|delay| super::now_ms().saturating_add(delay.min(i64::MAX as u64) as i64)),
    ]
    .into_iter()
    .flatten()
    .max()
}
fn retry_after_header(value: &str) -> Option<i64> {
    if let Ok(seconds) = value.trim().parse::<u64>() {
        return Some(
            super::now_ms()
                .saturating_add(seconds.saturating_mul(1000).min(i64::MAX as u64) as i64),
        );
    }
    time::OffsetDateTime::parse(value, &time::format_description::well_known::Rfc2822)
        .ok()
        .and_then(|date| i64::try_from(date.unix_timestamp_nanos() / 1_000_000).ok())
        .filter(|n| *n >= 0)
}

pub(crate) struct V2Transport {
    client: reqwest::Client,
    auth: crate::usage_backup::Auth,
    endpoint: String,
    snapshot: String,
    refreshed: bool,
    check_credentials: bool,
    timeout: Duration,
    upload_timeout: Duration,
}
impl V2Transport {
    pub(crate) fn new(timeout: Duration) -> Result<Self, String> {
        let auth = crate::usage_backup_host::load_auth()?;
        #[cfg(test)]
        if let Some(endpoint) = super::TEST_ENDPOINT.with(|value| value.borrow().clone()) {
            return Self::at(auth, timeout, endpoint, true);
        }
        Self::at(
            auth,
            timeout,
            "https://api.cargo-ai.org/account".into(),
            true,
        )
    }
    fn at(
        auth: crate::usage_backup::Auth,
        timeout: Duration,
        endpoint: String,
        check_credentials: bool,
    ) -> Result<Self, String> {
        let client = reqwest::Client::builder()
            // Protocol NACK replay must not bypass the persisted upload budget.
            .http1_only()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(timeout)
            .connect_timeout(timeout.min(Duration::from_secs(3)))
            .build()
            .map_err(|_| "Cannot initialize usage cloud transport")?;
        Ok(Self {
            client,
            snapshot: auth.access_token.clone(),
            auth,
            endpoint,
            refreshed: false,
            check_credentials,
            timeout,
            upload_timeout: timeout,
        })
    }
    pub(crate) fn with_upload_timeout(mut self, timeout: Duration) -> Self {
        self.upload_timeout = timeout;
        self
    }
    fn unchanged(&self) -> Result<(), String> {
        if super::scheduler::canceled() {
            return Err("Usage cloud operation canceled; pending payloads preserved".into());
        }
        if self.check_credentials
            && crate::usage_backup_host::load_auth()?.access_token != self.snapshot
        {
            return Err(
                "Account credentials changed; pending usage remains bound to its original account"
                    .into(),
            );
        }
        Ok(())
    }
    async fn send_bytes(
        &self,
        body: String,
        timeout: Option<Duration>,
    ) -> Result<Value, UploadFailure> {
        self.unchanged()?;
        if body.len() > MAX_ACCOUNT_BYTES {
            return Err("Usage cloud authenticated envelope exceeds its limit".into());
        }
        let mut request = self
            .client
            .post(&self.endpoint)
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .body(body);
        if let Some(timeout) = timeout {
            request = request.timeout(timeout);
        }
        let mut response = request.send().await.map_err(|_| {
            UploadFailure::transient(
                "Usage cloud connection failed; submitted outcomes remain unknown",
                None,
            )
        })?;
        let status = response.status();
        let header = response
            .headers()
            .get(reqwest::header::RETRY_AFTER)
            .and_then(|h| h.to_str().ok())
            .and_then(retry_after_header);
        if status.is_redirection() {
            return Err("Usage cloud endpoint redirect refused".into());
        }
        if response.content_length().is_some_and(|n| n > 320 * 1024) {
            return Err("Usage cloud response exceeds its limit".into());
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|_| {
            UploadFailure::transient(
                "Usage cloud response interrupted; pending payloads preserved",
                header,
            )
        })? {
            if bytes.len() + chunk.len() > 320 * 1024 {
                return Err("Usage cloud response exceeds its limit".into());
            }
            bytes.extend_from_slice(&chunk);
        }
        let parsed = std::str::from_utf8(&bytes)
            .ok()
            .and_then(|text| super::contract::parse_exact_json(text).ok());
        let transient =
            status.as_u16() == 429 || status.as_u16() == 408 || status.is_server_error();
        let Some(mut result) = parsed.filter(Value::is_object) else {
            return Err(if transient {
                UploadFailure::transient(
                    "Usage cloud service temporarily unavailable; submitted outcomes remain unknown",
                    header,
                )
            } else {
                "Usage cloud response is invalid".into()
            });
        };
        let retry = retry_at(&result, header);
        if !status.is_success() && result["type"] != "access_token_expired" {
            let mut failure = UploadFailure::rejected(&result, retry);
            if transient && !failure.invalid_payload {
                failure.retryable = true;
            }
            return Err(failure);
        }
        // Keep response metadata outside the public result body.
        if let Some(retry) = retry {
            result["__transport_retry_at_ms"] = json!(retry);
        }
        Ok(result)
    }
    fn consent(request: &Request) -> Result<(), String> {
        if let Request::Upload {
            source, provenance, ..
        } = request
        {
            let selected = super::selected()?.ok_or("Usage cloud consent is missing")?;
            if &selected.source != source
                || &selected.provenance != provenance
                || super::local_status()?["enabled"] != true
            {
                return Err("Usage cloud consent changed; further transmission stopped".into());
            }
        }
        Ok(())
    }
    pub(crate) fn check_upload(&self, text: &str) -> Result<(), String> {
        let request = text
            .parse::<Request>()
            .map_err(|_| "Usage cloud request is invalid")?;
        Self::consent(&request)?;
        self.unchanged()
    }
    pub(crate) async fn call(&mut self, request: &Request) -> Result<Value, String> {
        let text =
            serde_json::to_string(request).map_err(|_| "Cannot encode usage cloud request")?;
        self.call_exact(&text).await
    }
    pub(crate) async fn call_exact(&mut self, text: &str) -> Result<Value, String> {
        self.call_bounded(text, true)
            .await
            .map_err(|failure| failure.message)
    }
    /// Upload replay belongs to the persisted batch budget. Refreshing credentials
    /// can prepare the next attempt, but never sends a second upload here.
    pub(crate) async fn upload_exact(&mut self, text: &str) -> Result<Value, UploadFailure> {
        self.call_bounded(text, false).await
    }
    async fn call_bounded(
        &mut self,
        text: &str,
        replay_expired: bool,
    ) -> Result<Value, UploadFailure> {
        let timeout = if replay_expired {
            self.timeout
        } else {
            self.upload_timeout
        };
        tokio::time::timeout(timeout,async {
            tokio::select! {
                biased;
                result=self.call_inner(text,replay_expired)=>result,
                ()=async {while !super::scheduler::canceled(){tokio::time::sleep(Duration::from_millis(25)).await;}}=>Err("Usage cloud operation canceled; submitted outcomes remain unknown".into()),
            }
        }).await.map_err(|_|UploadFailure::transient("Usage cloud request deadline elapsed; submitted outcomes remain unknown",None))?
    }
    async fn call_inner(
        &mut self,
        text: &str,
        replay_expired: bool,
    ) -> Result<Value, UploadFailure> {
        if text.len() > MAX_ACCOUNT_BYTES {
            return Err("Usage cloud request exceeds its limit".into());
        }
        let request = text
            .parse::<Request>()
            .map_err(|_| UploadFailure::from("Usage cloud request is invalid"))?;
        Self::consent(&request)?;
        self.unchanged()?;
        let request_timeout = if replay_expired {
            None
        } else {
            Some(self.upload_timeout)
        };
        let mut result = self.send_exact(text, request_timeout).await?;
        if result["type"] == "access_token_expired" && !self.refreshed {
            Self::consent(&request)?;
            self.unchanged()?;
            self.refreshed = true;
            let refresh = self
                .auth
                .refresh_token
                .as_deref()
                .ok_or("Sign in explicitly to resume usage cloud")?;
            let body=json!({"action":"status","credentials":{"access_token":self.auth.access_token,"refresh_token":refresh},"session_policy":{"allow_refresh":true}}).to_string();
            let refreshed = self.send_bytes(body, None).await?;
            if refreshed["status"] != "success" {
                return Err("Usage cloud session refresh failed; sign in explicitly".into());
            }
            let token = refreshed["session"]["access_token"]
                .as_str()
                .ok_or("Usage cloud session refresh failed; sign in explicitly")?
                .to_owned();
            self.unchanged()?;
            Self::consent(&request)?;
            self.auth.access_token = token;
            if !replay_expired {
                return Err(UploadFailure::transient(
                    "Usage cloud session renewed; exact upload remains pending",
                    None,
                ));
            }
            result = self.send_exact(text, request_timeout).await?;
        }
        let header = result["__transport_retry_at_ms"].as_i64();
        if result["status"] == "success" && result["type"] == "usage_analytics_succeeded" {
            let body = result
                .get("usage_analytics")
                .filter(|value| value.is_object())
                .cloned()
                .ok_or("Usage cloud result is missing")?;
            if matches!(
                body["outcome"].as_str(),
                Some("rate_limited" | "concurrency_limited" | "invalid_request")
            ) || body["retryable"] == true
            {
                return Err(UploadFailure::rejected(&body, retry_at(&body, header)));
            }
            return Ok(body);
        }
        Err(UploadFailure::rejected(&result, retry_at(&result, header)))
    }
    async fn send_exact(
        &self,
        text: &str,
        timeout: Option<Duration>,
    ) -> Result<Value, UploadFailure> {
        let credentials = serde_json::to_string(&json!({"access_token":self.auth.access_token}))
            .map_err(|_| "Cannot encode usage cloud credentials")?;
        self.send_bytes(format!("{{\"action\":\"usage_analytics\",\"credentials\":{credentials},\"usage_analytics\":{text}}}"), timeout).await
    }
    #[cfg(test)]
    fn test_at(endpoint: String) -> Self {
        Self::at(
            crate::usage_backup::Auth {
                access_token: "fixture-access".into(),
                refresh_token: None,
            },
            Duration::from_secs(2),
            endpoint,
            false,
        )
        .unwrap()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn cloud_transport_is_closed_bounded_and_redacts_failures() {
        let mut server = mockito::Server::new_async().await;
        let request=server.mock("POST","/account").match_body(mockito::Matcher::Json(json!({"action":"usage_analytics","credentials":{"access_token":"fixture-access"},"usage_analytics":{"command":"status","protocol_version":2}})))
            .with_status(500).with_body(r#"{"status":"failed","message":"fixture-access private body"}"#).expect(1).create_async().await;
        let error = V2Transport::test_at(format!("{}/account", server.url()))
            .call(&Request::Status {
                protocol_version: super::super::elastic::Version,
            })
            .await
            .unwrap_err();
        assert!(!error.contains("fixture-access"));
        assert!(!error.contains("private body"));
        request.assert_async().await;
        let redirect = server
            .mock("POST", "/account")
            .with_status(302)
            .with_header("location", "http://127.0.0.1:1/leak")
            .expect(1)
            .create_async()
            .await;
        assert!(V2Transport::test_at(format!("{}/account", server.url()))
            .call(&Request::Status {
                protocol_version: super::super::elastic::Version
            })
            .await
            .unwrap_err()
            .contains("redirect refused"));
        redirect.assert_async().await;
    }
}

#[cfg(test)]
mod v2_tests {
    use super::*;
    fn status() -> Request {
        Request::Status {
            protocol_version: super::super::elastic::Version,
        }
    }
    #[tokio::test]
    async fn upload_budget_overrides_client_default_without_extending_controls_or_refresh() {
        let mut server = mockito::Server::new_async().await;
        let delayed = server.mock("POST", "/account")
            .with_body_from_request(|_| {
                std::thread::sleep(Duration::from_millis(60));
                json!({"status":"success","type":"usage_analytics_succeeded","usage_analytics":{"outcome":"accepted"}}).to_string().into_bytes()
            }).expect(2).create_async().await;
        let mut client = V2Transport::at(
            crate::usage_backup::Auth {
                access_token: "fixture-access".into(),
                refresh_token: None,
            },
            Duration::from_millis(20),
            format!("{}/account", server.url()),
            false,
        )
        .unwrap()
        .with_upload_timeout(Duration::from_millis(200));
        let text = serde_json::to_string(&status()).unwrap();
        assert_eq!(
            client.upload_exact(&text).await.unwrap()["outcome"],
            "accepted"
        );
        assert!(client.call(&status()).await.is_err());
        delayed.assert_async().await;
        delayed.remove_async().await;
        let expired = server
            .mock("POST", "/account")
            .match_body(mockito::Matcher::PartialJson(
                json!({"action":"usage_analytics"}),
            ))
            .with_body(json!({"type":"access_token_expired"}).to_string())
            .expect(1)
            .create_async()
            .await;
        let refresh = server
            .mock("POST", "/account")
            .match_body(mockito::Matcher::PartialJson(json!({"action":"status"})))
            .with_body_from_request(|_| {
                std::thread::sleep(Duration::from_millis(60));
                json!({"status":"success","session":{"access_token":"new-token"}})
                    .to_string()
                    .into_bytes()
            })
            .expect(1)
            .create_async()
            .await;
        client.auth.refresh_token = Some("fixture-refresh".into());
        let started = std::time::Instant::now();
        assert!(client.upload_exact(&text).await.is_err());
        assert!(started.elapsed() < Duration::from_millis(180));
        assert!(client.refreshed);
        assert_eq!(client.auth.access_token, "fixture-access");
        expired.assert_async().await;
        refresh.assert_async().await;
    }
    #[tokio::test]
    async fn response_duplicate_fields_size_and_unknown_fail_without_retry() {
        let mut server = mockito::Server::new_async().await;
        for body in [
            r#"{"status":"success","type":"usage_analytics_succeeded","usage_analytics":{"protocol_version":2,"protocol_version":2}}"#.to_owned(),
            "x".repeat(320 * 1024 + 1),
            r#"{"status":"failed","type":"ambiguous_failure","session":{"access_token":"secret-response-token"}}"#.to_owned(),
        ] {
            let request = server.mock("POST", "/account").with_body(body).expect(1).create_async().await;
            let mut client = V2Transport::test_at(format!("{}/account", server.url()));
            client.auth.refresh_token = Some("secret-refresh-token".into());
            let error = client.call(&status()).await.unwrap_err();
            assert!(!error.contains("secret-"));
            assert!(!client.refreshed);
            request.assert_async().await; request.remove_async().await;
        }
    }
    #[tokio::test]
    async fn actual_auth_envelope_is_bounded_before_network() {
        let mut client = V2Transport::test_at("http://127.0.0.1:1/account".into());
        client.auth.access_token = "a".repeat(MAX_ACCOUNT_BYTES);
        assert!(client
            .call(&status())
            .await
            .unwrap_err()
            .contains("authenticated envelope"));
    }
    #[tokio::test]
    async fn total_call_budget_includes_refresh_and_only_expiry_refreshes_once() {
        let mut server = mockito::Server::new_async().await;
        let expired = server
            .mock("POST", "/account")
            .match_body(mockito::Matcher::PartialJson(
                json!({"action":"usage_analytics"}),
            ))
            .with_body(json!({"status":"failed","type":"access_token_expired"}).to_string())
            .expect(2)
            .create_async()
            .await;
        let refresh = server
            .mock("POST", "/account")
            .match_body(mockito::Matcher::PartialJson(json!({"action":"status"})))
            .with_body(
                json!({"status":"success","session":{"access_token":"new-token"}}).to_string(),
            )
            .expect(1)
            .create_async()
            .await;
        let mut client = V2Transport::test_at(format!("{}/account", server.url()));
        client.auth.refresh_token = Some("fixture-refresh".into());
        assert!(client.call(&status()).await.is_err());
        assert!(client.refreshed);
        expired.assert_async().await;
        refresh.assert_async().await;
        expired.remove_async().await;
        refresh.remove_async().await;
        let slow = server
            .mock("POST", "/account")
            .with_body_from_request(|_| {
                std::thread::sleep(Duration::from_millis(100));
                b"{}".to_vec()
            })
            .expect(1)
            .create_async()
            .await;
        let mut client = V2Transport::test_at(format!("{}/account", server.url()));
        client.timeout = Duration::from_millis(20);
        let started = std::time::Instant::now();
        assert!(client
            .call(&status())
            .await
            .unwrap_err()
            .contains("deadline"));
        assert!(started.elapsed() < Duration::from_secs(1));
        slow.assert_async().await;
    }
}

#[cfg(test)]
mod retry_tests {
    use super::*;
    #[test]
    fn retry_after_accepts_seconds_and_http_dates_without_shortening_body_timing() {
        let before = super::super::now_ms();
        let seconds = retry_after_header("60").unwrap();
        assert!((before + 60_000..=super::super::now_ms() + 60_000).contains(&seconds));
        let date = time::OffsetDateTime::now_utc() + time::Duration::seconds(90);
        let text = date
            .format(&time::format_description::well_known::Rfc2822)
            .unwrap();
        assert_eq!(
            retry_after_header(&text),
            Some(date.unix_timestamp() * 1000)
        );
        assert_eq!(
            retry_at(
                &json!({"retry_at_ms":before+120_000,"retry_after_ms":30_000}),
                Some(seconds)
            ),
            Some(before + 120_000)
        );
        assert!(retry_after_header("invalid private diagnostic").is_none());
    }
    #[test]
    fn terminal_invalid_payload_overrides_a_retryable_flag() {
        let failure = UploadFailure::rejected(
            &json!({"type":"usage_analytics_invalid_request","retryable":true}),
            None,
        );
        assert!(failure.invalid_payload);
        assert!(!failure.retryable);
        for outcome in ["rate_limited", "concurrency_limited"] {
            assert!(UploadFailure::rejected(&json!({"outcome":outcome}), None).retryable);
        }
        assert!(
            !UploadFailure::rejected(&json!({"type":"usage_analytics_unavailable"}), None)
                .retryable
        );
        assert!(
            UploadFailure::rejected(
                &json!({"type":"usage_analytics_pending","retryable":true}),
                None
            )
            .retryable
        );
    }
}
