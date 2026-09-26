//! Read-only requirements derived from explicitly selected, validated definitions.
use clap::ArgMatches;
use serde::Deserialize;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Default, Deserialize)]
struct ProjectSelection {
    #[serde(default)]
    build: BTreeMap<String, BuildSelection>,
}

#[derive(Clone, Default, Deserialize)]
pub(crate) struct BuildSelection {
    #[serde(default)]
    pub(crate) agent_definitions: Vec<String>,
    #[serde(default)]
    pub(crate) hatched_agents: Vec<String>,
    #[serde(default)]
    pub(crate) tools: Vec<String>,
    #[serde(default)]
    pub(crate) assets: Vec<String>,
}

impl BuildSelection {
    fn definitions(&self) -> Vec<String> {
        let mut seen = BTreeSet::new();
        self.agent_definitions
            .iter()
            .chain(&self.hatched_agents)
            .filter(|path| seen.insert((*path).clone()))
            .cloned()
            .collect()
    }
}

pub fn run(matches: &ArgMatches) -> bool {
    let result = if let Some(path) = matches.get_one::<String>("config") {
        inspect_config(Path::new(path))
    } else if let Some(name) = matches.get_one::<String>("build_profile") {
        inspect_project(name)
    } else {
        Err("Choose --config FILE or --build-profile NAME.".to_string())
    };
    match result {
        Ok(report) => {
            print!("{report}");
            true
        }
        Err(error) => {
            eprintln!("x {error}");
            false
        }
    }
}

fn inspect_config(path: &Path) -> Result<String, String> {
    let report = definition_report(path, path.display().to_string())?;
    Ok(render("Explicit agent definition", &[report], &[], &[]))
}

fn inspect_project(name: &str) -> Result<String, String> {
    let cwd = std::env::current_dir().map_err(|e| format!("Cannot read current directory: {e}"))?;
    let root = super::package_dependencies::find_project_root(&cwd)?
        .ok_or("No .cargo-ai/project.toml was found from the current directory upward.")?;
    let metadata_relative = super::local_packages::normalize_portable_relative_path(
        ".cargo-ai/project.toml",
        "Project metadata",
    )?;
    let metadata_path = super::local_packages::resolve_existing_path_under_root(
        &root,
        &metadata_relative,
        "Project metadata",
    )?;
    let source = fs::read_to_string(&metadata_path)
        .map_err(|e| format!("Cannot read '{}': {e}", metadata_path.display()))?;
    let project: ProjectSelection = toml::from_str(&source).map_err(|error: toml::de::Error| {
        let location = error.span().map_or_else(String::new, |span| {
            let preceding = &source.as_bytes()[..span.start.min(source.len())];
            let line = preceding.iter().filter(|byte| **byte == b'\n').count() + 1;
            let byte_column = preceding
                .iter()
                .rposition(|byte| *byte == b'\n')
                .map_or(preceding.len(), |last_newline| {
                    preceding.len() - last_newline - 1
                })
                + 1;
            format!(" at line {line}, byte column {byte_column}")
        });
        format!(
            "Invalid project metadata '{}'{}. Fix TOML syntax or declared field types.",
            metadata_path.display(),
            location
        )
    })?;
    let profile = project.build.get(name).ok_or_else(|| {
        let available = project.build.keys().cloned().collect::<Vec<_>>();
        format!(
            "Build profile '{name}' was not found in '{}'. Available profiles: {}.",
            metadata_path.display(),
            if available.is_empty() {
                "none".to_string()
            } else {
                available.join(", ")
            }
        )
    })?;
    if profile.agent_definitions.is_empty()
        && profile.hatched_agents.is_empty()
        && profile.tools.is_empty()
        && profile.assets.is_empty()
    {
        return Err(format!(
            "Build profile '{name}' declares no agents, tools or assets."
        ));
    }
    render_selection(&root, &format!("Build profile {name}"), profile)
}

pub(crate) fn render_selection(
    root: &Path,
    title: &str,
    selection: &BuildSelection,
) -> Result<String, String> {
    let mut reports = Vec::new();
    for declared in selection.definitions() {
        let relative =
            super::local_packages::normalize_portable_relative_path(&declared, "Agent definition")?;
        let path = super::local_packages::resolve_existing_path_under_root(
            root,
            &relative,
            "Agent definition",
        )?;
        reports.push(definition_report(&path, declared)?);
    }
    Ok(render(title, &reports, &selection.tools, &selection.assets))
}

struct AgentReport {
    label: String,
    requirements: Vec<String>,
    steps: Vec<String>,
    dependencies: Vec<String>,
}

