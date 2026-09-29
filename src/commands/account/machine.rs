//! Typed observations collected at account operation and persistence boundaries.
use crate::commands::machine::Failure;
use serde_json::{json, Map, Value};

pub(crate) struct Report {
    pub active: bool,
    pub result: Result<Value, Failure>,
    pub session_persistence: &'static str,
    secrets: Vec<String>,
}
impl Report {
    pub fn new(active: bool) -> Self {
        Self {
            active,
            result: Err(Failure::new(
                "input.invalid",
                "The command arguments could not be applied.",
            )),
            session_persistence: "not_required",
            secrets: Vec::new(),
        }
    }
    pub fn fail(&mut self, code: &'static str, message: &'static str, mut data: Value) {
        if contains_secret(&data, &self.secrets) {
            data = json!({"response_withheld":true,"remote_effect":"unknown"});
        }
        if code == "operation.partial" {
            data["partial"] = json!(true);
        }
        self.result = Err(Failure::new(code, message).with_data(data));
    }
    pub fn prerequisites(&mut self) {
        self.fail(
            "account.prerequisite",
            "An existing account and accessible credentials are required.",
            json!({"remote_effect":"not_attempted"}),
        );
    }
    pub fn transmitting(&mut self, mutation: bool) {
        self.fail(if mutation { "operation.outcome_unknown" } else { "network.request_failed" }, "The service result could not be confirmed.", json!({"remote_effect": if mutation { "unknown" } else { "not_requested" }, "session_persistence":self.session_persistence}));
    }
    pub fn protect(&mut self, secret: &str) {
        if self.active && !secret.is_empty() && !self.secrets.iter().any(|known| known == secret) {
            self.secrets.push(secret.to_owned());
        }
    }
    pub fn accepted(&mut self, mut data: Value) {
        if contains_secret(&data, &self.secrets) {
            self.fail("response.private_data", "The operation completed but its response reflected protected credential material.", json!({"operation_completed":true,"response_withheld":true,"session_persistence":self.session_persistence}));
            return;
        }
        data["session_persistence"] = json!(self.session_persistence);
        if self.session_persistence == "failed" {
            self.fail("operation.partial", "The operation completed, but refreshed account credentials could not be persisted.", data);
        } else {
            self.result = Ok(data);
        }
    }
}

fn contains_secret(value: &Value, secrets: &[String]) -> bool {
    let contains = |text: &str| secrets.iter().any(|secret| text.contains(secret));
    match value {
        Value::String(text) => contains(text),
        Value::Array(values) => values.iter().any(|value| contains_secret(value, secrets)),
        Value::Object(values) => values
            .iter()
            .any(|(key, value)| contains(key) || contains_secret(value, secrets)),
        _ => false,
    }
}

/// Copy only explicitly named scalar contract facts. Backend UI and messages are never forwarded.
pub(crate) fn fields(source: &Value, names: &[&str]) -> Value {
    let mut result = Map::new();
    for name in names {
        if let Some(value) = source
            .get(*name)
            .filter(|value| !value.is_array() && !value.is_object())
        {
            result.insert((*name).to_owned(), value.clone());
        }
    }
    Value::Object(result)
}

pub(crate) fn response_ok(response: &Value, expected: &str) -> Result<(), Failure> {
    if response["status"] == "success" && response["type"] == expected {
        return Ok(());
    }
    let code = match response["type"].as_str() {
        Some(
            "access_token_expired"
            | "invalid_access_token"
            | "missing_access_token"
            | "missing_refresh_token",
        ) => "account.authentication",
        Some(
            "account_deactivated" | "account_deletion_pending" | "forbidden" | "permission_denied",
        ) => "account.authorization",
        Some("rate_limited" | "quota_exceeded") => "account.rate_limited",
        Some("account_not_found" | "agent_not_found" | "project_not_found") => "account.not_found",
        _ if response["status"] == "failure" || response["status"] == "error" => {
            "account.remote_rejected"
        }
        _ => "operation.outcome_unknown",
    };
    Err(Failure::new(code, "The service did not confirm the requested result.").with_data(json!({"remote_effect": if matches!(code, "operation.outcome_unknown" | "account.remote_rejected") { "unknown" } else { "unapplied" }})))
}

pub(crate) fn list_payload(
    response: &Value,
    key: &str,
    names: &[&str],
    original_count: usize,
) -> Result<Value, Failure> {
    let rows = response[key].as_array().ok_or_else(|| {
        Failure::new(
            "response.invalid",
            "The service returned an invalid inventory.",
        )
    })?;
    let identity = if key == "agents" {
        "agent_name"
    } else {
        "project_name"
    };
    if rows
        .iter()
        .any(|row| !row.is_object() || row[identity].as_str().is_none_or(|name| name.is_empty()))
    {
        return Err(Failure::new(
            "response.invalid",
            "The service returned an invalid inventory item.",
        ));
    }
    Ok(
        json!({"items":rows.iter().map(|row| fields(row, names)).collect::<Vec<_>>(),"count":rows.len(),"available_count":original_count,"complete":rows.len()==original_count,"next_cursor":null,"continuation":"repeat_with_all"}),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn machine_account_projection_rejects_credentials_reflected_into_allowed_metadata() {
        let mut report = Report::new(true);
        report.protect("private-access-token");
        report.accepted(json!({"account":{"handle":"prefix-private-access-token-suffix"}}));
        let failure = report.result.unwrap_err();
        assert_eq!(failure.code, "response.private_data");
        assert!(!failure.data.to_string().contains("private-access-token"));
        assert!(!failure.message.contains("private-access-token"));
    }
    #[test]
    fn scalar_projection_never_forwards_backend_ui_credentials_or_nested_fields() {
        let input = json!({"agent":"safe","definition_path":"/","ui":{"title":"secret"},"credentials":"secret","public":{"secret":"secret"}});
        assert_eq!(
            fields(&input, &["agent", "definition_path", "public"]),
            json!({"agent":"safe","definition_path":"/"})
        );
        assert!(response_ok(
            &json!({"status":"success","type":"unexpected","message":"secret"}),
            "expected"
        )
        .is_err());
    }
}

#[cfg(test)]
pub(crate) fn fixture_args(words: &[&str]) -> clap::ArgMatches {
    crate::args::parse_cli(
        "cargo-ai",
        words.iter().map(std::ffi::OsString::from).collect(),
    )
    .unwrap()
}
