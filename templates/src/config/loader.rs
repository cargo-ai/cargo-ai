use std::fs;
use std::path::PathBuf;
use crate::config::schema::{Config, Profile};

fn resolve_config_path(
    cargo_ai_home: Option<PathBuf>,
    cargo_home: Option<PathBuf>,
    home_dir: Option<PathBuf>,
) -> PathBuf {
    if let Some(cargo_ai_home) = cargo_ai_home {
        return cargo_ai_home.join("config.toml");
    }

    if let Some(cargo_home) = cargo_home {
        return cargo_home.join(".cargo-ai/config.toml");
    }

    if let Some(home_dir) = home_dir {
        return home_dir.join(".cargo/.cargo-ai/config.toml");
    }

    // Last-resort fallback for constrained environments where neither CARGO_HOME nor
    // HOME can be resolved. This avoids panic and keeps path behavior deterministic.
    PathBuf::from(".cargo/.cargo-ai/config.toml")
}

pub fn config_path() -> PathBuf {
    resolve_config_path(
        std::env::var_os("CARGO_AI_HOME").map(PathBuf::from),
        std::env::var_os("CARGO_HOME").map(PathBuf::from),
        dirs::home_dir(),
    )
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
    fn prefers_cargo_ai_home_when_present() {
        let path = resolve_config_path(
            Some(PathBuf::from("/tmp/cargo-ai-home")),
            Some(PathBuf::from("/tmp/cargo-home")),
            Some(PathBuf::from("/Users/example")),
        );

        assert_eq!(path, PathBuf::from("/tmp/cargo-ai-home/config.toml"));
    }

    #[test]
    fn prefers_cargo_home_when_present() {
        let path = resolve_config_path(
            None,
            Some(PathBuf::from("/tmp/cargo-home")),
            Some(PathBuf::from("/Users/example")),
        );

        assert_eq!(path, PathBuf::from("/tmp/cargo-home/.cargo-ai/config.toml"));
    }

    #[test]
    fn falls_back_to_home_dir_when_cargo_home_missing() {
        let path = resolve_config_path(None, None, Some(PathBuf::from("/Users/example")));

        assert_eq!(path, PathBuf::from("/Users/example/.cargo/.cargo-ai/config.toml"));
    }

    #[test]
    fn falls_back_to_relative_path_when_no_home_context_exists() {
        let path = resolve_config_path(None, None, None);

        assert_eq!(path, PathBuf::from(".cargo/.cargo-ai/config.toml"));
    }
}
