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

    // Extract `sample_outputs` from the config; this defines the LLM's expected JSON schema.
    let sample_outputs = json["sample_outputs"]
        .as_array()
        .expect("Expected `sample_outputs` to be an array");

    // Build struct fields from the first sample to define the struct
    let sample = &sample_outputs[0];

    let sample_map = sample
        .as_object()
        .expect("Expected element 0 of `sample_outputs` to be an object");

    let mut struct_fields = String::new();
    
    for (key, value) in sample_map.iter() {
        let rust_type = match value {
            Value::String(_) => "String",
            Value::Bool(_) => "bool",
            Value::Number(_) => "f64",
            _ => panic!("Unsupported type for field `{}`", key),
        };
        struct_fields.push_str(&format!("    pub {}: {},\n", key, rust_type));}

    // Build instance list from all samples
    let mut instances = String::new();
    for sample in sample_outputs {
        let sample_map = sample
            .as_object()
            .expect("Each `sample_output` must be an object");

        let mut fields = String::new();
        for (key, value) in sample_map.iter() {
            let rust_value = match value {
                Value::String(s) => format!("\"{}\".to_string()", s),
                Value::Bool(b) => b.to_string(),
                Value::Number(n) => n.to_string(),
                _ => unreachable!(),
            };
            fields.push_str(&format!("            {}: {},\n", key, rust_value));
        }
        instances.push_str(&format!(
            "        Output {{\n{fields}        }},\n",
            fields = fields
        ));
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
        "
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Output {{
{struct_fields}}}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ResourceUrl {{
    pub url: &'static str,
    pub description: &'static str,
}}

pub fn sample_outputs() -> Vec<Output> {{
    vec![
{instances}    ]
}}

pub fn resource_urls() -> Vec<ResourceUrl> {{
    vec![
{url_list}    ]
}}
",
        struct_fields = struct_fields,
        instances = instances,
        url_list = url_list
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