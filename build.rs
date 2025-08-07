use std::{
    env,
    fs::{self, File},
    io::Write,
    path::Path,
};
use serde_json::Value;

fn main() -> std::io::Result<()> {
    // Re-run this build script if .agentcfg ever changes:
    println!("cargo:rerun-if-changed=.agentcfg");

    // 1. Read & parse .agentcfg
    let json_str = fs::read_to_string(".agentcfg")
        .expect("Failed to read .agentcfg");
    let v: Value = serde_json::from_str(&json_str)
        .expect("Invalid JSON in .agentcfg");

    // Pull the array from the `sample_outputs` array
    let arr = v["sample_outputs"]
        .as_array()
        .expect("Expected `sample_outputs` to be an array");

    // Build struct fields from the first sample to define the struct
    let sample = &arr[0];

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
    for sample in arr {
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
            "        SampleOutput {{\n{fields}        }},\n",
            fields = fields
        ));
    }

    // Extract resource URLs
    let urls = v["resource_urls"]
        .as_array()
        .expect("Expected `resource_urls` to be an array");

    let mut url_list = String::new();
    for url in urls {
        let url_str = url.as_str().expect("Each resource URL must be a string");
        url_list.push_str(&format!("        \"{}\",\n", url_str));
    }

    // Generate code
    let generated_code = format!(
        "
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct SampleOutput {{
{struct_fields}}}


pub fn sample_outputs() -> Vec<SampleOutput> {{
    vec![
{instances}    ]
}}

pub fn resource_urls() -> Vec<&'static str> {{
    vec![
{url_list}    ]
}}
",
        struct_fields = struct_fields,
        instances = instances,
        url_list = url_list
    );

    // Print to stdout
    println!("Generated code:\n{}", generated_code);

    // Determine output path and create file
    let out_dir = env::var("OUT_DIR").unwrap();
    let dest_path = Path::new(&out_dir).join(".agentcfg");
    let mut file = File::create(&dest_path)?;

    // Write to file
    write!(file, "{}", generated_code)?;

    Ok(())
}