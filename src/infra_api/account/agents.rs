#[cfg(test)]
use crate::config::schema::CargoAiMetadata;
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
    let body = build_list_agents_body(access_token, owner_handle, include_archived);

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
    let body = build_push_agent_body(access_token, name, definition_path, definition_json);

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
    let body = build_pull_agent_body(access_token, name, owner_handle, definition_path);

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
    let body = build_set_agent_visibility_body(
        access_token,
        name,
        definition_path,
        is_public,
        public_from,
        public_until,
    );

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
    let body = build_set_agent_archive_body(access_token, name, definition_path, is_archived);

    let client = reqwest::Client::new();
    let resp = client.post(url).json(&body).send().await?;

    match resp.json::<Value>().await {
        Ok(v) => Ok(v),
        Err(e) => Err(e),
    }
}

fn build_list_agents_body(
    access_token: &str,
    owner_handle: Option<&str>,
    include_archived: bool,
) -> Value {
    let mut list_payload = json!({
        "include_archived": include_archived,
    });

    if let Some(handle) = owner_handle {
        list_payload["owner_handle"] = json!(handle);
    }

    super::with_cargo_ai_metadata(json!({
        "action": "agents",
        "credentials": {
            "access_token": access_token
        },
        "agents": {
            "list": list_payload
        }
    }))
}

#[cfg(test)]
fn build_list_agents_body_with_metadata(
    access_token: &str,
    owner_handle: Option<&str>,
    include_archived: bool,
    metadata: Option<CargoAiMetadata>,
) -> Value {
    let mut list_payload = json!({
        "include_archived": include_archived,
    });

    if let Some(handle) = owner_handle {
        list_payload["owner_handle"] = json!(handle);
    }

    super::with_cargo_ai_metadata_override(
        json!({
            "action": "agents",
            "credentials": {
                "access_token": access_token
            },
            "agents": {
                "list": list_payload
            }
        }),
        metadata,
    )
}

fn build_push_agent_body(
    access_token: &str,
    name: &str,
    definition_path: Option<&str>,
    definition_json: Value,
) -> Value {
    super::with_cargo_ai_metadata(json!({
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
    }))
}

#[cfg(test)]
fn build_push_agent_body_with_metadata(
    access_token: &str,
    name: &str,
    definition_path: Option<&str>,
    definition_json: Value,
    metadata: Option<CargoAiMetadata>,
) -> Value {
    super::with_cargo_ai_metadata_override(
        json!({
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
        }),
        metadata,
    )
}

fn build_pull_agent_body(
    access_token: &str,
    name: &str,
    owner_handle: Option<&str>,
    definition_path: Option<&str>,
) -> Value {
    let mut pull_payload = json!({
        "name": name,
        "definition_path": default_definition_path(definition_path),
    });

    if let Some(handle) = owner_handle {
        pull_payload["owner_handle"] = json!(handle);
    }

    super::with_cargo_ai_metadata(json!({
        "action": "agents",
        "credentials": {
            "access_token": access_token
        },
        "agents": {
            "pull": pull_payload
        }
    }))
}

#[cfg(test)]
fn build_pull_agent_body_with_metadata(
    access_token: &str,
    name: &str,
    owner_handle: Option<&str>,
    definition_path: Option<&str>,
    metadata: Option<CargoAiMetadata>,
) -> Value {
    let mut pull_payload = json!({
        "name": name,
        "definition_path": default_definition_path(definition_path),
    });

    if let Some(handle) = owner_handle {
        pull_payload["owner_handle"] = json!(handle);
    }

    super::with_cargo_ai_metadata_override(
        json!({
            "action": "agents",
            "credentials": {
                "access_token": access_token
            },
            "agents": {
                "pull": pull_payload
            }
        }),
        metadata,
    )
}

fn build_set_agent_visibility_body(
    access_token: &str,
    name: &str,
    definition_path: Option<&str>,
    is_public: bool,
    public_from: Option<&str>,
    public_until: Option<&str>,
) -> Value {
    let mut visibility_payload = json!({
        "name": name,
        "definition_path": default_definition_path(definition_path),
        "is_public": is_public,
    });

    if let Some(value) = public_from {
        visibility_payload["public_from"] = json!(value);
    }

    if let Some(value) = public_until {
        visibility_payload["public_until"] = json!(value);
    }

    super::with_cargo_ai_metadata(json!({
        "action": "agents",
        "credentials": {
            "access_token": access_token
        },
        "agents": {
            "visibility": visibility_payload
        }
    }))
}

#[cfg(test)]
fn build_set_agent_visibility_body_with_metadata(
    access_token: &str,
    name: &str,
    definition_path: Option<&str>,
    is_public: bool,
    public_from: Option<&str>,
    public_until: Option<&str>,
    metadata: Option<CargoAiMetadata>,
) -> Value {
    let mut visibility_payload = json!({
        "name": name,
        "definition_path": default_definition_path(definition_path),
        "is_public": is_public,
    });

    if let Some(value) = public_from {
        visibility_payload["public_from"] = json!(value);
    }

    if let Some(value) = public_until {
        visibility_payload["public_until"] = json!(value);
    }

    super::with_cargo_ai_metadata_override(
        json!({
            "action": "agents",
            "credentials": {
                "access_token": access_token
            },
            "agents": {
                "visibility": visibility_payload
            }
        }),
        metadata,
    )
}

