//! Shared build-time parsing, validation, and code generation utilities.
//!
//! This module is compiled in both the root crate build script and scaffolded
//! agent build scripts to keep behavior deterministic across both paths.
use std::{
    collections::BTreeMap,
    env,
    error::Error,
    fmt,
    fs::{self, File},
    io::Write,
    path::{Component, Path, PathBuf},
};

use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};
use uuid::Uuid;

const SCHEMA_VERSION_FORMAT: &str = "YYYY-MM-DD.rN";
const SCHEMA_VERSION_EXAMPLE: &str = "2026-03-03.r1";
const SUPPORTED_ACTION_PLATFORMS: [&str; 3] = ["macos", "linux", "windows"];
const SUPPORTED_FILE_EXTENSIONS: [&str; 24] = [
    "pdf", "docx", "csv", "xla", "xlb", "xlc", "xlm", "xls", "xlsx", "xlt", "xlw", "tsv", "iif",
    "doc", "dot", "odt", "rtf", "pot", "ppa", "pps", "ppt", "pptx", "pwz", "wiz",
];
const SUPPORTED_FILE_EXTENSIONS_MESSAGE: &str = "`.pdf`, `.docx`, `.csv`, `.xla`, `.xlb`, `.xlc`, `.xlm`, `.xls`, `.xlsx`, `.xlt`, `.xlw`, `.tsv`, `.iif`, `.doc`, `.dot`, `.odt`, `.rtf`, `.pot`, `.ppa`, `.pps`, `.ppt`, `.pptx`, `.pwz`, `.wiz`";

#[derive(Debug)]
pub enum BuildError {
    Config {
        path: String,
        message: String,
    },
    #[allow(dead_code)]
    Message(String),
    Io {
        context: String,
        source: std::io::Error,
    },
    EnvVar {
        name: &'static str,
        source: std::env::VarError,
    },
}

impl BuildError {
    fn config(path: impl Into<String>, message: impl Into<String>) -> Self {
        Self::Config {
            path: path.into(),
            message: message.into(),
        }
    }

    fn io(context: impl Into<String>, source: std::io::Error) -> Self {
        Self::Io {
            context: context.into(),
            source,
        }
    }
}

impl fmt::Display for BuildError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Config { path, message } => {
                write!(f, "Invalid `.agentcfg` at `{path}`: {message}")
            }
            Self::Message(message) => write!(f, "{message}"),
            Self::Io { context, source } => write!(f, "{context}: {source}"),
            Self::EnvVar { name, source } => {
                write!(
                    f,
                    "Missing required environment variable `{name}`: {source}"
                )
            }
        }
    }
}

impl Error for BuildError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Config { .. } => None,
            Self::Message(_) => None,
            Self::Io { source, .. } => Some(source),
            Self::EnvVar { source, .. } => Some(source),
        }
    }
}

#[derive(Debug, Clone)]
enum InputSpec {
    Text { text: String },
    Url { url: String },
    Image { path: String },
    File { path: String },
}

#[derive(Debug, Clone)]
enum ActionInputSpec {
    Text { text: Vec<RunArg> },
    Url { url: Vec<RunArg> },
    Image { path: Vec<RunArg> },
    File { path: Vec<RunArg> },
}

#[derive(Debug, Clone)]
struct RunStep {
    kind: String,
    program: Option<String>,
    output_variable: Option<String>,
    args: Vec<RunArg>,
    subject: Option<Vec<RunArg>>,
    text: Option<Vec<RunArg>>,
    agent: Option<String>,
    inputs: Option<Vec<ActionInputSpec>>,
    platforms: Option<Vec<String>>,
}

#[derive(Debug, Clone)]
enum RunArg {
    Literal(String),
    Variable(String),
}

