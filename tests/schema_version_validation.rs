#[path = "../templates/build_support.rs"]
#[allow(dead_code)]
mod build_support;

fn minimal_agentcfg(version: &str) -> String {
    format!(
        r#"{{
  "version": "{version}",
  "prompt": "Return a numeric answer.",
  "agent_schema": {{
    "type": "object",
    "properties": {{
      "answer": {{
        "type": "integer"
      }}
    }}
  }},
  "resource_urls": [],
  "actions": []
}}"#
    )
}

#[test]
fn accepts_date_revision_schema_version() {
    let parsed = build_support::generate_agent_model_from_str(&minimal_agentcfg("2026-03-03.r1"));
    assert!(parsed.is_ok());
}

#[test]
fn rejects_legacy_semver_schema_version() {
    let err = build_support::generate_agent_model_from_str(&minimal_agentcfg("0.0.10"))
        .expect_err("semver-style schema versions should be rejected")
        .to_string();

    assert!(err.contains("$.version"));
    assert!(err.contains("YYYY-MM-DD.rN"));
}

#[test]
fn rejects_invalid_calendar_dates() {
    let err = build_support::generate_agent_model_from_str(&minimal_agentcfg("2025-02-29.r1"))
        .expect_err("invalid dates should be rejected")
        .to_string();

    assert!(err.contains("$.version"));
    assert!(err.contains("YYYY-MM-DD.rN"));
}