fn build_set_agent_archive_body(
    access_token: &str,
    name: &str,
    definition_path: Option<&str>,
    is_archived: bool,
) -> Value {
    super::with_cargo_ai_metadata(json!({
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
    }))
}

#[cfg(test)]
fn build_set_agent_archive_body_with_metadata(
    access_token: &str,
    name: &str,
    definition_path: Option<&str>,
    is_archived: bool,
    metadata: Option<CargoAiMetadata>,
) -> Value {
    super::with_cargo_ai_metadata_override(
        json!({
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
        }),
        metadata,
    )
}

#[cfg(test)]
mod tests {
    use super::{
        build_list_agents_body_with_metadata, build_pull_agent_body_with_metadata,
        build_push_agent_body, build_push_agent_body_with_metadata,
        build_set_agent_archive_body_with_metadata, build_set_agent_visibility_body_with_metadata,
    };
    use crate::config::schema::CargoAiMetadata;
    use serde_json::json;

    fn sample_metadata() -> CargoAiMetadata {
        CargoAiMetadata {
            cargo_ai_version: Some(env!("CARGO_PKG_VERSION").to_string()),
            template_schema_version: Some("2026-03-03.r1".to_string()),
            cargo_ai_build_target: Some("aarch64-apple-darwin".to_string()),
            cargo_ai_install_id: Some("install-123".to_string()),
            cargo_ai_binary_sha256: Some("hash-456".to_string()),
        }
    }

    #[test]
    fn build_list_agents_body_includes_top_level_metadata() {
        let body = build_list_agents_body_with_metadata(
            "access-token-123",
            Some("owner-handle"),
            false,
            Some(sample_metadata()),
        );

        assert_eq!(body["action"], "agents");
        assert_eq!(body["agents"]["list"]["owner_handle"], "owner-handle");
        assert_eq!(
            body["cargo_ai_metadata"]["cargo_ai_version"],
            env!("CARGO_PKG_VERSION")
        );
    }

    #[test]
    fn build_push_agent_body_preserves_agents_payload_shape() {
        let body = build_push_agent_body(
            "access-token-123",
            "demo_agent",
            Some("/team"),
            json!({"inputs": [{"type": "text", "text": "hello"}]}),
        );

        assert_eq!(body["agents"]["push"]["name"], "demo_agent");
        assert_eq!(body["agents"]["push"]["definition_path"], "/team");
    }

    #[test]
    fn build_push_agent_body_includes_top_level_metadata() {
        let body = build_push_agent_body_with_metadata(
            "access-token-123",
            "demo_agent",
            Some("/team"),
            json!({"inputs": [{"type": "text", "text": "hello"}]}),
            Some(sample_metadata()),
        );

        assert_eq!(body["agents"]["push"]["name"], "demo_agent");
        assert_eq!(body["agents"]["push"]["definition_path"], "/team");
        assert_eq!(
            body["cargo_ai_metadata"]["cargo_ai_install_id"],
            "install-123"
        );
    }

    #[test]
    fn build_pull_agent_body_includes_top_level_metadata() {
        let body = build_pull_agent_body_with_metadata(
            "access-token-123",
            "demo_agent",
            None,
            Some("/team"),
            Some(sample_metadata()),
        );

        assert_eq!(body["agents"]["pull"]["name"], "demo_agent");
        assert_eq!(body["agents"]["pull"]["definition_path"], "/team");
        assert_eq!(
            body["cargo_ai_metadata"]["cargo_ai_binary_sha256"],
            "hash-456"
        );
    }

    #[test]
    fn build_set_agent_visibility_body_includes_top_level_metadata() {
        let body = build_set_agent_visibility_body_with_metadata(
            "access-token-123",
            "demo_agent",
            Some("/team"),
            true,
            Some("2026-03-05T12:00:00Z"),
            None,
            Some(sample_metadata()),
        );

        assert_eq!(body["agents"]["visibility"]["is_public"], true);
        assert_eq!(
            body["agents"]["visibility"]["public_from"],
            "2026-03-05T12:00:00Z"
        );
        assert_eq!(
            body["cargo_ai_metadata"]["cargo_ai_build_target"],
            "aarch64-apple-darwin"
        );
    }

    #[test]
    fn build_set_agent_archive_body_includes_top_level_metadata() {
        let body = build_set_agent_archive_body_with_metadata(
            "access-token-123",
            "demo_agent",
            None,
            true,
            Some(sample_metadata()),
        );

        assert_eq!(body["agents"]["archive"]["name"], "demo_agent");
        assert_eq!(body["agents"]["archive"]["is_archived"], true);
        assert_eq!(
            body["cargo_ai_metadata"]["template_schema_version"],
            "2026-03-03.r1"
        );
    }
}
