//! Runtime behavior for `cargo ai run`.
use clap::ArgMatches;
use std::path::Path;

/// Executes the interpreted runtime flow from a local JSON definition.
pub async fn run(sub_m: &ArgMatches) -> bool {
    let Some(config_path) = sub_m.get_one::<String>("config") else {
        eprintln!("x Missing config path. Use `cargo ai run --config <path-to-json>`.");
        return false;
    };

    let config_path_ref = Path::new(config_path);
    let definition = match crate::runtime_definition::RuntimeAgentDefinition::load_from_path(
        config_path_ref,
    ) {
        Ok(definition) => definition,
        Err(error) => {
            eprintln!("x {error}");
            return false;
        }
    };

    super::preflight::run_with_definition(sub_m, &definition).await
}
