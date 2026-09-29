//! Maintainer-only entry point for qualification orchestration.

mod qualification_catalog;
mod qualification_dashboard;
mod qualification_paths;
mod qualification_policy;

use qualification_policy::{Identity, Record, Result, Status};
use std::{
    collections::BTreeMap,
    env,
    fs::{self, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    process::Command,
};

fn variable(name: &str) -> Result<String> {
    env::var(name).map_err(|_| "missing qualification input")
}

fn read(path: &Path, limit: u64) -> Result<String> {
    let metadata = fs::symlink_metadata(path).map_err(|_| "missing qualification input file")?;
    if !metadata.is_file() || metadata.len() > limit {
        return Err("invalid qualification input file");
    }
    let mut value = String::new();
    fs::File::open(path)
        .map_err(|_| "cannot open qualification input")?
        .take(limit + 1)
        .read_to_string(&mut value)
        .map_err(|_| "qualification input must be UTF-8")?;
    if value.len() as u64 > limit {
        return Err("oversized qualification input");
    }
    Ok(value)
}

fn append(variable_name: &str, value: &str) -> Result<()> {
    OpenOptions::new()
        .create(true)
        .append(true)
        .open(variable(variable_name)?)
        .and_then(|mut file| file.write_all(value.as_bytes()))
        .map_err(|_| "cannot write qualification output")
}

struct ProbeDirectory(PathBuf);
const PROVIDER_KEYS: [&str; 6] = [
    "OPENAI_API_KEY",
    "ANTHROPIC_API_KEY",
    "GEMINI_API_KEY",
    "XAI_API_KEY",
    "MISTRAL_API_KEY",
    "TYPESAFE_API_KEY",
];

fn without_provider_keys(command: &mut Command) -> &mut Command {
    for name in PROVIDER_KEYS {
        command.env_remove(name);
    }
    command
}
impl ProbeDirectory {
    fn new() -> Result<Self> {
        let parent = PathBuf::from(variable("RUNNER_TEMP")?);
        if !parent.is_absolute() {
            return Err("probe parent must be absolute");
        }
        let path = parent.join(format!(
            "provider-qualification-{}",
            uuid::Uuid::new_v4().simple()
        ));
        let mut builder = fs::DirBuilder::new();
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        builder
            .create(&path)
            .map_err(|_| "cannot create private probe directory")?;
        Ok(Self(path))
    }
}
impl Drop for ProbeDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn probe(provider: &str) -> Result<()> {
    if !qualification_policy::PROVIDERS.contains(&provider) {
        return Err("unknown provider");
    }
    let candidate = variable("CARGO_AI_SHA")?;
    let run_id = variable("GITHUB_RUN_ID")?;
    let run_attempt = variable("GITHUB_RUN_ATTEMPT")?;
    if !qualification_policy::hexadecimal(&candidate, 40) {
        return Err("invalid candidate");
    }
    qualification_policy::positive(&run_id)?;
    qualification_policy::positive(&run_attempt)?;
    let checkout = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .output()
        .map_err(|_| "cannot verify checkout")?;
    if !checkout.status.success()
        || String::from_utf8(checkout.stdout)
            .map_err(|_| "invalid checkout identity")?
            .trim()
            != candidate
    {
        return Err("checkout does not match candidate");
    }
    let directory = ProbeDirectory::new()?;
    let mut attempts = Vec::new();
    // A Jev probe is already a bounded eight-request journey. Replaying it would
    // repeat successful calls as well as the failed case.
    let max_attempts = if provider == "typesafe" { 1 } else { 3 };
    let selector = if provider == "typesafe" {
        "typesafe_smoke::live_typesafe_journey_uses_isolated_stdin_credentials".into()
    } else {
        format!("live_{provider}_smoke_uses_isolated_stdin_credentials")
    };
    for attempt in 1..=max_attempts {
        let report = directory.0.join(format!("result-{attempt}.json"));
        let nonce = uuid::Uuid::new_v4().simple().to_string();
        let harness = Command::new("cargo")
            .args([
                "test",
                "--locked",
                "--test",
                "provider_smoke",
                &selector,
                "--",
                "--ignored",
                "--exact",
            ])
            .env("CARGO_AI_QUALIFICATION_REPORT", &report)
            .env("CARGO_AI_QUALIFICATION_PROBE", &nonce)
            .status()
            .map_err(|_| "cannot execute probe harness")?;
        if !harness.success() {
            return Err("probe harness failed; no exemption applies");
        }
        let raw = read(&report, 2048)?;
        let decision = qualification_policy::evaluate(
            provider,
            raw.as_bytes(),
            &Identity {
                candidate: &candidate,
                run_id: &run_id,
                run_attempt: &run_attempt,
                probe_id: Some(&nonce),
            },
            "success",
            "true",
        )?;
        let mut record = Record::parse(raw.as_bytes())?;
        attempts.push(qualification_policy::Attempt {
            outcome: record.outcome,
            diagnostic: record.diagnostic,
        });
        // Only typed, identity-checked data reaches public diagnostics.
        println!(
            "Qualification probe {provider}, attempt {attempt}: {:?} ({:?})",
            record.outcome, record.diagnostic
        );
        append(
            "GITHUB_STEP_SUMMARY",
            &format!(
                "- {provider} probe attempt {attempt}: {:?} ({:?})\n",
                record.outcome, record.diagnostic
            ),
        )?;
        if let Some(journey) = &record.journey {
            append(
                "GITHUB_STEP_SUMMARY",
                &format!(
                    "- Jev model `{}`; returned `{}`; completed {}/8 cases; {} request(s) started; no retries.\n",
                    journey.requested_model,
                    journey.returned_models.join(", "),
                    journey.completed_cases,
                    journey.requests_started,
                ),
            )?;
        }
        if record.outcome != qualification_policy::Outcome::Pass
            && record.diagnostic.retryable()
            && attempt < max_attempts
        {
            std::thread::sleep(std::time::Duration::from_secs(2u64.pow(attempt)));
            continue;
        }
        record.attempts = attempts;
        let encoded =
            serde_json::to_string(&record).map_err(|_| "cannot encode sanitized evidence")?;
        append("GITHUB_OUTPUT", &format!("evidence={encoded}\n"))?;
        if !decision.accepted {
            return Err("probe did not satisfy qualification policy");
        }
        if decision.status == Status::Unverified {
            println!("::warning title=Supplemental provider detail::{provider} live verification unavailable: rate limited");
        }
        return Ok(());
    }
    Err("probe attempt budget exhausted")
}

/// A single bounded catalog invocation; no generation, login or persisted keys.
fn discover(provider: &str) -> Result<()> {
    if !["openai", "anthropic", "gemini", "xai", "mistral"].contains(&provider) {
        return Err("unsupported discovery provider");
    }
    let candidate = variable("CARGO_AI_SHA")?;
    if !qualification_policy::hexadecimal(&candidate, 40) {
        return Err("invalid candidate");
    }
    let checkout = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .output()
        .map_err(|_| "cannot verify discovery candidate")?;
    if !checkout.status.success()
        || String::from_utf8(checkout.stdout)
            .map_err(|_| "invalid checkout identity")?
            .trim()
            != candidate
    {
        return Err("discovery checkout does not match candidate");
    }
    let directory = ProbeDirectory::new()?;
    let key = variable(&format!("{}_API_KEY", provider.to_uppercase()))?;
    if key.trim().is_empty() || key.len() > 16 * 1024 || key.chars().any(char::is_control) {
        return Err("invalid discovery credential input");
    }
    // Compile before handing a key to the CLI. Build output contains no key input.
    let key = key.trim().to_owned();
    let mut build = Command::new("cargo");
    if !without_provider_keys(&mut build)
        .args(["build", "--locked", "--bin", "cargo-ai"])
        .status()
        .map_err(|_| "cannot build discovery candidate")?
        .success()
    {
        return Err("discovery candidate build failed");
    }
    let mut command = Command::new(if cfg!(windows) {
        "target/debug/cargo-ai.exe"
    } else {
        "target/debug/cargo-ai"
    });
    command
        .args([
            "models",
            "list",
            "--server",
            provider,
            "--auth",
            "api_key",
            "--stdin",
            "--output-format",
            "json",
            "--output-schema-version",
            "1",
        ])
        .env("CARGO_AI_HOME", directory.0.join("absent-home"))
        .env("CARGO_AI_DISABLE_KEYCHAIN", "1")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    without_provider_keys(&mut command);
    let mut child = command
        .spawn()
        .map_err(|_| "cannot start discovery candidate")?;
    let mut input = child
        .stdin
        .take()
        .ok_or("cannot open discovery credential input")?;
    input
        .write_all(key.as_bytes())
        .map_err(|_| "cannot supply discovery credential input")?;
    drop(input);
    let output = child
        .wait_with_output()
        .map_err(|_| "discovery invocation failed")?;
    if output.stdout.len() > 8 * 1024 * 1024
        || output
            .stdout
            .windows(key.len())
            .any(|s| s == key.as_bytes())
        || output
            .stderr
            .windows(key.len())
            .any(|s| s == key.as_bytes())
    {
        return Err("discovery output failed safety validation");
    }
    let value: serde_json::Value =
        serde_json::from_slice(&output.stdout).map_err(|_| "invalid discovery contract")?;
    if !output.status.success()
        || value["schema_version"] != 1
        || value["command"] != "models list"
        || value["payload_schema"] != "cargo-ai.models.list.v1"
        || value["outcome"] != "succeeded"
        || value.get("error") != Some(&serde_json::Value::Null)
        || value["completion"]["terminal"] != true
        || value["completion"]["complete"] != true
        || value["data"]["schema_version"] != 1
        || value["data"]["complete"] != true
        || value["data"]["provider"] != provider
        || value["data"]["auth"] != "api_key"
        || value["data"]["source"] != "live"
        || value["data"].get("cache") != Some(&serde_json::Value::Null)
        || value["data"].get("continuation") != Some(&serde_json::Value::Null)
        || value["data"]["invocation_access"] != "unverified"
        || !value["data"]["pages_fetched"]
            .as_u64()
            .is_some_and(|n| (1..=20).contains(&n))
    {
        return Err("discovery did not prove complete provider listing");
    }
    let connection = &value["data"]["connection"];
    let endpoint = connection["endpoint"]
        .as_str()
        .and_then(|url| reqwest::Url::parse(url).ok())
        .ok_or("invalid discovery connection identity")?;
    if !matches!(endpoint.scheme(), "http" | "https")
        || endpoint.host_str().is_none()
        || !endpoint.username().is_empty()
        || endpoint.password().is_some()
        || endpoint.query().is_some()
        || endpoint.fragment().is_some()
        || endpoint.path() != "/"
        || connection.get("profile") != Some(&serde_json::Value::Null)
    {
        return Err("invalid discovery connection identity");
    }
    let models = value["data"]["models"]
        .as_array()
        .ok_or("invalid discovery catalog")?;
    if models.iter().any(|m| {
        m["id"].as_str().is_none_or(|id| id.is_empty())
            || !matches!(
                m.get("name"),
                Some(serde_json::Value::Null | serde_json::Value::String(_))
            )
            || !m["metadata"].is_object()
            || m["metadata_source"] != "provider"
            || m["invocation_access"] != "unverified"
    }) {
        return Err("invalid discovery model identity");
    }
    let ids = models
        .iter()
        .map(|m| m["id"].as_str().unwrap())
        .collect::<std::collections::BTreeSet<_>>();
    if ids.len() != models.len() {
        return Err("duplicate discovery model identity");
    }
    if directory.0.join("absent-home").exists() {
        return Err("discovery unexpectedly persisted local state");
    }
    println!("Discovery {provider}: complete live listing verified for candidate {candidate}; {} exact IDs; invocation access untested.",models.len());
    append("GITHUB_STEP_SUMMARY",&format!("- {provider}: complete model listing verified; {} IDs; candidate `{candidate}`; no inference; no retry.\n",models.len()))?;
    Ok(())
}

fn aggregate() -> Result<()> {
    let rendered = (|| {
        let inputs = qualification_dashboard::Inputs {
            env: env::vars_os()
                .filter_map(|(k, v)| Some((k.into_string().ok()?, v.into_string().ok()?)))
                .collect::<BTreeMap<_, _>>(),
            jobs: read(Path::new(&variable("JOBS_JSON")?), 4 * 1024 * 1024).unwrap_or_default(),
            catalog: read(Path::new(&variable("CATALOG_PATH")?), 1024 * 1024).unwrap_or_default(),
        };
        qualification_dashboard::render(&inputs)
    })();
    let (summary, accepted) = rendered.unwrap_or_else(|_| {
        (
            format!(
                "## Cargo AI Product Qualification\n\n{}\n",
                qualification_dashboard::BLOCKED
            ),
            false,
        )
    });
    append("GITHUB_STEP_SUMMARY", &summary)?;
    if accepted {
        Ok(())
    } else {
        Err("product qualification is blocked")
    }
}

fn catalog() -> Result<()> {
    let raw = read(Path::new(&variable("CATALOG_PATH")?), 1024 * 1024)?;
    let catalog = qualification_catalog::Catalog::parse(&raw)?;
    match env::var("REQUESTED_OFFICIAL_PACKAGE")
        .unwrap_or_default()
        .as_str()
    {
        "true" => {
            if ["REQUESTED_PACKAGE_REPOSITORY", "REQUESTED_PACKAGE_SHA"]
                .iter()
                .any(|key| env::var(key).is_ok_and(|v| !v.is_empty()))
                || env::var("REQUESTED_DECLARATION_PATH")
                    .is_ok_and(|v| !v.is_empty() && v != "cargo-ai-qualification.toml")
            {
                return Err("official qualification uses only the candidate catalog identity");
            }
            return append("GITHUB_OUTPUT", &catalog.resolve_official()?);
        }
        "" | "false" => {}
        _ => return Err("invalid official package selector"),
    }
    let output = catalog.resolve(
        &env::var("REQUESTED_PACKAGE_REPOSITORY").unwrap_or_default(),
        &env::var("REQUESTED_PACKAGE_SHA").unwrap_or_default(),
        &env::var("REQUESTED_DECLARATION_PATH").unwrap_or_default(),
    )?;
    append("GITHUB_OUTPUT", &output)
}

fn main() {
    let args: Vec<_> = env::args().skip(1).collect();
    let result = match args.as_slice() {
        [mode, provider] if mode == "probe" => probe(provider),
        [mode, provider] if mode == "discover" => discover(provider),
        [mode] if mode == "aggregate" => aggregate(),
        [mode] if mode == "catalog" => catalog(),
        [mode] if mode == "package-root" => package_root(),
        _ => Err("usage: qualification-gate probe <provider> | discover <provider> | aggregate | catalog | package-root"),
    };
    if let Err(message) = result {
        // Errors are fixed descriptions, never raw evidence, environment or service responses.
        eprintln!("Qualification failed: {message}.");
        std::process::exit(1);
    }
}

fn package_root() -> Result<()> {
    let path = qualification_paths::confined_file(
        Path::new(&variable("PACKAGE_CHECKOUT")?),
        &variable("PACKAGE_DECLARATION")?,
        64 * 1024,
    )?;
    let parent = path
        .parent()
        .and_then(Path::to_str)
        .ok_or("invalid package directory")?;
    let name = path
        .file_name()
        .and_then(|s| s.to_str())
        .ok_or("invalid declaration name")?;
    if parent.chars().any(char::is_control) {
        return Err("invalid package directory");
    }
    append(
        "GITHUB_OUTPUT",
        &format!("root={parent}\ndeclaration_file={name}\n"),
    )
}
