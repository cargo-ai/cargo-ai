//! Bounded authenticated account transport; diagnostics never expose response bodies.
use super::contract::{Request, MAX_BATCH_BYTES};
use serde_json::{json, Value};
use std::time::Duration;

pub(crate) struct Transport {
    client: reqwest::Client,
    auth: crate::usage_backup::Auth,
    endpoint: String,
    snapshot: String,
    refreshed: bool,
    check_credentials: bool,
    conflict: bool,
}
impl Transport {
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
            conflict: false,
        })
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
    async fn send(&self, body: Value) -> Result<Value, String> {
        let analytics = body["action"] == "usage_analytics";
        self.send_bytes(
            serde_json::to_string(&body).map_err(|_| "Cannot encode usage cloud request")?,
            analytics,
        )
        .await
    }
    async fn send_bytes(&self, body: String, analytics: bool) -> Result<Value, String> {
        self.unchanged()?;
        let mut response = self
            .client
            .post(&self.endpoint)
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .body(body)
            .send()
            .await
            .map_err(|_| "Usage cloud connection failed; submitted outcomes remain unknown")?;
        if response.status().is_redirection() {
            return Err("Usage cloud endpoint redirect refused".into());
        }
        if response.content_length().is_some_and(|n| n > 320 * 1024) {
            return Err("Usage cloud response exceeds its limit".into());
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| "Usage cloud response interrupted; pending payloads preserved")?
        {
            if bytes.len() + chunk.len() > 320 * 1024 {
                return Err("Usage cloud response exceeds its limit".into());
            }
            bytes.extend_from_slice(&chunk);
        }
        if analytics {
            let text =
                std::str::from_utf8(&bytes).map_err(|_| "Usage cloud response is invalid")?;
            super::contract::parse_account_document(text)
                .map_err(|_| "Usage cloud response is invalid".into())
        } else {
            serde_json::from_slice(&bytes).map_err(|_| "Usage cloud response is invalid".into())
        }
    }
    fn consent(request: &Request) -> Result<(), String> {
        if let Request::Sync {
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
    pub(crate) async fn call(&mut self, request: &Request) -> Result<Value, String> {
        let text =
            serde_json::to_string(request).map_err(|_| "Cannot encode usage cloud request")?;
        self.call_exact(&text).await
    }
    /// The frozen request is embedded verbatim, including on an authenticated
    /// expired-token retry. No stored event or request bytes are regenerated.
    pub(crate) async fn call_exact(&mut self, text: &str) -> Result<Value, String> {
        self.conflict = false;
        if text.len() >= MAX_BATCH_BYTES {
            return Err("Usage cloud request exceeds its limit".into());
        }
        let request = Request::parse(text).map_err(|_| "Usage cloud request is invalid")?;
        Self::consent(&request)?;
        self.unchanged()?;
        let mut result = self.send_exact(text).await?;
        if result["type"] == "access_token_expired" && !self.refreshed {
            Self::consent(&request)?;
            self.unchanged()?;
            self.refreshed = true;
            let refresh = self
                .auth
                .refresh_token
                .as_deref()
                .ok_or("Sign in explicitly to resume usage cloud")?;
            let refreshed=self.send(json!({"action":"status","credentials":{"access_token":self.auth.access_token,"refresh_token":refresh},"session_policy":{"allow_refresh":true}})).await?;
            let token = refreshed["session"]["access_token"]
                .as_str()
                .ok_or("Usage cloud session refresh failed; sign in explicitly")?
                .to_owned();
            self.unchanged()?;
            Self::consent(&request)?;
            self.auth.access_token = token;
            result = self.send_exact(text).await?;
        }
        if result["status"] == "success" && result["type"] == "usage_analytics_succeeded" {
            return result
                .get("usage_analytics")
                .filter(|value| value.is_object())
                .cloned()
                .ok_or("Usage cloud result is missing".into());
        }
        self.conflict = matches!(
            result["type"].as_str(),
            Some("usage_analytics_conflict" | "usage_analytics_event_conflict")
        );
        Err("Usage cloud operation was rejected; original history and pending payloads remain preserved".into())
    }
    async fn send_exact(&self, text: &str) -> Result<Value, String> {
        let credentials = serde_json::to_string(&json!({"access_token":self.auth.access_token}))
            .map_err(|_| "Cannot encode usage cloud credentials")?;
        self.send_bytes(format!("{{\"action\":\"usage_analytics\",\"credentials\":{credentials},\"usage_analytics\":{text}}}"),true).await
    }
    pub(crate) fn terminal_conflict(&self) -> bool {
        self.conflict
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
        let request=server.mock("POST","/account").match_body(mockito::Matcher::Json(json!({"action":"usage_analytics","credentials":{"access_token":"fixture-access"},"usage_analytics":{"command":"status","protocol_version":1}})))
            .with_status(500).with_body(r#"{"status":"failed","message":"fixture-access private body"}"#).expect(1).create_async().await;
        let error = Transport::test_at(format!("{}/account", server.url()))
            .call(&Request::Status {
                protocol_version: super::super::contract::Version,
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
        assert!(Transport::test_at(format!("{}/account", server.url()))
            .call(&Request::Status {
                protocol_version: super::super::contract::Version
            })
            .await
            .unwrap_err()
            .contains("redirect refused"));
        redirect.assert_async().await;
    }
}
