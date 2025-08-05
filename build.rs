use std::{
    env,
    fs::{self, File},
    io::Write,
    path::Path,
};
use serde_json::Value;

fn main() -> std::io::Result<()> {
    // Re-run this build script if answer.json ever changes:
    println!("cargo:rerun-if-changed=answer.json");

    // 1. Read & parse answer.json
    let json_str = fs::read_to_string("answer.json")
        .expect("Failed to read answer.json");
    let v: Value = serde_json::from_str(&json_str)
        .expect("Invalid JSON in answer.json");

    let out_dir = env::var("OUT_DIR").unwrap();
    let dest_path = Path::new(&out_dir).join("answer.rs");
    let mut file = File::create(&dest_path)?;

    // Generate struct fields
    let mut struct_fields = String::new();
    for (key, value) in v.as_object().expect("Expected JSON object").iter() {
        let rust_type = match value {
            Value::String(_) => "String",
            Value::Bool(_) => "bool",
            Value::Number(_) => "f64",
            _ => panic!("Unsupported value type for key {}", key),
        };
        struct_fields.push_str(&format!("    pub {}: {},\n", key, rust_type));
    }

    // Generate a single Answer instance
    let mut instances = String::new();
    {
        let mut fields = String::new();
        for (key, value) in v.as_object().expect("Expected JSON object").iter() {
            let rust_value = match value {
                Value::String(s) => format!("\"{}\".to_string()", s),
                Value::Bool(b) => b.to_string(),
                Value::Number(n) => n.to_string(),
                _ => panic!("Unsupported value type for key {}", key),
            };
            fields.push_str(&format!("{}: {}, ", key, rust_value));
        }
        instances.push_str(&format!("    Answer {{ {} }},\n", fields));
    }

    let generated_code = format!(
        "
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Answer {{
{struct_fields}
}}

pub fn answers() -> Vec<Answer> {{
    vec![
{instances}
    ]
}}
",
        struct_fields = struct_fields,
        instances = instances
    );

    // Print to stdout
    println!("Generated code:\n{}", generated_code);

    // Write to file
    write!(file, "{}", generated_code)?;

    Ok(())
}