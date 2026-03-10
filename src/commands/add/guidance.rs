//! Runtime behavior for `cargo ai add guidance`.
use clap::ArgMatches;
use std::fs;
use std::path::{Path, PathBuf};

const CODEX_GUIDANCE_TEMPLATE: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/templates/guidance/codex-agents.md.tmpl"
));

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum GuidanceStyle {
    Codex,
}

impl GuidanceStyle {
    fn from_cli(value: Option<&str>) -> Result<Self, String> {
        match value {
            Some("codex") => Ok(Self::Codex),
            Some(other) => Err(format!(
                "Unsupported guidance style '{}'. Use `--style codex`.",
                other
            )),
            None => Err(
                "Missing guidance style. Use `cargo ai add guidance --style codex`.".to_string(),
            ),
        }
    }

    fn output_file_name(self) -> &'static str {
        match self {
            Self::Codex => "AGENTS.md",
        }
    }

    fn output_file_contents(self) -> &'static str {
        match self {
            Self::Codex => CODEX_GUIDANCE_TEMPLATE,
        }
    }
}

fn write_guidance_file(target_dir: &Path, style: GuidanceStyle) -> Result<PathBuf, String> {
    let output_path = target_dir.join(style.output_file_name());
    if output_path.exists() {
        return Err(format!(
            "Guidance file '{}' already exists. Remove it first or choose another directory before retrying.",
            output_path.display()
        ));
    }

    fs::write(&output_path, style.output_file_contents()).map_err(|error| {
        format!(
            "Failed to write guidance file '{}': {}",
            output_path.display(),
            error
        )
    })?;

    Ok(output_path)
}

/// Executes the `guidance` subcommand flow from parsed CLI arguments.
pub fn run(sub_m: &ArgMatches) -> bool {
    if let Err(error) = run_impl(sub_m) {
        eprintln!("❌ {}", error);
        return false;
    }

    true
}

fn run_impl(sub_m: &ArgMatches) -> Result<(), String> {
    let style = GuidanceStyle::from_cli(sub_m.get_one::<String>("style").map(String::as_str))?;
    let current_dir = std::env::current_dir()
        .map_err(|error| format!("Failed to resolve current directory: {error}"))?;
    let output_path = write_guidance_file(&current_dir, style)?;

    println!("✅ Wrote guidance file: {}", output_path.display());
    println!("ℹ️ Added AGENTS.md only. No scaffold metadata or .cargo-ai assets were created.");

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{write_guidance_file, GuidanceStyle};
    use std::fs;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_dir_path(stem: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time should be after epoch")
            .as_nanos();
        std::env::temp_dir().join(format!("cargo-ai-add-guidance-test-{}-{}", stem, nanos))
    }

    #[test]
    fn write_guidance_file_writes_agents_md_for_codex() {
        let dir = temp_dir_path("codex");
        fs::create_dir_all(&dir).expect("test dir should be created");

        let output_path =
            write_guidance_file(&dir, GuidanceStyle::Codex).expect("guidance write should work");
        assert_eq!(
            output_path.file_name().and_then(|name| name.to_str()),
            Some("AGENTS.md")
        );

        let guidance =
            fs::read_to_string(&output_path).expect("guidance output should be readable");
        assert!(guidance.contains("Cargo AI Agent Definition Guidance"));
        assert!(guidance.contains("cargo ai hatch <agent-name> --config <config.json> --check"));
        assert!(guidance.contains("Do not use `cargo ai new` or `cargo ai init`"));

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn write_guidance_file_fails_when_agents_md_exists() {
        let dir = temp_dir_path("conflict");
        fs::create_dir_all(&dir).expect("test dir should be created");
        fs::write(dir.join("AGENTS.md"), "existing guidance\n")
            .expect("existing guidance file should be written");

        let error =
            write_guidance_file(&dir, GuidanceStyle::Codex).expect_err("existing file should fail");
        assert!(error.contains("AGENTS.md"));
        assert!(error.contains("already exists"));

        let _ = fs::remove_dir_all(dir);
    }
}
