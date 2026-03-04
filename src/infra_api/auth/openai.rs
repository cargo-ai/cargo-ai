use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Deserialize)]
pub struct LoginStartResponse {
    pub login_url: String,
    pub login_id: String,

    #[serde(default)]
    pub poll_interval_seconds: Option<u64>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct SessionCredentials {
    pub access_token: String,

    #[serde(default)]
    pub refresh_token: Option<String>,

    #[serde(default)]
    pub expires_in: Option<i32>,
}

#[derive(Debug, Deserialize)]
pub struct LoginPollResponse {
    #[serde(default)]
    pub status: Option<String>,

    #[serde(default)]
    pub message: Option<String>,

    #[serde(default)]
    pub credentials: Option<SessionCredentials>,
}

#[derive(Debug, Deserialize)]
pub struct SessionStatusResponse {
    #[serde(default)]
    pub session: Option<SessionStatusSession>,

    #[serde(default)]
    pub message: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct SessionStatusSession {
    #[serde(default)]
    pub access_token: Option<String>,

    #[serde(default)]
    pub refresh_token: Option<String>,

    #[serde(default)]
    pub expires_in_seconds: Option<i32>,
}

#[derive(Debug, Serialize)]
struct LoginStartRequest {
    action: &'static str,
}

#[derive(Debug, Serialize)]
struct LoginPollRequest<'a> {
    action: &'static str,
    login_id: &'a str,
}

#[derive(Debug, Serialize)]
struct SessionStatusRequest<'a> {
    action: &'static str,
    credentials: SessionStatusCredentials<'a>,
}

#[derive(Debug, Serialize)]
struct SessionStatusCredentials<'a> {
    access_token: &'a str,

    #[serde(skip_serializing_if = "Option::is_none")]
    refresh_token: Option<&'a str>,
}

#[derive(Debug, Serialize)]
struct LogoutRequest<'a> {
    action: &'static str,
    revoke: bool,
    credentials: SessionStatusCredentials<'a>,
}

async fn post_json(base_url: &str, body: &impl Serialize) -> Result<Value, String> {
    let url = format!("{}/auth/openai", base_url.trim_end_matches('/'));
    let response = reqwest::Client::new()
        .post(url)
        .header("Content-Type", "application/json")
        .json(body)
        .send()
        .await
        .map_err(|error| format!("request failed: {error}"))?;

    response
        .json::<Value>()
        .await
        .map_err(|error| format!("failed to parse response JSON: {error}"))
}

pub async fn start_login(base_url: &str) -> Result<LoginStartResponse, String> {
    let body = LoginStartRequest {
        action: "login_start",
    };
    let value = post_json(base_url, &body).await?;

    serde_json::from_value::<LoginStartResponse>(value)
        .map_err(|error| format!("invalid login_start response payload: {error}"))
}

pub async fn poll_login(base_url: &str, login_id: &str) -> Result<LoginPollResponse, String> {
    let body = LoginPollRequest {
        action: "login_poll",
        login_id,
    };
    let value = post_json(base_url, &body).await?;

    serde_json::from_value::<LoginPollResponse>(value)
        .map_err(|error| format!("invalid login_poll response payload: {error}"))
}

pub async fn session_status(
    base_url: &str,
    access_token: &str,
    refresh_token: Option<&str>,
) -> Result<SessionStatusResponse, String> {
    let body = SessionStatusRequest {
        action: "session_status",
        credentials: SessionStatusCredentials {
            access_token,
            refresh_token,
        },
    };
    let value = post_json(base_url, &body).await?;

    serde_json::from_value::<SessionStatusResponse>(value)
        .map_err(|error| format!("invalid session_status response payload: {error}"))
}

pub async fn logout(
    base_url: &str,
    access_token: &str,
    refresh_token: Option<&str>,
    revoke: bool,
) -> Result<Value, String> {
    let body = LogoutRequest {
        action: "logout",
        revoke,
        credentials: SessionStatusCredentials {
            access_token,
            refresh_token,
        },
    };

    post_json(base_url, &body).await
}
