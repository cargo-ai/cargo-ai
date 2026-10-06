//! Connection discovery bypasses inference setup and all persistence paths.
use crate::config::{
    loader::{load_config_from_path, ConfigLoad},
    schema::{ProfileAuthMode, SecretStoreMode},
};
pub use crate::providers::discovery::DiscoveryError;
use crate::providers::{
    account_discovery,
    discovery::{self, Connection},
    ProviderKind,
};
use clap::ArgMatches;
use serde_json::Value;

pub async fn run(matches: &ArgMatches) -> Result<Value, DiscoveryError> {
    let (matches, thinking) = match matches.subcommand() {
        Some(("thinking", child)) => (child, true),
        Some(("list", child)) => (child, false),
        _ => (
            matches,
            matches
                .try_get_one::<String>("model")
                .ok()
                .flatten()
                .is_some(),
        ),
    };
    let connection = if let Some(name) = matches.get_one::<String>("profile") {
        resolve_saved_for(
            &crate::config::paths::cargo_ai_root().join("config.toml"),
            name,
            thinking,
        )?
    } else {
        let server = matches.get_one::<String>("server").ok_or_else(|| {
            DiscoveryError::new(
                "invalid_request",
                "Select a saved profile or a draft server.",
            )
        })?;
        let provider = ProviderKind::from_server_value(server).ok_or_else(|| {
            DiscoveryError::new(
                "unsupported_provider",
                "This provider does not support model discovery.",
            )
        })?;
        let auth = match matches.get_one::<String>("auth").map(String::as_str) {
            Some("none") => ProfileAuthMode::None,
            Some("api_key") => ProfileAuthMode::ApiKey,
            Some("openai_account") => ProfileAuthMode::OpenaiAccount,
            _ => {
                return Err(DiscoveryError::new(
                    "invalid_request",
                    "Select an explicit draft authentication mode.",
                ))
            }
        };
        let endpoint_for = if thinking {
            discovery::thinking_endpoint
        } else {
            discovery::endpoint
        };
        let endpoint = endpoint_for(
            provider,
            matches.get_one::<String>("url").map(String::as_str),
            auth,
        )?;
        let stdin = matches.get_flag("stdin");
        if stdin != (auth == ProfileAuthMode::ApiKey) {
            return Err(DiscoveryError::new(
                "invalid_request",
                "Draft API keys require --stdin; account and no-auth drafts must omit it.",
            ));
        }
        let token = if stdin {
            super::secret_input::read_stdin(super::secret_input::PROFILE_TOKEN_LIMIT).map_err(|_| DiscoveryError::new("invalid_credentials", "Draft credential input must be one UTF-8 line from closed nonterminal stdin, at most 16 KiB."))?
        } else {
            String::new()
        };
        let account = if auth == ProfileAuthMode::OpenaiAccount {
            Some(account_discovery::snapshot(
                &crate::config::paths::cargo_ai_root().join("config.toml"),
            )?)
        } else {
            None
        };
        Connection {
            request_endpoint: matches
                .get_one::<String>("url")
                .cloned()
                .unwrap_or_else(|| provider.default_url().to_owned()),
            provider,
            auth,
            endpoint,
            token,
            profile: None,
            account,
        }
    };
    if thinking {
        let model = matches.get_one::<String>("model").ok_or_else(|| {
            DiscoveryError::new("invalid_request", "Select a model for thinking discovery.")
        })?;
        return discovery::thinking(connection, model).await;
    }
    let provider = discovery::provider_name(connection.provider);
    let auth = connection.auth.as_str();
    let request_endpoint = connection.request_endpoint.clone();
    let mut catalog = discovery::list(
        connection,
        *matches.get_one::<u32>("page-limit").unwrap_or(&20),
    )
    .await?;
    attach_operation_evidence(
        &mut catalog,
        provider,
        auth,
        &request_endpoint,
        time::OffsetDateTime::now_utc().unix_timestamp(),
    );
    Ok(catalog)
}

fn attach_operation_evidence(
    catalog: &mut Value,
    provider: &str,
    auth: &str,
    endpoint: &str,
    now_unix: i64,
) {
    if let Some(models) = catalog.get_mut("models").and_then(Value::as_array_mut) {
        for model in models {
            if let Some(id) = model.get("id").and_then(Value::as_str) {
                let evidence = crate::providers::operation_metadata::evidence_for_connection(
                    provider, id, auth, endpoint, now_unix,
                );
                model["operation_evidence"] = evidence;
            }
        }
    }
}

