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
    path::{Path, PathBuf},
};

use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use time::{format_description::well_known::Rfc3339, OffsetDateTime};
use uuid::Uuid;

const SCHEMA_VERSION_FORMAT: &str = "YYYY-MM-DD.rN";
const SCHEMA_VERSION_EXAMPLE: &str = "2026-03-03.r1";
const SUPPORTED_ACTION_PLATFORMS: [&str; 3] = ["macos", "linux", "windows"];

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
struct ResourceUrl {
    url: String,
    description: String,
}

#[derive(Debug, Clone)]
struct RunStep {
    kind: String,
    program: String,
    args: Vec<RunArg>,
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
    prompt: String,
    fields: Vec<(String, String)>,
    resource_urls: Vec<ResourceUrl>,
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

    let prompt = get_required_string(root_obj, "prompt", "$")?.to_string();

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

    let resource_urls = parse_resource_urls(root_obj)?;
    let actions = parse_actions(root_obj, &schema_field_types)?;

    Ok(AgentConfig {
        prompt,
        fields,
        resource_urls,
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

fn parse_resource_urls(root_obj: &Map<String, Value>) -> Result<Vec<ResourceUrl>, BuildError> {
    let urls = get_required_array(root_obj, "resource_urls", "$")?;
    let mut parsed = Vec::with_capacity(urls.len());

    for (index, entry) in urls.iter().enumerate() {
        let path = format!("$.resource_urls[{index}]");
        let entry_obj = expect_object(entry, &path)?;
        let url = get_required_string(entry_obj, "url", &path)?.to_string();
        let description = get_required_string(entry_obj, "description", &path)?.to_string();
        parsed.push(ResourceUrl { url, description });
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
        for (run_idx, run_value) in runs.iter().enumerate() {
            let run_path = format!("{action_path}.run[{run_idx}]");
            let run_obj = expect_object(run_value, &run_path)?;

            let kind = get_required_string(run_obj, "kind", &run_path)?.to_string();
            if kind != "exec" {
                return Err(BuildError::config(
                    format!("{run_path}.kind"),
                    format!("unsupported kind `{kind}` (supported: `exec`)"),
                ));
            }

            let program = get_required_string(run_obj, "program", &run_path)?.to_string();
            if program.trim().is_empty() {
                return Err(BuildError::config(
                    format!("{run_path}.program"),
                    "must be a non-empty string",
                ));
            }

            let args = parse_run_args(run_obj, &run_path, schema_field_types)?;

            let platforms = parse_optional_platforms(run_obj, &run_path)?;

            run_steps.push(RunStep {
                kind,
                program,
                args,
                platforms,
            });
        }

        parsed.push(Action {
            name,
            logic,
            run: run_steps,
        });
    }

    Ok(parsed)
}

fn parse_run_args(
    run_obj: &Map<String, Value>,
    run_path: &str,
    schema_field_types: &BTreeMap<String, FieldType>,
) -> Result<Vec<RunArg>, BuildError> {
    get_required_array(run_obj, "args", run_path)?
        .iter()
        .enumerate()
        .map(|(arg_idx, arg)| parse_run_arg(arg, &format!("{run_path}.args[{arg_idx}]"), schema_field_types))
        .collect()
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
        Value::String(platform) => Ok(Some(vec![normalize_platform_value(platform, &platform_path)?])),
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
        return Err(BuildError::config(path, "expected a non-empty platform value"));
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
            ))
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

fn render_agent_model(config: &AgentConfig) -> String {
    let mut struct_fields = String::new();
    for (name, rust_type) in &config.fields {
        struct_fields.push_str(&format!("    pub {name}: {rust_type},\n"));
    }

    let prompt_literal = rust_string_literal(&config.prompt);

    let mut url_list = String::new();
    for url in &config.resource_urls {
        url_list.push_str(&format!(
            "        ResourceUrl {{ url: {url}, description: {description} }},\n",
            url = rust_string_literal(&url.url),
            description = rust_string_literal(&url.description)
        ));
    }

    let mut action_code = String::new();
    for action in &config.actions {
        let logic_json = action.logic.to_string();
        let run_steps = action
            .run
            .iter()
            .map(|run_step| {
                let args = run_step
                    .args
                    .iter()
                    .map(|arg| match arg {
                        RunArg::Literal(literal) => format!(
                            "RunArg::Literal({}.to_string())",
                            rust_string_literal(literal)
                        ),
                        RunArg::Variable(variable) => format!(
                            "RunArg::Variable({}.to_string())",
                            rust_string_literal(variable)
                        ),
                    })
                    .collect::<Vec<_>>()
                    .join(", ");
                let platforms = run_step
                    .platforms
                    .as_ref()
                    .map(|platforms| {
                        let rendered = platforms
                            .iter()
                            .map(|platform| format!("{}.to_string()", rust_string_literal(platform)))
                            .collect::<Vec<_>>()
                            .join(", ");
                        format!("Some(vec![{}])", rendered)
                    })
                    .unwrap_or_else(|| "None".to_string());

                format!(
                    "RunStep {{
                        kind: {}.to_string(),
                        program: {}.to_string(),
                        args: vec![{}],
                        platforms: {},
                    }}",
                    rust_string_literal(&run_step.kind),
                    rust_string_literal(&run_step.program),
                    args,
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
pub struct ResourceUrl {{
    pub url: &'static str,
    pub description: &'static str,
}}

pub fn prompt() -> String {{
    String::from({prompt_literal})
}}

pub fn resource_urls() -> Vec<ResourceUrl> {{
    vec![{url_list}]
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
pub struct RunStep {{
    kind: String,
    program: String,
    args: Vec<RunArg>,
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
        prompt_literal = prompt_literal,
        url_list = url_list,
        action_code = action_code
    )
}

fn rust_string_literal(value: &str) -> String {
    format!("{value:?}")
}