#[derive(Debug, Clone)]
struct Action {
    name: String,
    logic: Value,
    run: Vec<RunStep>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum FieldType {
    String,
    Boolean,
    Number,
    Integer,
    Array,
}

#[derive(Debug, Clone)]
struct MappedPropertyType {
    rust_type: String,
    field_type: FieldType,
}

#[derive(Debug, Clone)]
struct AgentConfig {
    inputs: Vec<InputSpec>,
    fields: Vec<(String, String)>,
    actions: Vec<Action>,
}

// Shared build module: these symbols are used by cargo-ai's root build script,
// but not by scaffolded agent build scripts that only call `run_agent_codegen`.
#[allow(dead_code)]
#[derive(Debug, Clone, Copy)]
pub struct TemplateSource {
    pub destination: &'static str,
    pub source: &'static str,
}

/// Generates `agent_model.rs` from `.agentcfg` for scaffolded agent builds.
#[allow(dead_code)]
pub fn run_agent_codegen(rerun_paths: &[&str]) -> Result<(), BuildError> {
    emit_rerun_paths(rerun_paths);
    let cfg_text = read_agent_config_text()?;
    let generated_code = generate_agent_model_from_str(&cfg_text)?;
    write_out_file("agent_model.rs", &generated_code)?;
    Ok(())
}

/// Generates scaffolded agent build outputs, including per-build provenance.
#[allow(dead_code)]
pub fn run_agent_codegen_with_build_provenance() -> Result<(), BuildError> {
    let cfg_text = read_agent_config_text()?;
    let generated_code = generate_agent_model_from_str(&cfg_text)?;
    let provenance_code = generate_agent_build_provenance_source_with_values(
        &cfg_text,
        &current_target_triple()?,
        &Uuid::new_v4().to_string(),
        &current_build_timestamp_utc()?,
    )?;

    write_out_file("agent_model.rs", &generated_code)?;
    write_out_file("agent_build_provenance.rs", &provenance_code)?;
    Ok(())
}

#[allow(dead_code)]
fn emit_rerun_paths(rerun_paths: &[&str]) {
    for path in rerun_paths {
        println!("cargo:rerun-if-changed={path}");
    }
}

fn read_agent_config_text() -> Result<String, BuildError> {
    let cfg_path = Path::new(".agentcfg");
    fs::read_to_string(cfg_path)
        .map_err(|err| BuildError::io(format!("Failed to read `{}`", cfg_path.display()), err))
}

#[allow(dead_code)]
/// Writes the generated template tuple array used by `agent_builder::project`.
pub fn write_generated_templates(template_sources: &[TemplateSource]) -> Result<(), BuildError> {
    let mut generated = String::new();
    generated.push_str("// Auto-generated by build.rs - do not edit.\n\n");
    generated.push_str("/// Constant array of template files as (path, contents) tuples.\n");
    generated.push_str(&format!(
        "pub const TEMPLATES: [(&str, &str); {}] = [\n",
        template_sources.len()
    ));

    for template in template_sources {
        generated.push_str(&format!(
            "    ({destination:?}, include_str!(concat!(env!(\"CARGO_MANIFEST_DIR\"), \"/templates/{source}\"))),\n",
            destination = template.destination,
            source = template.source
        ));
    }
    generated.push_str("];\n");

    write_out_file(".generated_templates.rs", &generated)
}

/// Parses `.agentcfg` JSON and renders strongly-typed Rust agent model code.
pub fn generate_agent_model_from_str(json_str: &str) -> Result<String, BuildError> {
    let (_, parsed) = parse_and_validate_agent_config(json_str)?;
    Ok(render_agent_model(&parsed))
}

/// Generates scaffolded-agent build provenance constants from `.agentcfg`.
#[allow(dead_code)]
pub fn generate_agent_build_provenance_source_with_values(
    json_str: &str,
    target_triple: &str,
    agent_build_id: &str,
    build_timestamp_utc: &str,
) -> Result<String, BuildError> {
    let (root, _) = parse_and_validate_agent_config(json_str)?;
    let canonical_definition = canonicalize_json_value(&root);
    let embedded_definition_json = serde_json::to_string(&canonical_definition).map_err(|err| {
        BuildError::Message(format!(
            "Failed to serialize canonical agent definition JSON: {err}"
        ))
    })?;
    let definition_sha256 = sha256_hex(&embedded_definition_json);

    let mut generated = String::new();
    generated.push_str("// Auto-generated by build.rs - do not edit.\n\n");
    generated.push_str(&format!(
        "const AGENT_BUILD_ID: &str = {};\n",
        rust_string_literal(agent_build_id)
    ));
    generated.push_str(&format!(
        "const AGENT_TARGET_TRIPLE: &str = {};\n",
        rust_string_literal(target_triple)
    ));
    generated.push_str(&format!(
        "const AGENT_DEFINITION_SHA256: &str = {};\n",
        rust_string_literal(&definition_sha256)
    ));
    generated.push_str(&format!(
        "const AGENT_EMBEDDED_DEFINITION_JSON: &str = {};\n",
        rust_string_literal(&embedded_definition_json)
    ));
    generated.push_str(&format!(
        "const AGENT_BUILD_TIMESTAMP_UTC: &str = {};\n",
        rust_string_literal(build_timestamp_utc)
    ));

    Ok(generated)
}

fn write_out_file(file_name: &str, contents: &str) -> Result<(), BuildError> {
    let out_dir = resolve_out_dir()?;
    let out_path = out_dir.join(file_name);
    let mut file = File::create(&out_path)
        .map_err(|err| BuildError::io(format!("Failed to create `{}`", out_path.display()), err))?;
    file.write_all(contents.as_bytes())
        .map_err(|err| BuildError::io(format!("Failed to write `{}`", out_path.display()), err))
}

fn resolve_out_dir() -> Result<PathBuf, BuildError> {
    env::var("OUT_DIR")
        .map(PathBuf::from)
        .map_err(|source| BuildError::EnvVar {
            name: "OUT_DIR",
            source,
        })
}

fn parse_and_validate_agent_config(json_str: &str) -> Result<(Value, AgentConfig), BuildError> {
    let root: Value = serde_json::from_str(json_str)
        .map_err(|err| BuildError::config("$", format!("invalid JSON syntax: {err}")))?;
    let parsed = parse_agent_config(&root)?;
    Ok((root, parsed))
}

#[allow(dead_code)]
fn current_target_triple() -> Result<String, BuildError> {
    env::var("TARGET").map_err(|source| BuildError::EnvVar {
        name: "TARGET",
        source,
    })
}

#[allow(dead_code)]
fn current_build_timestamp_utc() -> Result<String, BuildError> {
    OffsetDateTime::now_utc().format(&Rfc3339).map_err(|error| {
        BuildError::Message(format!(
            "Failed to format generated-agent build timestamp as RFC 3339 UTC: {error}"
        ))
    })
}

#[allow(dead_code)]
fn canonicalize_json_value(value: &Value) -> Value {
    match value {
        Value::Array(items) => Value::Array(items.iter().map(canonicalize_json_value).collect()),
        Value::Object(map) => {
            let mut keys = map.keys().cloned().collect::<Vec<_>>();
            keys.sort();

            let mut canonical = Map::new();
            for key in keys {
                if let Some(entry) = map.get(&key) {
                    canonical.insert(key, canonicalize_json_value(entry));
                }
            }

            Value::Object(canonical)
        }
        _ => value.clone(),
    }
}

#[allow(dead_code)]
fn sha256_hex(contents: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(contents.as_bytes());
    format!("{:x}", hasher.finalize())
}

fn parse_agent_config(root: &Value) -> Result<AgentConfig, BuildError> {
    let root_obj = expect_object(root, "$")?;

    let schema_version = get_required_string(root_obj, "version", "$")?;
    validate_schema_version(schema_version, "$.version")?;

    let inputs = parse_inputs(root_obj)?;

    let schema = get_required_object(root_obj, "agent_schema", "$")?;
    let schema_type = get_required_string(schema, "type", "$.agent_schema")?;
    if schema_type != "object" {
        return Err(BuildError::config(
            "$.agent_schema.type",
            format!("expected `object`, got `{schema_type}`"),
        ));
    }
    let properties = get_required_object(schema, "properties", "$.agent_schema")?;

    let mut schema_field_types: BTreeMap<String, FieldType> = BTreeMap::new();
    let mut fields = Vec::with_capacity(properties.len());
    for (name, prop_value) in properties {
        validate_rust_identifier(name, &format!("$.agent_schema.properties.{name}"))?;
        let prop_obj = expect_object(prop_value, &format!("$.agent_schema.properties.{name}"))?;
        let mapped_type =
            map_property_type(prop_obj, &format!("$.agent_schema.properties.{name}"))?;
        schema_field_types.insert(name.clone(), mapped_type.field_type);
        fields.push((name.clone(), mapped_type.rust_type));
    }

    let actions = parse_actions(root_obj, &schema_field_types)?;

    Ok(AgentConfig {
        inputs,
        fields,
        actions,
    })
}

fn validate_schema_version(value: &str, path: &str) -> Result<(), BuildError> {
    if is_valid_schema_version(value) {
        return Ok(());
    }

    Err(BuildError::config(
        path,
        format!(
            "expected schema version format `{}` (example: `{}`)",
            SCHEMA_VERSION_FORMAT, SCHEMA_VERSION_EXAMPLE
        ),
    ))
}

fn is_valid_schema_version(value: &str) -> bool {
    let Some((date, revision)) = value.split_once(".r") else {
        return false;
    };

    if !is_valid_date_prefix(date) {
        return false;
    }

    if revision.is_empty() || !revision.chars().all(|ch| ch.is_ascii_digit()) {
        return false;
    }

    revision.parse::<u32>().ok().filter(|n| *n > 0).is_some()
}

fn is_valid_date_prefix(value: &str) -> bool {
    let bytes = value.as_bytes();
    if bytes.len() != 10 {
        return false;
    }

    if bytes[4] != b'-' || bytes[7] != b'-' {
        return false;
    }

    let year = match value[0..4].parse::<u32>() {
        Ok(year) => year,
        Err(_) => return false,
    };
    let month = match value[5..7].parse::<u32>() {
        Ok(month) => month,
        Err(_) => return false,
    };
    let day = match value[8..10].parse::<u32>() {
        Ok(day) => day,
        Err(_) => return false,
    };

    if !(1..=12).contains(&month) {
        return false;
    }

    let max_day = match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => {
            if is_leap_year(year) {
                29
            } else {
                28
            }
        }
        _ => return false,
    };

    (1..=max_day).contains(&day)
}

fn is_leap_year(year: u32) -> bool {
    (year % 4 == 0 && year % 100 != 0) || year % 400 == 0
}

fn parse_inputs(root_obj: &Map<String, Value>) -> Result<Vec<InputSpec>, BuildError> {
    let inputs = get_required_array(root_obj, "inputs", "$")?;
    if inputs.is_empty() {
        return Err(BuildError::config(
            "$.inputs",
            "must contain at least one entry",
        ));
    }

    parse_input_specs(inputs, "$.inputs")
}

