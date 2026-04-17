use serde_json::{Map, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

const SUPPORTED_FILE_EXTENSIONS: [&str; 24] = [
    "pdf", "docx", "csv", "xla", "xlb", "xlc", "xlm", "xls", "xlsx", "xlt", "xlw", "tsv", "iif",
    "doc", "dot", "odt", "rtf", "pot", "ppa", "pps", "ppt", "pptx", "pwz", "wiz",
];
const SUPPORTED_GENERATED_IMAGE_EXTENSIONS: [&str; 4] = ["png", "jpg", "jpeg", "webp"];

#[derive(Clone, Debug, PartialEq)]
enum SchemaFieldType {
    String,
    Number,
    Integer,
    Boolean,
}

#[derive(Clone, Debug)]
enum NumericConstraintValue {
    Number(f64),
    Integer(i64),
}

#[derive(Clone, Debug, Default)]
struct NumericConstraints {
    minimum: Option<NumericConstraintValue>,
    maximum: Option<NumericConstraintValue>,
    exclusive_minimum: Option<NumericConstraintValue>,
    exclusive_maximum: Option<NumericConstraintValue>,
}

#[derive(Clone, Debug)]
struct SchemaProperty {
    name: String,
    field_type: SchemaFieldType,
    schema_value: Value,
    enum_values: Option<Vec<String>>,
    numeric_constraints: NumericConstraints,
}

#[derive(Clone, Debug)]
pub(crate) struct RuntimeAgentDefinition {
    named_inputs: Vec<crate::Input>,
    runtime_var_specs: Vec<crate::RuntimeVarSpec>,
    schema_properties: Vec<SchemaProperty>,
    action_execution: crate::ActionExecutionMode,
    actions: Vec<crate::Action>,
}

impl RuntimeAgentDefinition {
    pub(crate) fn load_from_path(path: &Path) -> Result<Self, String> {
        let raw = fs::read_to_string(path)
            .map_err(|error| format!("failed to read '{}': {error}", path.display()))?;
        Self::from_str(raw.as_str())
    }

    pub(crate) fn from_str(json_str: &str) -> Result<Self, String> {
        let root = serde_json::from_str::<Value>(json_str)
            .map_err(|error| format!("failed to parse agent JSON: {error}"))?;
        let root_obj = expect_object(&root, "$")?;

        let version = required_string(root_obj, "version", "$")?;
        validate_schema_version(version, "$.version")?;

        let schema_properties = parse_schema_properties(root_obj)?;
        let has_output_schema_properties = !schema_properties.is_empty();
        let named_inputs = parse_inputs(root_obj, has_output_schema_properties)?;
        let runtime_var_specs = parse_runtime_vars(root_obj)?;
        let action_execution = parse_action_execution(root_obj)?;
        let actions = parse_actions(root_obj)?;

        Ok(Self {
            named_inputs,
            runtime_var_specs,
            schema_properties,
            action_execution,
            actions,
        })
    }

    pub(crate) fn named_inputs(&self) -> Vec<crate::Input> {
        self.named_inputs.clone()
    }

    pub(crate) fn runtime_var_specs(&self) -> Vec<crate::RuntimeVarSpec> {
        self.runtime_var_specs.clone()
    }

    pub(crate) fn actions(&self) -> Vec<crate::Action> {
        self.actions.clone()
    }

    pub(crate) fn action_execution(&self) -> crate::ActionExecutionMode {
        self.action_execution
    }

    pub(crate) fn has_output_schema_properties(&self) -> bool {
        !self.schema_properties.is_empty()
    }

    pub(crate) fn json_schema_value(&self) -> Value {
        let mut properties = Map::new();
        let mut required = Vec::with_capacity(self.schema_properties.len());

        for property in &self.schema_properties {
            properties.insert(property.name.clone(), property.schema_value.clone());
            required.push(Value::String(property.name.clone()));
        }

        let mut schema = Map::new();
        schema.insert("type".to_string(), Value::String("object".to_string()));
        schema.insert("properties".to_string(), Value::Object(properties));
        schema.insert("additionalProperties".to_string(), Value::Bool(false));
        schema.insert("required".to_string(), Value::Array(required));
        Value::Object(schema)
    }

    pub(crate) fn validate_provider_output(&self, raw: &str) -> Result<Value, String> {
        let parsed = serde_json::from_str::<Value>(raw).map_err(|_| {
            "The provider returned output that could not be parsed as JSON.".to_string()
        })?;
        self.validate_response_value(&parsed)?;
        Ok(parsed)
    }

    fn validate_response_value(&self, value: &Value) -> Result<(), String> {
        if self.schema_properties.is_empty() {
            match value {
                Value::Object(object) if object.is_empty() => return Ok(()),
                Value::Object(_) => {
                    return Err(
                        "Expected an empty JSON object because this agent skips model output."
                            .to_string(),
                    )
                }
                _ => return Err("Expected a top-level JSON object.".to_string()),
            }
        }

        let object = value
            .as_object()
            .ok_or_else(|| "Expected a top-level JSON object.".to_string())?;

        let allowed = self
            .schema_properties
            .iter()
            .map(|property| property.name.as_str())
            .collect::<BTreeSet<_>>();

        for property in &self.schema_properties {
            let field_value = object.get(property.name.as_str()).ok_or_else(|| {
                format!(
                    "Missing required field `{}` in provider output.",
                    property.name
                )
            })?;
            validate_property_value(property, field_value)?;
        }

        for key in object.keys() {
            if !allowed.contains(key.as_str()) {
                return Err(format!(
                    "Unexpected field `{}` in provider output; the schema is strict.",
                    key
                ));
            }
        }

        Ok(())
    }
}

impl crate::commands::runtime::InvocationDefinition for RuntimeAgentDefinition {
    fn named_inputs(&self) -> Vec<crate::Input> {
        self.named_inputs()
    }

    fn runtime_var_specs(&self) -> Vec<crate::RuntimeVarSpec> {
        self.runtime_var_specs()
    }

    fn action_execution(&self) -> crate::ActionExecutionMode {
        self.action_execution()
    }

    fn has_output_schema_properties(&self) -> bool {
        self.has_output_schema_properties()
    }

    fn json_schema_value(&self) -> Value {
        self.json_schema_value()
    }

    fn actions(&self) -> Vec<crate::Action> {
        self.actions()
    }

    fn validate_provider_output(&self, raw: &str) -> Result<Value, String> {
        self.validate_provider_output(raw)
    }
}

fn parse_action_execution(
    root_obj: &Map<String, Value>,
) -> Result<crate::ActionExecutionMode, String> {
    let Some(value) = root_obj.get("action_execution") else {
        return Ok(crate::ActionExecutionMode::Sequential);
    };

    let execution = value
        .as_str()
        .ok_or_else(|| "$.action_execution: expected a string".to_string())?
        .trim()
        .to_ascii_lowercase();

    match execution.as_str() {
        "sequential" => Ok(crate::ActionExecutionMode::Sequential),
        "parallel" => Ok(crate::ActionExecutionMode::Parallel),
        _ => Err("$.action_execution: expected `sequential` or `parallel`".to_string()),
    }
}

fn parse_inputs(
    root_obj: &Map<String, Value>,
    has_output_schema_properties: bool,
) -> Result<Vec<crate::Input>, String> {
    let Some(raw_inputs) = root_obj.get("inputs") else {
        return Ok(Vec::new());
    };
    let inputs = raw_inputs
        .as_array()
        .ok_or_else(|| "$.inputs: expected an array".to_string())?;
    if inputs.is_empty() {
        return Err("$.inputs: must contain at least one entry".to_string());
    }

    let mut parsed = Vec::with_capacity(inputs.len());
    let mut seen_names = BTreeSet::new();
    for (index, entry) in inputs.iter().enumerate() {
        let path = format!("$.inputs[{index}]");
        let entry_obj = expect_object(entry, path.as_str())?;

        let name = optional_named_identifier(entry_obj, "name", path.as_str())?;
        if !has_output_schema_properties && name.is_none() {
            return Err(format!(
                "{path}.name: top-level `inputs` in the structural action-only shape must declare `name`"
            ));
        }

        if let Some(name) = name.as_ref() {
            if !seen_names.insert(name.clone()) {
                return Err(format!("{path}.name: duplicate named input `{name}`"));
            }
        }

        let input_type = required_string(entry_obj, "type", path.as_str())?
            .trim()
            .to_ascii_lowercase();
        let (kind, value) = match input_type.as_str() {
            "text" => (
                crate::InputKind::Text,
                optional_string(entry_obj, "text", path.as_str())?,
            ),
            "url" => (
                crate::InputKind::Url,
                optional_string(entry_obj, "url", path.as_str())?,
            ),
            "image" => (
                crate::InputKind::Image,
                optional_owned_path(entry_obj, "path", path.as_str(), false)?,
            ),
            "file" => (
                crate::InputKind::File,
                optional_owned_path(entry_obj, "path", path.as_str(), true)?,
            ),
            other => {
                return Err(format!(
                    "{path}.type: unsupported input type `{other}` (supported: `text`, `url`, `image`, `file`)"
                ))
            }
        };

        if value.is_none() && name.is_none() {
            let value_field = match kind {
                crate::InputKind::Text => "text",
                crate::InputKind::Url => "url",
                crate::InputKind::Image | crate::InputKind::File => "path",
            };
            return Err(format!(
                "{path}.{value_field}: unnamed top-level inputs must include a baked value"
            ));
        }

        parsed.push(crate::Input { name, kind, value });
    }

    Ok(parsed)
}

fn parse_runtime_vars(root_obj: &Map<String, Value>) -> Result<Vec<crate::RuntimeVarSpec>, String> {
    let Some(value) = root_obj.get("runtime_vars") else {
        return Ok(Vec::new());
    };

    let runtime_vars = expect_object(value, "$.runtime_vars")?;
    let mut parsed = Vec::with_capacity(runtime_vars.len());
    for (name, raw_spec) in runtime_vars {
        validate_flat_identifier(
            name,
            format!("$.runtime_vars.{name}").as_str(),
            "runtime variable",
        )?;
        let path = format!("$.runtime_vars.{name}");
        let spec = expect_object(raw_spec, path.as_str())?;
        let field_type = parse_runtime_var_type(spec, path.as_str())?;
        let default_value = parse_runtime_var_default(spec, path.as_str(), field_type)?;
        parsed.push(crate::RuntimeVarSpec {
            name: name.clone(),
            field_type,
            default_value,
        });
    }

    Ok(parsed)
}

fn parse_runtime_var_type(
    spec: &Map<String, Value>,
    path: &str,
) -> Result<crate::RuntimeVarType, String> {
    let raw = required_string(spec, "type", path)?
        .trim()
        .to_ascii_lowercase();
    match raw.as_str() {
        "string" => Ok(crate::RuntimeVarType::String),
        "boolean" => Ok(crate::RuntimeVarType::Boolean),
        "number" => Ok(crate::RuntimeVarType::Number),
        "integer" => Ok(crate::RuntimeVarType::Integer),
        _ => Err(format!(
            "{path}.type: unsupported runtime var type `{raw}` (supported: `string`, `boolean`, `number`, `integer`)"
        )),
    }
}

fn parse_runtime_var_default(
    spec: &Map<String, Value>,
    path: &str,
    field_type: crate::RuntimeVarType,
) -> Result<Option<Value>, String> {
    let Some(default_value) = spec.get("default") else {
        return Ok(None);
    };

    validate_runtime_var_default(
        field_type,
        default_value,
        format!("{path}.default").as_str(),
    )?;
    Ok(Some(default_value.clone()))
}

fn parse_schema_properties(root_obj: &Map<String, Value>) -> Result<Vec<SchemaProperty>, String> {
    let schema = expect_object(
        required_field(root_obj, "agent_schema", "$")?,
        "$.agent_schema",
    )?;
    let schema_type = required_string(schema, "type", "$.agent_schema")?;
    if schema_type != "object" {
        return Err(format!(
            "$.agent_schema.type: expected `object`, got `{schema_type}`"
        ));
    }

    let properties = expect_object(
        required_field(schema, "properties", "$.agent_schema")?,
        "$.agent_schema.properties",
    )?;

    let mut parsed = Vec::with_capacity(properties.len());
    for (name, raw_property) in properties {
        if name == "runtime" {
            return Err(format!(
                "$.agent_schema.properties.{name}: `runtime` is reserved for invocation-scoped runtime variables"
            ));
        }

        let property_path = format!("$.agent_schema.properties.{name}");
        let property = expect_object(raw_property, property_path.as_str())?;
        let field_type = parse_schema_field_type(property, property_path.as_str())?;
        let enum_values = parse_enum_values(property, property_path.as_str(), &field_type)?;
        let numeric_constraints =
            parse_numeric_constraints(property, property_path.as_str(), &field_type)?;

        parsed.push(SchemaProperty {
            name: name.clone(),
            field_type,
            schema_value: raw_property.clone(),
            enum_values,
            numeric_constraints,
        });
    }

    Ok(parsed)
}

fn parse_schema_field_type(
    property: &Map<String, Value>,
    path: &str,
) -> Result<SchemaFieldType, String> {
    let raw = required_string(property, "type", path)?
        .trim()
        .to_ascii_lowercase();
    match raw.as_str() {
        "string" => Ok(SchemaFieldType::String),
        "number" => Ok(SchemaFieldType::Number),
        "integer" => Ok(SchemaFieldType::Integer),
        "boolean" => Ok(SchemaFieldType::Boolean),
        _ => Err(format!(
            "{path}.type: unsupported schema type `{raw}` (supported: `string`, `number`, `integer`, `boolean`)"
        )),
    }
}

fn parse_enum_values(
    property: &Map<String, Value>,
    path: &str,
    field_type: &SchemaFieldType,
) -> Result<Option<Vec<String>>, String> {
    let Some(raw_enum) = property.get("enum") else {
        return Ok(None);
    };

    if *field_type != SchemaFieldType::String {
        return Err(format!(
            "{path}.enum: `enum` is only supported for `string` fields"
        ));
    }

    let values = raw_enum
        .as_array()
        .ok_or_else(|| format!("{path}.enum: expected an array of string values"))?;
    if values.is_empty() {
        return Err(format!("{path}.enum: expected at least one value"));
    }

    let mut parsed = Vec::with_capacity(values.len());
    let mut seen = BTreeSet::new();
    for (index, value) in values.iter().enumerate() {
        let item_path = format!("{path}.enum[{index}]");
        let item = value
            .as_str()
            .ok_or_else(|| format!("{item_path}: expected a string enum value"))?;
        if item.is_empty() {
            return Err(format!("{item_path}: enum values must be non-empty"));
        }
        if !seen.insert(item.to_string()) {
            return Err(format!("{item_path}: duplicate enum value `{item}`"));
        }
        parsed.push(item.to_string());
    }

    Ok(Some(parsed))
}

fn parse_numeric_constraints(
    property: &Map<String, Value>,
    path: &str,
    field_type: &SchemaFieldType,
) -> Result<NumericConstraints, String> {
    if !matches!(
        field_type,
        SchemaFieldType::Number | SchemaFieldType::Integer
    ) {
        for key in ["minimum", "maximum", "exclusiveMinimum", "exclusiveMaximum"] {
            if property.contains_key(key) {
                return Err(format!(
                    "{path}.{key}: numeric constraints are only supported for `number` and `integer` fields"
                ));
            }
        }
        return Ok(NumericConstraints::default());
    }

    let minimum = parse_numeric_constraint(property, "minimum", path, field_type)?;
    let maximum = parse_numeric_constraint(property, "maximum", path, field_type)?;
    let exclusive_minimum =
        parse_numeric_constraint(property, "exclusiveMinimum", path, field_type)?;
    let exclusive_maximum =
        parse_numeric_constraint(property, "exclusiveMaximum", path, field_type)?;

    if minimum.is_some() && exclusive_minimum.is_some() {
        return Err(format!(
            "{path}: use either `minimum` or `exclusiveMinimum`, not both"
        ));
    }
    if maximum.is_some() && exclusive_maximum.is_some() {
        return Err(format!(
            "{path}: use either `maximum` or `exclusiveMaximum`, not both"
        ));
    }

    Ok(NumericConstraints {
        minimum,
        maximum,
        exclusive_minimum,
        exclusive_maximum,
    })
}

fn parse_numeric_constraint(
    property: &Map<String, Value>,
    key: &str,
    path: &str,
    field_type: &SchemaFieldType,
) -> Result<Option<NumericConstraintValue>, String> {
    let Some(value) = property.get(key) else {
        return Ok(None);
    };

    let key_path = format!("{path}.{key}");
    match field_type {
        SchemaFieldType::Integer => value
            .as_i64()
            .map(NumericConstraintValue::Integer)
            .ok_or_else(|| format!("{key_path}: expected an integer value"))
            .map(Some),
        SchemaFieldType::Number => value
            .as_f64()
            .filter(|number| number.is_finite())
            .map(NumericConstraintValue::Number)
            .ok_or_else(|| format!("{key_path}: expected a finite numeric value"))
            .map(Some),
        _ => Ok(None),
    }
}

fn parse_actions(root_obj: &Map<String, Value>) -> Result<Vec<crate::Action>, String> {
    let actions = required_array(root_obj, "actions", "$")?;
    let mut parsed = Vec::with_capacity(actions.len());
    for (action_index, raw_action) in actions.iter().enumerate() {
        let action_path = format!("$.actions[{action_index}]");
        let action_obj = expect_object(raw_action, action_path.as_str())?;
        let name = required_string(action_obj, "name", action_path.as_str())?.to_string();
        if name.trim().is_empty() {
            return Err(format!("{action_path}.name: must be a non-empty string"));
        }
        let logic = required_field(action_obj, "logic", action_path.as_str())?.clone();
        if !logic.is_object() {
            return Err(format!(
                "{action_path}.logic: expected a JSON Logic object expression"
            ));
        }

        let run_steps = required_array(action_obj, "run", action_path.as_str())?;
        if run_steps.is_empty() {
            return Err(format!("{action_path}.run: must contain at least one step"));
        }

        let mut steps = Vec::with_capacity(run_steps.len());
        for (step_index, raw_step) in run_steps.iter().enumerate() {
            let run_path = format!("{action_path}.run[{step_index}]");
            steps.push(parse_run_step(raw_step, run_path.as_str())?);
        }

        parsed.push(crate::Action {
            name,
            logic,
            run: steps,
        });
    }

    Ok(parsed)
}

fn parse_run_step(value: &Value, path: &str) -> Result<crate::RunStep, String> {
    let run_obj = expect_object(value, path)?;
    let kind = required_string(run_obj, "kind", path)?.to_string();
    let status_variable = optional_capture_name(run_obj, "status_variable", path)?;
    let error_variable = optional_capture_name(run_obj, "error_variable", path)?;
    let failure_mode = optional_failure_mode(run_obj, path)?;
    let when = optional_logic_object(run_obj, "when", path)?;
    let platforms = optional_platforms(run_obj, path)?;

    match kind.as_str() {
        "exec" => Ok(crate::RunStep {
            kind,
            program: Some(required_non_empty_string(run_obj, "program", path)?),
            model: None,
            profile: None,
            output_variable: optional_capture_name(run_obj, "output_variable", path)?,
            status_variable,
            error_variable,
            failure_mode,
            when,
            args: required_run_args(run_obj, path)?,
            prompt: None,
            path: None,
            subject: None,
            text: None,
            agent: None,
            tool_name: None,
            tool_params: BTreeMap::new(),
            run_vars: None,
            input_overrides: None,
            inputs: None,
            input_mode: None,
            ignore_tools: false,
            platforms,
        }),
        "email_me" => Ok(crate::RunStep {
            kind,
            program: None,
            model: None,
            profile: None,
            output_variable: None,
            status_variable,
            error_variable,
            failure_mode,
            when,
            args: Vec::new(),
            prompt: None,
            path: None,
            subject: Some(parse_string_parts_field(run_obj, "subject", path)?),
            text: Some(parse_string_parts_field(run_obj, "text", path)?),
            agent: None,
            tool_name: None,
            tool_params: BTreeMap::new(),
            run_vars: None,
            input_overrides: None,
            inputs: None,
            input_mode: None,
            ignore_tools: false,
            platforms,
        }),
        "agent" => {
            let (agent, agent_path) = required_child_artifact(run_obj, path)?;
            validate_child_agent_target(agent.as_str(), agent_path.as_str())?;
            let input_mode = optional_action_input_mode(run_obj, path)?;
            let inputs = optional_action_inputs(run_obj, path)?;
            if input_mode.is_some() && inputs.is_none() {
                return Err(format!("{path}.input_mode: `input_mode` requires `inputs`"));
            }
            Ok(crate::RunStep {
                kind,
                program: None,
                model: None,
                profile: optional_string_run_arg(run_obj, "profile", path)?,
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
                tool_name: None,
                tool_params: BTreeMap::new(),
                run_vars: optional_action_run_vars(run_obj, path)?,
                input_overrides: optional_action_input_overrides(run_obj, path)?,
                inputs,
                input_mode,
                ignore_tools: optional_boolean_field(run_obj, "ignore_tools", path)?.unwrap_or(false),
                platforms,
            })
        }
        "tool" => Ok(crate::RunStep {
            kind,
            program: None,
            model: None,
            profile: None,
            output_variable: optional_capture_name(run_obj, "output_variable", path)?,
            status_variable,
            error_variable,
            failure_mode,
            when,
            args: Vec::new(),
            prompt: None,
            path: None,
            subject: None,
            text: None,
            agent: None,
            tool_name: Some(required_tool_name(run_obj, path)?),
            tool_params: optional_tool_params(run_obj, path)?,
            run_vars: None,
            input_overrides: None,
            inputs: None,
            input_mode: None,
            ignore_tools: false,
            platforms,
        }),
        "generate_image" => {
            let path_parts = parse_string_parts_field(run_obj, "path", path)?;
            if let Some(literal_path) = resolve_literal_run_args(&path_parts) {
                validate_definition_owned_local_path(
                    literal_path.as_str(),
                    format!("{path}.path").as_str(),
                    "generated image output",
                )?;
                validate_generated_image_output_extension(
                    literal_path.as_str(),
                    format!("{path}.path").as_str(),
                )?;
            }

            Ok(crate::RunStep {
                kind,
                program: None,
                model: optional_string_run_arg(run_obj, "model", path)?,
                profile: optional_string_run_arg(run_obj, "profile", path)?,
                output_variable: None,
                status_variable,
                error_variable,
                failure_mode,
                when,
                args: Vec::new(),
                prompt: Some(parse_string_parts_field(run_obj, "prompt", path)?),
                path: Some(path_parts),
                subject: None,
                text: None,
                agent: None,
                tool_name: None,
                tool_params: BTreeMap::new(),
                run_vars: None,
                input_overrides: None,
                inputs: None,
                input_mode: None,
                ignore_tools: false,
                platforms,
            })
        }
        other => Err(format!(
            "{path}.kind: unsupported kind `{other}` (supported: `exec`, `email_me`, `agent`, `tool`, `generate_image`)"
        )),
    }
}

fn required_tool_name(run_obj: &Map<String, Value>, path: &str) -> Result<String, String> {
    let name = required_non_empty_string(run_obj, "name", path)?;
    validate_tool_identifier(name.as_str(), format!("{path}.name").as_str())?;
    Ok(name)
}

fn optional_tool_params(
    run_obj: &Map<String, Value>,
    path: &str,
) -> Result<BTreeMap<String, crate::ToolParamValue>, String> {
    let Some(value) = run_obj.get("params") else {
        return Ok(BTreeMap::new());
    };

    let params = expect_object(value, format!("{path}.params").as_str())?;
    let mut parsed = BTreeMap::new();
    for (name, value) in params {
        validate_tool_identifier(name, format!("{path}.params.{name}").as_str())?;
        parsed.insert(
            name.clone(),
            parse_tool_param_value(value, format!("{path}.params.{name}").as_str())?,
        );
    }
    Ok(parsed)
}

fn parse_tool_param_value(value: &Value, path: &str) -> Result<crate::ToolParamValue, String> {
    match value {
        Value::String(_) | Value::Bool(_) => Ok(crate::ToolParamValue::Literal(value.clone())),
        Value::Number(number) if number.is_i64() || number.is_u64() || number.is_f64() => {
            Ok(crate::ToolParamValue::Literal(value.clone()))
        }
        Value::Object(map) => {
            if map.len() != 1 {
                return Err(format!(
                    "{path}: expected a scalar literal or an object of the form `{{ \"var\": \"field_name\" }}`"
                ));
            }
            let Some(variable) = map.get("var") else {
                return Err(format!(
                    "{path}: expected a scalar literal or an object of the form `{{ \"var\": \"field_name\" }}`"
                ));
            };
            let variable_name = variable
                .as_str()
                .ok_or_else(|| format!("{path}.var: expected `var` to be a string field name"))?
                .trim()
                .to_string();
            validate_tool_variable_lookup_name(
                variable_name.as_str(),
                format!("{path}.var").as_str(),
            )?;
            Ok(crate::ToolParamValue::Variable(variable_name))
        }
        _ => Err(format!(
            "{path}: expected a string, boolean, number, or an object of the form `{{ \"var\": \"field_name\" }}`"
        )),
    }
}

fn required_run_args(
    run_obj: &Map<String, Value>,
    path: &str,
) -> Result<Vec<crate::RunArg>, String> {
    let args = required_array(run_obj, "args", path)?;
    args.iter()
        .enumerate()
        .map(|(index, arg)| parse_run_arg(arg, format!("{path}.args[{index}]").as_str()))
        .collect()
}

fn parse_run_arg(value: &Value, path: &str) -> Result<crate::RunArg, String> {
    match value {
        Value::String(literal) => Ok(crate::RunArg::Literal(literal.to_string())),
        Value::Object(map) => {
            if map.len() != 1 {
                return Err(format!(
                    "{path}: expected an arg object with exactly one key (`var`)"
                ));
            }
            let Some(variable) = map.get("var") else {
                return Err(format!(
                    "{path}: expected a string literal arg or an object of the form `{{ \"var\": \"field_name\" }}`"
                ));
            };
            let variable_name = variable
                .as_str()
                .ok_or_else(|| format!("{path}.var: expected `var` to be a string field name"))?
                .trim()
                .to_string();
            if variable_name.is_empty() {
                return Err(format!("{path}.var: variable name cannot be empty"));
            }
            Ok(crate::RunArg::Variable(variable_name))
        }
        _ => Err(format!(
            "{path}: expected a string literal arg or an object of the form `{{ \"var\": \"field_name\" }}`"
        )),
    }
}

fn parse_string_parts_field(
    run_obj: &Map<String, Value>,
    field_name: &str,
    path: &str,
) -> Result<Vec<crate::RunArg>, String> {
    parse_string_parts_value(
        required_field(run_obj, field_name, path)?,
        format!("{path}.{field_name}").as_str(),
    )
}

fn parse_string_parts_value(value: &Value, path: &str) -> Result<Vec<crate::RunArg>, String> {
    match value {
        Value::String(literal) => {
            if literal.trim().is_empty() {
                return Err(format!("{path}: must be a non-empty string"));
            }
            Ok(vec![crate::RunArg::Literal(literal.to_string())])
        }
        Value::Array(parts) => {
            if parts.is_empty() {
                return Err(format!(
                    "{path}: expected at least one string or variable part"
                ));
            }
            parts
                .iter()
                .enumerate()
                .map(|(index, part)| parse_run_arg(part, format!("{path}[{index}]").as_str()))
                .collect()
        }
        _ => Err(format!(
            "{path}: expected a string or an array of string/variable parts"
        )),
    }
}

fn resolve_literal_run_args(parts: &[crate::RunArg]) -> Option<String> {
    let mut resolved = String::new();
    for part in parts {
        match part {
            crate::RunArg::Literal(literal) => resolved.push_str(literal),
            crate::RunArg::Variable(_) => return None,
        }
    }
    Some(resolved)
}

fn optional_string_run_arg(
    run_obj: &Map<String, Value>,
    field_name: &str,
    path: &str,
) -> Result<Option<crate::RunArg>, String> {
    let Some(value) = run_obj.get(field_name) else {
        return Ok(None);
    };
    let parsed = parse_run_arg(value, format!("{path}.{field_name}").as_str())?;
    if matches!(&parsed, crate::RunArg::Literal(text) if text.trim().is_empty()) {
        return Err(format!("{path}.{field_name}: must be a non-empty string"));
    }
    Ok(Some(parsed))
}

fn optional_failure_mode(
    run_obj: &Map<String, Value>,
    path: &str,
) -> Result<Option<crate::FailureMode>, String> {
    let Some(value) = run_obj.get("failure_mode") else {
        return Ok(None);
    };
    let raw = value
        .as_str()
        .ok_or_else(|| {
            format!(
                "{path}.failure_mode: expected `failure_mode` to be a string (`stop`, `continue`, or `abort`)"
            )
        })?
        .trim();
    match raw {
        "stop" => Ok(Some(crate::FailureMode::Stop)),
        "continue" => Ok(Some(crate::FailureMode::Continue)),
        "abort" => Ok(Some(crate::FailureMode::Abort)),
        _ => Err(format!(
            "{path}.failure_mode: unsupported `failure_mode` (supported: `stop`, `continue`, `abort`)"
        )),
    }
}

fn optional_boolean_field(
    run_obj: &Map<String, Value>,
    field_name: &str,
    path: &str,
) -> Result<Option<bool>, String> {
    let Some(value) = run_obj.get(field_name) else {
        return Ok(None);
    };

    value
        .as_bool()
        .ok_or_else(|| format!("{path}.{field_name}: expected a boolean"))
        .map(Some)
}

fn optional_logic_object(
    run_obj: &Map<String, Value>,
    field_name: &str,
    path: &str,
) -> Result<Option<Value>, String> {
    let Some(value) = run_obj.get(field_name) else {
        return Ok(None);
    };
    if !value.is_object() {
        return Err(format!("{path}.{field_name}: expected a JSON Logic object"));
    }
    Ok(Some(value.clone()))
}

fn optional_action_input_mode(
    run_obj: &Map<String, Value>,
    path: &str,
) -> Result<Option<crate::ActionInputMode>, String> {
    let Some(value) = run_obj.get("input_mode") else {
        return Ok(None);
    };
    let raw = value
        .as_str()
        .ok_or_else(|| {
            format!(
                "{path}.input_mode: expected `input_mode` to be a string (`replace`, `append`, or `prepend`)"
            )
        })?
        .trim();
    match raw {
        "replace" => Ok(Some(crate::ActionInputMode::Replace)),
        "append" => Ok(Some(crate::ActionInputMode::Append)),
        "prepend" => Ok(Some(crate::ActionInputMode::Prepend)),
        _ => Err(format!(
            "{path}.input_mode: unsupported `input_mode` (supported: `replace`, `append`, `prepend`)"
        )),
    }
}

fn optional_action_input_overrides(
    run_obj: &Map<String, Value>,
    path: &str,
) -> Result<Option<Vec<crate::ActionInputOverride>>, String> {
    let Some(value) = run_obj.get("input_overrides") else {
        return Ok(None);
    };
    let overrides = expect_object(value, format!("{path}.input_overrides").as_str())?;
    if overrides.is_empty() {
        return Err(format!(
            "{path}.input_overrides: must contain at least one named child input override"
        ));
    }

    let mut parsed = Vec::with_capacity(overrides.len());
    for (name, entry_value) in overrides {
        validate_flat_identifier(
            name,
            format!("{path}.input_overrides.{name}").as_str(),
            "named input",
        )?;
        parsed.push(crate::ActionInputOverride {
            name: name.clone(),
            value: parse_action_input_override_value(
                entry_value,
                format!("{path}.input_overrides.{name}").as_str(),
            )?,
        });
    }

    Ok(Some(parsed))
}

fn parse_action_input_override_value(
    value: &Value,
    path: &str,
) -> Result<crate::ActionInputOverrideValue, String> {
    match value {
        Value::String(literal) => Ok(crate::ActionInputOverrideValue::Literal(literal.clone())),
        Value::Object(map) => {
            if map.len() != 1 {
                return Err(format!(
                    "{path}: expected a string literal override or an object with exactly one key (`var` or `input`)"
                ));
            }
            if let Some(variable) = map.get("var") {
                let variable_name = variable
                    .as_str()
                    .ok_or_else(|| format!("{path}.var: expected `var` to be a string field name"))?
                    .trim()
                    .to_string();
                if variable_name.is_empty() {
                    return Err(format!("{path}.var: variable name cannot be empty"));
                }
                return Ok(crate::ActionInputOverrideValue::Variable(variable_name));
            }
            if let Some(input) = map.get("input") {
                let input_name = input
                    .as_str()
                    .ok_or_else(|| format!("{path}.input: expected `input` to be a string named parent input reference"))?
                    .trim()
                    .to_string();
                validate_flat_identifier(input_name.as_str(), format!("{path}.input").as_str(), "named input")?;
                return Ok(crate::ActionInputOverrideValue::NamedInput { input: input_name });
            }
            Err(format!(
                "{path}: expected a string literal override or an object of the form `{{ \"var\": \"field_name\" }}` or `{{ \"input\": \"name\" }}`"
            ))
        }
        _ => Err(format!(
            "{path}: expected a string literal override or an object of the form `{{ \"var\": \"field_name\" }}` or `{{ \"input\": \"name\" }}`"
        )),
    }
}

fn optional_action_run_vars(
    run_obj: &Map<String, Value>,
    path: &str,
) -> Result<Option<Vec<crate::ActionRunVar>>, String> {
    let Some(value) = run_obj.get("run_vars") else {
        return Ok(None);
    };
    let run_vars = expect_object(value, format!("{path}.run_vars").as_str())?;
    if run_vars.is_empty() {
        return Err(format!(
            "{path}.run_vars: must contain at least one child runtime var"
        ));
    }

    let mut parsed = Vec::with_capacity(run_vars.len());
    for (name, entry_value) in run_vars {
        validate_flat_identifier(
            name,
            format!("{path}.run_vars.{name}").as_str(),
            "runtime variable",
        )?;
        parsed.push(crate::ActionRunVar {
            name: name.clone(),
            value: parse_action_run_var_value(
                entry_value,
                format!("{path}.run_vars.{name}").as_str(),
            )?,
        });
    }

    Ok(Some(parsed))
}

fn parse_action_run_var_value(
    value: &Value,
    path: &str,
) -> Result<crate::ActionRunVarValue, String> {
    match value {
        Value::String(_) | Value::Bool(_) | Value::Number(_) => {
            Ok(crate::ActionRunVarValue::Literal(value.clone()))
        }
        Value::Object(map) => {
            if map.len() != 1 {
                return Err(format!(
                    "{path}: expected a scalar literal or an object with exactly one key (`var`)"
                ));
            }
            let Some(variable) = map.get("var") else {
                return Err(format!(
                    "{path}: expected a string, boolean, or number literal, or an object of the form `{{ \"var\": \"field_name\" }}`"
                ));
            };
            let variable_name = variable
                .as_str()
                .ok_or_else(|| format!("{path}.var: expected `var` to be a string field name"))?
                .trim()
                .to_string();
            if variable_name.is_empty() {
                return Err(format!("{path}.var: variable name cannot be empty"));
            }
            Ok(crate::ActionRunVarValue::Variable(variable_name))
        }
        _ => Err(format!(
            "{path}: expected a string, boolean, or number literal, or an object of the form `{{ \"var\": \"field_name\" }}`"
        )),
    }
}

fn optional_action_inputs(
    run_obj: &Map<String, Value>,
    path: &str,
) -> Result<Option<Vec<crate::ActionInput>>, String> {
    let Some(value) = run_obj.get("inputs") else {
        return Ok(None);
    };
    let inputs = value.as_array().ok_or_else(|| {
        format!("{path}.inputs: expected `inputs` to be an array of ordered input parts")
    })?;
    if inputs.is_empty() {
        return Err(format!(
            "{path}.inputs: must contain at least one child input"
        ));
    }

    inputs
        .iter()
        .enumerate()
        .map(|(index, input)| parse_action_input(input, format!("{path}.inputs[{index}]").as_str()))
        .collect::<Result<Vec<_>, _>>()
        .map(Some)
}

fn parse_action_input(value: &Value, path: &str) -> Result<crate::ActionInput, String> {
    let entry_obj = expect_object(value, path)?;
    if let Some(named_input) = entry_obj.get("input") {
        if entry_obj.len() != 1 {
            return Err(format!(
                "{path}: named child input references must use exactly `{{ \"input\": \"<name>\" }}`"
            ));
        }
        let input_name = named_input
            .as_str()
            .ok_or_else(|| format!("{path}.input: expected a string"))?
            .trim()
            .to_string();
        validate_flat_identifier(
            input_name.as_str(),
            format!("{path}.input").as_str(),
            "named input",
        )?;
        return Ok(crate::ActionInput::Named { input: input_name });
    }

    let input_type = required_string(entry_obj, "type", path)?
        .trim()
        .to_ascii_lowercase();
    match input_type.as_str() {
        "text" => Ok(crate::ActionInput::Text {
            text: parse_string_parts_field(entry_obj, "text", path)?,
        }),
        "url" => Ok(crate::ActionInput::Url {
            url: parse_string_parts_field(entry_obj, "url", path)?,
        }),
        "image" => Ok(crate::ActionInput::Image {
            path: parse_string_parts_field(entry_obj, "path", path)?,
        }),
        "file" => Ok(crate::ActionInput::File {
            path: parse_string_parts_field(entry_obj, "path", path)?,
        }),
        other => Err(format!(
            "{path}.type: unsupported input type `{other}` (supported: `text`, `url`, `image`, `file`; or use `{{ \"input\": \"<name>\" }}`)"
        )),
    }
}

fn optional_platforms(
    run_obj: &Map<String, Value>,
    path: &str,
) -> Result<Option<Vec<String>>, String> {
    let Some(value) = run_obj.get("platform") else {
        return Ok(None);
    };

    let values = match value {
        Value::String(platform) => vec![platform.to_string()],
        Value::Array(platforms) => {
            if platforms.is_empty() {
                return Err(format!(
                    "{path}.platform: expected at least one platform entry"
                ));
            }
            let mut parsed = Vec::with_capacity(platforms.len());
            for (index, platform) in platforms.iter().enumerate() {
                parsed.push(
                    platform
                        .as_str()
                        .ok_or_else(|| {
                            format!("{path}.platform[{index}]: expected a string platform value")
                        })?
                        .to_string(),
                );
            }
            parsed
        }
        _ => {
            return Err(format!(
                "{path}.platform: expected a string platform value or an array of string platform values"
            ))
        }
    };

    let mut parsed = Vec::with_capacity(values.len());
    for platform in values {
        let normalized = platform.trim().to_ascii_lowercase();
        if normalized.is_empty() {
            return Err(format!(
                "{path}.platform: expected a non-empty platform value"
            ));
        }
        match normalized.as_str() {
            "macos" | "linux" | "windows" => parsed.push(normalized),
            _ => {
                return Err(format!(
                    "{path}.platform: unsupported platform `{normalized}` (supported: `macos`, `linux`, `windows`)"
                ))
            }
        }
    }

    Ok(Some(parsed))
}

fn optional_capture_name(
    run_obj: &Map<String, Value>,
    field_name: &str,
    path: &str,
) -> Result<Option<String>, String> {
    let Some(value) = run_obj.get(field_name) else {
        return Ok(None);
    };
    let name = value
        .as_str()
        .ok_or_else(|| format!("{path}.{field_name}: expected a string"))?
        .trim()
        .to_string();
    validate_capture_name(name.as_str(), format!("{path}.{field_name}").as_str())?;
    Ok(Some(name))
}

fn validate_capture_name(name: &str, path: &str) -> Result<(), String> {
    if name.is_empty() {
        return Err(format!("{path}: captured variable name cannot be empty"));
    }
    if name.contains('.') {
        return Err(format!(
            "{path}: captured variable name must be flat; nested names with `.` are not supported"
        ));
    }
    Ok(())
}

fn validate_flat_identifier(name: &str, path: &str, label: &str) -> Result<(), String> {
    if name.trim().is_empty() {
        return Err(format!("{path}: {label} name cannot be empty"));
    }
    if name != name.trim() {
        return Err(format!(
            "{path}: {label} names cannot start or end with whitespace"
        ));
    }
    if name.chars().any(char::is_whitespace) {
        return Err(format!("{path}: {label} names cannot contain whitespace"));
    }
    if name.contains('.') {
        return Err(format!("{path}: {label} names must be flat"));
    }
    Ok(())
}

fn validate_tool_identifier(name: &str, path: &str) -> Result<(), String> {
    if name.trim().is_empty() {
        return Err(format!("{path}: tool names cannot be empty"));
    }
    if name != name.trim() {
        return Err(format!(
            "{path}: tool names cannot start or end with whitespace"
        ));
    }
    if name.chars().any(char::is_whitespace) {
        return Err(format!("{path}: tool names cannot contain whitespace"));
    }
    Ok(())
}

fn validate_tool_variable_lookup_name(name: &str, path: &str) -> Result<(), String> {
    if name.trim().is_empty() {
        return Err(format!("{path}: variable name cannot be empty"));
    }
    if name != name.trim() {
        return Err(format!(
            "{path}: variable names cannot start or end with whitespace"
        ));
    }
    if name.chars().any(char::is_whitespace) {
        return Err(format!("{path}: variable names cannot contain whitespace"));
    }

    let mut segments = name.split('.');
    let Some(first) = segments.next() else {
        return Err(format!("{path}: variable name cannot be empty"));
    };
    if first.is_empty() {
        return Err(format!("{path}: variable name cannot be empty"));
    }
    if first == "runtime" {
        let Some(runtime_name) = segments.next() else {
            return Err(format!(
                "{path}: runtime variable lookups must use the form `runtime.<name>`"
            ));
        };
        if runtime_name.is_empty() || segments.next().is_some() {
            return Err(format!(
                "{path}: runtime variable lookups must use the form `runtime.<name>`"
            ));
        }
        return Ok(());
    }
    if segments.next().is_some() {
        return Err(format!(
            "{path}: variable lookups must be flat or use `runtime.<name>`"
        ));
    }
    Ok(())
}

fn optional_named_identifier(
    entry_obj: &Map<String, Value>,
    field_name: &str,
    path: &str,
) -> Result<Option<String>, String> {
    let Some(value) = entry_obj.get(field_name) else {
        return Ok(None);
    };
    let name = value
        .as_str()
        .ok_or_else(|| format!("{path}.{field_name}: expected a string"))?
        .trim()
        .to_string();
    validate_flat_identifier(
        name.as_str(),
        format!("{path}.{field_name}").as_str(),
        "named input",
    )?;
    Ok(Some(name))
}

fn optional_owned_path(
    entry_obj: &Map<String, Value>,
    field_name: &str,
    path: &str,
    validate_file_extension: bool,
) -> Result<Option<String>, String> {
    let Some(value) = entry_obj.get(field_name) else {
        return Ok(None);
    };
    let raw_path = value
        .as_str()
        .ok_or_else(|| format!("{path}.{field_name}: expected a string"))?
        .trim()
        .to_string();
    validate_definition_owned_local_path(
        raw_path.as_str(),
        format!("{path}.{field_name}").as_str(),
        if validate_file_extension {
            "file input"
        } else {
            "image input"
        },
    )?;
    if validate_file_extension {
        validate_supported_file_extension(
            raw_path.as_str(),
            format!("{path}.{field_name}").as_str(),
        )?;
    }
    Ok(Some(raw_path))
}

fn required_child_artifact(
    object: &Map<String, Value>,
    path: &str,
) -> Result<(String, String), String> {
    let artifact_value = object.get("artifact");
    let legacy_agent_value = object.get("agent");

    if artifact_value.is_some() && legacy_agent_value.is_some() {
        return Err(format!(
            "{path}: specify only one of `artifact` or legacy `agent` for `kind: \"agent\"` steps"
        ));
    }

    let (field_name, value) = if let Some(value) = artifact_value {
        ("artifact", value)
    } else if let Some(value) = legacy_agent_value {
        ("agent", value)
    } else {
        return Err(format!("{path}.artifact: missing required field"));
    };

    let field_path = format!("{path}.{field_name}");
    let artifact = value
        .as_str()
        .ok_or_else(|| format!("{field_path}: expected a string"))?
        .trim()
        .to_string();

    if artifact.is_empty() {
        return Err(format!("{field_path}: must be a non-empty string"));
    }

    Ok((artifact, field_path))
}

fn validate_child_agent_target(raw_agent: &str, path: &str) -> Result<(), String> {
    let agent = raw_agent.trim();
    if agent.is_empty() {
        return Err(format!(
            "{path}: must use explicit same-level `./childagent` form"
        ));
    }

    let candidate = Path::new(agent);
    if candidate.is_absolute() {
        return Err(format!(
            "{path}: must use explicit same-level `./childagent` form; absolute paths are not allowed"
        ));
    }

    if path_uses_parent_traversal(candidate) {
        return Err(format!(
            "{path}: must use explicit same-level `./childagent` form; parent traversal (`..`) is not allowed"
        ));
    }

    if !agent.starts_with("./") {
        let message = if contains_path_separator(agent) {
            "must use explicit same-level `./childagent` form; nested child-agent paths are not allowed"
        } else {
            "must use explicit same-level `./childagent` form; bare child-agent names are not allowed"
        };
        return Err(format!("{path}: {message}"));
    }

    let sibling = &agent[2..];
    if sibling.is_empty() || !is_single_normal_path_component(sibling) {
        return Err(format!(
            "{path}: must stay at the same level; nested child-agent paths such as `./agents/childagent` are not allowed"
        ));
    }

    Ok(())
}

fn validate_definition_owned_local_path(
    raw_path: &str,
    path: &str,
    label: &str,
) -> Result<(), String> {
    if raw_path.trim().is_empty() {
        return Err(format!(
            "{path}: {label} path must be a non-empty relative path"
        ));
    }
    let candidate = Path::new(raw_path);
    if candidate.is_absolute() {
        return Err(format!(
            "{path}: {label} path must be relative and stay at the current level or below"
        ));
    }
    if path_uses_parent_traversal(candidate) {
        return Err(format!(
            "{path}: {label} path must stay at the current level or below; parent traversal (`..`) is not allowed"
        ));
    }
    Ok(())
}

fn validate_supported_file_extension(raw_path: &str, path: &str) -> Result<(), String> {
    let extension = Path::new(raw_path)
        .extension()
        .and_then(|value| value.to_str())
        .map(|value| value.to_ascii_lowercase());

    match extension.as_deref() {
        Some(extension) if SUPPORTED_FILE_EXTENSIONS.contains(&extension) => Ok(()),
        _ => Err(format!(
            "{path}: file input path must use a supported extension"
        )),
    }
}

fn validate_generated_image_output_extension(raw_path: &str, path: &str) -> Result<(), String> {
    let extension = Path::new(raw_path)
        .extension()
        .and_then(|value| value.to_str())
        .map(|value| value.to_ascii_lowercase());

    match extension.as_deref() {
        Some(extension) if SUPPORTED_GENERATED_IMAGE_EXTENSIONS.contains(&extension) => Ok(()),
        _ => Err(format!(
            "{path}: generated image output path must use a supported extension"
        )),
    }
}

fn path_uses_parent_traversal(path: &Path) -> bool {
    path.components()
        .any(|component| matches!(component, std::path::Component::ParentDir))
}

fn contains_path_separator(value: &str) -> bool {
    value.contains('/') || value.contains('\\')
}

fn is_single_normal_path_component(component: &str) -> bool {
    !component.is_empty()
        && !contains_path_separator(component)
        && component != "."
        && component != ".."
}

fn validate_schema_version(value: &str, path: &str) -> Result<(), String> {
    if is_valid_schema_version(value) {
        Ok(())
    } else {
        Err(format!(
            "{path}: expected schema version format `YYYY-MM-DD.rN` (example: `2026-03-03.r1`)"
        ))
    }
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
    revision
        .parse::<u32>()
        .ok()
        .filter(|number| *number > 0)
        .is_some()
}

fn is_valid_date_prefix(value: &str) -> bool {
    let bytes = value.as_bytes();
    if bytes.len() != 10 || bytes[4] != b'-' || bytes[7] != b'-' {
        return false;
    }

    let Ok(year) = value[0..4].parse::<u32>() else {
        return false;
    };
    let Ok(month) = value[5..7].parse::<u32>() else {
        return false;
    };
    let Ok(day) = value[8..10].parse::<u32>() else {
        return false;
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

fn validate_runtime_var_default(
    field_type: crate::RuntimeVarType,
    value: &Value,
    path: &str,
) -> Result<(), String> {
    match field_type {
        crate::RuntimeVarType::String => {
            if value.is_string() {
                Ok(())
            } else {
                Err(format!("{path}: expected a string default"))
            }
        }
        crate::RuntimeVarType::Boolean => {
            if value.is_boolean() {
                Ok(())
            } else {
                Err(format!("{path}: expected a boolean default"))
            }
        }
        crate::RuntimeVarType::Integer => {
            if value.as_i64().is_some() {
                Ok(())
            } else {
                Err(format!("{path}: expected an integer default"))
            }
        }
        crate::RuntimeVarType::Number => {
            if value.as_f64().filter(|number| number.is_finite()).is_some() {
                Ok(())
            } else {
                Err(format!("{path}: expected a finite numeric default"))
            }
        }
    }
}

fn validate_property_value(property: &SchemaProperty, value: &Value) -> Result<(), String> {
    match property.field_type {
        SchemaFieldType::String => {
            let text = value
                .as_str()
                .ok_or_else(|| format!("field `{}` must be a string", property.name))?;
            if let Some(allowed_values) = property.enum_values.as_ref() {
                if !allowed_values.iter().any(|candidate| candidate == text) {
                    return Err(format!(
                        "field `{}` must be one of: {}",
                        property.name,
                        allowed_values
                            .iter()
                            .map(|value| format!("`{value}`"))
                            .collect::<Vec<_>>()
                            .join(", ")
                    ));
                }
            }
        }
        SchemaFieldType::Boolean => {
            if !value.is_boolean() {
                return Err(format!("field `{}` must be a boolean", property.name));
            }
        }
        SchemaFieldType::Integer => {
            let integer_value = value
                .as_i64()
                .ok_or_else(|| format!("field `{}` must be an integer", property.name))?;
            validate_integer_constraints(property, integer_value)?;
        }
        SchemaFieldType::Number => {
            let number_value = value
                .as_f64()
                .filter(|number| number.is_finite())
                .ok_or_else(|| format!("field `{}` must be a number", property.name))?;
            validate_number_constraints(property, number_value)?;
        }
    }

    Ok(())
}

fn validate_integer_constraints(property: &SchemaProperty, value: i64) -> Result<(), String> {
    if let Some(NumericConstraintValue::Integer(minimum)) = property.numeric_constraints.minimum {
        if value < minimum {
            return Err(format!(
                "field `{}` must be greater than or equal to {}",
                property.name, minimum
            ));
        }
    }
    if let Some(NumericConstraintValue::Integer(minimum)) =
        property.numeric_constraints.exclusive_minimum
    {
        if value <= minimum {
            return Err(format!(
                "field `{}` must be greater than {}",
                property.name, minimum
            ));
        }
    }
    if let Some(NumericConstraintValue::Integer(maximum)) = property.numeric_constraints.maximum {
        if value > maximum {
            return Err(format!(
                "field `{}` must be less than or equal to {}",
                property.name, maximum
            ));
        }
    }
    if let Some(NumericConstraintValue::Integer(maximum)) =
        property.numeric_constraints.exclusive_maximum
    {
        if value >= maximum {
            return Err(format!(
                "field `{}` must be less than {}",
                property.name, maximum
            ));
        }
    }
    Ok(())
}

fn validate_number_constraints(property: &SchemaProperty, value: f64) -> Result<(), String> {
    if let Some(NumericConstraintValue::Number(minimum)) = property.numeric_constraints.minimum {
        if value < minimum {
            return Err(format!(
                "field `{}` must be greater than or equal to {}",
                property.name, minimum
            ));
        }
    }
    if let Some(NumericConstraintValue::Number(minimum)) =
        property.numeric_constraints.exclusive_minimum
    {
        if value <= minimum {
            return Err(format!(
                "field `{}` must be greater than {}",
                property.name, minimum
            ));
        }
    }
    if let Some(NumericConstraintValue::Number(maximum)) = property.numeric_constraints.maximum {
        if value > maximum {
            return Err(format!(
                "field `{}` must be less than or equal to {}",
                property.name, maximum
            ));
        }
    }
    if let Some(NumericConstraintValue::Number(maximum)) =
        property.numeric_constraints.exclusive_maximum
    {
        if value >= maximum {
            return Err(format!(
                "field `{}` must be less than {}",
                property.name, maximum
            ));
        }
    }
    Ok(())
}

fn required_field<'a>(
    object: &'a Map<String, Value>,
    key: &str,
    path: &str,
) -> Result<&'a Value, String> {
    object
        .get(key)
        .ok_or_else(|| format!("{path}.{key}: missing required field"))
}

fn required_string<'a>(
    object: &'a Map<String, Value>,
    key: &str,
    path: &str,
) -> Result<&'a str, String> {
    required_field(object, key, path)?
        .as_str()
        .ok_or_else(|| format!("{path}.{key}: expected a string"))
}

