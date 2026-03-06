//! Focused schema/codegen hardening tests for `templates/build_support.rs`.

#[allow(dead_code)]
#[path = "../templates/build_support.rs"]
mod build_support;

use sha2::{Digest, Sha256};

/// Constructs a minimal `.agentcfg` JSON document with caller-provided schema
/// properties and actions sections.
fn config_with(properties: &str, actions: &str) -> String {
    format!(
        r#"{{
    "version": "2026-03-03.r1",
    "prompt": "Test prompt",
    "agent_schema": {{
        "type": "object",
        "properties": {{
            {properties}
        }}
    }},
    "resource_urls": [],
    "actions": {actions}
}}"#
    )
}

#[test]
fn maps_array_of_integers_to_vec_i64() {
    let cfg = config_with(
        r#""numbers": { "type": "array", "items": { "type": "integer" } }"#,
        "[]",
    );

    let generated = build_support::generate_agent_model_from_str(&cfg).unwrap();
    assert!(generated.contains("pub numbers: Vec<i64>,"));
}

#[test]
fn rejects_nested_arrays_with_actionable_path() {
    let cfg = config_with(
        r#""matrix": {
          "type": "array",
          "items": { "type": "array", "items": { "type": "integer" } }
        }"#,
        "[]",
    );

    let err = build_support::generate_agent_model_from_str(&cfg)
        .unwrap_err()
        .to_string();

    assert!(err.contains("$.agent_schema.properties.matrix.items.type"));
    assert!(err.contains("nested arrays are not supported yet"));
}

#[test]
fn rejects_union_types_with_actionable_path() {
    let cfg = config_with(r#""value": { "type": ["string", "integer"] }"#, "[]");

    let err = build_support::generate_agent_model_from_str(&cfg)
        .unwrap_err()
        .to_string();

    assert!(err.contains("$.agent_schema.properties.value.type"));
    assert!(err.contains("union schema types are not supported yet"));
}

#[test]
fn rejects_invalid_field_identifiers() {
    let cfg = config_with(r#""bad-name": { "type": "string" }"#, "[]");

    let err = build_support::generate_agent_model_from_str(&cfg)
        .unwrap_err()
        .to_string();

    assert!(err.contains("$.agent_schema.properties.bad-name"));
    assert!(err.contains("must contain only ASCII letters, digits, or underscores"));
}

#[test]
fn rejects_reserved_keyword_field_identifiers() {
    let cfg = config_with(r#""union": { "type": "string" }"#, "[]");

    let err = build_support::generate_agent_model_from_str(&cfg)
        .unwrap_err()
        .to_string();

    assert!(err.contains("$.agent_schema.properties.union"));
    assert!(err.contains("reserved Rust keyword"));
}

#[test]
fn rejects_unsupported_action_kind_with_actionable_path() {
    let cfg = config_with(
        r#""value": { "type": "integer" }"#,
        r#"[
          {
            "name": "unsupported",
            "logic": { "==": [ { "var": "value" }, 1 ] },
            "run": [
              { "kind": "http", "program": "echo", "args": [] }
            ]
          }
        ]"#,
    );

    let err = build_support::generate_agent_model_from_str(&cfg)
        .unwrap_err()
        .to_string();

    assert!(err.contains("$.actions[0].run[0].kind"));
    assert!(err.contains("supported: `exec`"));
}

#[test]
fn rejects_non_string_action_args_with_actionable_path() {
    let cfg = config_with(
        r#""value": { "type": "integer" }"#,
        r#"[
          {
            "name": "bad_args",
            "logic": { "==": [ { "var": "value" }, 1 ] },
            "run": [
              { "kind": "exec", "program": "echo", "args": [1, "ok"] }
            ]
          }
        ]"#,
    );

    let err = build_support::generate_agent_model_from_str(&cfg)
        .unwrap_err()
        .to_string();

    assert!(err.contains("$.actions[0].run[0].args[0]"));
    assert!(err.contains("expected a string argument"));
}

#[test]
fn rejects_non_object_action_logic_with_actionable_path() {
    let cfg = config_with(
        r#""value": { "type": "integer" }"#,
        r#"[
          {
            "name": "bad_logic_root",
            "logic": true,
            "run": [
              { "kind": "exec", "program": "echo", "args": [] }
            ]
          }
        ]"#,
    );

    let err = build_support::generate_agent_model_from_str(&cfg)
        .unwrap_err()
        .to_string();

    assert!(err.contains("$.actions[0].logic"));
    assert!(err.contains("expected a JSON Logic object expression"));
}

