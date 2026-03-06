use crate::config::loader::{config_path, load_config};
use crate::config::schema::{
    default_secret_store_mode, Config, OpenAiAuth, ProfileAuthMode, SecretStoreMode,
};
use std::fs;

fn default_config() -> Config {
    Config {
        profile: Vec::new(),
        cargo_ai_token: None,
        default_profile: None,
        secret_store: Some(default_secret_store_mode()),
        account: None,
        openai_auth: None,
        web_resources: None,
        update_check: None,
        cargo_ai_metadata: None,
    }
}

fn write_config(cfg: &Config) -> Result<(), String> {
    let path = config_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| {
            format!(
                "failed to create config directory '{}': {error}",
                parent.display()
            )
        })?;
    }

    let serialized = toml::to_string_pretty(cfg)
        .map_err(|error| format!("failed to serialize config: {error}"))?;
    fs::write(&path, serialized)
        .map_err(|error| format!("failed to write '{}': {error}", path.display()))
}

fn mutate_config<F>(mutator: F) -> Result<(), String>
where
    F: FnOnce(&mut Config) -> Result<(), String>,
{
    let mut cfg = load_config().unwrap_or_else(default_config);
    mutator(&mut cfg)?;
    write_config(&cfg)
}

pub fn set_secret_store_mode(mode: SecretStoreMode) -> Result<(), String> {
    mutate_config(|cfg| {
        cfg.secret_store = Some(mode);
        Ok(())
    })
}

pub fn set_default_profile(profile_name: &str) -> Result<(), String> {
    let trimmed = profile_name.trim();
    if trimmed.is_empty() {
        return Err("profile name cannot be empty".to_string());
    }

    mutate_config(|cfg| {
        if !cfg.profile.iter().any(|profile| profile.name == trimmed) {
            return Err(format!("profile '{}' not found", trimmed));
        }

        cfg.default_profile = Some(trimmed.to_string());
        Ok(())
    })
}

pub fn set_profile_auth_mode(profile_name: &str, mode: ProfileAuthMode) -> Result<(), String> {
    let trimmed = profile_name.trim();
    if trimmed.is_empty() {
        return Err("profile name cannot be empty".to_string());
    }

    mutate_config(|cfg| {
        let profile = cfg
            .profile
            .iter_mut()
            .find(|profile| profile.name == trimmed)
            .ok_or_else(|| format!("profile '{}' not found", trimmed))?;
        profile.auth_mode = mode;
        Ok(())
    })
}

#[allow(dead_code)]
pub fn set_openai_auth_metadata(
    access_token_expires_in: Option<i32>,
    access_token_issued_at: Option<i64>,
) -> Result<(), String> {
    mutate_config(|cfg| {
        let locally_disabled = cfg
            .openai_auth
            .as_ref()
            .and_then(|auth| auth.locally_disabled);
        cfg.openai_auth = Some(OpenAiAuth {
            access_token_expires_in,
            access_token_issued_at,
            locally_disabled,
        });
        Ok(())
    })
}

pub fn clear_openai_auth_metadata() -> Result<(), String> {
    mutate_config(|cfg| {
        cfg.openai_auth = None;
        Ok(())
    })
}

pub fn set_openai_auth_locally_disabled(disabled: bool) -> Result<(), String> {
    mutate_config(|cfg| {
        let mut openai_auth = cfg.openai_auth.take().unwrap_or_default();
        openai_auth.locally_disabled = Some(disabled);
        cfg.openai_auth = Some(openai_auth);
        Ok(())
    })
}
