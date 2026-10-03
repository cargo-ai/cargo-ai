//! Declared client action discovery and validation arguments.
use clap::{Arg, ArgAction, ArgGroup, Command};

fn target(command: Command) -> Command {
    command
        .arg(
            Arg::new("project")
                .long("project")
                .value_name("DIRECTORY")
                .help("Explicit source project root"),
        )
        .arg(
            Arg::new("package")
                .long("package")
                .value_name("ALIAS")
                .help("Installed package alias"),
        )
        .group(
            ArgGroup::new("action_target")
                .args(["project", "package"])
                .required(true),
        )
}

fn request(command: Command) -> Command {
    target(command)
        .arg(
            Arg::new("interface")
                .long("interface")
                .value_name("ID")
                .required(true)
                .help("Declared interface identity"),
        )
        .arg(
            Arg::new("request_stdin")
                .long("request-stdin")
                .action(ArgAction::SetTrue)
                .required(true)
                .help("Read a bounded action, resource or artifact request from stdin"),
        )
}

pub(crate) fn command() -> Command {
    Command::new("actions")
        .about("Discover and validate declared client actions without executing agents")
        .subcommand_required(true)
        .arg_required_else_help(true)
        .subcommand(target(Command::new("list").about(
            "Inspect declared actions, interfaces and resource bindings",
        )))
        .subcommand(
            request(
                Command::new("validate")
                    .about("Validate an action request without execution authorization"),
            )
            .arg(
                Arg::new("action")
                    .long("action")
                    .value_name("ID")
                    .required(true),
            ),
        )
        .subcommand(request(Command::new("artifact").about(
            "Read an authorized artifact under its exact content and context identity",
        )))
        .subcommand(
            request(
                Command::new("resource")
                    .about("Read a declared resource under its expected binding"),
            )
            .arg(
                Arg::new("resource")
                    .long("resource")
                    .value_name("ID")
                    .required(true),
            ),
        )
}

#[cfg(test)]
mod tests {
    use super::command;

    #[test]
    fn requires_an_explicit_unique_target_and_scoped_request() {
        assert!(command().try_get_matches_from(["actions", "list"]).is_err());
        assert!(command()
            .try_get_matches_from(["actions", "list", "--project", ".", "--package", "demo"])
            .is_err());
        assert!(command()
            .try_get_matches_from(["actions", "list", "--project", "."])
            .is_ok());
        assert!(command()
            .try_get_matches_from([
                "actions",
                "validate",
                "--project",
                ".",
                "--action",
                "draw",
                "--request-stdin"
            ])
            .is_err());
        assert!(command()
            .try_get_matches_from([
                "actions",
                "resource",
                "--package",
                "demo",
                "--interface",
                "board",
                "--resource",
                "index",
                "--request-stdin"
            ])
            .is_ok());
    }
}
