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

    // Pull the first element from the `sample_outputs` array
    let arr = v["sample_outputs"]
        .as_array()
        .expect("Expected `sample_outputs` to be an array");
    let sample = &arr[0];

    let sample_map = sample
        .as_object()
        .expect("Expected element 0 of `sample_outputs` to be an object");

    // Build struct fields from `sample_output`
    let mut struct_fields = String::new();
    let mut instance_fields = String::new();
    for (key, value) in sample_map.iter() {
        let rust_type = match value {
            Value::String(_) => "String",
            Value::Bool(_) => "bool",
            Value::Number(_) => "f64",
            _ => panic!("Unsupported type for field `{}`", key),
        };
        struct_fields.push_str(&format!("    pub {}: {},\n", key, rust_type));

        let rust_value = match value {
            Value::String(s) => format!("\"{}\".to_string()", s),
            Value::Bool(b) => b.to_string(),
            Value::Number(n) => n.to_string(),
            _ => unreachable!(),
        };
        instance_fields.push_str(&format!("        {}: {},\n", key, rust_value));
    }

    // Generate the Rust code for SampleOutput and sample_output()
    let generated_code = format!(
        "
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct SampleOutput {{
{struct_fields}}}

pub fn sample_output() -> SampleOutput {{
    SampleOutput {{
{instance_fields}    }}
}}
",
        struct_fields = struct_fields,
        instance_fields = instance_fields,
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