//! Process-level regression coverage for account-agent command dispatch.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

static NEXT_HOME_ID: AtomicU64 = AtomicU64::new(0);

struct IsolatedCargoAiHome {
    path: PathBuf,
}

impl IsolatedCargoAiHome {
    fn new(command_name: &str) -> Self {
        let sequence = NEXT_HOME_ID.fetch_add(1, Ordering::Relaxed);
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "cargo-ai-account-agents-dispatch-{command_name}-{}-{timestamp}-{sequence}",
            std::process::id(),
        ));

        fs::create_dir(&path).expect("isolated Cargo AI Home should be created");
        Self { path }
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for IsolatedCargoAiHome {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

fn run_isolated_agents_command(command_name: &str, state_flag: &str) -> Output {
    let home = IsolatedCargoAiHome::new(command_name);

    Command::new(env!("CARGO_BIN_EXE_cargo-ai"))
        .arg("--no-update-check")
        .args([
            "agents",
            command_name,
            "--name",
            "dispatch_smoke",
            state_flag,
        ])
        .current_dir(home.path())
        .env("CARGO_AI_HOME", home.path())
        .env("CARGO_AI_DISABLE_KEYCHAIN", "1")
        .output()
        .expect("Cargo AI process should start")
}

fn assert_reaches_missing_account_boundary(output: &Output) {
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let combined = format!("{stdout}\n{stderr}");
    let normalized = combined.to_ascii_lowercase();

    assert_eq!(
        output.status.code(),
        Some(1),
        "an isolated command without an account should return the normal command-failure status\n{combined}"
    );
    assert!(
        combined.contains("No account found in config. You must confirm your account first."),
        "command should reach account authentication after dispatch\n{combined}"
    );

    for panic_marker in [
        "panicked at",
        "mismatch between definition and access",
        "unknown argument or group id",
    ] {
        assert!(
            !normalized.contains(panic_marker),
            "command should not hit a Clap dispatcher panic ({panic_marker})\n{combined}"
        );
    }
}

#[test]
fn visibility_reaches_account_boundary_without_dispatch_panic() {
    let output = run_isolated_agents_command("visibility", "--public");
    assert_reaches_missing_account_boundary(&output);
}

#[test]
fn archive_reaches_account_boundary_without_dispatch_panic() {
    let output = run_isolated_agents_command("archive", "--archive");
    assert_reaches_missing_account_boundary(&output);
}