fn parse_input_specs(inputs: &[Value], base_path: &str) -> Result<Vec<InputSpec>, BuildError> {
    if inputs.is_empty() {
        return Err(BuildError::config(
            base_path,
            "must contain at least one entry",
        ));
    }

    let mut parsed = Vec::with_capacity(inputs.len());
    for (index, entry) in inputs.iter().enumerate() {
        let path = format!("{base_path}[{index}]");
        let entry_obj = expect_object(entry, &path)?;
        let input_type = get_required_string(entry_obj, "type", &path)?
            .trim()
            .to_ascii_lowercase();

        let input = match input_type.as_str() {
            "text" => InputSpec::Text {
                text: get_required_string(entry_obj, "text", &path)?.to_string(),
            },
            "url" => InputSpec::Url {
                url: get_required_string(entry_obj, "url", &path)?.to_string(),
            },
            "image" => InputSpec::Image {
                path: {
                    let image_path = get_required_string(entry_obj, "path", &path)?
                        .trim()
                        .to_string();
                    validate_definition_owned_local_path(
                        &image_path,
                        &format!("{path}.path"),
                        "image input",
                    )?;
                    image_path
                },
            },
            "file" => InputSpec::File {
                path: {
                    let file_path = get_required_string(entry_obj, "path", &path)?
                        .trim()
                        .to_string();
                    validate_definition_owned_local_path(
                        &file_path,
                        &format!("{path}.path"),
                        "file input",
                    )?;
                    validate_supported_file_extension(
                        &file_path,
                        &format!("{path}.path"),
                        "file input",
                    )?;
                    file_path
                },
            },
            _ => {
                return Err(BuildError::config(
                    format!("{path}.type"),
                    format!(
                        "unsupported input type `{input_type}` (supported: `text`, `url`, `image`, `file`)"
                    ),
                ));
            }
        };

        parsed.push(input);
    }

    Ok(parsed)
}

fn parse_actions(
    root_obj: &Map<String, Value>,
    schema_field_types: &BTreeMap<String, FieldType>,
) -> Result<Vec<Action>, BuildError> {
    let actions = get_required_array(root_obj, "actions", "$")?;
    let mut parsed = Vec::with_capacity(actions.len());

    for (action_idx, action_value) in actions.iter().enumerate() {
        let action_path = format!("$.actions[{action_idx}]");
        let action_obj = expect_object(action_value, &action_path)?;

        let name = get_required_string(action_obj, "name", &action_path)?.to_string();
        let logic = get_required_field(action_obj, "logic", &action_path)?.clone();
        validate_logic_expression(&logic, &format!("{action_path}.logic"), schema_field_types)?;

        let runs = get_required_array(action_obj, "run", &action_path)?;
        let mut run_steps = Vec::with_capacity(runs.len());
        let mut available_field_types = schema_field_types.clone();
        let mut captured_output_names = BTreeMap::new();
        for (run_idx, run_value) in runs.iter().enumerate() {
            let run_path = format!("{action_path}.run[{run_idx}]");
            let run_obj = expect_object(run_value, &run_path)?;

            let platforms = parse_optional_platforms(run_obj, &run_path)?;
            let kind = get_required_string(run_obj, "kind", &run_path)?.to_string();

            let run_step = match kind.as_str() {
                "exec" => {
                    if run_obj.contains_key("subject") {
                        return Err(BuildError::config(
                            format!("{run_path}.subject"),
                            "`subject` is only supported for `email_me` actions",
                        ));
                    }
                    if run_obj.contains_key("text") {
                        return Err(BuildError::config(
                            format!("{run_path}.text"),
                            "`text` is only supported for `email_me` actions",
                        ));
                    }
                    if run_obj.contains_key("agent") {
                        return Err(BuildError::config(
                            format!("{run_path}.agent"),
                            "`agent` is only supported for `agent` actions",
                        ));
                    }
                    if run_obj.contains_key("inputs") {
                        return Err(BuildError::config(
                            format!("{run_path}.inputs"),
                            "`inputs` is only supported for `agent` actions",
                        ));
                    }

                    let program = get_required_string(run_obj, "program", &run_path)?.to_string();
                    if program.trim().is_empty() {
                        return Err(BuildError::config(
                            format!("{run_path}.program"),
                            "must be a non-empty string",
                        ));
                    }

                    let output_variable = parse_optional_output_variable(
                        run_obj,
                        &run_path,
                        schema_field_types,
                        &captured_output_names,
                    )?;
                    let args = parse_run_args(run_obj, &run_path, &available_field_types)?;
                    RunStep {
                        kind,
                        program: Some(program),
                        output_variable,
                        args,
                        subject: None,
                        text: None,
                        agent: None,
                        inputs: None,
                        platforms,
                    }
                }
                "email_me" => {
                    if run_obj.contains_key("program") {
                        return Err(BuildError::config(
                            format!("{run_path}.program"),
                            "`program` is not supported for `email_me` actions",
                        ));
                    }
                    if run_obj.contains_key("args") {
                        return Err(BuildError::config(
                            format!("{run_path}.args"),
                            "`args` is not supported for `email_me` actions",
                        ));
                    }
                    if run_obj.contains_key("agent") {
                        return Err(BuildError::config(
                            format!("{run_path}.agent"),
                            "`agent` is not supported for `email_me` actions",
                        ));
                    }
                    if run_obj.contains_key("inputs") {
                        return Err(BuildError::config(
                            format!("{run_path}.inputs"),
                            "`inputs` is not supported for `email_me` actions",
                        ));
                    }
                    if run_obj.contains_key("output_variable") {
                        return Err(BuildError::config(
                            format!("{run_path}.output_variable"),
                            "`output_variable` is only supported for `exec` actions",
                        ));
                    }

                    let subject = parse_string_parts_field(
                        run_obj,
                        "subject",
                        &run_path,
                        &available_field_types,
                    )?;
                    let text = parse_string_parts_field(
                        run_obj,
                        "text",
                        &run_path,
                        &available_field_types,
                    )?;

                    RunStep {
                        kind,
                        program: None,
                        output_variable: None,
                        args: Vec::new(),
                        subject: Some(subject),
                        text: Some(text),
                        agent: None,
                        inputs: None,
                        platforms,
                    }
                }
                "agent" => {
                    if run_obj.contains_key("program") {
                        return Err(BuildError::config(
                            format!("{run_path}.program"),
                            "`program` is not supported for `agent` actions",
                        ));
                    }
                    if run_obj.contains_key("args") {
                        return Err(BuildError::config(
                            format!("{run_path}.args"),
                            "`args` is not supported for `agent` actions",
                        ));
                    }
                    if run_obj.contains_key("subject") {
                        return Err(BuildError::config(
                            format!("{run_path}.subject"),
                            "`subject` is not supported for `agent` actions",
                        ));
                    }
                    if run_obj.contains_key("text") {
                        return Err(BuildError::config(
                            format!("{run_path}.text"),
                            "`text` is not supported for `agent` actions",
                        ));
                    }
                    if run_obj.contains_key("output_variable") {
                        return Err(BuildError::config(
                            format!("{run_path}.output_variable"),
                            "`output_variable` is only supported for `exec` actions",
                        ));
                    }

                    let agent = get_required_string(run_obj, "agent", &run_path)?
                        .trim()
                        .to_string();
                    let agent_path = format!("{run_path}.agent");
                    validate_child_agent_target(&agent, &agent_path)?;

                    let inputs = match run_obj.get("inputs") {
                        Some(input_value) => {
                            let input_path = format!("{run_path}.inputs");
                            let input_array = input_value.as_array().ok_or_else(|| {
                                BuildError::config(
                                    &input_path,
                                    "expected `inputs` to be an array of ordered input parts",
                                )
                            })?;
                            Some(parse_action_input_specs(
                                input_array,
                                &input_path,
                                &available_field_types,
                            )?)
                        }
                        None => None,
                    };

                    RunStep {
                        kind,
                        program: None,
                        output_variable: None,
                        args: Vec::new(),
                        subject: None,
                        text: None,
                        agent: Some(agent),
                        inputs,
                        platforms,
                    }
                }
                _ => {
                    return Err(BuildError::config(
                        format!("{run_path}.kind"),
                        format!(
                            "unsupported kind `{kind}` (supported: `exec`, `email_me`, `agent`)"
                        ),
                    ));
                }
            };

            if let Some(output_variable) = run_step.output_variable.as_ref() {
                captured_output_names.insert(output_variable.clone(), run_path.clone());
                available_field_types.insert(output_variable.clone(), FieldType::String);
            }
            run_steps.push(run_step);
        }

        parsed.push(Action {
            name,
            logic,
            run: run_steps,
        });
    }

    Ok(parsed)
}