#[test]
fn rejects_multi_operator_action_logic_object_with_actionable_path() {
    let cfg = config_with(
        r#""value": { "type": "integer" }"#,
        r#"[
          {
            "name": "bad_logic_shape",
            "logic": { "and": [true], "or": [false] },
            "run": [
              { "kind": "exec", "program": "echo", "args": [] }
            ]
          }
        ]"#,
    );

    let err = build_support::generate_agent_model_from_str(&cfg)
        .unwrap_err()
        .to_string();

    assert!(err.contains("$.actions[0].logic"));
    assert!(err.contains("exactly one operator key"));
}

#[test]
fn rejects_unknown_logic_var_with_actionable_path() {
    let cfg = config_with(
        r#""answer": { "type": "integer" }"#,
        r#"[
          {
            "name": "unknown_var",
            "logic": { "==": [ { "var": "missing_field" }, 4 ] },
            "run": [
              { "kind": "exec", "program": "echo", "args": [] }
            ]
          }
        ]"#,
    );

    let err = build_support::generate_agent_model_from_str(&cfg)
        .unwrap_err()
        .to_string();

    assert!(err.contains("$.actions[0].logic.==[0].var"));
    assert!(err.contains("unknown variable `missing_field`"));
}

#[test]
fn rejects_logic_type_mismatch_with_actionable_path() {
    let cfg = config_with(
        r#""answer": { "type": "integer" }"#,
        r#"[
          {
            "name": "bad_type_match",
            "logic": { "==": [ { "var": "answer" }, "4" ] },
            "run": [
              { "kind": "exec", "program": "echo", "args": [] }
            ]
          }
        ]"#,
    );

    let err = build_support::generate_agent_model_from_str(&cfg)
        .unwrap_err()
        .to_string();

    assert!(err.contains("$.actions[0].logic.=="));
    assert!(err.contains("incompatible operand types"));
}

#[test]
fn escapes_action_literals_and_logic_payload_safely() {
    let cfg = config_with(
        r#""value": { "type": "string" }"#,
        r#"[
          {
            "name": "special \"name\"\nline",
            "logic": { "==": [ { "var": "value" }, "v#\"x\nz" ] },
            "run": [
              {
                "kind": "exec",
                "program": "echo",
                "args": ["quote \" arg", "line\nbreak", "hash#\"marker"]
              }
            ]
          }
        ]"#,
    );

    let generated = build_support::generate_agent_model_from_str(&cfg).unwrap();

    assert!(generated.contains("logic: serde_json::from_str(\""));
    assert!(!generated.contains("serde_json::from_str(r#\""));
    assert!(generated.contains("name: "));
    assert!(generated.contains("\\\"name\\\""));
    assert!(generated.contains("\\nline"));
    assert!(generated.contains("\"quote \\\" arg\".to_string()"));
    assert!(generated.contains("\"line\\nbreak\".to_string()"));
    assert!(generated.contains("\"hash#\\\"marker\".to_string()"));
}

#[test]
fn generates_canonical_build_provenance_constants() {
    let cfg = r#"{
      "resource_urls": [],
      "prompt": "Return a numeric answer.",
      "actions": [],
      "version": "2026-03-03.r1",
      "agent_schema": {
        "properties": {
          "answer": {
            "type": "integer"
          }
        },
        "type": "object"
      }
    }"#;

    let generated = build_support::generate_agent_build_provenance_source_with_values(
        cfg,
        "aarch64-apple-darwin",
        "11111111-2222-4333-8444-555555555555",
        "2026-03-05T23:14:29Z",
    )
    .expect("build provenance source should be generated");

    let expected_definition = r#"{"actions":[],"agent_schema":{"properties":{"answer":{"type":"integer"}},"type":"object"},"prompt":"Return a numeric answer.","resource_urls":[],"version":"2026-03-03.r1"}"#;
    let mut hasher = Sha256::new();
    hasher.update(expected_definition.as_bytes());
    let expected_hash = format!("{:x}", hasher.finalize());

    assert!(generated.contains(
        r#"const AGENT_BUILD_ID: &str = "11111111-2222-4333-8444-555555555555";"#
    ));
    assert!(generated.contains(
        r#"const AGENT_TARGET_TRIPLE: &str = "aarch64-apple-darwin";"#
    ));
    assert!(generated.contains(
        r#"const AGENT_BUILD_TIMESTAMP_UTC: &str = "2026-03-05T23:14:29Z";"#
    ));
    assert!(generated.contains(&format!(
        r#"const AGENT_DEFINITION_SHA256: &str = "{}";"#,
        expected_hash
    )));
    assert!(generated.contains(&format!(
        r#"const AGENT_EMBEDDED_DEFINITION_JSON: &str = "{}";"#,
        expected_definition.replace('"', "\\\"")
    )));
}