fn definition_report(path: &Path, label: String) -> Result<AgentReport, String> {
    let metadata = fs::metadata(path)
        .map_err(|e| format!("Cannot inspect definition '{}': {e}", path.display()))?;
    if !metadata.is_file() {
        return Err(format!(
            "Definition '{}' must be a regular file.",
            path.display()
        ));
    }
    let source = fs::read_to_string(path)
        .map_err(|e| format!("Cannot read definition '{}': {e}", path.display()))?;
    crate::runtime_definition::RuntimeAgentDefinition::from_str(&source).map_err(|error| {
        // Validation errors can contain authored values. Keep the JSON location,
        // but never echo definition content or prompts in a diagnostic.
        let candidate = error.split(':').next().unwrap_or("");
        let location = if candidate.starts_with('$')
            && candidate.len() <= 120
            && candidate.chars().all(|ch| {
                ch.is_ascii_alphanumeric() || matches!(ch, '$' | '.' | '_' | '[' | ']' | '-')
            }) {
            candidate
        } else {
            "$"
        };
        format!(
            "Invalid definition '{}', near {location}. Check the agent schema and field values.",
            path.display()
        )
    })?;
    let document: Value = serde_json::from_str(&source)
        .map_err(|_| format!("Invalid JSON in definition '{}'.", path.display()))?;
    Ok(extract(label, &document))
}

fn extract(label: String, document: &Value) -> AgentReport {
    let mut requirements = Vec::new();
    let mut steps = Vec::new();
    let mut dependencies = Vec::new();
    let output_properties = document
        .pointer("/agent_schema/properties")
        .and_then(Value::as_object);
    let root_model_runs = output_properties.is_some_and(|properties| !properties.is_empty());
    if let Some(inputs) = document.get("inputs").and_then(Value::as_array) {
        for (index, input) in inputs.iter().enumerate() {
            let kind = input
                .get("type")
                .and_then(Value::as_str)
                .unwrap_or("unknown")
                .trim()
                .to_ascii_lowercase();
            if root_model_runs {
                requirements.push(format!("input {kind} (inputs[{index}])"));
            }
            if matches!(kind.as_str(), "image" | "file") {
                if let Some(path) = input.get("path") {
                    dependencies.push(format!("{kind} input: {}", literal_or_unknown(path)));
                }
            }
        }
    }
    if let Some(properties) = output_properties {
        for (name, property) in properties {
            requirements.push(format!(
                "output {name}: {}",
                schema_features(property, true)
            ));
        }
    }
    if let Some(actions) = document.get("actions").and_then(Value::as_array) {
        for action in actions {
            let action_name = action
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or("unnamed");
            if let Some(run) = action.get("run").and_then(Value::as_array) {
                for (index, step) in run.iter().enumerate() {
                    let kind = step
                        .get("kind")
                        .and_then(Value::as_str)
                        .unwrap_or("unknown");
                    let mut detail = match kind {
                        "generate_image" => {
                            let format = step
                                .get("path")
                                .map(image_format)
                                .unwrap_or("unknown".into());
                            let references = step
                                .get("reference_images")
                                .and_then(Value::as_array)
                                .map(|values| {
                                    values
                                        .iter()
                                        .map(|value| {
                                            if let Some(name) =
                                                value.get("input").and_then(Value::as_str)
                                            {
                                                format!("input {name}")
                                            } else {
                                                format!(
                                                    "path {}",
                                                    value
                                                        .get("path")
                                                        .map(literal_or_unknown)
                                                        .unwrap_or("unknown".into())
                                                )
                                            }
                                        })
                                        .collect::<Vec<_>>()
                                        .join(", ")
                                })
                                .unwrap_or_else(|| "none declared".into());
                            requirements.push(format!(
                                "image generation (format {format}; references {references})"
                            ));
                            if let Some(references) =
                                step.get("reference_images").and_then(Value::as_array)
                            {
                                for reference in references {
                                    if let Some(path) = reference.get("path") {
                                        dependencies.push(format!(
                                            "reference image: {}",
                                            literal_or_unknown(path)
                                        ));
                                    }
                                }
                            }
                            format!(
                                "image generation; output format {format}; references {references}"
                            )
                        }
                        "generate_audio" => {
                            let format = step
                                .get("path")
                                .map(audio_format)
                                .unwrap_or("unknown".into());
                            let voice = step
                                .get("voice")
                                .map(literal_or_unknown)
                                .unwrap_or("unknown".into());
                            requirements.push(format!(
                                "speech generation (format {format}; voice {voice})"
                            ));
                            format!("speech generation; output format {format}; voice {voice}")
                        }
                        "transcribe_audio" => {
                            let source = step
                                .pointer("/audio/path")
                                .map(literal_or_unknown)
                                .unwrap_or("unknown".into());
                            requirements.push(format!("audio transcription (source {source})"));
                            dependencies.push(format!("transcription source: {source}"));
                            format!("audio transcription; source {source}")
                        }
                        "agent" => {
                            let child = step
                                .get("artifact")
                                .or_else(|| step.get("agent"))
                                .and_then(Value::as_str)
                                .unwrap_or("unknown");
                            dependencies.push(format!("child agent: {child} (requirements unassessed unless separately selected)"));
                            format!("child agent {child}; child requirements unassessed")
                        }
                        "tool" => {
                            let tool = step
                                .get("name")
                                .and_then(Value::as_str)
                                .unwrap_or("unknown");
                            dependencies.push(format!("tool: {tool}"));
                            format!("tool {tool}")
                        }
                        _ => kind.to_string(),
                    };
                    for key in ["model", "profile"] {
                        if let Some(value) = step.get(key) {
                            detail.push_str(&format!(
                                "; declared {key} {}",
                                literal_or_unknown(value)
                            ));
                        }
                    }
                    if action.get("logic").is_some() || step.get("when").is_some() {
                        detail.push_str("; conditional applicability unknown");
                    }
                    steps.push(format!("{action_name} / step {}: {detail}", index + 1));
                }
            }
        }
    }
    AgentReport {
        label,
        requirements,
        steps,
        dependencies,
    }
}

