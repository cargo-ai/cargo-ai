//! Action execution helpers for preflight/test flows.
use jsonlogic::apply;

/// Applies configured action rules to model output and executes matching steps.
pub(crate) fn apply_actions(output: &crate::Output, actions: &[crate::Action]) {
    // println!("DEBUG: Applying actions -> {:?}", actions);

    let data = match serde_json::to_value(output) {
        Ok(data) => data,
        Err(error) => {
            eprintln!("❌ Failed to serialize output for action evaluation: {error}");
            return;
        }
    };

    for action in actions {
        match apply(&action.logic, &data) {
            Ok(result) => {
                // println!("Action Loop: {:?}", action);
                if result.as_bool() == Some(true) {
                    for step in &action.run {
                        if !step.kind.eq_ignore_ascii_case("exec") {
                            eprintln!(
                                "⚠️ Skipping action '{}' with unsupported step kind '{}'.",
                                action.name, step.kind
                            );
                            continue;
                        }

                        println!(
                            "Running '{}': {} {:?}",
                            action.name, step.program, step.args
                        );

                        // Execute the command
                        let status = std::process::Command::new(&step.program)
                            .args(&step.args)
                            .status();

                        match status {
                            Ok(status) if status.success() => {
                                println!("Command completed successfully.");
                            }
                            Ok(status) => {
                                println!("Command exited with status: {}", status);
                            }
                            Err(err) => {
                                println!("Failed to execute command: {}", err);
                            }
                        }
                    }
                }
            }
            Err(error) => {
                println!(
                    "Failed to evaluate logic for action '{}': {}",
                    action.name, error
                );
            }
        }
    }
}