fn parse_optional_output_variable(
    run_obj: &Map<String, Value>,
    run_path: &str,
    schema_field_types: &BTreeMap<String, FieldType>,
    captured_output_names: &BTreeMap<String, String>,
) -> Result<Option<String>, BuildError> {
    let Some(value) = run_obj.get("output_variable") else {
        return Ok(None);
    };

    let output_variable_path = format!("{run_path}.output_variable");
    let output_variable = value.as_str().ok_or_else(|| {
        BuildError::config(
            &output_variable_path,
            "expected `output_variable` to be a non-empty string name",
        )
    })?;
    let normalized = output_variable.trim();
    validate_action_output_variable_name(normalized, &output_variable_path)?;

    if schema_field_types.contains_key(normalized) {
        return Err(BuildError::config(
            &output_variable_path,
            format!(
                "captured output name `{normalized}` collides with an agent output field; choose a different name"
            ),
        ));
    }

    if captured_output_names.contains_key(normalized) {
        return Err(BuildError::config(
            &output_variable_path,
            format!(
                "duplicate captured output name `{normalized}` within this action; choose a unique name"
            ),
        ));
    }

    Ok(Some(normalized.to_string()))
}

fn parse_run_args(
    run_obj: &Map<String, Value>,
    run_path: &str,
    schema_field_types: &BTreeMap<String, FieldType>,
) -> Result<Vec<RunArg>, BuildError> {
    get_required_array(run_obj, "args", run_path)?
        .iter()
        .enumerate()
        .map(|(arg_idx, arg)| {
            parse_run_arg(
                arg,
                &format!("{run_path}.args[{arg_idx}]"),
                schema_field_types,
            )
        })
        .collect()
}

fn validate_action_output_variable_name(name: &str, path: &str) -> Result<(), BuildError> {
    if name.is_empty() {
        return Err(BuildError::config(
            path,
            "output variable name cannot be empty",
        ));
    }

    if name.contains('.') {
        return Err(BuildError::config(
            path,
            "output variable name must be flat; nested names with `.` are not supported",
        ));
    }

    Ok(())
}

fn parse_action_input_specs(
    inputs: &[Value],
    base_path: &str,
    schema_field_types: &BTreeMap<String, FieldType>,
) -> Result<Vec<ActionInputSpec>, BuildError> {
    if inputs.is_empty() {
        return Err(BuildError::config(
            base_path,
            "must contain at least one entry",
        ));
    }

    let mut parsed = Vec::with_capacity(inputs.len());
    for (index, entry) in inputs.iter().enumerate() {
        let path = format!("{base_path}[{index}]");
        let entry_obj = expect_object(entry, &path)?;
        let input_type = get_required_string(entry_obj, "type", &path)?
            .trim()
            .to_ascii_lowercase();

        let input = match input_type.as_str() {
            "text" => ActionInputSpec::Text {
                text: parse_string_parts_value(
                    get_required_field(entry_obj, "text", &path)?,
                    &format!("{path}.text"),
                    schema_field_types,
                )?,
            },
            "url" => ActionInputSpec::Url {
                url: parse_string_parts_value(
                    get_required_field(entry_obj, "url", &path)?,
                    &format!("{path}.url"),
                    schema_field_types,
                )?,
            },
            "image" => {
                let path_parts = parse_string_parts_value(
                    get_required_field(entry_obj, "path", &path)?,
                    &format!("{path}.path"),
                    schema_field_types,
                )?;
                if let Some(resolved_path) = resolve_literal_run_args(&path_parts) {
                    validate_definition_owned_local_path(
                        &resolved_path,
                        &format!("{path}.path"),
                        "image input",
                    )?;
                }
                ActionInputSpec::Image { path: path_parts }
            }
            "file" => {
                let path_parts = parse_string_parts_value(
                    get_required_field(entry_obj, "path", &path)?,
                    &format!("{path}.path"),
                    schema_field_types,
                )?;
                if let Some(resolved_path) = resolve_literal_run_args(&path_parts) {
                    validate_definition_owned_local_path(
                        &resolved_path,
                        &format!("{path}.path"),
                        "file input",
                    )?;
                    validate_supported_file_extension(
                        &resolved_path,
                        &format!("{path}.path"),
                        "file input",
                    )?;
                }
                ActionInputSpec::File { path: path_parts }
            }
            _ => {
                return Err(BuildError::config(
                    format!("{path}.type"),
                    format!(
                        "unsupported input type `{input_type}` (supported: `text`, `url`, `image`, `file`)"
                    ),
                ));
            }
        };

        parsed.push(input);
    }

    Ok(parsed)
}

fn parse_string_parts_field(
    run_obj: &Map<String, Value>,
    field_name: &str,
    run_path: &str,
    schema_field_types: &BTreeMap<String, FieldType>,
) -> Result<Vec<RunArg>, BuildError> {
    let field_path = format!("{run_path}.{field_name}");
    parse_string_parts_value(
        get_required_field(run_obj, field_name, run_path)?,
        &field_path,
        schema_field_types,
    )
}

fn parse_string_parts_value(
    value: &Value,
    field_path: &str,
    schema_field_types: &BTreeMap<String, FieldType>,
) -> Result<Vec<RunArg>, BuildError> {
    match value {
        Value::String(literal) => {
            if literal.trim().is_empty() {
                return Err(BuildError::config(field_path, "must be a non-empty string"));
            }
            Ok(vec![RunArg::Literal(literal.to_string())])
        }
        Value::Array(parts) => {
            if parts.is_empty() {
                return Err(BuildError::config(
                    field_path,
                    "expected at least one string or variable part",
                ));
            }

            parts
                .iter()
                .enumerate()
                .map(|(index, part)| {
                    parse_run_arg(part, &format!("{field_path}[{index}]"), schema_field_types)
                })
                .collect()
        }
        _ => Err(BuildError::config(
            field_path,
            "expected a string or an array of string/variable parts",
        )),
    }
}

