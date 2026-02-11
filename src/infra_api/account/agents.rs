use serde_json::{json, Value};

fn default_definition_path(definition_path: Option<&str>) -> &str {
    definition_path.unwrap_or("/")
}

/// List agents for the authenticated account or for a specific owner handle.
///
/// POST /account
/// {
///   "action": "agents",
///   "credentials": { "access_token": "<access_token>" },
///   "agents": {
///     "list": {
///       "owner_handle": "<optional>",
///       "include_archived": false
///     }
///   }
/// }
///
/// Returns the raw JSON response from the infra API (success or failure).
pub async fn list_agents(
    base_url: &str,
    access_token: &str,
    owner_handle: Option<&str>,
    include_archived: bool,
) -> Result<Value, reqwest::Error> {
    let url = format!("{}/account", base_url.trim_end_matches('/'));

    let mut list_payload = json!({
        "include_archived": include_archived,
    });

    if let Some(handle) = owner_handle {
        list_payload["owner_handle"] = json!(handle);
    }

    let body = json!({
        "action": "agents",
        "credentials": {
            "access_token": access_token
        },
        "agents": {
            "list": list_payload
        }
    });

    let client = reqwest::Client::new();
    let resp = client.post(url).json(&body).send().await?;

    match resp.json::<Value>().await {
        Ok(v) => Ok(v),
        Err(e) => Err(e),
    }
}

/// Push (upload/overwrite) an agent definition.
///
/// POST /account
/// {
///   "action": "agents",
///   "credentials": { "access_token": "<access_token>" },
///   "agents": {
///     "push": {
///       "name": "<name>",
///       "definition_path": "/",
///       "definition_json": { ... }
///     }
///   }
/// }
///
/// Returns the raw JSON response from the infra API (success or failure).
pub async fn push_agent(
    base_url: &str,
    access_token: &str,
    name: &str,
    definition_path: Option<&str>,
    definition_json: Value,
) -> Result<Value, reqwest::Error> {
    let url = format!("{}/account", base_url.trim_end_matches('/'));

    let body = json!({
        "action": "agents",
        "credentials": {
            "access_token": access_token
        },
        "agents": {
            "push": {
                "name": name,
                "definition_path": default_definition_path(definition_path),
                "definition_json": definition_json
            }
        }
    });

    let client = reqwest::Client::new();
    let resp = client.post(url).json(&body).send().await?;

    match resp.json::<Value>().await {
        Ok(v) => Ok(v),
        Err(e) => Err(e),
    }
}

/// Pull (fetch) an agent definition.
///
/// POST /account
/// {
///   "action": "agents",
///   "credentials": { "access_token": "<access_token>" },
///   "agents": {
///     "pull": {
///       "name": "<name>",
///       "owner_handle": "<optional>",
///       "definition_path": "/"
///     }
///   }
/// }
///
/// Returns the raw JSON response from the infra API (success or failure).
pub async fn pull_agent(
    base_url: &str,
    access_token: &str,
    name: &str,
    owner_handle: Option<&str>,
    definition_path: Option<&str>,
) -> Result<Value, reqwest::Error> {
    let url = format!("{}/account", base_url.trim_end_matches('/'));

    let mut pull_payload = json!({
        "name": name,
        "definition_path": default_definition_path(definition_path),
    });

    if let Some(handle) = owner_handle {
        pull_payload["owner_handle"] = json!(handle);
    }

    let body = json!({
        "action": "agents",
        "credentials": {
            "access_token": access_token
        },
        "agents": {
            "pull": pull_payload
        }
    });

    let client = reqwest::Client::new();
    let resp = client.post(url).json(&body).send().await?;

    match resp.json::<Value>().await {
        Ok(v) => Ok(v),
        Err(e) => Err(e),
    }
}

/// Set public visibility state for an agent.
///
/// POST /account
/// {
///   "action": "agents",
///   "credentials": { "access_token": "<access_token>" },
///   "agents": {
///     "visibility": {
///       "name": "<name>",
///       "definition_path": "/",
///       "is_public": true,
///       "public_from": null,
///       "public_until": null
///     }
///   }
/// }
///
/// Returns the raw JSON response from the infra API (success or failure).
pub async fn set_agent_visibility(
    base_url: &str,
    access_token: &str,
    name: &str,
    definition_path: Option<&str>,
    is_public: bool,
    public_from: Option<&str>,
    public_until: Option<&str>,
) -> Result<Value, reqwest::Error> {
    let url = format!("{}/account", base_url.trim_end_matches('/'));

    let mut visibility_payload = json!({
        "name": name,
        "definition_path": default_definition_path(definition_path),
        "is_public": is_public,
    });

    if let Some(v) = public_from {
        visibility_payload["public_from"] = json!(v);
    }

    if let Some(v) = public_until {
        visibility_payload["public_until"] = json!(v);
    }

    let body = json!({
        "action": "agents",
        "credentials": {
            "access_token": access_token
        },
        "agents": {
            "visibility": visibility_payload
        }
    });

    let client = reqwest::Client::new();
    let resp = client.post(url).json(&body).send().await?;

    match resp.json::<Value>().await {
        Ok(v) => Ok(v),
        Err(e) => Err(e),
    }
}

/// Set archive state for an agent.
///
/// POST /account
/// {
///   "action": "agents",
///   "credentials": { "access_token": "<access_token>" },
///   "agents": {
///     "archive": {
///       "name": "<name>",
///       "definition_path": "/",
///       "is_archived": true
///     }
///   }
/// }
///
/// Returns the raw JSON response from the infra API (success or failure).
pub async fn set_agent_archive(
    base_url: &str,
    access_token: &str,
    name: &str,
    definition_path: Option<&str>,
    is_archived: bool,
) -> Result<Value, reqwest::Error> {
    let url = format!("{}/account", base_url.trim_end_matches('/'));

    let body = json!({
        "action": "agents",
        "credentials": {
            "access_token": access_token
        },
        "agents": {
            "archive": {
                "name": name,
                "definition_path": default_definition_path(definition_path),
                "is_archived": is_archived
            }
        }
    });

    let client = reqwest::Client::new();
    let resp = client.post(url).json(&body).send().await?;

    match resp.json::<Value>().await {
        Ok(v) => Ok(v),
        Err(e) => Err(e),
    }
}
