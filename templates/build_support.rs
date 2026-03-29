//! Shared build-time parsing, validation, and code generation utilities.
//!
//! This module is compiled in both the root crate build script and scaffolded
//! agent build scripts to keep behavior deterministic across both paths.
use std::{
    collections::{BTreeMap, BTreeSet},
    env,
    error::Error,
    fmt,
    fs::{self, File},
    io::Write,
    path::{Component, Path, PathBuf},
};

use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use time::{format_description::well_known::Rfc3339, OffsetDateTime};
use uuid::Uuid;

const SCHEMA_VERSION_FORMAT: &str = "YYYY-MM-DD.rN";
const SCHEMA_VERSION_EXAMPLE: &str = "2026-03-03.r1";
const SUPPORTED_ACTION_PLATFORMS: [&str; 3] = ["macos", "linux", "windows"];
const SUPPORTED_FILE_EXTENSIONS: [&str; 24] = [
    "pdf", "docx", "csv", "xla", "xlb", "xlc", "xlm", "xls", "xlsx", "xlt", "xlw", "tsv", "iif",
    "doc", "dot", "odt", "rtf", "pot", "ppa", "pps", "ppt", "pptx", "pwz", "wiz",
];
const SUPPORTED_FILE_EXTENSIONS_MESSAGE: &str = "`.pdf`, `.docx`, `.csv`, `.xla`, `.xlb`, `.xlc`, `.xlm`, `.xls`, `.xlsx`, `.xlt`, `.xlw`, `.tsv`, `.iif`, `.doc`, `.dot`, `.odt`, `.rtf`, `.pot`, `.ppa`, `.pps`, `.ppt`, `.pptx`, `.pwz`, `.wiz`";
const SUPPORTED_GENERATED_IMAGE_EXTENSIONS: [&str; 4] = ["png", "jpg", "jpeg", "webp"];
const SUPPORTED_GENERATED_IMAGE_EXTENSIONS_MESSAGE: &str = "`.png`, `.jpg`, `.jpeg`, `.webp`";

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
    model: Option<RunArg>,
    output_variable: Option<String>,
    status_variable: Option<String>,
    error_variable: Option<String>,
    failure_mode: Option<FailureMode>,
    when: Option<Value>,
    args: Vec<RunArg>,
    prompt: Option<Vec<RunArg>>,
    path: Option<Vec<RunArg>>,
    subject: Option<Vec<RunArg>>,
    text: Option<Vec<RunArg>>,
    agent: Option<String>,
    inputs: Option<Vec<ActionInputSpec>>,
    input_mode: Option<ActionInputMode>,
    platforms: Option<Vec<String>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FailureMode {
    Stop,
    Continue,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ActionInputMode {
    Replace,
    Append,
    Prepend,
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

#[derive(Debug, Clone)]
struct RuntimeVarSpec {
    name: String,
    field_type: FieldType,
    default_value: Option<Value>,
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
enum NumericConstraintValue {
    Number(f64),
    Integer(i64),
}

#[derive(Debug, Clone, Default)]
struct NumericConstraints {
    minimum: Option<NumericConstraintValue>,
    maximum: Option<NumericConstraintValue>,
    exclusive_minimum: Option<NumericConstraintValue>,
    exclusive_maximum: Option<NumericConstraintValue>,
}

impl NumericConstraints {
    fn has_any(&self) -> bool {
        self.minimum.is_some()
            || self.maximum.is_some()
            || self.exclusive_minimum.is_some()
            || self.exclusive_maximum.is_some()
    }
}

#[derive(Debug, Clone)]
struct AgentProperty {
    name: String,
    rust_type: String,
    field_type: FieldType,
    description: Option<String>,
    enum_values: Option<Vec<String>>,
    numeric_constraints: NumericConstraints,
}

#[derive(Debug, Clone)]
struct AgentConfig {
    inputs: Vec<InputSpec>,
    runtime_vars: Vec<RuntimeVarSpec>,
    properties: Vec<AgentProperty>,
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
    let runtime_vars = parse_runtime_vars(root_obj)?;

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
    let mut parsed_properties = Vec::with_capacity(properties.len());
    for (name, prop_value) in properties {
        validate_reserved_top_level_name(name, &format!("$.agent_schema.properties.{name}"))?;
        validate_rust_identifier(name, &format!("$.agent_schema.properties.{name}"))?;
        let prop_obj = expect_object(prop_value, &format!("$.agent_schema.properties.{name}"))?;
        let parsed_property =
            parse_agent_property(name, prop_obj, &format!("$.agent_schema.properties.{name}"))?;
        schema_field_types.insert(name.clone(), parsed_property.field_type.clone());
        parsed_properties.push(parsed_property);
    }

    let action_field_types = action_field_types(&schema_field_types, &runtime_vars);
    let actions = parse_actions(root_obj, &schema_field_types, &action_field_types)?;

    Ok(AgentConfig {
        inputs,
        runtime_vars,
        properties: parsed_properties,
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
    action_field_types: &BTreeMap<String, FieldType>,
) -> Result<Vec<Action>, BuildError> {
    let actions = get_required_array(root_obj, "actions", "$")?;
    let mut parsed = Vec::with_capacity(actions.len());

    for (action_idx, action_value) in actions.iter().enumerate() {
        let action_path = format!("$.actions[{action_idx}]");
        let action_obj = expect_object(action_value, &action_path)?;

        let name = get_required_string(action_obj, "name", &action_path)?.to_string();
        let logic = get_required_field(action_obj, "logic", &action_path)?.clone();
        validate_logic_expression(&logic, &format!("{action_path}.logic"), action_field_types)?;

        let runs = get_required_array(action_obj, "run", &action_path)?;
        let mut run_steps = Vec::with_capacity(runs.len());
        let mut available_field_types = action_field_types.clone();
        let mut captured_variable_names = BTreeMap::new();
        for (run_idx, run_value) in runs.iter().enumerate() {
            let run_path = format!("{action_path}.run[{run_idx}]");
            let run_obj = expect_object(run_value, &run_path)?;

            let platforms = parse_optional_platforms(run_obj, &run_path)?;
            let when = parse_optional_when(run_obj, &run_path, &available_field_types)?;
            let failure_mode = parse_optional_failure_mode(run_obj, &run_path)?;
            let status_variable = parse_optional_capture_variable(
                run_obj,
                "status_variable",
                &run_path,
                schema_field_types,
                &captured_variable_names,
            )?;
            let error_variable = parse_optional_capture_variable(
                run_obj,
                "error_variable",
                &run_path,
                schema_field_types,
                &captured_variable_names,
            )?;
            let kind = get_required_string(run_obj, "kind", &run_path)?.to_string();

            let run_step = match kind.as_str() {
                "exec" => {
                    if run_obj.contains_key("model") {
                        return Err(BuildError::config(
                            format!("{run_path}.model"),
                            "`model` is only supported for `generate_image` actions",
                        ));
                    }
                    if run_obj.contains_key("prompt") {
                        return Err(BuildError::config(
                            format!("{run_path}.prompt"),
                            "`prompt` is only supported for `generate_image` actions",
                        ));
                    }
                    if run_obj.contains_key("path") {
                        return Err(BuildError::config(
                            format!("{run_path}.path"),
                            "`path` is only supported for `generate_image` actions",
                        ));
                    }
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
                    if run_obj.contains_key("input_mode") {
                        return Err(BuildError::config(
                            format!("{run_path}.input_mode"),
                            "`input_mode` is only supported for `agent` actions",
                        ));
                    }

                    let program = get_required_string(run_obj, "program", &run_path)?.to_string();
                    if program.trim().is_empty() {
                        return Err(BuildError::config(
                            format!("{run_path}.program"),
                            "must be a non-empty string",
                        ));
                    }

                    let output_variable = parse_optional_capture_variable(
                        run_obj,
                        "output_variable",
                        &run_path,
                        schema_field_types,
                        &captured_variable_names,
                    )?;
                    let args = parse_run_args(run_obj, &run_path, &available_field_types)?;
                    RunStep {
                        kind,
                        program: Some(program),
                        model: None,
                        output_variable,
                        status_variable,
                        error_variable,
                        failure_mode,
                        when,
                        args,
                        prompt: None,
                        path: None,
                        subject: None,
                        text: None,
                        agent: None,
                        inputs: None,
                        input_mode: None,
                        platforms,
                    }
                }
                "email_me" => {
                    if run_obj.contains_key("model") {
                        return Err(BuildError::config(
                            format!("{run_path}.model"),
                            "`model` is only supported for `generate_image` actions",
                        ));
                    }
                    if run_obj.contains_key("prompt") {
                        return Err(BuildError::config(
                            format!("{run_path}.prompt"),
                            "`prompt` is only supported for `generate_image` actions",
                        ));
                    }
                    if run_obj.contains_key("path") {
                        return Err(BuildError::config(
                            format!("{run_path}.path"),
                            "`path` is only supported for `generate_image` actions",
                        ));
                    }
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
                    if run_obj.contains_key("input_mode") {
                        return Err(BuildError::config(
                            format!("{run_path}.input_mode"),
                            "`input_mode` is only supported for `agent` actions",
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
                        model: None,
                        output_variable: None,
                        status_variable,
                        error_variable,
                        failure_mode,
                        when,
                        args: Vec::new(),
                        prompt: None,
                        path: None,
                        subject: Some(subject),
                        text: Some(text),
                        agent: None,
                        inputs: None,
                        input_mode: None,
                        platforms,
                    }
                }
                "agent" => {
                    if run_obj.contains_key("model") {
                        return Err(BuildError::config(
                            format!("{run_path}.model"),
                            "`model` is only supported for `generate_image` actions",
                        ));
                    }
                    if run_obj.contains_key("prompt") {
                        return Err(BuildError::config(
                            format!("{run_path}.prompt"),
                            "`prompt` is only supported for `generate_image` actions",
                        ));
                    }
                    if run_obj.contains_key("path") {
                        return Err(BuildError::config(
                            format!("{run_path}.path"),
                            "`path` is only supported for `generate_image` actions",
                        ));
                    }
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
                    let input_mode = parse_optional_action_input_mode(run_obj, &run_path)?;

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

                    if input_mode.is_some() && inputs.is_none() {
                        return Err(BuildError::config(
                            format!("{run_path}.input_mode"),
                            "`input_mode` requires `inputs` for `agent` actions",
                        ));
                    }

                    RunStep {
                        kind,
                        program: None,
                        model: None,
                        output_variable: None,
                        status_variable,
                        error_variable,
                        failure_mode,
                        when,
                        args: Vec::new(),
                        prompt: None,
                        path: None,
                        subject: None,
                        text: None,
                        agent: Some(agent),
                        inputs,
                        input_mode,
                        platforms,
                    }
                }
                "generate_image" => {
                    if run_obj.contains_key("program") {
                        return Err(BuildError::config(
                            format!("{run_path}.program"),
                            "`program` is not supported for `generate_image` actions",
                        ));
                    }
                    if run_obj.contains_key("args") {
                        return Err(BuildError::config(
                            format!("{run_path}.args"),
                            "`args` is not supported for `generate_image` actions",
                        ));
                    }
                    if run_obj.contains_key("subject") {
                        return Err(BuildError::config(
                            format!("{run_path}.subject"),
                            "`subject` is not supported for `generate_image` actions",
                        ));
                    }
                    if run_obj.contains_key("text") {
                        return Err(BuildError::config(
                            format!("{run_path}.text"),
                            "`text` is not supported for `generate_image` actions",
                        ));
                    }
                    if run_obj.contains_key("agent") {
                        return Err(BuildError::config(
                            format!("{run_path}.agent"),
                            "`agent` is not supported for `generate_image` actions",
                        ));
                    }
                    if run_obj.contains_key("inputs") {
                        return Err(BuildError::config(
                            format!("{run_path}.inputs"),
                            "`inputs` is not supported for `generate_image` actions",
                        ));
                    }
                    if run_obj.contains_key("output_variable") {
                        return Err(BuildError::config(
                            format!("{run_path}.output_variable"),
                            "`output_variable` is not supported for `generate_image` actions",
                        ));
                    }
                    if run_obj.contains_key("input_mode") {
                        return Err(BuildError::config(
                            format!("{run_path}.input_mode"),
                            "`input_mode` is only supported for `agent` actions",
                        ));
                    }

                    let model =
                        parse_generate_image_model_field(run_obj, &run_path, action_field_types)?;

                    let prompt = parse_string_parts_field(
                        run_obj,
                        "prompt",
                        &run_path,
                        &available_field_types,
                    )?;
                    let path = parse_string_parts_field(
                        run_obj,
                        "path",
                        &run_path,
                        &available_field_types,
                    )?;
                    if let Some(resolved_path) = resolve_literal_run_args(&path) {
                        validate_definition_owned_local_path(
                            &resolved_path,
                            &format!("{run_path}.path"),
                            "generated image output",
                        )?;
                        validate_generated_image_output_extension(
                            &resolved_path,
                            &format!("{run_path}.path"),
                            "generated image output",
                        )?;
                    }

                    RunStep {
                        kind,
                        program: None,
                        model: Some(model),
                        output_variable: None,
                        status_variable,
                        error_variable,
                        failure_mode,
                        when,
                        args: Vec::new(),
                        prompt: Some(prompt),
                        path: Some(path),
                        subject: None,
                        text: None,
                        agent: None,
                        inputs: None,
                        input_mode: None,
                        platforms,
                    }
                }
                _ => {
                    return Err(BuildError::config(
                        format!("{run_path}.kind"),
                        format!(
                            "unsupported kind `{kind}` (supported: `exec`, `email_me`, `agent`, `generate_image`)"
                        ),
                    ));
                }
            };

            for captured_name in [
                run_step.output_variable.as_ref(),
                run_step.status_variable.as_ref(),
                run_step.error_variable.as_ref(),
            ]
            .into_iter()
            .flatten()
            {
                captured_variable_names.insert(captured_name.clone(), run_path.clone());
                available_field_types.insert(captured_name.clone(), FieldType::String);
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

fn parse_optional_capture_variable(
    run_obj: &Map<String, Value>,
    field_name: &str,
    run_path: &str,
    schema_field_types: &BTreeMap<String, FieldType>,
    captured_variable_names: &BTreeMap<String, String>,
) -> Result<Option<String>, BuildError> {
    let Some(value) = run_obj.get(field_name) else {
        return Ok(None);
    };

    let variable_path = format!("{run_path}.{field_name}");
    let variable_name = value.as_str().ok_or_else(|| {
        BuildError::config(
            &variable_path,
            format!("expected `{field_name}` to be a non-empty string name"),
        )
    })?;
    let normalized = variable_name.trim();
    validate_action_capture_variable_name(normalized, &variable_path)?;
    validate_reserved_top_level_name(normalized, &variable_path)?;

    if schema_field_types.contains_key(normalized) {
        return Err(BuildError::config(
            &variable_path,
            format!(
                "captured variable name `{normalized}` collides with an agent output field; choose a different name"
            ),
        ));
    }

    if captured_variable_names.contains_key(normalized) {
        return Err(BuildError::config(
            &variable_path,
            format!(
                "duplicate captured variable name `{normalized}` within this action; choose a unique name"
            ),
        ));
    }

    Ok(Some(normalized.to_string()))
}

fn parse_optional_failure_mode(
    run_obj: &Map<String, Value>,
    run_path: &str,
) -> Result<Option<FailureMode>, BuildError> {
    let Some(value) = run_obj.get("failure_mode") else {
        return Ok(None);
    };

    let failure_mode_path = format!("{run_path}.failure_mode");
    let failure_mode = value.as_str().ok_or_else(|| {
        BuildError::config(
            &failure_mode_path,
            "expected `failure_mode` to be a string (`stop` or `continue`)",
        )
    })?;

    match failure_mode.trim() {
        "stop" => Ok(Some(FailureMode::Stop)),
        "continue" => Ok(Some(FailureMode::Continue)),
        _ => Err(BuildError::config(
            &failure_mode_path,
            "unsupported `failure_mode` (supported: `stop`, `continue`)",
        )),
    }
}

fn parse_optional_action_input_mode(
    run_obj: &Map<String, Value>,
    run_path: &str,
) -> Result<Option<ActionInputMode>, BuildError> {
    let Some(value) = run_obj.get("input_mode") else {
        return Ok(None);
    };

    let input_mode_path = format!("{run_path}.input_mode");
    let input_mode = value.as_str().ok_or_else(|| {
        BuildError::config(
            &input_mode_path,
            "expected `input_mode` to be a string (`replace`, `append`, or `prepend`)",
        )
    })?;

    match input_mode.trim() {
        "replace" => Ok(Some(ActionInputMode::Replace)),
        "append" => Ok(Some(ActionInputMode::Append)),
        "prepend" => Ok(Some(ActionInputMode::Prepend)),
        _ => Err(BuildError::config(
            &input_mode_path,
            "unsupported `input_mode` (supported: `replace`, `append`, `prepend`)",
        )),
    }
}

fn parse_optional_when(
    run_obj: &Map<String, Value>,
    run_path: &str,
    schema_field_types: &BTreeMap<String, FieldType>,
) -> Result<Option<Value>, BuildError> {
    let Some(value) = run_obj.get("when") else {
        return Ok(None);
    };

    let when_path = format!("{run_path}.when");
    if !value.is_object() {
        return Err(BuildError::config(
            &when_path,
            "expected `when` to be a JSON Logic object",
        ));
    }

    validate_logic_expression(value, &when_path, schema_field_types)?;
    Ok(Some(value.clone()))
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

fn parse_generate_image_model_field(
    run_obj: &Map<String, Value>,
    run_path: &str,
    action_field_types: &BTreeMap<String, FieldType>,
) -> Result<RunArg, BuildError> {
    let field_path = format!("{run_path}.model");
    let value = get_required_field(run_obj, "model", run_path)?;
    let parsed = parse_run_arg(value, &field_path, action_field_types)?;

    match &parsed {
        RunArg::Literal(model) => {
            if model.trim().is_empty() {
                return Err(BuildError::config(
                    &field_path,
                    "must be a non-empty string",
                ));
            }
        }
        RunArg::Variable(variable) => {
            let field_type = action_field_types.get(variable).ok_or_else(|| {
                BuildError::config(
                    &field_path,
                    format!(
                        "unknown variable `{variable}`; expected one of: {}",
                        action_field_types
                            .keys()
                            .cloned()
                            .collect::<Vec<_>>()
                            .join(", ")
                    ),
                )
            })?;

            if *field_type != FieldType::String {
                return Err(BuildError::config(
                    &field_path,
                    format!(
                        "generate_image `model` variables must resolve from string fields; `{variable}` is {}",
                        type_name(field_type)
                    ),
                ));
            }
        }
    }

    Ok(parsed)
}

fn validate_action_capture_variable_name(name: &str, path: &str) -> Result<(), BuildError> {
    if name.is_empty() {
        return Err(BuildError::config(
            path,
            "captured variable name cannot be empty",
        ));
    }

    if name.contains('.') {
        return Err(BuildError::config(
            path,
            "captured variable name must be flat; nested names with `.` are not supported",
        ));
    }

    Ok(())
}

fn validate_reserved_top_level_name(name: &str, path: &str) -> Result<(), BuildError> {
    if name == "runtime" {
        return Err(BuildError::config(
            path,
            "`runtime` is reserved for invocation-scoped runtime variables",
        ));
    }

    Ok(())
}

fn parse_runtime_vars(root_obj: &Map<String, Value>) -> Result<Vec<RuntimeVarSpec>, BuildError> {
    let Some(value) = root_obj.get("runtime_vars") else {
        return Ok(Vec::new());
    };

    let runtime_vars = expect_object(value, "$.runtime_vars")?;
    let mut parsed = Vec::with_capacity(runtime_vars.len());

    for (name, raw_spec) in runtime_vars {
        let path = format!("$.runtime_vars.{name}");
        validate_runtime_var_name(name, &path)?;
        let spec = expect_object(raw_spec, &path)?;
        let field_type = parse_runtime_var_type(spec, &path)?;
        let default_value = parse_runtime_var_default(spec, &path, &field_type)?;
        parsed.push(RuntimeVarSpec {
            name: name.clone(),
            field_type,
            default_value,
        });
    }

    Ok(parsed)
}

fn validate_runtime_var_name(name: &str, path: &str) -> Result<(), BuildError> {
    if name.trim().is_empty() {
        return Err(BuildError::config(
            path,
            "runtime variable name cannot be empty",
        ));
    }

    if name != name.trim() {
        return Err(BuildError::config(
            path,
            "runtime variable names cannot start or end with whitespace",
        ));
    }

    if name.chars().any(char::is_whitespace) {
        return Err(BuildError::config(
            path,
            "runtime variable names cannot contain whitespace",
        ));
    }

    if name.contains('.') {
        return Err(BuildError::config(
            path,
            "runtime variable names must be flat; nested names with `.` are not supported",
        ));
    }

    validate_reserved_top_level_name(name, path)
}

fn parse_runtime_var_type(spec: &Map<String, Value>, path: &str) -> Result<FieldType, BuildError> {
    let schema_type = get_schema_type(spec, path)?;
    match schema_type.as_str() {
        "string" => Ok(FieldType::String),
        "boolean" => Ok(FieldType::Boolean),
        "number" => Ok(FieldType::Number),
        "integer" => Ok(FieldType::Integer),
        "array" => Err(BuildError::config(
            format!("{path}.type"),
            "runtime variables do not support `array` values in this story",
        )),
        "object" => Err(BuildError::config(
            format!("{path}.type"),
            "runtime variables do not support `object` values in this story",
        )),
        other => Err(BuildError::config(
            format!("{path}.type"),
            format!("unsupported runtime variable type `{other}`"),
        )),
    }
}

fn parse_runtime_var_default(
    spec: &Map<String, Value>,
    path: &str,
    field_type: &FieldType,
) -> Result<Option<Value>, BuildError> {
    let Some(default_value) = spec.get("default") else {
        return Ok(None);
    };

    validate_runtime_default_value(default_value, &format!("{path}.default"), field_type)?;
    Ok(Some(default_value.clone()))
}

fn validate_runtime_default_value(
    value: &Value,
    path: &str,
    field_type: &FieldType,
) -> Result<(), BuildError> {
    match field_type {
        FieldType::String => {
            if value.is_string() {
                Ok(())
            } else {
                Err(BuildError::config(
                    path,
                    "default must be a string for `type: \"string\"` runtime variables",
                ))
            }
        }
        FieldType::Boolean => {
            if value.is_boolean() {
                Ok(())
            } else {
                Err(BuildError::config(
                    path,
                    "default must be a boolean for `type: \"boolean\"` runtime variables",
                ))
            }
        }
        FieldType::Integer => {
            if value.as_i64().is_some() {
                Ok(())
            } else {
                Err(BuildError::config(
                    path,
                    "default must be an integer for `type: \"integer\"` runtime variables",
                ))
            }
        }
        FieldType::Number => {
            if value.as_f64().is_some() {
                Ok(())
            } else {
                Err(BuildError::config(
                    path,
                    "default must be a number for `type: \"number\"` runtime variables",
                ))
            }
        }
        FieldType::Array => Err(BuildError::config(
            path,
            "array runtime variable defaults are not supported in this story",
        )),
    }
}

fn action_field_types(
    schema_field_types: &BTreeMap<String, FieldType>,
    runtime_vars: &[RuntimeVarSpec],
) -> BTreeMap<String, FieldType> {
    let mut field_types = schema_field_types.clone();
    for runtime_var in runtime_vars {
        field_types.insert(
            format!("runtime.{}", runtime_var.name),
            runtime_var.field_type.clone(),
        );
    }
    field_types
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

fn validate_generated_image_output_extension(
    raw_path: &str,
    path: &str,
    label: &str,
) -> Result<(), BuildError> {
    let extension = Path::new(raw_path)
        .extension()
        .and_then(|value| value.to_str())
        .map(|value| value.to_ascii_lowercase());

    match extension.as_deref() {
        Some(extension) if SUPPORTED_GENERATED_IMAGE_EXTENSIONS.contains(&extension) => Ok(()),
        _ => Err(BuildError::config(
            path,
            format!(
                "{label} path must use a supported extension: {SUPPORTED_GENERATED_IMAGE_EXTENSIONS_MESSAGE}"
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
            validate_variable_lookup_name(normalized_name, &variable_path)?;

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

    validate_variable_lookup_name(var_name.trim(), path)?;

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

fn validate_variable_lookup_name(name: &str, path: &str) -> Result<(), BuildError> {
    if name.is_empty() {
        return Err(BuildError::config(path, "variable name cannot be empty"));
    }

    if let Some(runtime_name) = name.strip_prefix("runtime.") {
        if runtime_name.is_empty() {
            return Err(BuildError::config(
                path,
                "runtime variable lookup must include a name after `runtime.`",
            ));
        }
        if runtime_name.contains('.') {
            return Err(BuildError::config(
                path,
                "runtime variable names must be flat after the reserved `runtime.` prefix",
            ));
        }
        return Ok(());
    }

    if name.contains('.') {
        return Err(BuildError::config(
            path,
            "nested variable paths are not supported outside the reserved `runtime.` namespace",
        ));
    }

    Ok(())
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

fn parse_agent_property(
    name: &str,
    property: &Map<String, Value>,
    path: &str,
) -> Result<AgentProperty, BuildError> {
    let schema_type = get_schema_type(property, path)?;
    let (rust_type, field_type) = match schema_type.as_str() {
        "string" => ("String".to_string(), FieldType::String),
        "boolean" => ("bool".to_string(), FieldType::Boolean),
        "number" => ("f64".to_string(), FieldType::Number),
        "integer" => ("i64".to_string(), FieldType::Integer),
        "array" => {
            return Err(BuildError::config(
                format!("{path}.type"),
                "top-level array output fields are not supported in this story",
            ));
        }
        "object" => {
            return Err(BuildError::config(
                format!("{path}.type"),
                "nested object fields are not supported yet",
            ));
        }
        other => {
            return Err(BuildError::config(
                format!("{path}.type"),
                format!("unsupported schema type `{other}`"),
            ));
        }
    };

    Ok(AgentProperty {
        name: name.to_string(),
        rust_type,
        field_type: field_type.clone(),
        description: parse_optional_description(property, path)?,
        enum_values: parse_optional_string_enum(property, path, &field_type)?,
        numeric_constraints: parse_numeric_constraints(property, path, &field_type)?,
    })
}

fn parse_optional_description(
    property: &Map<String, Value>,
    path: &str,
) -> Result<Option<String>, BuildError> {
    let Some(description_value) = property.get("description") else {
        return Ok(None);
    };

    let description_path = format!("{path}.description");
    let description = description_value.as_str().ok_or_else(|| {
        BuildError::config(&description_path, "expected `description` to be a string")
    })?;

    if description.trim().is_empty() {
        return Err(BuildError::config(
            description_path,
            "description cannot be empty when provided",
        ));
    }

    Ok(Some(description.to_string()))
}

fn parse_optional_string_enum(
    property: &Map<String, Value>,
    path: &str,
    field_type: &FieldType,
) -> Result<Option<Vec<String>>, BuildError> {
    let Some(enum_value) = property.get("enum") else {
        return Ok(None);
    };

    let enum_path = format!("{path}.enum");
    if field_type != &FieldType::String {
        return Err(BuildError::config(
            &enum_path,
            "`enum` is supported only for `type: \"string\"` fields",
        ));
    }

    let values = enum_value.as_array().ok_or_else(|| {
        BuildError::config(
            &enum_path,
            "expected `enum` to be an array of string values",
        )
    })?;
    if values.is_empty() {
        return Err(BuildError::config(
            &enum_path,
            "expected `enum` to contain at least one value",
        ));
    }

    let mut seen_values = BTreeSet::new();
    let mut parsed_values = Vec::with_capacity(values.len());
    for (index, value) in values.iter().enumerate() {
        let value_path = format!("{enum_path}[{index}]");
        let value = value
            .as_str()
            .ok_or_else(|| BuildError::config(&value_path, "expected an enum value string"))?;
        if value.trim().is_empty() {
            return Err(BuildError::config(
                &value_path,
                "enum values cannot be empty",
            ));
        }
        if !seen_values.insert(value.to_string()) {
            return Err(BuildError::config(
                &value_path,
                format!("duplicate enum value `{value}`"),
            ));
        }
        parsed_values.push(value.to_string());
    }

    Ok(Some(parsed_values))
}

fn parse_numeric_constraints(
    property: &Map<String, Value>,
    path: &str,
    field_type: &FieldType,
) -> Result<NumericConstraints, BuildError> {
    let constraints = NumericConstraints {
        minimum: parse_numeric_constraint(property, "minimum", path, field_type)?,
        maximum: parse_numeric_constraint(property, "maximum", path, field_type)?,
        exclusive_minimum: parse_numeric_constraint(
            property,
            "exclusiveMinimum",
            path,
            field_type,
        )?,
        exclusive_maximum: parse_numeric_constraint(
            property,
            "exclusiveMaximum",
            path,
            field_type,
        )?,
    };

    if constraints.minimum.is_some() && constraints.exclusive_minimum.is_some() {
        return Err(BuildError::config(
            format!("{path}.exclusiveMinimum"),
            "`exclusiveMinimum` cannot be combined with `minimum`",
        ));
    }
    if constraints.maximum.is_some() && constraints.exclusive_maximum.is_some() {
        return Err(BuildError::config(
            format!("{path}.exclusiveMaximum"),
            "`exclusiveMaximum` cannot be combined with `maximum`",
        ));
    }

    validate_numeric_constraint_bounds(path, field_type, &constraints)?;
    Ok(constraints)
}

fn parse_numeric_constraint(
    property: &Map<String, Value>,
    key: &str,
    path: &str,
    field_type: &FieldType,
) -> Result<Option<NumericConstraintValue>, BuildError> {
    let Some(value) = property.get(key) else {
        return Ok(None);
    };

    let value_path = format!("{path}.{key}");
    if !is_numeric_type(field_type) {
        return Err(BuildError::config(
            &value_path,
            format!(
                "`{key}` is supported only for `type: \"number\"` or `type: \"integer\"` fields"
            ),
        ));
    }

    let number = value.as_number().ok_or_else(|| {
        BuildError::config(
            &value_path,
            format!("expected `{key}` to be a numeric value"),
        )
    })?;

    match field_type {
        FieldType::Integer => number
            .as_i64()
            .map(NumericConstraintValue::Integer)
            .ok_or_else(|| {
                BuildError::config(
                    &value_path,
                    format!(
                        "expected `{key}` to be an integer value for `type: \"integer\"` fields"
                    ),
                )
            })
            .map(Some),
        FieldType::Number => number
            .as_f64()
            .map(NumericConstraintValue::Number)
            .ok_or_else(|| {
                BuildError::config(
                    &value_path,
                    format!("expected `{key}` to be a finite numeric value"),
                )
            })
            .map(Some),
        _ => Ok(None),
    }
}

fn validate_numeric_constraint_bounds(
    path: &str,
    field_type: &FieldType,
    constraints: &NumericConstraints,
) -> Result<(), BuildError> {
    let lower_bound = constraints
        .minimum
        .as_ref()
        .map(|value| (value, false))
        .or_else(|| {
            constraints
                .exclusive_minimum
                .as_ref()
                .map(|value| (value, true))
        });
    let upper_bound = constraints
        .maximum
        .as_ref()
        .map(|value| (value, false))
        .or_else(|| {
            constraints
                .exclusive_maximum
                .as_ref()
                .map(|value| (value, true))
        });

    let Some((lower_value, lower_exclusive)) = lower_bound else {
        return Ok(());
    };
    let Some((upper_value, upper_exclusive)) = upper_bound else {
        return Ok(());
    };

    let ordering = match field_type {
        FieldType::Integer => {
            integer_constraint_value(lower_value).cmp(&integer_constraint_value(upper_value))
        }
        FieldType::Number => number_constraint_value(lower_value)
            .partial_cmp(&number_constraint_value(upper_value))
            .expect("finite numeric constraints must be comparable"),
        _ => return Ok(()),
    };

    if ordering.is_gt() {
        return Err(BuildError::config(
            path,
            "lower bound cannot exceed upper bound",
        ));
    }

    if ordering.is_eq() && (lower_exclusive || upper_exclusive) {
        return Err(BuildError::config(
            path,
            "equal lower and upper bounds are not allowed when either bound is exclusive",
        ));
    }

    Ok(())
}

fn integer_constraint_value(value: &NumericConstraintValue) -> i64 {
    match value {
        NumericConstraintValue::Integer(value) => *value,
        NumericConstraintValue::Number(_) => unreachable!("integer bounds should remain integer"),
    }
}

fn number_constraint_value(value: &NumericConstraintValue) -> f64 {
    match value {
        NumericConstraintValue::Number(value) => *value,
        NumericConstraintValue::Integer(value) => *value as f64,
    }
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
    let mut validation_calls = String::new();
    let mut schema_metadata_calls = String::new();
    let mut has_enum_validation = false;
    let mut has_i64_validation = false;
    let mut has_f64_validation = false;

    let mut input_list = String::new();
    for property in &config.properties {
        struct_fields.push_str(&format!(
            "    pub {}: {},\n",
            property.name, property.rust_type
        ));

        if let Some(enum_values) = &property.enum_values {
            has_enum_validation = true;
            let allowed_values = enum_values
                .iter()
                .map(|value| rust_string_literal(value))
                .collect::<Vec<_>>()
                .join(", ");
            validation_calls.push_str(&format!(
                "        validate_enum_field(&self.{name}, {field_name}, &[{allowed_values}])?;\n",
                name = property.name,
                field_name = rust_string_literal(&property.name)
            ));
        }

        if property.numeric_constraints.has_any() {
            let validation_call = match property.field_type {
                FieldType::Integer => {
                    has_i64_validation = true;
                    format!(
                        "        validate_i64_range(self.{name}, {field_name}, {minimum}, {exclusive_minimum}, {maximum}, {exclusive_maximum})?;\n",
                        name = property.name,
                        field_name = rust_string_literal(&property.name),
                        minimum = render_optional_i64_literal(property.numeric_constraints.minimum.as_ref()),
                        exclusive_minimum = render_optional_i64_literal(
                            property.numeric_constraints.exclusive_minimum.as_ref()
                        ),
                        maximum = render_optional_i64_literal(property.numeric_constraints.maximum.as_ref()),
                        exclusive_maximum = render_optional_i64_literal(
                            property.numeric_constraints.exclusive_maximum.as_ref()
                        ),
                    )
                }
                FieldType::Number => {
                    has_f64_validation = true;
                    format!(
                        "        validate_f64_range(self.{name}, {field_name}, {minimum}, {exclusive_minimum}, {maximum}, {exclusive_maximum})?;\n",
                        name = property.name,
                        field_name = rust_string_literal(&property.name),
                        minimum = render_optional_f64_literal(property.numeric_constraints.minimum.as_ref()),
                        exclusive_minimum = render_optional_f64_literal(
                            property.numeric_constraints.exclusive_minimum.as_ref()
                        ),
                        maximum = render_optional_f64_literal(property.numeric_constraints.maximum.as_ref()),
                        exclusive_maximum = render_optional_f64_literal(
                            property.numeric_constraints.exclusive_maximum.as_ref()
                        ),
                    )
                }
                _ => String::new(),
            };
            validation_calls.push_str(&validation_call);
        }

        if property.description.is_some()
            || property.enum_values.is_some()
            || property.numeric_constraints.has_any()
        {
            schema_metadata_calls.push_str(&format!(
                "    apply_property_schema_metadata(
        properties,
        {field_name},
        {description},
        {enum_values},
        {minimum},
        {maximum},
        {exclusive_minimum},
        {exclusive_maximum},
    );\n",
                field_name = rust_string_literal(&property.name),
                description = render_optional_description(&property.description),
                enum_values = render_optional_enum_values(property.enum_values.as_ref()),
                minimum =
                    render_optional_constraint_json(property.numeric_constraints.minimum.as_ref()),
                maximum =
                    render_optional_constraint_json(property.numeric_constraints.maximum.as_ref()),
                exclusive_minimum = render_optional_constraint_json(
                    property.numeric_constraints.exclusive_minimum.as_ref()
                ),
                exclusive_maximum = render_optional_constraint_json(
                    property.numeric_constraints.exclusive_maximum.as_ref()
                ),
            ));
        }
    }

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
                let prompt = run_step
                    .prompt
                    .as_ref()
                    .map(|parts| format!("Some(vec![{}])", render_run_arg_parts(parts)))
                    .unwrap_or_else(|| "None".to_string());
                let path = run_step
                    .path
                    .as_ref()
                    .map(|parts| format!("Some(vec![{}])", render_run_arg_parts(parts)))
                    .unwrap_or_else(|| "None".to_string());
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
                let model = run_step
                    .model
                    .as_ref()
                    .map(|model| format!("Some({})", render_run_arg(model)))
                    .unwrap_or_else(|| "None".to_string());
                let status_variable = run_step
                    .status_variable
                    .as_ref()
                    .map(|name| format!("Some({}.to_string())", rust_string_literal(name)))
                    .unwrap_or_else(|| "None".to_string());
                let error_variable = run_step
                    .error_variable
                    .as_ref()
                    .map(|name| format!("Some({}.to_string())", rust_string_literal(name)))
                    .unwrap_or_else(|| "None".to_string());
                let failure_mode = run_step
                    .failure_mode
                    .as_ref()
                    .map(|mode| match mode {
                        FailureMode::Stop => "Some(FailureMode::Stop)".to_string(),
                        FailureMode::Continue => "Some(FailureMode::Continue)".to_string(),
                    })
                    .unwrap_or_else(|| "None".to_string());
                let when = run_step
                    .when
                    .as_ref()
                    .map(|value| {
                        format!(
                            "Some(serde_json::from_str({}).expect(\"generated step `when` must be valid JSON\"))",
                            rust_string_literal(&value.to_string())
                        )
                    })
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
                let input_mode = run_step
                    .input_mode
                    .as_ref()
                    .map(|input_mode| match input_mode {
                        ActionInputMode::Replace => "Some(ActionInputMode::Replace)".to_string(),
                        ActionInputMode::Append => "Some(ActionInputMode::Append)".to_string(),
                        ActionInputMode::Prepend => "Some(ActionInputMode::Prepend)".to_string(),
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
                        model: {},
                        output_variable: {},
                        status_variable: {},
                        error_variable: {},
                        failure_mode: {},
                        when: {},
                        args: vec![{}],
                        prompt: {},
                        path: {},
                        subject: {},
                        text: {},
                        agent: {},
                        inputs: {},
                        input_mode: {},
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
                    model,
                    output_variable,
                    status_variable,
                    error_variable,
                    failure_mode,
                    when,
                    args,
                    prompt,
                    path,
                    subject,
                    text,
                    agent,
                    inputs,
                    input_mode,
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

    let validation_helpers =
        render_validation_helpers(has_enum_validation, has_i64_validation, has_f64_validation);
    let schema_metadata_helpers = render_schema_metadata_helpers(&schema_metadata_calls);
    let schema_metadata_apply = if schema_metadata_calls.is_empty() {
        String::new()
    } else {
        "    apply_output_schema_metadata(&mut v);\n".to_string()
    };
    let runtime_var_specs_code = config
        .runtime_vars
        .iter()
        .map(|runtime_var| {
            let field_type = render_runtime_var_type_expr(&runtime_var.field_type);
            let default_value = runtime_var
                .default_value
                .as_ref()
                .map(|value| {
                    format!(
                        "Some(serde_json::from_str({}).expect(\"generated runtime-var default must be valid JSON\"))",
                        rust_string_literal(&value.to_string())
                    )
                })
                .unwrap_or_else(|| "None".to_string());

            format!(
                "RuntimeVarSpec {{
                    name: {}.to_string(),
                    field_type: {},
                    default_value: {},
                }},",
                rust_string_literal(&runtime_var.name),
                field_type,
                default_value
            )
        })
        .collect::<Vec<_>>()
        .join("");

    format!(
        r##"
use schemars::{{JsonSchema, schema_for}};
use serde_json;

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
pub struct Output {{
{struct_fields}}}

impl crate::providers::ValidatedResponse for Output {{
    fn validate_response(&self) -> Result<(), String> {{
{validation_calls}        Ok(())
    }}
}}

{validation_helpers}

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

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub enum RuntimeVarType {{
    String,
    Boolean,
    Number,
    Integer,
}}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct RuntimeVarSpec {{
    name: String,
    field_type: RuntimeVarType,
    default_value: Option<serde_json::Value>,
}}

pub fn runtime_var_specs() -> Vec<RuntimeVarSpec> {{
    vec![{runtime_var_specs_code}]
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

{schema_metadata_apply}
    v
}}

{schema_metadata_helpers}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub enum ActionInput {{
    Text {{ text: Vec<RunArg> }},
    Url {{ url: Vec<RunArg> }},
    Image {{ path: Vec<RunArg> }},
    File {{ path: Vec<RunArg> }},
}}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub enum FailureMode {{
    Stop,
    Continue,
}}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub enum ActionInputMode {{
    Replace,
    Append,
    Prepend,
}}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct RunStep {{
    kind: String,
    program: Option<String>,
    model: Option<RunArg>,
    output_variable: Option<String>,
    status_variable: Option<String>,
    error_variable: Option<String>,
    failure_mode: Option<FailureMode>,
    when: Option<serde_json::Value>,
    args: Vec<RunArg>,
    prompt: Option<Vec<RunArg>>,
    path: Option<Vec<RunArg>>,
    subject: Option<Vec<RunArg>>,
    text: Option<Vec<RunArg>>,
    agent: Option<String>,
    inputs: Option<Vec<ActionInput>>,
    input_mode: Option<ActionInputMode>,
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
        validation_calls = validation_calls,
        validation_helpers = validation_helpers,
        input_list = input_list,
        schema_metadata_apply = schema_metadata_apply,
        schema_metadata_helpers = schema_metadata_helpers,
        runtime_var_specs_code = runtime_var_specs_code,
        action_code = action_code
    )
}

fn render_validation_helpers(
    has_enum_validation: bool,
    has_i64_validation: bool,
    has_f64_validation: bool,
) -> String {
    let mut helpers = String::new();

    if has_enum_validation {
        helpers.push_str(
            r#"
fn validate_enum_field(value: &str, field_name: &str, allowed_values: &[&str]) -> Result<(), String> {
    if allowed_values.contains(&value) {
        return Ok(());
    }

    let allowed = allowed_values
        .iter()
        .map(|value| format!("`{value}`"))
        .collect::<Vec<_>>()
        .join(", ");
    Err(format!("field `{field_name}` must be one of: {allowed}"))
}
"#,
        );
    }

    if has_i64_validation {
        helpers.push_str(
            r#"
fn validate_i64_range(
    value: i64,
    field_name: &str,
    minimum: Option<i64>,
    exclusive_minimum: Option<i64>,
    maximum: Option<i64>,
    exclusive_maximum: Option<i64>,
) -> Result<(), String> {
    if let Some(minimum) = minimum {
        if value < minimum {
            return Err(format!("field `{field_name}` must be greater than or equal to {minimum}"));
        }
    }
    if let Some(exclusive_minimum) = exclusive_minimum {
        if value <= exclusive_minimum {
            return Err(format!("field `{field_name}` must be greater than {exclusive_minimum}"));
        }
    }
    if let Some(maximum) = maximum {
        if value > maximum {
            return Err(format!("field `{field_name}` must be less than or equal to {maximum}"));
        }
    }
    if let Some(exclusive_maximum) = exclusive_maximum {
        if value >= exclusive_maximum {
            return Err(format!("field `{field_name}` must be less than {exclusive_maximum}"));
        }
    }
    Ok(())
}
"#,
        );
    }

    if has_f64_validation {
        helpers.push_str(
            r#"

fn validate_f64_range(
    value: f64,
    field_name: &str,
    minimum: Option<f64>,
    exclusive_minimum: Option<f64>,
    maximum: Option<f64>,
    exclusive_maximum: Option<f64>,
) -> Result<(), String> {
    if let Some(minimum) = minimum {
        if value < minimum {
            return Err(format!("field `{field_name}` must be greater than or equal to {minimum}"));
        }
    }
    if let Some(exclusive_minimum) = exclusive_minimum {
        if value <= exclusive_minimum {
            return Err(format!("field `{field_name}` must be greater than {exclusive_minimum}"));
        }
    }
    if let Some(maximum) = maximum {
        if value > maximum {
            return Err(format!("field `{field_name}` must be less than or equal to {maximum}"));
        }
    }
    if let Some(exclusive_maximum) = exclusive_maximum {
        if value >= exclusive_maximum {
            return Err(format!("field `{field_name}` must be less than {exclusive_maximum}"));
        }
    }
    Ok(())
}
"#,
        );
    }

    helpers
}

fn render_schema_metadata_helpers(schema_metadata_calls: &str) -> String {
    if schema_metadata_calls.is_empty() {
        return String::new();
    }

    format!(
        r#"
fn apply_output_schema_metadata(schema: &mut serde_json::Value) {{
    let Some(obj) = schema.as_object_mut() else {{
        return;
    }};
    let Some(properties) = obj
        .get_mut("properties")
        .and_then(serde_json::Value::as_object_mut)
    else {{
        return;
    }};

{schema_metadata_calls}}}

fn apply_property_schema_metadata(
    properties: &mut serde_json::Map<String, serde_json::Value>,
    field_name: &str,
    description: Option<&str>,
    enum_values: Option<Vec<&str>>,
    minimum: Option<serde_json::Value>,
    maximum: Option<serde_json::Value>,
    exclusive_minimum: Option<serde_json::Value>,
    exclusive_maximum: Option<serde_json::Value>,
) {{
    let Some(property_schema) = properties
        .get_mut(field_name)
        .and_then(serde_json::Value::as_object_mut)
    else {{
        return;
    }};

    if let Some(description) = description {{
        property_schema.insert(
            "description".to_string(),
            serde_json::Value::String(description.to_string()),
        );
    }}
    if let Some(enum_values) = enum_values {{
        property_schema.insert(
            "enum".to_string(),
            serde_json::Value::Array(
                enum_values
                    .into_iter()
                    .map(|value| serde_json::Value::String(value.to_string()))
                    .collect(),
            ),
        );
    }}
    if let Some(minimum) = minimum {{
        property_schema.insert("minimum".to_string(), minimum);
    }}
    if let Some(maximum) = maximum {{
        property_schema.insert("maximum".to_string(), maximum);
    }}
    if let Some(exclusive_minimum) = exclusive_minimum {{
        property_schema.insert("exclusiveMinimum".to_string(), exclusive_minimum);
    }}
    if let Some(exclusive_maximum) = exclusive_maximum {{
        property_schema.insert("exclusiveMaximum".to_string(), exclusive_maximum);
    }}
}}
"#,
        schema_metadata_calls = schema_metadata_calls
    )
}

fn render_optional_description(description: &Option<String>) -> String {
    description
        .as_ref()
        .map(|description| format!("Some({})", rust_string_literal(description)))
        .unwrap_or_else(|| "None".to_string())
}

fn render_optional_enum_values(enum_values: Option<&Vec<String>>) -> String {
    enum_values
        .map(|values| {
            let rendered = values
                .iter()
                .map(|value| rust_string_literal(value))
                .collect::<Vec<_>>()
                .join(", ");
            format!("Some(vec![{rendered}])")
        })
        .unwrap_or_else(|| "None".to_string())
}

fn render_optional_constraint_json(value: Option<&NumericConstraintValue>) -> String {
    value
        .map(|value| format!("Some({})", render_constraint_json(value)))
        .unwrap_or_else(|| "None".to_string())
}

fn render_constraint_json(value: &NumericConstraintValue) -> String {
    match value {
        NumericConstraintValue::Integer(value) => {
            format!("serde_json::Value::Number(serde_json::Number::from({value}i64))")
        }
        NumericConstraintValue::Number(value) => {
            format!("serde_json::json!({value})")
        }
    }
}

fn render_optional_i64_literal(value: Option<&NumericConstraintValue>) -> String {
    value
        .map(|value| format!("Some({})", render_i64_literal(value)))
        .unwrap_or_else(|| "None".to_string())
}

fn render_i64_literal(value: &NumericConstraintValue) -> String {
    match value {
        NumericConstraintValue::Integer(value) => value.to_string(),
        NumericConstraintValue::Number(_) => {
            unreachable!("integer constraints should remain integer")
        }
    }
}

fn render_optional_f64_literal(value: Option<&NumericConstraintValue>) -> String {
    value
        .map(|value| format!("Some({})", render_f64_literal(value)))
        .unwrap_or_else(|| "None".to_string())
}

fn render_f64_literal(value: &NumericConstraintValue) -> String {
    let numeric_value = match value {
        NumericConstraintValue::Number(value) => value.to_string(),
        NumericConstraintValue::Integer(value) => (*value as f64).to_string(),
    };

    if numeric_value.contains(['.', 'e', 'E']) {
        numeric_value
    } else {
        format!("{numeric_value}.0")
    }
}

fn rust_string_literal(value: &str) -> String {
    format!("{value:?}")
}

fn render_runtime_var_type_expr(field_type: &FieldType) -> &'static str {
    match field_type {
        FieldType::String => "RuntimeVarType::String",
        FieldType::Boolean => "RuntimeVarType::Boolean",
        FieldType::Number => "RuntimeVarType::Number",
        FieldType::Integer => "RuntimeVarType::Integer",
        FieldType::Array => unreachable!("array runtime vars are rejected during parsing"),
    }
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
