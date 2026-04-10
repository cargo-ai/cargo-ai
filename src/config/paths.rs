use std::path::PathBuf;

fn non_empty_env_path(name: &str) -> Option<PathBuf> {
    std::env::var_os(name)
        .map(PathBuf::from)
        .filter(|path| !path.as_os_str().is_empty())
}

fn resolve_cargo_ai_root(
    cargo_ai_home: Option<PathBuf>,
    cargo_home: Option<PathBuf>,
    home_dir: Option<PathBuf>,
) -> PathBuf {
    if let Some(cargo_ai_home) = cargo_ai_home {
        return cargo_ai_home;
    }

    if let Some(cargo_home) = cargo_home {
        return cargo_home.join(".cargo-ai");
    }

    if let Some(home_dir) = home_dir {
        return home_dir.join(".cargo/.cargo-ai");
    }

    PathBuf::from(".cargo/.cargo-ai")
}

pub fn cargo_ai_root() -> PathBuf {
    resolve_cargo_ai_root(
        non_empty_env_path("CARGO_AI_HOME"),
        non_empty_env_path("CARGO_HOME"),
        dirs::home_dir(),
    )
}

#[cfg(test)]
mod tests {
    use super::resolve_cargo_ai_root;
    use std::path::PathBuf;

    #[test]
    fn prefers_cargo_ai_home_when_present() {
        let path = resolve_cargo_ai_root(
            Some(PathBuf::from("/tmp/cargo-ai-home")),
            Some(PathBuf::from("/tmp/cargo-home")),
            Some(PathBuf::from("/Users/example")),
        );

        assert_eq!(path, PathBuf::from("/tmp/cargo-ai-home"));
    }

    #[test]
    fn falls_back_to_cargo_home_when_override_missing() {
        let path = resolve_cargo_ai_root(
            None,
            Some(PathBuf::from("/tmp/cargo-home")),
            Some(PathBuf::from("/Users/example")),
        );

        assert_eq!(path, PathBuf::from("/tmp/cargo-home/.cargo-ai"));
    }

    #[test]
    fn falls_back_to_home_dir_when_cargo_home_missing() {
        let path = resolve_cargo_ai_root(None, None, Some(PathBuf::from("/Users/example")));

        assert_eq!(path, PathBuf::from("/Users/example/.cargo/.cargo-ai"));
    }

    #[test]
    fn falls_back_to_relative_path_when_no_home_context_exists() {
        let path = resolve_cargo_ai_root(None, None, None);

        assert_eq!(path, PathBuf::from(".cargo/.cargo-ai"));
    }
}
