//! Runtime behavior for `cargo ai shipyard`.
use clap::ArgMatches;

/// Executes the shipyard command, honoring feature and explicit enable gates.
pub fn run(sub_m: &ArgMatches) {
    #[cfg(feature = "shipyard-ui")]
    {
        let enabled_by_flag = sub_m.get_flag("experimental");
        let enabled_by_env = std::env::var("CARGO_AI_ENABLE_SHIPYARD")
            .map(|value| {
                let normalized = value.trim().to_ascii_lowercase();
                matches!(normalized.as_str(), "1" | "true" | "yes" | "on")
            })
            .unwrap_or(false);

        if !(enabled_by_flag || enabled_by_env) {
            eprintln!("⚠️ Shipyard is experimental and currently hidden.");
            eprintln!(
                "To launch it, run `cargo ai shipyard --experimental` or set `CARGO_AI_ENABLE_SHIPYARD=1`."
            );
            return;
        }

        if let Err(e) = crate::shipyard_ui::launch() {
            eprintln!("❌ Failed to launch Shipyard UI: {e}");
        }
    }

    #[cfg(not(feature = "shipyard-ui"))]
    {
        let _ = sub_m;
        eprintln!("⚠️ Shipyard UI is not included in this build.");
        eprintln!("Reinstall with `cargo install --path . --features shipyard-ui` to enable it.");
    }
}