fn schema_features(property: &Value, root: bool) -> String {
    let kind = match property.get("type") {
        Some(Value::String(kind)) => kind.clone(),
        Some(Value::Array(kinds)) => kinds
            .iter()
            .filter_map(Value::as_str)
            .collect::<Vec<_>>()
            .join(" | "),
        _ => "structured".to_string(),
    };
    let mut features = vec![kind];
    if let Some(choices) = property.get("enum").and_then(Value::as_array) {
        features.push(format!(
            "choices {}",
            choices
                .iter()
                .map(Value::to_string)
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    if root && property.get("rubric").is_some() {
        features.push("authored rubric score".into());
    }
    for bound in [
        "minimum",
        "maximum",
        "exclusiveMinimum",
        "exclusiveMaximum",
        "minLength",
        "maxLength",
        "minItems",
        "maxItems",
    ] {
        if let Some(value) = property.get(bound) {
            features.push(format!("{bound}={value}"));
        }
    }
    if let Some(items) = property.get("items") {
        features.push(format!("items ({})", schema_features(items, false)));
    }
    if let Some(properties) = property.get("properties").and_then(Value::as_object) {
        let nested = properties
            .iter()
            .map(|(name, value)| format!("{name}: {}", schema_features(value, false)))
            .collect::<Vec<_>>();
        features.push(format!("fields {{{}}}", nested.join("; ")));
    }
    features.join(", ")
}

fn literal_or_unknown(value: &Value) -> String {
    match value {
        Value::String(value) => value.clone(),
        Value::Array(parts) if parts.iter().all(Value::is_string) => {
            parts.iter().filter_map(Value::as_str).collect::<String>()
        }
        Value::Object(map) if map.contains_key("var") => {
            format!(
                "dynamic ({})",
                map.get("var")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown variable")
            )
        }
        _ => "dynamic / unknown".into(),
    }
}

fn image_format(path: &Value) -> String {
    extension(path, &["png", "jpg", "jpeg", "webp"])
}
fn audio_format(path: &Value) -> String {
    extension(path, &["mp3", "wav"])
}

fn extension(path: &Value, known: &[&str]) -> String {
    let raw = match path {
        Value::String(value) => value.clone(),
        Value::Array(parts) if parts.iter().all(Value::is_string) => {
            parts.iter().filter_map(Value::as_str).collect::<String>()
        }
        _ => return "dynamic / unknown".into(),
    };
    let ext = PathBuf::from(raw)
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase);
    match ext {
        Some(ext) if known.contains(&ext.as_str()) => ext,
        _ => "unknown".into(),
    }
}

fn render(title: &str, agents: &[AgentReport], tools: &[String], assets: &[String]) -> String {
    let mut out = format!("Requirements: {title}\n");
    let mut summary: BTreeMap<String, BTreeSet<&str>> = BTreeMap::new();
    if agents.is_empty() {
        out.push_str(
            "No agent definitions are declared; agent workload requirements are unassessed.\n",
        );
    }
    for agent in agents {
        out.push_str(&format!("\nAgent: {}\n", agent.label));
        if agent.requirements.is_empty() {
            out.push_str("  No model-facing requirement declared.\n");
        }
        for item in &agent.requirements {
            out.push_str(&format!("  - {item}\n"));
            let capability = item.split(" (inputs[").next().unwrap_or(item).to_string();
            summary.entry(capability).or_default().insert(&agent.label);
        }
        if !agent.steps.is_empty() {
            out.push_str("  Direct steps:\n");
            for step in &agent.steps {
                out.push_str(&format!("    - {step}\n"));
            }
        }
        if !agent.dependencies.is_empty() {
            out.push_str("  Dependencies:\n");
            for dependency in &agent.dependencies {
                out.push_str(&format!("    - {dependency}\n"));
            }
        }
    }
    if !tools.is_empty() || !assets.is_empty() {
        out.push_str("\nBuild declarations (dependencies, not model capabilities):\n");
        for tool in tools {
            out.push_str(&format!("  - tool: {tool}\n"));
        }
        for asset in assets {
            out.push_str(&format!("  - asset: {asset}\n"));
        }
    }
    out.push_str("\nSelected-agent capability summary:\n");
    if summary.is_empty() {
        out.push_str("  No model-facing capabilities inferred from selected definitions.\n");
    }
    for (item, sources) in summary {
        out.push_str(&format!(
            "  - {item} (agents: {})\n",
            sources.into_iter().collect::<Vec<_>>().join(", ")
        ));
    }
    out.push_str("This report describes declarations only. Conditional steps, dynamic values, child requirements and model support remain unverified; one model need not meet every selected agent's requirements.\n");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn media_requirements_and_structured_choices_keep_their_distinctions() {
        let source: Value = serde_json::json!({
            "inputs": [{"type":"image","path":"./asset.png"}],
            "agent_schema":{"properties":{
                "choice":{"type":"string","enum":["yes","no"]},
                "score":{"type":"number","minimum":0,"maximum":100,"rubric":["low","high"]},
                "count":{"type":"integer","minimum":1}
            }},
            "actions":[{"name":"make","logic":{},"run":[
                {"kind":"generate_image","path":"out.webp","reference_images":[{"input":"photo"}]},
                {"kind":"generate_audio","path":"out.mp3","voice":{"var":"voice"}},
                {"kind":"transcribe_audio","audio":{"path":"input.wav"}}
            ]}]
        });
        let report = render("fixture", &[extract("demo".into(), &source)], &[], &[]);
        for expected in [
            "input image",
            "choices \"yes\", \"no\"",
            "authored rubric score",
            "count: integer, minimum=1",
            "image generation; output format webp",
            "speech generation; output format mp3; voice dynamic",
            "audio transcription; source input.wav",
        ] {
            assert!(report.contains(expected), "missing {expected}: {report}");
        }
        assert!(!report.contains("score: number, minimum=0, maximum=100, authored rubric"));
    }

    #[test]
    fn selection_deduplicates_agent_lists_without_following_children() {
        let selection = BuildSelection {
            agent_definitions: vec!["a.json".into(), "b.json".into()],
            hatched_agents: vec!["a.json".into()],
            tools: vec!["helper".into()],
            assets: vec!["archive.json".into()],
        };
        assert_eq!(selection.definitions(), vec!["a.json", "b.json"]);
    }

    #[test]
    fn child_and_tool_steps_remain_dependencies_with_conditional_unknowns() {
        let source = serde_json::json!({
            "agent_schema": {"properties": {}},
            "actions": [{"name": "delegate", "logic": {"==": [1, 1]}, "run": [
                {"kind": "agent", "artifact": "./child.json", "profile": {"var": "runtime.profile"}},
                {"kind": "tool", "name": "lookup", "when": {"==": [1, 1]}}
            ]}]
        });
        let report = render("fixture", &[extract("parent".into(), &source)], &[], &[]);
        assert!(report.contains("child agent: ./child.json"));
        assert!(report.contains("tool: lookup"));
        assert!(report.contains("declared profile dynamic (runtime.profile)"));
        assert!(report.contains("conditional applicability unknown"));
        assert!(report.contains("No model-facing capabilities inferred"));
    }

    #[test]
    fn action_only_inputs_do_not_imply_root_model_capability() {
        let source = serde_json::json!({
            "inputs": [{"name": "photo", "type": "image", "path": "./photo.png"}],
            "agent_schema": {"properties": {}},
            "actions": [{"name": "make", "logic": {"==": [1, 1]}, "run": [
                {"kind": "generate_image", "path": "./out.png", "reference_images": [{"input": "photo"}]}
            ]}]
        });
        let report = render(
            "fixture",
            &[extract("action_only".into(), &source)],
            &[],
            &[],
        );
        assert!(!report.contains("input image (inputs[0])"));
        assert!(report.contains("image generation"));
        assert!(report.contains("image input: ./photo.png"));
    }
}
