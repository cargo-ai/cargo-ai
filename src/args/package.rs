//! CLI parser definition for `cargo ai package`.
use clap::{Arg, ArgAction, Command};

/// Builds the `package` command schema.
pub fn command() -> Command {
    Command::new("package")
        .about("Assemble a portable project source package from a build profile")
        .arg(
            Arg::new("profile")
                .help("Build profile name from .cargo-ai/project.toml (defaults to default)")
                .required(false)
                .value_name("PROFILE"),
        )
        .arg(
            Arg::new("output_dir")
                .long("output-dir")
                .help(
                    "Destination directory for the assembled package root (defaults to target/cargo-ai/package/<profile>)",
                )
                .value_name("DIR")
                .num_args(1),
        )
        .arg(
            Arg::new("force")
                .long("force")
                .short('f')
                .help("Replace an existing explicit output directory before assembling the package")
                .required(false)
                .action(ArgAction::SetTrue),
        )
        .after_help(
            "The package profile reuses `[build.<profile>]` from `.cargo-ai/project.toml`.\nPackage output stays source-portable: listed agent JSON, listed tool source plus metadata, listed assets, and generated package metadata. It does not include target binaries.",
        )
}

#[cfg(test)]
mod tests {
    #[test]
    fn package_supports_profile_output_dir_and_force() {
        let matches = super::command()
            .try_get_matches_from(["package", "default", "--output-dir", "./pkg", "--force"])
            .expect("package command should parse");

        assert_eq!(
            matches.get_one::<String>("profile").map(String::as_str),
            Some("default")
        );
        assert_eq!(
            matches.get_one::<String>("output_dir").map(String::as_str),
            Some("./pkg")
        );
        assert!(matches.get_flag("force"));
    }

    #[test]
    fn package_defaults_profile_when_omitted() {
        let matches = super::command()
            .try_get_matches_from(["package"])
            .expect("package command should parse without a profile");

        assert!(matches.get_one::<String>("profile").is_none());
        assert!(!matches.get_flag("force"));
    }
}
