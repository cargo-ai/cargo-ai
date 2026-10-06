use crate::config::loader::{config_path, load_config};
use crate::credentials::store;
use std::fs;

pub fn remove_profile(name: &str) -> Result<(), Box<dyn std::error::Error>> {
    remove_profile_impl(name, true).map(|_| ())
}

/// Reports credential cleanup separately from the persisted profile removal.
pub(crate) fn remove_profile_noninteractive(
    name: &str,
) -> Result<bool, Box<dyn std::error::Error>> {
    remove_profile_impl(name, false)
}

fn remove_profile_impl(name: &str, render: bool) -> Result<bool, Box<dyn std::error::Error>> {
    let _context_lock =
        crate::credentials::role_context::lock_at(&crate::credentials::role_context::root()?)?;
    // Removal can make the profile unavailable even when protected metadata is
    // unreadable. Report incomplete cleanup; addition still requires successful
    // identity invalidation before a same-name profile can be recreated.
    let mut cleanup_succeeded =
        crate::credentials::role_context::before_mutation(Some(name), true).is_ok();
    if let Some(mut cfg) = load_config() {
        let before_count = cfg.profile.len();

        // If the removed profile is the current default, clear it
        let mut removed_default = false;
        if cfg.default_profile.as_deref() == Some(name) {
            cfg.default_profile = None;
            removed_default = true;
        }

        cfg.profile.retain(|p| p.name != name);

        if cfg.profile.len() == before_count {
            if render {
                println!("Profile '{}' not found.", name);
            }
        } else {
            let serialized = toml::to_string_pretty(&cfg)?;
            fs::write(config_path(), serialized)?;
            if let Err(error) = store::clear_profile_token(name) {
                cleanup_succeeded = false;
                if render {
                    eprintln!(
                        "⚠️ Profile removed from config, but token cleanup failed for '{}': {}",
                        name, error
                    );
                }
            }

            if render && removed_default {
                println!(
                    "Profile '{}' removed successfully (was default). Default profile cleared — this may affect agent behavior.",
                    name
                );
            } else if render {
                println!("Profile '{}' removed successfully.", name);
            }
        }
    } else {
        if render {
            println!("No config file found.");
        }
    }
    Ok(cleanup_succeeded)
}
