//! Bounded account API transport. Errors never include bodies or credentials.
use serde_json::{json, Value};
use std::time::Duration;

pub(crate) struct Auth {
    pub(crate) access_token: String,
    pub(crate) refresh_token: Option<String>,
}
pub(super) struct Transport {
    client: reqwest::Client,
    auth: Auth,
    endpoint: String,
    refreshed: bool,
    credential_snapshot: String,
}
impl Transport {
    pub(super) fn new(auth: Auth, timeout: Duration) -> Result<Self, String> {
        Self::at(auth, timeout, "https://api.cargo-ai.org/account".into())
    }
    pub(super) fn at(auth: Auth, timeout: Duration, endpoint: String) -> Result<Self, String> {
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(timeout)
            .connect_timeout(timeout.min(Duration::from_secs(3)))
            .build()
            .map_err(|_| "Cannot initialize backup transport")?;
        let credential_snapshot = auth.access_token.clone();
        Ok(Self {
            client,
            auth,
            credential_snapshot,
            endpoint,
            refreshed: false,
        })
    }
    fn check_account_unchanged(&self) -> Result<(), String> {
        if crate::usage_backup_host::load_auth()?.access_token != self.credential_snapshot {
            return Err(
                "Account credentials changed during backup; further transmission stopped".into(),
            );
        }
        Ok(())
    }
    async fn send(&self, body: &Value) -> Result<Value, String> {
        let mut response = self
            .client
            .post(&self.endpoint)
            .json(body)
            .send()
            .await
            .map_err(|_| {
                "Backup connection failed or timed out; local history and pending selections remain"
            })?;
        if response.status().is_redirection() {
            return Err("Backup endpoint redirect refused".into());
        }
        if response.content_length().is_some_and(|n| n > 320 * 1024) {
            return Err("Backup response exceeds its size limit".into());
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| "Backup response was interrupted; pending selections remain")?
        {
            if bytes.len() + chunk.len() > 320 * 1024 {
                return Err("Backup response exceeds its size limit".into());
            }
            bytes.extend_from_slice(&chunk);
        }
        serde_json::from_slice(&bytes)
            .map_err(|_| "Backup service returned an invalid response".into())
    }
    pub(super) async fn call(&mut self, payload: Value) -> Result<Value, String> {
        check_upload_consent(&payload)?;
        if payload["command"] == "ingest" {
            self.check_account_unchanged()?;
        }
        if serde_json::to_vec(&payload)
            .map_err(|_| "Cannot encode backup request")?
            .len()
            > 256 * 1024
        {
            return Err("Backup batch exceeds its size limit".into());
        }
        let mut result=self.send(&json!({"action":"usage_backup","credentials":{"access_token":self.auth.access_token},"usage_backup":payload})).await?;
        if result["type"] == "access_token_expired" && !self.refreshed {
            check_upload_consent(&payload)?;
            if payload["command"] == "ingest" {
                self.check_account_unchanged()?;
            }
            self.refreshed = true;
            let refresh = self
                .auth
                .refresh_token
                .as_deref()
                .ok_or("Account session expired; sign in explicitly to resume backup")?;
            let status=self.send(&json!({"action":"status","credentials":{"access_token":self.auth.access_token,"refresh_token":refresh},"session_policy":{"allow_refresh":true}})).await?;
            let token = status["session"]["access_token"]
                .as_str()
                .ok_or("Account session could not be refreshed; sign in explicitly")?;
            self.check_account_unchanged()?;
            // Keep this short-lived token in memory. Background backup never
            // overwrites account credentials that another process may update.
            self.auth.access_token = token.to_owned();
            check_upload_consent(&payload)?;
            if payload["command"] == "ingest" {
                self.check_account_unchanged()?;
            }
            // An explicit expired-token rejection proves the operation was not
            // admitted. No other failure causes a retry within this pass.
            result=self.send(&json!({"action":"usage_backup","credentials":{"access_token":self.auth.access_token},"usage_backup":payload})).await?;
        }
        if result["status"] == "success" && result["type"] == "usage_backup_succeeded" {
            return result
                .get("usage_backup")
                .filter(|v| v.is_object())
                .cloned()
                .ok_or("Backup service omitted its result".into());
        }
        let reason=match result["type"].as_str(){
            Some("usage_backup_account_mismatch")=>"Signed-in account does not match the selected backup; prior queues remain bound to their original account",
            Some("usage_backup_stale_generation")=>"Backup consent expired after cloud deletion; enable again and explicitly select any older history",
            Some("usage_backup_disabled")=>"Cloud backup is disabled; enable it explicitly",
            Some("usage_backup_conflict")=>"Cloud event identity conflict; original facts and local pending selections preserved",
            Some("usage_backup_invalid_request")=>"Backup metadata was rejected; local facts and pending selections preserved",
            Some("access_token_expired")=>"Account session expired; sign in explicitly to resume backup",
            _=>"Backup service did not accept this operation; local history and pending selections preserved",
        };
        Err(reason.into())
    }
    #[cfg(test)]
    pub(super) fn test_at(endpoint: String) -> Self {
        Self::at(
            Auth {
                access_token: "test-access-token".into(),
                refresh_token: None,
            },
            Duration::from_secs(2),
            endpoint,
        )
        .unwrap()
    }
}

fn check_upload_consent(payload: &Value) -> Result<(), String> {
    if payload["command"] != "ingest" {
        return Ok(());
    }
    let settings = crate::usage_store::settings()?;
    if !settings.backup_enabled
        || settings.backup_account_binding.as_deref() != payload["account_binding"].as_str()
        || settings.backup_generation != payload["generation"].as_u64()
    {
        return Err("Upload consent changed; no additional records were transmitted".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn status_contains_no_generic_metadata_and_redirects_are_refused() {
        let mut server = mockito::Server::new_async().await;
        let expected = json!({"action":"usage_backup","credentials":{"access_token":"test-access-token"},"usage_backup":{"command":"status"}});
        let request = server
            .mock("POST", "/account")
            .match_body(mockito::Matcher::Json(expected))
            .with_status(302)
            .with_header("location", "http://127.0.0.1:1/leak")
            .create_async()
            .await;
        let error = Transport::test_at(format!("{}/account", server.url()))
            .call(json!({"command":"status"}))
            .await
            .unwrap_err();
        assert_eq!(error, "Backup endpoint redirect refused");
        request.assert_async().await;
    }
    #[tokio::test]
    async fn failure_body_and_tokens_are_never_returned() {
        let mut server = mockito::Server::new_async().await;
        let request=server.mock("POST","/account").with_status(500).with_body(r#"{"status":"failed","type":"unknown","message":"test-access-token private payload"}"#).expect(1).create_async().await;
        let error = Transport::test_at(format!("{}/account", server.url()))
            .call(json!({"command":"status"}))
            .await
            .unwrap_err();
        assert!(!error.contains("test-access-token"));
        assert!(!error.contains("private payload"));
        request.assert_async().await;
    }
}
