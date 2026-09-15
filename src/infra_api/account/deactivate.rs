//! Authenticated hosted-account deactivation requests.
use serde_json::{json, Value};

pub async fn deactivate(
    base_url: &str,
    access_token: &str,
    request_deletion: bool,
    confirmation_email: Option<&str>,
) -> Result<Value, reqwest::Error> {
    let body = super::with_cargo_ai_metadata(json!({
        "action": "deactivate",
        "credentials": { "access_token": access_token },
        "deactivate": {
            "request_deletion": request_deletion,
            "confirmed": true,
            "confirmation_email": confirmation_email
        }
    }));
    reqwest::Client::new()
        .post(format!("{}/account", base_url.trim_end_matches('/')))
        .json(&body)
        .send()
        .await?
        .json()
        .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn deactivation_sends_authenticated_consent_and_returns_the_receipt() {
        let mut server = mockito::Server::new_async().await;
        let expected = server.mock("POST", "/account")
            .match_body(mockito::Matcher::PartialJson(json!({
                "action":"deactivate",
                "credentials":{"access_token":"synthetic-token"},
                "deactivate":{"request_deletion":true,"confirmed":true,"confirmation_email":"owner@example.test"}
            })))
            .with_header("content-type", "application/json")
            .with_body(r#"{"status":"success","type":"account_deactivated","deletion_requested":true}"#)
            .expect(1).create_async().await;
        let result = deactivate(
            &server.url(),
            "synthetic-token",
            true,
            Some("owner@example.test"),
        )
        .await
        .unwrap();
        assert_eq!(result["deletion_requested"], true);
        expected.assert_async().await;
    }

    #[tokio::test]
    async fn uncertain_deactivation_response_is_not_automatically_replayed() {
        let mut server = mockito::Server::new_async().await;
        let expected = server
            .mock("POST", "/account")
            .with_status(503)
            .with_body("response unavailable")
            .expect(1)
            .create_async()
            .await;
        assert!(deactivate(&server.url(), "synthetic-token", false, None)
            .await
            .is_err());
        expected.assert_async().await;
    }
}