fn resolve_literal_run_args(parts: &[RunArg]) -> Option<String> {
    let mut resolved = String::new();

    for part in parts {
        let RunArg::Literal(literal) = part else {
            return None;
        };
        resolved.push_str(literal);
    }

    Some(resolved)
}

fn validate_definition_owned_local_path(
    raw_path: &str,
    path: &str,
    label: &str,
) -> Result<(), BuildError> {
    if raw_path.trim().is_empty() {
        return Err(BuildError::config(
            path,
            format!("{label} path must be a non-empty relative path"),
        ));
    }

    let candidate = Path::new(raw_path);
    if candidate.is_absolute() {
        return Err(BuildError::config(
            path,
            format!("{label} path must be relative and stay at the current level or below"),
        ));
    }

    if path_uses_parent_traversal(candidate) {
        return Err(BuildError::config(
            path,
            format!(
                "{label} path must stay at the current level or below; parent traversal (`..`) is not allowed"
            ),
        ));
    }

    Ok(())
}

fn validate_child_agent_target(raw_agent: &str, path: &str) -> Result<(), BuildError> {
    let agent = raw_agent.trim();
    if agent.is_empty() {
        return Err(BuildError::config(
            path,
            "must use explicit same-level `./childagent` form",
        ));
    }

    let candidate = Path::new(agent);
    if candidate.is_absolute() {
        return Err(BuildError::config(
            path,
            "must use explicit same-level `./childagent` form; absolute paths are not allowed",
        ));
    }

    if path_uses_parent_traversal(candidate) {
        return Err(BuildError::config(
            path,
            "must use explicit same-level `./childagent` form; parent traversal (`..`) is not allowed",
        ));
    }

    if !agent.starts_with("./") {
        let message = if contains_path_separator(agent) {
            "must use explicit same-level `./childagent` form; nested child-agent paths are not allowed"
        } else {
            "must use explicit same-level `./childagent` form; bare child-agent names are not allowed"
        };
        return Err(BuildError::config(path, message));
    }

    let sibling = &agent[2..];
    if sibling.is_empty() || !is_single_normal_path_component(sibling) {
        return Err(BuildError::config(
            path,
            "must stay at the same level; nested child-agent paths such as `./agents/childagent` are not allowed",
        ));
    }

    Ok(())
}

fn validate_supported_file_extension(
    raw_path: &str,
    path: &str,
    label: &str,
) -> Result<(), BuildError> {
    let extension = Path::new(raw_path)
        .extension()
        .and_then(|value| value.to_str())
        .map(|value| value.to_ascii_lowercase());

    match extension.as_deref() {
        Some(extension) if SUPPORTED_FILE_EXTENSIONS.contains(&extension) => Ok(()),
        _ => Err(BuildError::config(
            path,
            format!(
                "{label} path must use a supported extension: {SUPPORTED_FILE_EXTENSIONS_MESSAGE}"
            ),
        )),
    }
}

fn path_uses_parent_traversal(path: &Path) -> bool {
    path.components()
        .any(|component| matches!(component, Component::ParentDir))
}

fn contains_path_separator(path: &str) -> bool {
    path.contains('/') || path.contains('\\')
}

fn is_single_normal_path_component(path: &str) -> bool {
    let mut components = Path::new(path).components();
    matches!(components.next(), Some(Component::Normal(_))) && components.next().is_none()
}

fn parse_run_arg(
    value: &Value,
    path: &str,
    schema_field_types: &BTreeMap<String, FieldType>,
) -> Result<RunArg, BuildError> {
    match value {
        Value::String(literal) => Ok(RunArg::Literal(literal.to_string())),
        Value::Object(map) => {
            if map.len() != 1 {
                return Err(BuildError::config(
                    path,
                    "expected an arg object with exactly one key (`var`)",
                ));
            }

            let Some((key, variable_value)) = map.iter().next() else {
                return Err(BuildError::config(
                    path,
                    "expected an arg object with exactly one key (`var`)",
                ));
            };

            if key != "var" {
                return Err(BuildError::config(
                    path,
                    format!("unsupported arg object key `{key}` (supported: `var`)"),
                ));
            }

            let variable_path = format!("{path}.var");
            let variable_name = variable_value.as_str().ok_or_else(|| {
                BuildError::config(&variable_path, "expected `var` to be a string field name")
            })?;
            let normalized_name = variable_name.trim();
            if normalized_name.is_empty() {
                return Err(BuildError::config(
                    &variable_path,
                    "variable name cannot be empty",
                ));
            }

            let field_type = resolve_var_field_type(
                &Value::String(normalized_name.to_string()),
                schema_field_types,
                &variable_path,
            )?;
            if field_type == FieldType::Array {
                return Err(BuildError::config(
                    &variable_path,
                    format!(
                        "array-valued field `{}` cannot be used as an action arg variable in this story",
                        normalized_name
                    ),
                ));
            }

            Ok(RunArg::Variable(normalized_name.to_string()))
        }
        _ => Err(BuildError::config(
            path,
            "expected a string literal arg or an object of the form `{ \"var\": \"field_name\" }`",
        )),
    }
}

fn parse_optional_platforms(
    run_obj: &Map<String, Value>,
    run_path: &str,
) -> Result<Option<Vec<String>>, BuildError> {
    let Some(platform_value) = run_obj.get("platform") else {
        return Ok(None);
    };

    let platform_path = format!("{run_path}.platform");
    match platform_value {
        Value::String(platform) => Ok(Some(vec![normalize_platform_value(
            platform,
            &platform_path,
        )?])),
        Value::Array(platforms) => {
            if platforms.is_empty() {
                return Err(BuildError::config(
                    &platform_path,
                    "expected at least one platform entry",
                ));
            }

            let mut normalized = Vec::with_capacity(platforms.len());
            for (index, platform_value) in platforms.iter().enumerate() {
                let entry_path = format!("{platform_path}[{index}]");
                let Some(platform) = platform_value.as_str() else {
                    return Err(BuildError::config(
                        &entry_path,
                        "expected a string platform value",
                    ));
                };

                let normalized_platform = normalize_platform_value(platform, &entry_path)?;
                if normalized.contains(&normalized_platform) {
                    return Err(BuildError::config(
                        &entry_path,
                        format!(
                            "duplicate platform `{}` after normalization",
                            normalized_platform
                        ),
                    ));
                }
                normalized.push(normalized_platform);
            }

            Ok(Some(normalized))
        }
        _ => Err(BuildError::config(
            &platform_path,
            "expected a string platform value or an array of string platform values",
        )),
    }
}

fn normalize_platform_value(value: &str, path: &str) -> Result<String, BuildError> {
    let normalized = value.trim().to_ascii_lowercase();
    if normalized.is_empty() {
        return Err(BuildError::config(
            path,
            "expected a non-empty platform value",
        ));
    }

    if SUPPORTED_ACTION_PLATFORMS.contains(&normalized.as_str()) {
        return Ok(normalized);
    }

    Err(BuildError::config(
        path,
        format!(
            "unsupported platform `{}` (supported: `macos`, `linux`, `windows`)",
            value
        ),
    ))
}

