use std::{
    env,                    // read OUT_DIR and other env vars
    fs::{self, File},       // fs ops + File handle creation
    io::Write,              // trait for write! / .write() on File
    path::Path,             // path construction / manipulation
};
use serde_json::Value;      // dynamic JSON parsing

fn main() -> std::io::Result<()> {

    // Hint to Cargo: only rerun build.rs when the config changes (and when build.rs itself changes).
    println!("cargo:rerun-if-changed=.agentcfg");

    // Step 1: Read .agentcfg into memory and parse as JSON.
    // For now, this build script only supports JSON; future versions may support TOML/YAML.
    let json_str = fs::read_to_string(".agentcfg")
        .expect("Failed to read .agentcfg");
    let json: Value = serde_json::from_str(&json_str)
        .expect("Invalid JSON in .agentcfg");

    // Extract schema: properties + required (optional)
    let schema = json["json_schema"].as_object().expect("Expected `json_schema` to be an object");
    let props = schema
        .get("properties")
        .and_then(|p| p.as_object())
        .expect("Expected `json_schema.properties` to be an object");
    let required: Vec<String> = schema
        .get("required")
        .and_then(|r| r.as_array())
        .map(|arr| arr.iter().filter_map(|v| v.as_str().map(|s| s.to_string())).collect())
        .unwrap_or_else(|| Vec::new());

    let schema_json = serde_json::to_string_pretty(&json["json_schema"]).expect("Failed to serialize json_schema");

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

    // Generate code
    let generated_code = format!(
        r##"
#[derive(Clone, Debug, Deserialize, Serialize)]
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
/// Returned as a raw string for easy inclusion in prompts or API calls.
pub fn json_schema() -> &'static str {{
  r#"{schema_json}"#
    }}
"##,
        struct_fields = struct_fields,
        url_list = url_list,
        schema_json = schema_json
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