#[path = "../templates/build_support.rs"]
#[allow(dead_code)]
mod build_support;

fn minimal_agentcfg_with_header(header: &str) -> String {
    format!(
        r#"{{
  {header},
  "inputs": [
    {{ "type": "text", "text": "Return a numeric answer." }}
  ],
  "agent_schema": {{
    "type": "object",
    "properties": {{
      "answer": {{
        "type": "integer"
      }}
    }}
  }},
  "actions": []
}}"#
    )
}

fn minimal_agentcfg(version: &str) -> String {
    minimal_agentcfg_with_header(&format!(
        r#""agent_definition_schema_version": "{version}""#
    ))
}

#[test]
fn accepts_date_revision_schema_version() {
    let parsed = build_support::generate_agent_model_from_str(&minimal_agentcfg("2026-03-03.r1"));
    assert!(parsed.is_ok());
}

#[test]
fn rejects_unsupported_future_version() {
    let config = minimal_agentcfg_with_header(
        r#""agent_definition_schema_version": "2099-12-31.r42",
  "unrelated_root_field": { "preserved": true }"#,
    );

    let error = build_support::generate_agent_model_from_str(&config)
        .expect_err("future revisions need explicit support");
    assert!(error.to_string().contains("unsupported_schema_version"));
}

#[test]
fn rejects_invalid_schema_version_value_at_canonical_path() {
    let err = build_support::generate_agent_model_from_str(&minimal_agentcfg("0.0.10"))
        .expect_err("semver-style schema versions should be rejected")
        .to_string();

    assert!(err.contains("$.agent_definition_schema_version"));
    assert!(err.contains("YYYY-MM-DD.rN"));
}

#[test]
fn rejects_invalid_calendar_dates() {
    let err = build_support::generate_agent_model_from_str(&minimal_agentcfg("2025-02-29.r1"))
        .expect_err("invalid dates should be rejected")
        .to_string();

    assert!(err.contains("$.agent_definition_schema_version"));
    assert!(err.contains("YYYY-MM-DD.rN"));
}

#[test]
fn rejects_legacy_version_key_with_actionable_rename_guidance() {
    let config = minimal_agentcfg_with_header(r#""version": "2026-03-03.r1""#);
    let err = build_support::generate_agent_model_from_str(&config)
        .expect_err("legacy schema key should be rejected")
        .to_string();

    assert!(err.contains("$.version"));
    assert!(err.contains("rename it to `agent_definition_schema_version`"));
}

#[test]
fn rejects_both_schema_keys_via_legacy_key_error() {
    for legacy_version in ["2026-03-03.r1", "2026-03-04.r1"] {
        let config = minimal_agentcfg_with_header(&format!(
            r#""agent_definition_schema_version": "2026-03-03.r1",
  "version": "{legacy_version}""#
        ));
        let err = build_support::generate_agent_model_from_str(&config)
            .expect_err("definitions containing the legacy key should be rejected")
            .to_string();

        assert!(err.contains("$.version"));
        assert!(err.contains("rename it to `agent_definition_schema_version`"));
    }
}

#[test]
fn requires_canonical_schema_version_key() {
    let config = minimal_agentcfg_with_header(r#""unrelated_root_field": true"#);
    let err = build_support::generate_agent_model_from_str(&config)
        .expect_err("canonical schema key should be required")
        .to_string();

    assert!(err.contains("$.agent_definition_schema_version"));
    assert!(err.contains("missing required field"));
}

#[test]
fn preserves_older_date_revisions_and_unknown_root_fields() {
    for version in [
        "2026-03-03.r1",
        "2026-03-11.r1",
        "2026-03-28.r1",
        "2026-09-08.r42",
    ] {
        let config = minimal_agentcfg_with_header(&format!(
            r#""agent_definition_schema_version": "{version}", "unrelated_root_field": true"#
        ));
        assert!(
            build_support::generate_agent_model_from_str(&config).is_ok(),
            "{version}"
        );
    }
}

#[test]
fn both_supported_strict_revisions_keep_unknown_keyword_rejection() {
    for version in ["2026-09-09.r1", "2026-09-19.r1"] {
        assert!(build_support::generate_agent_model_from_str(&minimal_agentcfg(version)).is_ok());
        let config = minimal_agentcfg_with_header(&format!(
            r#""agent_definition_schema_version": "{version}", "unknown_root_field": true"#
        ));
        let error = build_support::generate_agent_model_from_str(&config)
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("unknown_field at $.unknown_root_field"),
            "{error}"
        );
        let root: serde_json::Value = serde_json::from_str(&minimal_agentcfg(version)).unwrap();
        assert_eq!(
            build_support::definition_validation::definition_revision(&root).unwrap(),
            build_support::definition_validation::DefinitionRevision::Strict
        );
    }
}

#[test]
fn unrecognized_revisions_after_strict_cutoff_never_become_legacy() {
    for version in [
        "2026-09-09.r2",
        "2026-09-10.r1",
        "2026-09-19.r2",
        "2026-09-20.r1",
    ] {
        let error = build_support::generate_agent_model_from_str(&minimal_agentcfg(version))
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("unsupported_schema_version"),
            "{version}: {error}"
        );
    }
}