#[cfg(test)]
fn resolve_saved(path: &std::path::Path, name: &str) -> Result<Connection, DiscoveryError> {
    resolve_saved_for(path, name, false)
}
fn resolve_saved_for(
    path: &std::path::Path,
    name: &str,
    thinking: bool,
) -> Result<Connection, DiscoveryError> {
    let loaded = match load_config_from_path(path).map_err(|_| {
        DiscoveryError::new(
            "invalid_configuration",
            "The selected home configuration is unreadable or invalid.",
        )
    })? {
        ConfigLoad::Missing => {
            return Err(DiscoveryError::new(
                "profile_not_found",
                "The selected profile does not exist.",
            ))
        }
        ConfigLoad::Loaded(loaded) => loaded,
    };
    let config = loaded.config();
    let profile = config
        .profile
        .iter()
        .find(|profile| profile.name == name)
        .ok_or_else(|| {
            DiscoveryError::new("profile_not_found", "The selected profile does not exist.")
        })?;
    let provider = ProviderKind::from_server_value(&profile.server).ok_or_else(|| {
        DiscoveryError::new(
            "unsupported_provider",
            "This provider does not support model discovery.",
        )
    })?;
    let endpoint_for = if thinking {
        discovery::thinking_endpoint
    } else {
        discovery::endpoint
    };
    let endpoint = endpoint_for(provider, profile.url.as_deref(), profile.auth_mode)?;
    let account = if profile.auth_mode == ProfileAuthMode::OpenaiAccount {
        Some(account_discovery::snapshot_saved(
            path,
            loaded.original_contents(),
        )?)
    } else {
        None
    };
    let token = match profile.auth_mode {
        ProfileAuthMode::None => String::new(),
        ProfileAuthMode::OpenaiAccount => String::new(),
        ProfileAuthMode::ApiKey => {
            if config.secret_store != Some(SecretStoreMode::File) {
                return Err(DiscoveryError::new("unsupported_secret_store", "Saved API-key discovery requires an explicit file secret store; use a draft stdin key for other stores."));
            }
            crate::credentials::store::load_scoped_file_profile_token(&loaded, name)
                .map_err(|_| {
                    DiscoveryError::new(
                        "invalid_credentials",
                        "The selected file credential store is unreadable or invalid.",
                    )
                })?
                .filter(|token| !token.trim().is_empty())
                .ok_or_else(|| {
                    DiscoveryError::new(
                        "missing_credentials",
                        "The selected file store has no profile API key.",
                    )
                })?
        }
    };
    if token.len() > super::secret_input::PROFILE_TOKEN_LIMIT || token.chars().any(char::is_control)
    {
        return Err(DiscoveryError::new(
            "invalid_credentials",
            "The selected profile API key is not valid bounded credential input.",
        ));
    }
    Ok(Connection {
        request_endpoint: profile
            .url
            .clone()
            .unwrap_or_else(|| provider.default_url().to_owned()),
        provider,
        auth: profile.auth_mode,
        endpoint,
        token,
        profile: Some(name.to_owned()),
        account,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn reviewed_operation_evidence_is_separate_from_provider_catalog_metadata() {
        let mut catalog = serde_json::json!({"models":[
            {"id":"gpt-image-2","metadata":{},"metadata_source":"provider","invocation_access":"unverified"},
            {"id":"gpt-4o-mini-tts","metadata":{},"metadata_source":"provider","invocation_access":"unverified"},
            {"id":"gpt-5.2","metadata":{},"metadata_source":"provider","invocation_access":"unverified"},
            {"id":"catalog-presence-only","metadata":{},"metadata_source":"provider","invocation_access":"unverified"}
        ]});
        attach_operation_evidence(
            &mut catalog,
            "openai",
            "api_key",
            "https://api.openai.com/v1/chat/completions",
            1_791_244_801,
        );
        for model in &catalog["models"].as_array().unwrap()[..3] {
            assert_eq!(
                model["operation_evidence"]["records"]
                    .as_array()
                    .unwrap()
                    .len(),
                1
            );
            assert_eq!(
                model["operation_evidence"]["invocation_access"],
                "unverified"
            );
            assert_eq!(model["metadata"], serde_json::json!({}));
            assert_eq!(model["metadata_source"], "provider");
            assert_eq!(model["invocation_access"], "unverified");
        }
        assert!(catalog["models"][3]["operation_evidence"]["records"]
            .as_array()
            .unwrap()
            .is_empty());
        attach_operation_evidence(
            &mut catalog,
            "openai",
            "openai_account",
            "https://chatgpt.com/backend-api/codex/responses",
            1_791_244_801,
        );
        assert!(catalog["models"]
            .as_array()
            .unwrap()
            .iter()
            .all(|model| model["operation_evidence"]["records"]
                .as_array()
                .unwrap()
                .is_empty()));
    }

    fn home(mode: Option<&str>, auth: &str) -> std::path::PathBuf {
        let path =
            std::env::temp_dir().join(format!("cargo-ai-discovery-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&path).unwrap();
        let mode = mode
            .map(|mode| format!("secret_store='{mode}'\n"))
            .unwrap_or_default();
        std::fs::write(path.join("config.toml"), format!("{mode}[[profile]]\nname='fixture'\nserver='ollama'\nmodel='obsolete-model'\nauth_mode='{auth}'\n")).unwrap();
        path
    }
    #[test]
    fn discovery_saved_store_guards_and_no_auth_do_not_read_credentials() {
        let before = crate::credentials::store::discovery_keychain_lookup_count();
        for mode in [None, Some("keychain")] {
            let path = home(mode, "api_key");
            assert_eq!(
                resolve_saved(&path.join("config.toml"), "fixture")
                    .err()
                    .unwrap()
                    .code,
                "unsupported_secret_store"
            );
            std::fs::remove_dir_all(path).unwrap();
        }
        let path = home(Some("keychain"), "none");
        std::fs::write(
            path.join("credentials.toml"),
            "invalid credentials must never be parsed",
        )
        .unwrap();
        assert!(resolve_saved(&path.join("config.toml"), "fixture")
            .unwrap()
            .token
            .is_empty());
        std::fs::remove_dir_all(path).unwrap();
        assert_eq!(
            crate::credentials::store::discovery_keychain_lookup_count(),
            before
        );
    }
    #[test]
    fn discovery_missing_home_is_not_initialized() {
        let path = std::env::temp_dir().join(format!("absent-discovery-{}", uuid::Uuid::new_v4()));
        assert_eq!(
            resolve_saved(&path.join("config.toml"), "fixture")
                .err()
                .unwrap()
                .code,
            "profile_not_found"
        );
        assert!(!path.exists());
    }
}