fn required_non_empty_string(
    object: &Map<String, Value>,
    key: &str,
    path: &str,
) -> Result<String, String> {
    let value = required_string(object, key, path)?.trim().to_string();
    if value.is_empty() {
        return Err(format!("{path}.{key}: must be a non-empty string"));
    }
    Ok(value)
}

fn optional_string(
    object: &Map<String, Value>,
    key: &str,
    path: &str,
) -> Result<Option<String>, String> {
    let Some(value) = object.get(key) else {
        return Ok(None);
    };
    Ok(Some(
        value
            .as_str()
            .ok_or_else(|| format!("{path}.{key}: expected a string"))?
            .to_string(),
    ))
}

fn required_array<'a>(
    object: &'a Map<String, Value>,
    key: &str,
    path: &str,
) -> Result<&'a Vec<Value>, String> {
    required_field(object, key, path)?
        .as_array()
        .ok_or_else(|| format!("{path}.{key}: expected an array"))
}

fn expect_object<'a>(value: &'a Value, path: &str) -> Result<&'a Map<String, Value>, String> {
    value
        .as_object()
        .ok_or_else(|| format!("{path}: expected an object"))
}

#[cfg(test)]
#[allow(dead_code)]
#[path = "../templates/build_support.rs"]
mod build_support_test_support;