fn validate_logic_expression(
    value: &Value,
    path: &str,
    schema_field_types: &BTreeMap<String, FieldType>,
) -> Result<(), BuildError> {
    match value {
        Value::Object(map) => {
            if map.is_empty() {
                return Err(BuildError::config(
                    path,
                    "expected a non-empty JSON Logic object",
                ));
            }

            if map.len() != 1 {
                return Err(BuildError::config(
                    path,
                    "expected a JSON Logic object with exactly one operator key",
                ));
            }

            let Some((operator, arguments)) = map.iter().next() else {
                return Err(BuildError::config(
                    path,
                    "expected a non-empty JSON Logic object",
                ));
            };
            if operator.trim().is_empty() {
                return Err(BuildError::config(path, "operator name cannot be empty"));
            }

            if operator == "literal" {
                return Ok(());
            }

            if operator == "var" {
                resolve_var_field_type(arguments, schema_field_types, &format!("{path}.var"))?;
                return Ok(());
            }

            let operator_path = format!("{path}.{operator}");
            validate_logic_arguments(arguments, &operator_path, schema_field_types)?;
            validate_operator_type_constraints(
                operator,
                arguments,
                &operator_path,
                schema_field_types,
            )
        }
        _ => Err(BuildError::config(
            path,
            "expected a JSON Logic object expression",
        )),
    }
}

fn validate_logic_arguments(
    value: &Value,
    path: &str,
    schema_field_types: &BTreeMap<String, FieldType>,
) -> Result<(), BuildError> {
    match value {
        Value::Array(items) => {
            for (idx, item) in items.iter().enumerate() {
                if item.is_object() {
                    validate_logic_expression(item, &format!("{path}[{idx}]"), schema_field_types)?;
                }
            }
            Ok(())
        }
        Value::Object(_) => validate_logic_expression(value, path, schema_field_types),
        _ => Ok(()),
    }
}

fn validate_operator_type_constraints(
    operator: &str,
    arguments: &Value,
    operator_path: &str,
    schema_field_types: &BTreeMap<String, FieldType>,
) -> Result<(), BuildError> {
    if !matches!(operator, "==" | "!=" | ">" | ">=" | "<" | "<=") {
        return Ok(());
    }

    let operands = arguments
        .as_array()
        .ok_or_else(|| BuildError::config(operator_path, "expected an array of operands"))?;

    if operands.len() != 2 {
        return Err(BuildError::config(
            operator_path,
            "expected exactly two operands",
        ));
    }

    let left_path = format!("{operator_path}[0]");
    let right_path = format!("{operator_path}[1]");

    let left = infer_logic_value_type(&operands[0], schema_field_types, &left_path)?;
    let right = infer_logic_value_type(&operands[1], schema_field_types, &right_path)?;

    match operator {
        ">" | ">=" | "<" | "<=" => {
            if !is_numeric_type(&left) {
                return Err(BuildError::config(
                    left_path,
                    "expected a numeric operand for comparison",
                ));
            }
            if !is_numeric_type(&right) {
                return Err(BuildError::config(
                    right_path,
                    "expected a numeric operand for comparison",
                ));
            }
        }
        "==" | "!=" => {
            if !are_compatible_types(&left, &right) {
                return Err(BuildError::config(
                    operator_path,
                    format!(
                        "incompatible operand types for `{operator}` ({}, {})",
                        type_name(&left),
                        type_name(&right)
                    ),
                ));
            }
        }
        _ => {}
    }

    Ok(())
}

fn infer_logic_value_type(
    value: &Value,
    schema_field_types: &BTreeMap<String, FieldType>,
    path: &str,
) -> Result<FieldType, BuildError> {
    match value {
        Value::String(_) => Ok(FieldType::String),
        Value::Bool(_) => Ok(FieldType::Boolean),
        Value::Number(n) => {
            if n.is_i64() || n.is_u64() {
                Ok(FieldType::Integer)
            } else {
                Ok(FieldType::Number)
            }
        }
        Value::Array(_) => Ok(FieldType::Array),
        Value::Null => Err(BuildError::config(
            path,
            "null operands are not supported here",
        )),
        Value::Object(map) => {
            if map.len() != 1 {
                return Err(BuildError::config(
                    path,
                    "expected a JSON Logic object with exactly one operator key",
                ));
            }

            let Some((operator, arguments)) = map.iter().next() else {
                return Err(BuildError::config(
                    path,
                    "expected a JSON Logic object with exactly one operator key",
                ));
            };
            if operator == "var" {
                return resolve_var_field_type(
                    arguments,
                    schema_field_types,
                    &format!("{path}.var"),
                );
            }

            if matches!(operator.as_str(), "==" | "!=" | ">" | ">=" | "<" | "<=") {
                validate_operator_type_constraints(
                    operator,
                    arguments,
                    &format!("{path}.{operator}"),
                    schema_field_types,
                )?;
                return Ok(FieldType::Boolean);
            }

            if matches!(operator.as_str(), "and" | "or" | "!") {
                return Ok(FieldType::Boolean);
            }

            if operator == "literal" {
                return infer_logic_value_type(arguments, schema_field_types, path);
            }

            Ok(FieldType::String)
        }
    }
}

fn resolve_var_field_type(
    arguments: &Value,
    schema_field_types: &BTreeMap<String, FieldType>,
    path: &str,
) -> Result<FieldType, BuildError> {
    let var_name = match arguments {
        Value::String(name) => name.as_str(),
        Value::Array(items) => items.first().and_then(|v| v.as_str()).ok_or_else(|| {
            BuildError::config(path, "expected first `var` argument to be a string")
        })?,
        _ => {
            return Err(BuildError::config(
                path,
                "expected `var` arguments as a string or array",
            ));
        }
    };

    if var_name.trim().is_empty() {
        return Err(BuildError::config(path, "variable name cannot be empty"));
    }

    if var_name.contains('.') {
        return Err(BuildError::config(
            path,
            "nested variable paths are not supported; use top-level schema property names",
        ));
    }

    schema_field_types.get(var_name).cloned().ok_or_else(|| {
        BuildError::config(
            path,
            format!(
                "unknown variable `{var_name}`; expected one of: {}",
                schema_field_types
                    .keys()
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        )
    })
}

fn is_numeric_type(field_type: &FieldType) -> bool {
    matches!(field_type, FieldType::Number | FieldType::Integer)
}

fn are_compatible_types(left: &FieldType, right: &FieldType) -> bool {
    left == right || (is_numeric_type(left) && is_numeric_type(right))
}

fn type_name(field_type: &FieldType) -> &'static str {
    match field_type {
        FieldType::String => "string",
        FieldType::Boolean => "boolean",
        FieldType::Number => "number",
        FieldType::Integer => "integer",
        FieldType::Array => "array",
    }
}

fn map_property_type(
    property: &Map<String, Value>,
    path: &str,
) -> Result<MappedPropertyType, BuildError> {
    let schema_type = get_schema_type(property, path)?;
    match schema_type.as_str() {
        "string" => Ok(MappedPropertyType {
            rust_type: "String".to_string(),
            field_type: FieldType::String,
        }),
        "boolean" => Ok(MappedPropertyType {
            rust_type: "bool".to_string(),
            field_type: FieldType::Boolean,
        }),
        "number" => Ok(MappedPropertyType {
            rust_type: "f64".to_string(),
            field_type: FieldType::Number,
        }),
        "integer" => Ok(MappedPropertyType {
            rust_type: "i64".to_string(),
            field_type: FieldType::Integer,
        }),
        "array" => map_array_type(property, path),
        "object" => Err(BuildError::config(
            format!("{path}.type"),
            "nested object fields are not supported yet",
        )),
        other => Err(BuildError::config(
            format!("{path}.type"),
            format!("unsupported schema type `{other}`"),
        )),
    }
}

