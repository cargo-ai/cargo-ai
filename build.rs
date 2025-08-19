use std::{
    env,                    // read OUT_DIR and other env vars
    fs::{self, File},       // fs ops + File handle creation
    io::Write,              // trait for write! / .write() on File
    path::Path,             // path construction / manipulation
};
use serde_json;      // dynamic JSON parsing

#[derive(Debug)]
struct RunStep {
    kind: String,
    program: String,
    args: Vec<String>,
}

#[derive(Debug)]
struct Action {
    name: String,
    logic: serde_json::Value, // Follows the JSON Logic Standard
    run: Vec<RunStep>,
}

fn main() -> std::io::Result<()> {

    // Hint to Cargo: only rerun build.rs when the config changes (and when build.rs itself changes).
    println!("cargo:rerun-if-changed=.agentcfg");

    // Step 1: Read .agentcfg into memory and parse as JSON.
    // For now, this build script only supports JSON; future versions may support TOML/YAML.
    let json_str = fs::read_to_string(".agentcfg")
        .expect("Failed to read .agentcfg");
    let json: serde_json::Value = serde_json::from_str(&json_str)
        .expect("Invalid JSON in .agentcfg");

    // Extract schema: properties + required (optional)
    let schema = json["agent_schema"].as_object().expect("Expected `agent_schema` to be an object");
    let props = schema
        .get("properties")
        .and_then(|p| p.as_object())
        .expect("Expected `agent_schema.properties` to be an object");
    let required: Vec<String> = schema
        .get("required")
        .and_then(|r| r.as_array())
        .map(|arr| arr.iter().filter_map(|v| v.as_str().map(|s| s.to_string())).collect())
        .unwrap_or_else(|| Vec::new());

    // Build struct fields from schema
    let mut struct_fields = String::new();
    for (key, prop) in props.iter() {
        let t = prop.get("type").and_then(|v| v.as_str()).unwrap_or("object");
        let base_ty = match t {
            "string" => "String".to_string(),
            "boolean" => "bool".to_string(),
            "number" => "f64".to_string(),
            "integer" => "i64".to_string(),
            "array" => {
                // Infer array item type if provided; default to serde_json::Value
                let item_ty = prop
                    .get("items")
                    .and_then(|it| it.get("type"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("any");
                let inner = match item_ty {
                    "string" => "String",
                    "boolean" => "bool",
                    "number" => "f64",
                    "integer" => "i64",
                    _ => "serde_json::Value",
                };
                format!("Vec<{}>", inner)
            }
            _ => "serde_json::Value".to_string(),
        };
        let is_required = required.iter().any(|r| r == key);
        let rust_ty = if is_required { base_ty } else { format!("Option<{}>", base_ty) };
        struct_fields.push_str(&format!("    pub {}: {},\n", key, rust_ty));
    }

    // Extract resource URLs as objects
    let urls = json["resource_urls"]
        .as_array()
        .expect("Expected `resource_urls` to be an array");
    let mut url_list = String::new();
    for entry in urls {
        let obj = entry.as_object().expect("Each resource must be an object");
        let url_str = obj["url"].as_str().expect("Expected url to be a string");
        let desc_str = obj["description"].as_str().expect("Expected description to be a string");
        url_list.push_str(&format!(
            "        ResourceUrl {{ url: \"{}\", description: \"{}\" }},\n",
            url_str, desc_str
        ));
    }

    let mut actions: Vec<Action> = Vec::new(); // For configured 'action' objects.

    // Extract actions as objects
    let actions_cfg = json["actions"]
        .as_array()
        .expect("Expected `actions` to be an array");

    for action_cfg in actions_cfg {
        let name = action_cfg["name"].as_str().expect("Expected action name").to_string();
        println!("Action Name: {name}");
        
        let logic = action_cfg["logic"].clone();

        let mut run_steps: Vec<RunStep> = Vec::new();

        let runs_cfg = action_cfg["run"]
            .as_array()
            .expect("Expected 'run' to be an array.");

        for run_cfg in runs_cfg {
            let kind = run_cfg["kind"].as_str().expect("Expected 'kind' to be a string").to_string();
            let program = run_cfg["program"].as_str().expect("Expected 'program' to be a string").to_string();
            let args: Vec<String> = run_cfg["args"].as_array().expect("Expect 'args' to be an array.").iter().map(|x| x.as_str().unwrap().to_string()).collect();

            let run_step = RunStep {
                kind,
                program,
                args
            };

            run_steps.push(run_step);
        }

        let action = Action {
            name,
            logic,
            run: run_steps,
        };
        
        actions.push(action);
    }

    // Generation Action Code
    let mut action_code = String::new();

    for action in actions {

        let name = action.name;

        let logic = action.logic.to_string();

        let run_steps = action.run.iter()
            .map(|run_step| {
                let args = run_step.args
                    .iter()
                    .map(|arg| format!("\"{}\".to_string()", arg))  // wrap each arg as String
                    .collect::<Vec<String>>()
                    .join(", "); // join with commas
                format!(
                    "RunStep {{
                        kind: \"{}\".to_string(),
                        program: \"{}\".to_string(),
                        args: vec![{}],
                    }}", run_step.kind, run_step.program, args)
                })
            .collect::<Vec<_>>()
            .join(",");

        action_code.push_str(&format!(
            "Action {{
                name: \"{}\".to_string(),
                logic: serde_json::from_str(r#\"{}\"#).unwrap(),
                run: vec![{}],
            }},", name, logic, run_steps
        ));
    }

    // Generate code
    let generated_code = format!(
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

pub fn resource_urls() -> Vec<ResourceUrl> {{
    vec![
{url_list}    ]
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
    args: Vec<String>,
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
        url_list = url_list,
        action_code = action_code,
    );

    // Print each generated line as a separate Cargo warning (multi-line warning output)
    for line in generated_code.lines() {
        println!("cargo:warning={}", line);
    }

    // OUT_DIR is a Cargo-provided scratch dir for generated artifacts consumed by this crate.
    let out_dir = env::var("OUT_DIR").unwrap();
    let dest_path = Path::new(&out_dir).join("agent_model.rs");
    let mut file = File::create(&dest_path)?;

    // Write to file
    write!(file, "{}", generated_code)?;

    Ok(())
}