#[cfg(test)]
mod tests {
    use super::{build_support_test_support as build_support, RuntimeAgentDefinition};

    fn config_with(properties: &str, actions: &str) -> String {
        config_with_runtime_vars(properties, "", actions)
    }

    fn config_with_runtime_vars(properties: &str, runtime_vars: &str, actions: &str) -> String {
        config_with_optional_inputs(
            properties,
            runtime_vars,
            actions,
            Some(
                r#"[
        { "type": "text", "text": "Test prompt" }
    ]"#,
            ),
            None,
        )
    }

    fn config_with_optional_inputs(
        properties: &str,
        runtime_vars: &str,
        actions: &str,
        inputs: Option<&str>,
        action_execution: Option<&str>,
    ) -> String {
        let inputs_block = if let Some(inputs) = inputs {
            format!(
                r#",
    "inputs": {inputs}"#
            )
        } else {
            String::new()
        };
        let runtime_vars_block = if runtime_vars.trim().is_empty() {
            String::new()
        } else {
            format!(
                r#",
    "runtime_vars": {{
        {runtime_vars}
    }}"#
            )
        };
        let action_execution_block = if let Some(action_execution) = action_execution {
            format!(
                r#",
    "action_execution": "{action_execution}""#
            )
        } else {
            String::new()
        };

        format!(
            r#"{{
    "version": "2026-03-03.r1"{inputs_block}{action_execution_block},
    "agent_schema": {{
        "type": "object",
        "properties": {{
            {properties}
        }}
    }}{runtime_vars_block},
    "actions": {actions}
}}"#
        )
    }

    fn assert_runtime_and_codegen_accept(cfg: &str) -> RuntimeAgentDefinition {
        build_support::generate_agent_model_from_str(cfg)
            .expect("build-time codegen should accept the config");
        RuntimeAgentDefinition::from_str(cfg)
            .expect("runtime definition parser should accept the config")
    }

    fn assert_runtime_and_codegen_reject(cfg: &str) -> (String, String) {
        let build_error = build_support::generate_agent_model_from_str(cfg)
            .expect_err("build-time codegen should reject the config")
            .to_string();
        let runtime_error = RuntimeAgentDefinition::from_str(cfg)
            .expect_err("runtime definition parser should reject the config");
        (build_error, runtime_error)
    }

    #[test]
    fn parses_adder_like_agent_definition() {
        let definition = RuntimeAgentDefinition::from_str(
            r#"{
                "version": "2026-03-03.r1",
                "inputs": [{ "type": "text", "text": "What is 2 + 2?" }],
                "agent_schema": {
                    "type": "object",
                    "properties": {
                        "answer": { "type": "integer" }
                    }
                },
                "actions": [
                    {
                        "name": "print",
                        "logic": { "==": [{ "var": "answer" }, 4] },
                        "run": [{ "kind": "exec", "program": "echo", "args": ["ok"] }]
                    }
                ]
            }"#,
        )
        .expect("definition should parse");

        assert!(definition.has_output_schema_properties());
        assert_eq!(definition.named_inputs().len(), 1);
        assert_eq!(definition.actions().len(), 1);
    }

    #[test]
    fn validates_provider_output_against_schema() {
        let definition = RuntimeAgentDefinition::from_str(
            r#"{
                "version": "2026-03-03.r1",
                "inputs": [{ "type": "text", "text": "Return a unit." }],
                "agent_schema": {
                    "type": "object",
                    "properties": {
                        "unit": { "type": "string", "enum": ["F", "C"] },
                        "confidence": { "type": "number", "exclusiveMinimum": 0, "maximum": 1 }
                    }
                },
                "actions": [
                    {
                        "name": "print",
                        "logic": { "==": [{ "var": "unit" }, "F"] },
                        "run": [{ "kind": "exec", "program": "echo", "args": ["ok"] }]
                    }
                ]
            }"#,
        )
        .expect("definition should parse");

        definition
            .validate_provider_output(r#"{"unit":"F","confidence":0.9}"#)
            .expect("valid output should pass");

        let error = definition
            .validate_provider_output(r#"{"unit":"K","confidence":0.9}"#)
            .expect_err("invalid enum should fail");
        assert!(error.contains("must be one of"));
    }

    #[test]
    fn structural_action_only_requires_named_inputs() {
        let error = RuntimeAgentDefinition::from_str(
            r#"{
                "version": "2026-03-03.r1",
                "inputs": [{ "type": "text", "text": "x" }],
                "agent_schema": { "type": "object", "properties": {} },
                "actions": [
                    {
                        "name": "print",
                        "logic": { "==": [1, 1] },
                        "run": [{ "kind": "exec", "program": "echo", "args": ["ok"] }]
                    }
                ]
            }"#,
        )
        .expect_err("unnamed structural input should fail");

        assert!(error.contains("must declare `name`"));
    }

    #[test]
    fn matches_codegen_for_schema_backed_agents_without_baked_inputs() {
        let definition = assert_runtime_and_codegen_accept(&config_with_optional_inputs(
            r#""answer": { "type": "integer" }"#,
            "",
            "[]",
            None,
            None,
        ));

        assert!(definition.has_output_schema_properties());
        assert!(definition.named_inputs().is_empty());
    }

    #[test]
    fn matches_codegen_for_structural_action_only_agents_without_inputs() {
        let definition = assert_runtime_and_codegen_accept(&config_with_optional_inputs(
            "",
            r#""generate_images": { "type": "boolean", "default": true }"#,
            "[]",
            None,
            None,
        ));

        assert!(!definition.has_output_schema_properties());
        assert!(definition.named_inputs().is_empty());
    }

    #[test]
    fn matches_codegen_for_invalid_action_execution_values() {
        let cfg = config_with_optional_inputs(
            r#""answer": { "type": "integer" }"#,
            "",
            "[]",
            None,
            Some("fanout"),
        );

        let (build_error, runtime_error) = assert_runtime_and_codegen_reject(&cfg);

        assert!(build_error.contains("$.action_execution"));
        assert!(build_error.contains("expected `sequential` or `parallel`"));
        assert!(runtime_error.contains("$.action_execution"));
        assert!(runtime_error.contains("expected `sequential` or `parallel`"));
    }

    #[test]
    fn matches_codegen_for_agent_steps_with_inputs_run_vars_and_overrides() {
        let cfg = config_with_optional_inputs(
            r#""value": { "type": "integer" }"#,
            "",
            r#"[
          {
            "name": "child_agent",
            "logic": { "==": [ { "var": "value" }, 1 ] },
            "run": [
              {
                "kind": "agent",
                "artifact": "./summary_agent",
                "input_mode": "append",
                "inputs": [
                  { "type": "text", "text": "Summarize this." },
                  { "input": "menu_image" }
                ],
                "run_vars": {
                  "year": 2026,
                  "month": { "var": "value" }
                },
                "input_overrides": {
                  "menu_image": { "input": "menu_image" },
                  "menu_note": "Spring menu"
                }
              }
            ]
          }
        ]"#,
            Some(
                r#"[
        { "name": "menu_image", "type": "image", "path": "./artifacts/menu.png" }
    ]"#,
            ),
            None,
        );

        let definition = assert_runtime_and_codegen_accept(&cfg);
        let child_step = &definition.actions()[0].run[0];

        assert_eq!(child_step.agent.as_deref(), Some("./summary_agent"));
        assert!(child_step
            .inputs
            .as_ref()
            .is_some_and(|inputs| inputs.len() == 2));
        assert!(child_step
            .run_vars
            .as_ref()
            .is_some_and(|run_vars| run_vars.len() == 2));
        assert!(child_step
            .input_overrides
            .as_ref()
            .is_some_and(|overrides| overrides.len() == 2));
    }

    #[test]
    fn matches_codegen_for_explicit_same_level_child_agent_targets() {
        let cfg = config_with(
            r#""ok": { "type": "boolean" }"#,
            r#"[
          {
            "name": "invoke_child",
            "logic": { "==": [ { "var": "ok" }, true ] },
            "run": [
              { "kind": "agent", "artifact": "./childagent" }
            ]
          }
        ]"#,
        );

        let definition = assert_runtime_and_codegen_accept(&cfg);
        assert_eq!(
            definition.actions()[0].run[0].agent.as_deref(),
            Some("./childagent")
        );
    }

    #[test]
    fn matches_codegen_for_rejecting_bare_child_agent_targets() {
        let cfg = config_with(
            r#""ok": { "type": "boolean" }"#,
            r#"[
          {
            "name": "invoke_child",
            "logic": { "==": [ { "var": "ok" }, true ] },
            "run": [
              { "kind": "agent", "artifact": "childagent" }
            ]
          }
        ]"#,
        );

        let (build_error, runtime_error) = assert_runtime_and_codegen_reject(&cfg);

        assert!(build_error.contains("bare child-agent names are not allowed"));
        assert!(runtime_error.contains("bare child-agent names are not allowed"));
    }

    #[test]
    fn matches_codegen_for_rejecting_nested_child_agent_targets() {
        let cfg = config_with(
            r#""ok": { "type": "boolean" }"#,
            r#"[
          {
            "name": "invoke_child",
            "logic": { "==": [ { "var": "ok" }, true ] },
            "run": [
              { "kind": "agent", "artifact": "./agents/childagent" }
            ]
          }
        ]"#,
        );

        let (build_error, runtime_error) = assert_runtime_and_codegen_reject(&cfg);

        assert!(build_error.contains("nested child-agent paths"));
        assert!(runtime_error.contains("nested child-agent paths"));
    }
}