fn map_array_type(
    property: &Map<String, Value>,
    path: &str,
) -> Result<MappedPropertyType, BuildError> {
    let items_value = get_required_field(property, "items", path)?;
    let items_path = format!("{path}.items");
    let items_obj = expect_object(items_value, &items_path)?;
    let item_type = get_schema_type(items_obj, &items_path)?;

    let primitive = match item_type.as_str() {
        "string" => "String",
        "boolean" => "bool",
        "number" => "f64",
        "integer" => "i64",
        "array" => {
            return Err(BuildError::config(
                format!("{items_path}.type"),
                "nested arrays are not supported yet",
            ));
        }
        "object" => {
            return Err(BuildError::config(
                format!("{items_path}.type"),
                "array items of type `object` are not supported yet",
            ));
        }
        other => {
            return Err(BuildError::config(
                format!("{items_path}.type"),
                format!("unsupported array item schema type `{other}`"),
            ));
        }
    };

    Ok(MappedPropertyType {
        rust_type: format!("Vec<{primitive}>"),
        field_type: FieldType::Array,
    })
}

fn get_schema_type(property: &Map<String, Value>, path: &str) -> Result<String, BuildError> {
    let type_value = get_required_field(property, "type", path)?;
    let type_path = format!("{path}.type");
    match type_value {
        Value::String(schema_type) => Ok(schema_type.clone()),
        Value::Array(_) => Err(BuildError::config(
            type_path,
            "union schema types are not supported yet",
        )),
        _ => Err(BuildError::config(
            type_path,
            "expected a string schema type",
        )),
    }
}

fn get_required_field<'a>(
    obj: &'a Map<String, Value>,
    key: &str,
    parent_path: &str,
) -> Result<&'a Value, BuildError> {
    obj.get(key).ok_or_else(|| {
        BuildError::config(
            format!("{parent_path}.{key}"),
            format!("missing required field `{key}`"),
        )
    })
}

fn get_required_string<'a>(
    obj: &'a Map<String, Value>,
    key: &str,
    parent_path: &str,
) -> Result<&'a str, BuildError> {
    let path = format!("{parent_path}.{key}");
    let value = get_required_field(obj, key, parent_path)?;
    value
        .as_str()
        .ok_or_else(|| BuildError::config(path, "expected a string"))
}

fn get_required_array<'a>(
    obj: &'a Map<String, Value>,
    key: &str,
    parent_path: &str,
) -> Result<&'a [Value], BuildError> {
    let path = format!("{parent_path}.{key}");
    let value = get_required_field(obj, key, parent_path)?;
    value
        .as_array()
        .map(Vec::as_slice)
        .ok_or_else(|| BuildError::config(path, "expected an array"))
}

fn get_required_object<'a>(
    obj: &'a Map<String, Value>,
    key: &str,
    parent_path: &str,
) -> Result<&'a Map<String, Value>, BuildError> {
    let path = format!("{parent_path}.{key}");
    let value = get_required_field(obj, key, parent_path)?;
    expect_object(value, &path)
}

fn expect_object<'a>(value: &'a Value, path: &str) -> Result<&'a Map<String, Value>, BuildError> {
    value
        .as_object()
        .ok_or_else(|| BuildError::config(path, "expected an object"))
}

fn validate_rust_identifier(name: &str, path: &str) -> Result<(), BuildError> {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return Err(BuildError::config(path, "field name cannot be empty"));
    };

    if first != '_' && !first.is_ascii_alphabetic() {
        return Err(BuildError::config(
            path,
            format!(
                "field name `{name}` must start with an ASCII letter or underscore for Rust codegen"
            ),
        ));
    }

    if !chars.all(|ch| ch == '_' || ch.is_ascii_alphanumeric()) {
        return Err(BuildError::config(
            path,
            format!(
                "field name `{name}` must contain only ASCII letters, digits, or underscores for Rust codegen"
            ),
        ));
    }

    if is_rust_keyword(name) {
        return Err(BuildError::config(
            path,
            format!("field name `{name}` is a reserved Rust keyword"),
        ));
    }

    Ok(())
}

fn is_rust_keyword(name: &str) -> bool {
    matches!(
        name,
        "as" | "break"
            | "const"
            | "continue"
            | "crate"
            | "else"
            | "enum"
            | "extern"
            | "false"
            | "fn"
            | "for"
            | "if"
            | "impl"
            | "in"
            | "let"
            | "loop"
            | "match"
            | "mod"
            | "move"
            | "mut"
            | "pub"
            | "ref"
            | "return"
            | "self"
            | "Self"
            | "static"
            | "struct"
            | "super"
            | "trait"
            | "true"
            | "type"
            | "unsafe"
            | "use"
            | "where"
            | "while"
            | "async"
            | "await"
            | "dyn"
            | "union"
            | "abstract"
            | "become"
            | "box"
            | "do"
            | "final"
            | "macro"
            | "override"
            | "priv"
            | "try"
            | "typeof"
            | "unsized"
            | "virtual"
            | "yield"
            | "gen"
    )
}

fn render_run_arg(arg: &RunArg) -> String {
    match arg {
        RunArg::Literal(literal) => format!(
            "RunArg::Literal({}.to_string())",
            rust_string_literal(literal)
        ),
        RunArg::Variable(variable) => format!(
            "RunArg::Variable({}.to_string())",
            rust_string_literal(variable)
        ),
    }
}

fn render_run_arg_parts(parts: &[RunArg]) -> String {
    parts
        .iter()
        .map(render_run_arg)
        .collect::<Vec<_>>()
        .join(", ")
}

