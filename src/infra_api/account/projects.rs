#[cfg(test)]
use crate::config::schema::CargoAiMetadata;
use serde_json::{json, Value};

/// List projects for the authenticated account or for a specific owner handle.
pub async fn list_projects(
    base_url: &str,
    access_token: &str,
    owner_handle: Option<&str>,
    include_archived: bool,
) -> Result<Value, reqwest::Error> {
    let url = format!("{}/account", base_url.trim_end_matches('/'));
    let body = build_list_projects_body(access_token, owner_handle, include_archived);

    let client = reqwest::Client::new();
    let resp = client.post(url).json(&body).send().await?;

    match resp.json::<Value>().await {
        Ok(v) => Ok(v),
        Err(e) => Err(e),
    }
}

/// Publish a packaged project archive.
pub async fn publish_project(
    base_url: &str,
    access_token: &str,
    project_name: &str,
    project_version: &str,
    package_manifest: Value,
    package_sha256: &str,
    package_size_bytes: i64,
    package_archive_base64: &str,
) -> Result<Value, reqwest::Error> {
    let url = format!("{}/account", base_url.trim_end_matches('/'));
    let body = build_publish_project_body(
        access_token,
        project_name,
        project_version,
        package_manifest,
        package_sha256,
        package_size_bytes,
        package_archive_base64,
    );

    let client = reqwest::Client::new();
    let resp = client.post(url).json(&body).send().await?;

    match resp.json::<Value>().await {
        Ok(v) => Ok(v),
        Err(e) => Err(e),
    }
}

/// Pull a published project package.
pub async fn pull_project(
    base_url: &str,
    access_token: &str,
    name: &str,
    owner_handle: Option<&str>,
    version: Option<&str>,
) -> Result<Value, reqwest::Error> {
    let url = format!("{}/account", base_url.trim_end_matches('/'));
    let body = build_pull_project_body(access_token, name, owner_handle, version);

    let client = reqwest::Client::new();
    let resp = client.post(url).json(&body).send().await?;

    match resp.json::<Value>().await {
        Ok(v) => Ok(v),
        Err(e) => Err(e),
    }
}

pub async fn set_project_visibility(
    base_url: &str,
    access_token: &str,
    name: &str,
    is_public: bool,
) -> Result<Value, reqwest::Error> {
    let url = format!("{}/account", base_url.trim_end_matches('/'));
    let body = build_set_project_visibility_body(access_token, name, is_public);

    let client = reqwest::Client::new();
    let resp = client.post(url).json(&body).send().await?;

    match resp.json::<Value>().await {
        Ok(v) => Ok(v),
        Err(e) => Err(e),
    }
}

pub async fn set_project_archive(
    base_url: &str,
    access_token: &str,
    name: &str,
    is_archived: bool,
) -> Result<Value, reqwest::Error> {
    let url = format!("{}/account", base_url.trim_end_matches('/'));
    let body = build_set_project_archive_body(access_token, name, is_archived);

    let client = reqwest::Client::new();
    let resp = client.post(url).json(&body).send().await?;

    match resp.json::<Value>().await {
        Ok(v) => Ok(v),
        Err(e) => Err(e),
    }
}

fn build_list_projects_body(
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
        "action": "projects",
        "credentials": {
            "access_token": access_token
        },
        "projects": {
            "list": list_payload
        }
    }))
}

fn build_publish_project_body(
    access_token: &str,
    project_name: &str,
    project_version: &str,
    package_manifest: Value,
    package_sha256: &str,
    package_size_bytes: i64,
    package_archive_base64: &str,
) -> Value {
    super::with_cargo_ai_metadata(json!({
        "action": "projects",
        "credentials": {
            "access_token": access_token
        },
        "projects": {
            "publish": {
                "project_name": project_name,
                "project_version": project_version,
                "package_manifest": package_manifest,
                "package_sha256": package_sha256,
                "package_size_bytes": package_size_bytes,
                "package_archive_base64": package_archive_base64
            }
        }
    }))
}

fn build_pull_project_body(
    access_token: &str,
    name: &str,
    owner_handle: Option<&str>,
    version: Option<&str>,
) -> Value {
    let mut pull_payload = json!({
        "name": name
    });

    if let Some(handle) = owner_handle {
        pull_payload["owner_handle"] = json!(handle);
    }
    if let Some(version) = version {
        pull_payload["version"] = json!(version);
    }

    super::with_cargo_ai_metadata(json!({
        "action": "projects",
        "credentials": {
            "access_token": access_token
        },
        "projects": {
            "pull": pull_payload
        }
    }))
}

fn build_set_project_visibility_body(access_token: &str, name: &str, is_public: bool) -> Value {
    super::with_cargo_ai_metadata(json!({
        "action": "projects",
        "credentials": {
            "access_token": access_token
        },
        "projects": {
            "visibility": {
                "name": name,
                "is_public": is_public
            }
        }
    }))
}

fn build_set_project_archive_body(access_token: &str, name: &str, is_archived: bool) -> Value {
    super::with_cargo_ai_metadata(json!({
        "action": "projects",
        "credentials": {
            "access_token": access_token
        },
        "projects": {
            "archive": {
                "name": name,
                "is_archived": is_archived
            }
        }
    }))
}

#[cfg(test)]
fn build_publish_project_body_with_metadata(
    access_token: &str,
    project_name: &str,
    project_version: &str,
    package_manifest: Value,
    package_sha256: &str,
    package_size_bytes: i64,
    package_archive_base64: &str,
    metadata: Option<CargoAiMetadata>,
) -> Value {
    super::with_cargo_ai_metadata_override(
        json!({
            "action": "projects",
            "credentials": {
                "access_token": access_token
            },
            "projects": {
                "publish": {
                    "project_name": project_name,
                    "project_version": project_version,
                    "package_manifest": package_manifest,
                    "package_sha256": package_sha256,
                    "package_size_bytes": package_size_bytes,
                    "package_archive_base64": package_archive_base64
                }
            }
        }),
        metadata,
    )
}

#[cfg(test)]
mod tests {
    use super::build_publish_project_body_with_metadata;
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
    fn build_publish_project_body_includes_top_level_metadata() {
        let body = build_publish_project_body_with_metadata(
            "access-token-123",
            "demo_project",
            "0.1.0",
            json!({
                "format_version": 1,
                "profile": "default"
            }),
            "abc123",
            1024,
            "cGFja2FnZQ==",
            Some(sample_metadata()),
        );

        assert_eq!(body["action"], "projects");
        assert_eq!(body["projects"]["publish"]["project_name"], "demo_project");
        assert_eq!(body["projects"]["publish"]["project_version"], "0.1.0");
        assert_eq!(body["projects"]["publish"]["package_sha256"], "abc123");
        assert_eq!(body["projects"]["publish"]["package_size_bytes"], 1024);
        assert_eq!(
            body["cargo_ai_metadata"]["cargo_ai_binary_sha256"],
            "hash-456"
        );
    }
}
