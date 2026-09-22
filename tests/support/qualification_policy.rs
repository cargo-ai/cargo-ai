//! Typed, sanitized evidence shared by probe execution and qualification summaries.

use serde::{Deserialize, Serialize};

pub type Result<T> = std::result::Result<T, &'static str>;
pub const PROVIDERS: [&str; 6] = [
    "openai",
    "anthropic",
    "gemini",
    "xai",
    "mistral",
    "typesafe",
];

pub fn hexadecimal(value: &str, length: usize) -> bool {
    value.len() == length
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

pub fn positive(value: &str) -> Result<u64> {
    if value.is_empty() || value.starts_with('0') || !value.bytes().all(|b| b.is_ascii_digit()) {
        return Err("invalid workflow identity");
    }
    value
        .parse()
        .map_err(|_| "workflow identity exceeds supported bounds")
}

pub fn required(provider: &str) -> bool {
    matches!(provider, "openai" | "anthropic")
}

pub fn label(provider: &str) -> &'static str {
    match provider {
        "openai" => "OpenAI",
        "anthropic" => "Anthropic",
        "gemini" => "Gemini",
        "xai" => "xAI",
        "mistral" => "Mistral",
        "typesafe" => "TypeSafe Jev",
        _ => "Unknown",
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    Pass,
    RateLimited,
    Failure,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Diagnostic {
    #[default]
    Unspecified,
    None,
    RateLimited,
    ServerError,
    Connectivity,
    Timeout,
    Unauthorized,
    ModelNotFound,
    InvalidRequest,
    InvalidResponse,
    ExecutionFailure,
    Unknown,
}
impl Diagnostic {
    pub fn retryable(self) -> bool {
        matches!(
            self,
            Self::RateLimited | Self::ServerError | Self::Connectivity | Self::Timeout
        )
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Attempt {
    pub outcome: Outcome,
    pub diagnostic: Diagnostic,
}

/// Public journey evidence contains identities and assertion counts, never raw usage.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Journey {
    pub requested_model: String,
    pub returned_models: Vec<String>,
    pub requests_started: u8,
    pub completed_cases: u8,
}

pub fn safe_model(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._:/-".contains(&b))
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Record {
    pub schema_version: u32,
    pub candidate: String,
    pub provider: String,
    pub run_id: String,
    pub run_attempt: String,
    pub probe_id: String,
    pub outcome: Outcome,
    #[serde(default)]
    pub diagnostic: Diagnostic,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attempts: Vec<Attempt>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub journey: Option<Journey>,
}

impl Record {
    pub fn parse(raw: &[u8]) -> Result<Self> {
        if raw.len() > 2048 {
            return Err("oversized probe evidence");
        }
        // Deserialize directly into a struct: duplicate and unknown fields must fail.
        let record: Self = serde_json::from_slice(raw).map_err(|_| "invalid probe evidence")?;
        if record.schema_version != 1
            || !hexadecimal(&record.candidate, 40)
            || !hexadecimal(&record.probe_id, 32)
            || !PROVIDERS.contains(&record.provider.as_str())
        {
            return Err("invalid probe identity");
        }
        if record.attempts.len() > 3
            || record.attempts.last().is_some_and(|last| {
                last.outcome != record.outcome || last.diagnostic != record.diagnostic
            })
            || record
                .attempts
                .iter()
                .take(record.attempts.len().saturating_sub(1))
                .any(|a| a.outcome == Outcome::Pass || !a.diagnostic.retryable())
        {
            return Err("invalid probe attempt history");
        }
        match (&record.journey, record.provider.as_str()) {
            (Some(journey), "typesafe") => {
                if !safe_model(&journey.requested_model)
                    || journey.returned_models.len() > 8
                    || journey
                        .returned_models
                        .iter()
                        .any(|model| !safe_model(model))
                    || journey.requests_started > 8
                    || journey.completed_cases > journey.requests_started
                    || journey.requests_started - journey.completed_cases > 1
                    || record.attempts.len() > 1
                    || (record.outcome == Outcome::Pass
                        && (journey.completed_cases != 8
                            || journey.returned_models.is_empty()
                            || record.diagnostic != Diagnostic::None))
                    || (record.outcome != Outcome::Pass && journey.completed_cases == 8)
                    || (record.outcome == Outcome::RateLimited
                        && (record.diagnostic != Diagnostic::RateLimited
                            || journey.requests_started != journey.completed_cases + 1))
                {
                    return Err("invalid Jev journey evidence");
                }
            }
            (None, "typesafe") | (Some(_), _) => return Err("unexpected journey evidence"),
            _ => {}
        }
        positive(&record.run_id)?;
        positive(&record.run_attempt)?;
        Ok(record)
    }
}

pub struct Identity<'a> {
    pub candidate: &'a str,
    pub run_id: &'a str,
    pub run_attempt: &'a str,
    pub probe_id: Option<&'a str>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    Pass,
    Fail,
    Cancelled,
    Skipped,
    NotConfigured,
    Missing,
    Unverified,
}

impl Status {
    pub fn label(self) -> &'static str {
        match self {
            Self::Pass => "✅ pass",
            Self::Fail => "❌ fail",
            Self::Cancelled => "🚫 cancelled",
            Self::Skipped => "⏭️ skipped",
            Self::NotConfigured => "⚪ not configured",
            Self::Missing => "❓ missing",
            Self::Unverified => "⚠️ not verified — rate limited",
        }
    }

    pub fn job_result(self) -> &'static str {
        match self {
            Self::Pass => "success",
            Self::Fail => "failure",
            Self::Cancelled => "cancelled",
            Self::Skipped => "skipped",
            _ => "missing",
        }
    }
}

pub struct Decision {
    pub status: Status,
    pub accepted: bool,
}

pub fn evaluate(
    provider: &str,
    raw: &[u8],
    identity: &Identity<'_>,
    job: &str,
    enrollment: &str,
) -> Result<Decision> {
    if !PROVIDERS.contains(&provider) || !hexadecimal(identity.candidate, 40) {
        return Err("invalid expected identity");
    }
    positive(identity.run_id)?;
    positive(identity.run_attempt)?;
    if !required(provider) {
        match enrollment {
            "" | "false" if job == "skipped" && raw.is_empty() => {
                return Ok(Decision {
                    status: Status::NotConfigured,
                    accepted: true,
                });
            }
            "true" => {}
            _ => return Err("invalid enrollment or unexpected provider execution"),
        }
    }
    if job != "success" {
        return Err("provider job did not complete successfully");
    }
    let record = Record::parse(raw)?;
    if record.provider != provider
        || record.candidate != identity.candidate
        || record.run_id != identity.run_id
        || record.run_attempt != identity.run_attempt
        || identity.probe_id.is_some_and(|id| id != record.probe_id)
    {
        return Err("probe identity mismatch");
    }
    Ok(Decision {
        status: match record.outcome {
            Outcome::Pass => Status::Pass,
            Outcome::RateLimited => Status::Unverified,
            Outcome::Failure => Status::Fail,
        },
        accepted: record.outcome == Outcome::Pass
            || (!required(provider) && record.outcome == Outcome::RateLimited),
    })
}