fn render_agent_model(config: &AgentConfig) -> String {
    let mut struct_fields = String::new();
    for (name, rust_type) in &config.fields {
        struct_fields.push_str(&format!("    pub {name}: {rust_type},\n"));
    }

    let mut input_list = String::new();
    for input in &config.inputs {
        let rendered = match input {
            InputSpec::Text { text } => format!(
                "        Input::Text {{ text: {}.to_string() }},\n",
                rust_string_literal(text)
            ),
            InputSpec::Url { url } => format!(
                "        Input::Url {{ url: {}.to_string() }},\n",
                rust_string_literal(url)
            ),
            InputSpec::Image { path } => format!(
                "        Input::Image {{ path: {}.to_string() }},\n",
                rust_string_literal(path)
            ),
            InputSpec::File { path } => format!(
                "        Input::File {{ path: {}.to_string() }},\n",
                rust_string_literal(path)
            ),
        };
        input_list.push_str(&rendered);
    }

    let mut action_code = String::new();
    for action in &config.actions {
        let logic_json = action.logic.to_string();
        let run_steps = action
            .run
            .iter()
            .map(|run_step| {
                let args = render_run_arg_parts(&run_step.args);
                let subject = run_step
                    .subject
                    .as_ref()
                    .map(|parts| format!("Some(vec![{}])", render_run_arg_parts(parts)))
                    .unwrap_or_else(|| "None".to_string());
                let text = run_step
                    .text
                    .as_ref()
                    .map(|parts| format!("Some(vec![{}])", render_run_arg_parts(parts)))
                    .unwrap_or_else(|| "None".to_string());
                let output_variable = run_step
                    .output_variable
                    .as_ref()
                    .map(|name| format!("Some({}.to_string())", rust_string_literal(name)))
                    .unwrap_or_else(|| "None".to_string());
                let agent = run_step
                    .agent
                    .as_ref()
                    .map(|agent| format!("Some({}.to_string())", rust_string_literal(agent)))
                    .unwrap_or_else(|| "None".to_string());
                let inputs = run_step
                    .inputs
                    .as_ref()
                    .map(|inputs| {
                        let rendered = inputs
                            .iter()
                            .map(|input| match input {
                                ActionInputSpec::Text { text } => format!(
                                    "ActionInput::Text {{ text: vec![{}] }}",
                                    render_run_arg_parts(text)
                                ),
                                ActionInputSpec::Url { url } => format!(
                                    "ActionInput::Url {{ url: vec![{}] }}",
                                    render_run_arg_parts(url)
                                ),
                                ActionInputSpec::Image { path } => format!(
                                    "ActionInput::Image {{ path: vec![{}] }}",
                                    render_run_arg_parts(path)
                                ),
                                ActionInputSpec::File { path } => format!(
                                    "ActionInput::File {{ path: vec![{}] }}",
                                    render_run_arg_parts(path)
                                ),
                            })
                            .collect::<Vec<_>>()
                            .join(", ");
                        format!("Some(vec![{}])", rendered)
                    })
                    .unwrap_or_else(|| "None".to_string());
                let platforms = run_step
                    .platforms
                    .as_ref()
                    .map(|platforms| {
                        let rendered = platforms
                            .iter()
                            .map(|platform| {
                                format!("{}.to_string()", rust_string_literal(platform))
                            })
                            .collect::<Vec<_>>()
                            .join(", ");
                        format!("Some(vec![{}])", rendered)
                    })
                    .unwrap_or_else(|| "None".to_string());

                format!(
                    "RunStep {{
                        kind: {}.to_string(),
                        program: {},
                        output_variable: {},
                        args: vec![{}],
                        subject: {},
                        text: {},
                        agent: {},
                        inputs: {},
                        platforms: {},
                    }}",
                    rust_string_literal(&run_step.kind),
                    run_step
                        .program
                        .as_ref()
                        .map(|program| {
                            format!("Some({}.to_string())", rust_string_literal(program))
                        })
                        .unwrap_or_else(|| "None".to_string()),
                    output_variable,
                    args,
                    subject,
                    text,
                    agent,
                    inputs,
                    platforms
                )
            })
            .collect::<Vec<_>>()
            .join(",");

        action_code.push_str(&format!(
            "Action {{
                name: {}.to_string(),
                logic: serde_json::from_str({}).expect(\"generated action logic must be valid JSON\"),
                run: vec![{}],
            }},",
            rust_string_literal(&action.name),
            rust_string_literal(&logic_json),
            run_steps
        ));
    }

    format!(
        r##"
use schemars::{{JsonSchema, schema_for}};
use serde_json;

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
pub struct Output {{
{struct_fields}}}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub enum Input {{
    Text {{ text: String }},
    Url {{ url: String }},
    Image {{ path: String }},
    File {{ path: String }},
}}

pub fn inputs() -> Vec<Input> {{
    vec![{input_list}]
}}

/// JSON Schema that defines the expected LLM output structure.
/// Derived from the `Output` struct to ensure single source of truth.
/// Returned as a `serde_json::Value` for direct API use.
pub fn json_schema_value() -> serde_json::Value {{
    let schema = schema_for!(Output);
    let mut v = serde_json::to_value(&schema).expect("Failed to serialize derived schema");

    if let Some(obj) = v.as_object_mut() {{
        // Ensure strictness expected by some providers
        obj.entry("additionalProperties")
            .or_insert(serde_json::Value::Bool(false));

        // Add required array based on properties if not already present
        if let Some(props) = obj.get("properties").and_then(|p| p.as_object()) {{
            let required_fields: Vec<serde_json::Value> =
                props.keys().map(|k| serde_json::Value::String(k.clone())).collect();
            obj.insert("required".to_string(), serde_json::Value::Array(required_fields));
        }}
    }}

    v
}}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub enum ActionInput {{
    Text {{ text: Vec<RunArg> }},
    Url {{ url: Vec<RunArg> }},
    Image {{ path: Vec<RunArg> }},
    File {{ path: Vec<RunArg> }},
}}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct RunStep {{
    kind: String,
    program: Option<String>,
    output_variable: Option<String>,
    args: Vec<RunArg>,
    subject: Option<Vec<RunArg>>,
    text: Option<Vec<RunArg>>,
    agent: Option<String>,
    inputs: Option<Vec<ActionInput>>,
    platforms: Option<Vec<String>>,
}}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub enum RunArg {{
    Literal(String),
    Variable(String),
}}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Action {{
    name: String,
    logic: serde_json::Value,
    run: Vec<RunStep>,
}}

pub fn actions() -> Vec<Action> {{
    vec![
{action_code}    ]
}}
"##,
        struct_fields = struct_fields,
        input_list = input_list,
        action_code = action_code
    )
}

fn rust_string_literal(value: &str) -> String {
    format!("{value:?}")
}

#[cfg(test)]
mod tests {
    use super::generate_agent_model_from_str;

    fn config_with_child_agent_target(target: &str) -> String {
        let encoded_target =
            serde_json::to_string(target).expect("child agent target should encode as JSON");
        format!(
            r#"{{
    "version": "2026-03-03.r1",
    "inputs": [
        {{ "type": "text", "text": "Test prompt" }}
    ],
    "agent_schema": {{
        "type": "object",
        "properties": {{
            "ok": {{ "type": "boolean" }}
        }}
    }},
    "actions": [
        {{
            "name": "invoke_child",
            "logic": {{ "==": [ {{ "var": "ok" }}, true ] }},
            "run": [
                {{ "kind": "agent", "agent": {encoded_target} }}
            ]
        }}
    ]
}}"#
        )
    }

    #[test]
    fn accepts_explicit_same_level_child_agent_target() {
        let generated =
            generate_agent_model_from_str(&config_with_child_agent_target("./childagent"))
                .expect("explicit same-level child agent target should compile");

        assert!(generated.contains("agent: Some(\"./childagent\".to_string())"));
    }

    #[test]
    fn rejects_bare_child_agent_target() {
        let error = generate_agent_model_from_str(&config_with_child_agent_target("childagent"))
            .expect_err("bare child agent names should be rejected")
            .to_string();

        assert!(error.contains("bare child-agent names are not allowed"));
    }

    #[test]
    fn rejects_nested_child_agent_target() {
        let error =
            generate_agent_model_from_str(&config_with_child_agent_target("./agents/childagent"))
                .expect_err("nested child agent paths should be rejected")
                .to_string();

        assert!(error.contains("nested child-agent paths"));
    }

    #[test]
    fn rejects_parent_traversal_child_agent_target() {
        let error =
            generate_agent_model_from_str(&config_with_child_agent_target("./../childagent"))
                .expect_err("parent traversal should be rejected")
                .to_string();

        assert!(error.contains("parent traversal"));
    }
}
