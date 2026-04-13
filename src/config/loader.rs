use crate::config::schema::{Config, Profile};
use std::fs;
use std::path::PathBuf;

fn resolve_config_path(cargo_ai_root: PathBuf) -> PathBuf {
    cargo_ai_root.join("config.toml")
}

pub fn config_path() -> PathBuf {
    resolve_config_path(crate::config::paths::cargo_ai_root())
}

pub fn load_config() -> Option<Config> {
    let path = config_path();
    if !path.exists() {
        return None;
    }

    let contents = fs::read_to_string(path).ok()?;
    toml::from_str::<Config>(&contents).ok()
}

pub fn find_profile<'a>(config: &'a Config, name: &str) -> Option<&'a Profile> {
    config.profile.iter().find(|p| p.name == name)
}

#[cfg(test)]
mod tests {
    use super::resolve_config_path;
    use std::path::PathBuf;

    #[test]
    fn joins_config_file_under_resolved_root() {
        let path = resolve_config_path(PathBuf::from("/tmp/cargo-ai-home"));

        assert_eq!(path, PathBuf::from("/tmp/cargo-ai-home/config.toml"));
    }
}
