//! TypeSafe System One transport and the explicit Choice/Score mapping.

use super::{
    compatibility::validate_typesafe_settings,
    runtime::{ContentPart, ProviderTextRequest, ProviderTextResponse, ProviderUsage},
    ProviderError, ProviderKind,
};
use serde::Deserialize;
use serde_json::{json, Map, Value};
use std::time::Duration;

const PROVIDER: ProviderKind = ProviderKind::TypeSafe;

fn incompatible(path: &str, reason: &str) -> ProviderError {
    ProviderError::invalid_request(PROVIDER, format!("{path}: {reason}"))
}

fn invalid_answer(path: &str, reason: &str) -> ProviderError {
    ProviderError::invalid_response(PROVIDER, format!("{path}: {reason}"))
}

fn score_bounds(field: &Value, path: &str) -> Result<(f64, f64), ProviderError> {
    let minimum = field.get("minimum").and_then(Value::as_f64);
    let maximum = field.get("maximum").and_then(Value::as_f64);
    match (minimum, maximum) {
        (Some(a), Some(b)) if a.is_finite() && b.is_finite() && a < b && (b - a).is_finite()
            && field.get("exclusiveMinimum").is_none() && field.get("exclusiveMaximum").is_none() => Ok((a, b)),
        _ => Err(incompatible(path, "Jev Score requires finite inclusive minimum < maximum with a finite span; remove exclusive bounds or select another profile.")),
    }
}

pub(super) fn questions(
    schema: &Value,
    rubric_enabled: bool,
) -> Result<Map<String, Value>, ProviderError> {
    if schema.get("type").and_then(Value::as_str) != Some("object") {
        return Err(incompatible("$.agent_schema", "Jev requires a flat object of Choice and explicit Score fields; select a compatible profile or redesign the output."));
    }
    let properties = schema
        .get("properties")
        .and_then(Value::as_object)
        .ok_or_else(|| {
            incompatible(
                "$.agent_schema.properties",
                "Jev requires an object of declared outputs.",
            )
        })?;
    let mut result = Map::new();
    for (name, field) in properties {
        let path = format!("$.agent_schema.properties.{name}");
        let description = field
            .get("description")
            .and_then(Value::as_str)
            .filter(|text| !text.trim().is_empty())
            .ok_or_else(|| {
                incompatible(
                    &path,
                    "Jev requires a nonblank description stating the question to answer.",
                )
            })?;
        match field.get("type").and_then(Value::as_str) {
            Some("string") => {
                let labels = field.get("enum").and_then(Value::as_array)
                    .filter(|labels| !labels.is_empty() && labels.len() <= 255)
                    .ok_or_else(|| incompatible(&path, "Jev Choice requires 1–255 unique string enum labels; select a capable profile for free-form output or intentionally author categories."))?;
                let mut criteria = Map::new();
                for label in labels {
                    let label = label.as_str().ok_or_else(|| incompatible(&path, "Jev Choice enum labels must all be strings."))?;
                    if label.trim().is_empty() {
                        return Err(incompatible(&path, "Jev Choice enum labels must be nonblank; author a descriptive label without changing its intended meaning."));
                    }
                    if criteria.insert(label.to_string(), Value::String(label.to_string())).is_some() {
                        return Err(incompatible(&path, "Jev Choice enum labels must be unique."));
                    }
                }
                result.insert(name.clone(), json!({"type":"choice","instructions":description,"criteria":criteria}));
            }
            Some("number") => {
                if !rubric_enabled {
                    return Err(incompatible(&path, "Jev Score requires the rubric-enabled schema revision and an explicit rubric; select another profile for ordinary numeric output."));
                }
                let rubric = field.get("rubric").and_then(Value::as_array)
                    .filter(|levels| (2..=10).contains(&levels.len()) && levels.iter().all(|level| level.as_str().is_some_and(|text| !text.trim().is_empty())))
                    .ok_or_else(|| incompatible(&path, "Jev Score requires an explicit rubric of 2–10 nonblank descriptive levels; add levels or select a provider supporting ordinary numeric output."))?;
                score_bounds(field, &path)?;
                result.insert(name.clone(), json!({"type":"score","instructions":description,"criteria":rubric}));
            }
            _ => return Err(incompatible(&path, "Jev supports string-enum Choice and explicit number Score only; nullable, integer, boolean, array and object fields require another compatible profile or an intentional output redesign.")),
        }
    }
    Ok(result)
}

fn state(content_parts: &[ContentPart]) -> Result<Vec<&str>, ProviderError> {
    content_parts.iter().enumerate().map(|(index, part)| match part {
        ContentPart::Text(text) => Ok(text.as_str()),
        _ => Err(incompatible(&format!("$.inputs[{index}]"), "Jev accepts resolved text only; select a compatible profile for image or file content.")),
    }).collect()
}

fn scale_score(
    score: f64,
    levels: usize,
    minimum: f64,
    maximum: f64,
    path: &str,
) -> Result<f64, ProviderError> {
    let last = (levels - 1) as f64;
    if !score.is_finite() || score < 0.0 || score > last {
        return Err(invalid_answer(
            path,
            "Jev score must be finite and inside the requested native rubric range.",
        ));
    }
    let mapped = if score == 0.0 {
        minimum
    } else if score == last {
        maximum
    } else {
        minimum + (score / last) * (maximum - minimum)
    };
    if !mapped.is_finite() || mapped < minimum || mapped > maximum {
        return Err(invalid_answer(
            path,
            "Mapped Jev score is outside the authored inclusive bounds.",
        ));
    }
    Ok(mapped)
}

#[derive(Deserialize)]
struct Response {
    model: String,
    answers: Map<String, Value>,
    usage: Usage,
}

#[derive(Deserialize)]
struct Usage {
    #[serde(default)]
    input_tokens: Option<u64>,
    #[serde(default)]
    output_tokens: Option<u64>,
}

fn map_response(
    body: &[u8],
    schema: &Value,
    questions: &Map<String, Value>,
) -> Result<ProviderTextResponse, ProviderError> {
    // Deserializer details can quote untrusted response values; keep them out of diagnostics.
    let response: Response = serde_json::from_slice(body)
        .map_err(|_| invalid_answer("$", "Malformed TypeSafe response: expected model, answers object and usage object with valid counters."))?;
    if response.model.is_empty()
        || response.model.len() > 128
        || !response
            .model
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._-/".contains(&byte))
    {
        return Err(invalid_answer(
            "$.model",
            "TypeSafe returned an invalid model identifier.",
        ));
    }
    if response.answers.len() != questions.len()
        || response
            .answers
            .keys()
            .any(|key| !questions.contains_key(key))
    {
        return Err(invalid_answer("$.answers", "TypeSafe answer IDs must match the requested outputs exactly; missing or extra answers are invalid."));
    }
    let mut output = Map::new();
    for (name, question) in questions {
        let path = format!("$.answers.{name}");
        let answer = &response.answers[name];
        if !answer.is_object() || answer.get("type") != question.get("type") {
            return Err(invalid_answer(
                &path,
                "TypeSafe answer type must match the requested primitive.",
            ));
        }
        let value = if question["type"] == "choice" {
            let label = answer
                .get("choice")
                .and_then(Value::as_str)
                .filter(|label| question["criteria"].get(*label).is_some())
                .ok_or_else(|| {
                    invalid_answer(
                        &path,
                        "TypeSafe Choice must return an exact declared enum label.",
                    )
                })?;
            Value::String(label.to_string())
        } else {
            let score = answer.get("score").and_then(Value::as_f64).ok_or_else(|| {
                invalid_answer(&path, "TypeSafe Score must return a finite numeric score.")
            })?;
            let field = &schema["properties"][name];
            let (a, b) = score_bounds(field, &path)?;
            let levels = question["criteria"].as_array().unwrap().len();
            let mapped = scale_score(score, levels, a, b, &path)?;
            // Retain the authored JSON number at exact endpoints, including
            // integer bounds whose decimal precision exceeds f64 precision.
            if score == 0.0 {
                field["minimum"].clone()
            } else if score == (levels - 1) as f64 {
                field["maximum"].clone()
            } else {
                json!(mapped)
            }
        };
        output.insert(name.clone(), value);
    }
    let total_tokens = match (response.usage.input_tokens, response.usage.output_tokens) {
        (Some(input), Some(output)) => Some(input.checked_add(output).ok_or_else(|| {
            invalid_answer(
                "$.usage",
                "TypeSafe token counters overflow the supported total.",
            )
        })?),
        _ => None,
    };
    Ok(ProviderTextResponse {
        provider_request_id: None,
        finish_reason: None,
        text: Value::Object(output).to_string(),
        usage: Some(ProviderUsage {
            total_tokens_source: Some("derived_input_plus_output".to_string()),
            input_tokens: response.usage.input_tokens,
            output_tokens: response.usage.output_tokens,
            total_tokens,
            input_token_details: None,
            output_token_details: None,
        }),
        resolved_model: Some(response.model),
    })
}

fn http_error(status: reqwest::StatusCode) -> ProviderError {
    let message = match status.as_u16() {
        401 | 403 => "TypeSafe rejected authentication; verify the profile API key and model access.",
        422 => "$.state / $.questions: TypeSafe rejected request validation or context limits; check the schema and reduce input size if necessary. No content was truncated.",
        429 => "TypeSafe rate limit reached; retry later within your runtime budget.",
        529 => "TypeSafe is temporarily overloaded; retry later within your runtime budget.",
        _ => "TypeSafe returned an HTTP error response; check the endpoint and request settings.",
    };
    ProviderError::from_http_status(PROVIDER, status, message)
}

pub(super) async fn send_request(
    url: &str,
    request: ProviderTextRequest<'_>,
) -> Result<ProviderTextResponse, ProviderError> {
    validate_typesafe_settings(request.max_output_tokens, request.temperature)?;
    let questions = questions(request.response_schema, request.rubric_enabled)?;
    let state = state(request.content_parts)?;
    super::validate_provider_request(PROVIDER, request.model, url, request.token)
        .map_err(|issues| incompatible("$.profile", &issues.join(" ")))?;
    let client = reqwest::ClientBuilder::new()
        .timeout(Duration::from_secs(request.timeout_in_sec))
        .build()
        .map_err(|error| ProviderError::from_reqwest(PROVIDER, error))?;
    let response = client
        .post(url)
        .bearer_auth(request.token)
        .json(&json!({"model":request.model,"state":state,"questions":questions}))
        .send()
        .await
        .map_err(|error| ProviderError::from_reqwest(PROVIDER, error))?;
    let response_id = response
        .headers()
        .get("x-request-id")
        .or_else(|| response.headers().get("request-id"))
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let status = response.status();
    if !status.is_success() {
        // Error bodies can echo input and credentials. Status alone selects safe guidance.
        return Err(http_error(status).with_request_id(response_id.as_deref(), request.token));
    }
    let body = response
        .bytes()
        .await
        .map_err(|error| ProviderError::from_reqwest(PROVIDER, error))?;
    let facts = super::runtime::ProviderFacts::from_body(&body, PROVIDER)
        .with_request_id(response_id.as_deref())
        .redact_token(request.token);
    let mapped = map_response(&body, request.response_schema, &questions)
        .map_err(|error: ProviderError| facts.error(error.redact_token(request.token)))?;
    if !request.token.is_empty()
        && mapped
            .resolved_model
            .as_deref()
            .is_some_and(|value| value.contains(request.token))
    {
        return Err(facts.error(invalid_answer(
            "$.model",
            "TypeSafe returned an invalid model identifier.",
        )));
    }
    Ok(facts.text(mapped))
}

#[cfg(test)]
mod tests {
    use super::*;
    use mockito::{Matcher, Server};

    fn schema() -> Value {
        json!({"type":"object","properties":{
            "department":{"type":"string","enum":["billing","technical","sales"],"description":"Which team?"},
            "urgency":{"type":"number","minimum":0,"maximum":100,"description":"How urgent?","rubric":["Routine","Today","Blocking"]}
        },"required":["department","urgency"],"additionalProperties":false})
    }

    fn response() -> Value {
        json!({"model":"jev-1.13.0","answers":{
            "department":{"type":"choice","choice":"technical","probabilities":{"technical":1}},
            "urgency":{"type":"score","score":1.6,"confidence":0.9,"legend":{"0":"Routine"}}
        },"usage":{"input_tokens":12,"output_tokens":3,"debug":"never-copy"}})
    }

    #[test]
    fn mapping_preserves_labels_criteria_and_fractional_scale() {
        let schema = schema();
        let questions = questions(&schema, true).unwrap();
        assert_eq!(
            questions["department"]["criteria"],
            json!({"billing":"billing","technical":"technical","sales":"sales"})
        );
        assert_eq!(
            questions["urgency"]["criteria"],
            json!(["Routine", "Today", "Blocking"])
        );
        assert_eq!(questions["urgency"]["instructions"], "How urgent?");
        let mapped = map_response(response().to_string().as_bytes(), &schema, &questions).unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(&mapped.text).unwrap(),
            json!({"department":"technical","urgency":80.0})
        );
        assert_eq!(mapped.usage.as_ref().unwrap().total_tokens, Some(15));
        assert_eq!(mapped.usage.as_ref().unwrap().input_token_details, None);
        assert_eq!(mapped.resolved_model.as_deref(), Some("jev-1.13.0"));
    }

    #[test]
    fn choice_labels_reject_blank_values_and_preserve_exact_nonblank_values() {
        for blank in ["", " ", "\t\n"] {
            let mut schema = schema();
            schema["properties"]["department"]["enum"] = json!(["billing", blank]);
            assert!(questions(&schema, true)
                .unwrap_err()
                .to_string()
                .contains("nonblank"));
        }
        let mut schema = schema();
        schema["properties"]["department"]["enum"] = json!([" billing ", "billing"]);
        assert_eq!(
            questions(&schema, true).unwrap()["department"]["criteria"],
            json!({" billing ":" billing ","billing":"billing"})
        );
    }

    #[test]
    fn scaling_preserves_endpoints_without_clamping() {
        for (a, b) in [
            (0.0, 100.0),
            (-8.0, 3.0),
            (1.0e100, 1.1e100),
            (-1.0e-100, 2.0e-100),
        ] {
            assert_eq!(scale_score(0.0, 3, a, b, "$.score").unwrap(), a);
            assert_eq!(scale_score(2.0, 3, a, b, "$.score").unwrap(), b);
            let middle = scale_score(1.0, 3, a, b, "$.score").unwrap();
            assert!(middle >= a && middle <= b);
        }
        for invalid in [-0.01, 2.01, f64::INFINITY, f64::NEG_INFINITY, f64::NAN] {
            assert!(scale_score(invalid, 3, 0.0, 100.0, "$.score").is_err());
        }
    }

    #[test]
    fn mapped_native_endpoints_retain_authored_json_number_precision() {
        let mut schema = schema();
        schema["properties"]["urgency"]["minimum"] = json!(9_007_199_254_740_993_u64);
        schema["properties"]["urgency"]["maximum"] = json!(9_007_199_254_741_093_u64);
        let questions = questions(&schema, true).unwrap();
        for (native, bound) in [(0.0, "minimum"), (2.0, "maximum")] {
            let mut response = response();
            response["answers"]["urgency"]["score"] = json!(native);
            let mapped =
                map_response(response.to_string().as_bytes(), &schema, &questions).unwrap();
            let output: Value = serde_json::from_str(&mapped.text).unwrap();
            assert_eq!(output["urgency"], schema["properties"]["urgency"][bound]);
        }
    }

    #[test]
    fn invalid_answers_and_envelopes_fail_closed_without_echoing_values() {
        let schema = schema();
        let questions = questions(&schema, true).unwrap();
        let mut cases = vec![
            json!(null),
            json!([]),
            json!({}),
            json!({"model":"jev","answers":[],"usage":{}}),
        ];
        for (pointer, value) in [
            ("/model", json!("private\nmodel")),
            ("/model", json!(17)),
            ("/answers/department/type", json!("score")),
            ("/answers/department/choice", json!("private-data")),
            ("/answers/department", json!(null)),
            ("/answers/urgency/score", json!("private-score")),
            ("/answers/urgency/score", json!(-0.01)),
            ("/answers/urgency/score", json!(2.01)),
            ("/usage/input_tokens", json!(-1)),
            ("/usage/output_tokens", json!(1.5)),
        ] {
            let mut candidate = response();
            *candidate.pointer_mut(pointer).unwrap() = value;
            cases.push(candidate);
        }
        let mut missing = response();
        missing["answers"]
            .as_object_mut()
            .unwrap()
            .remove("urgency");
        cases.push(missing);
        let mut extra = response();
        extra["answers"]["private-extra"] = json!({});
        cases.push(extra);
        let mut overflow = response();
        overflow["usage"]["input_tokens"] = json!(u64::MAX);
        cases.push(overflow);
        for candidate in cases {
            let error =
                map_response(candidate.to_string().as_bytes(), &schema, &questions).unwrap_err();
            assert!(!error.to_string().contains("private"));
        }
        assert!(map_response(b"{bad private json}", &schema, &questions).is_err());
    }

    #[test]
    fn compatibility_rejects_unsupported_fields_and_settings() {
        let mut incompatible_fields = vec![
            json!({"type":"number","minimum":0,"maximum":100,"description":"n"}),
            json!({"type":"string","description":"n"}),
            json!({"type":"string","description":"n","enum":["a","a"]}),
            json!({"type":"string","description":"n","enum":[1]}),
            json!({"type":"string","description":" ","enum":["a"]}),
            json!({"type":"string","description":"n","enum":[]}),
        ];
        for kind in [
            json!("integer"),
            json!("boolean"),
            json!("array"),
            json!("object"),
            json!(["string", "null"]),
        ] {
            incompatible_fields.push(json!({"type":kind,"description":"n"}));
        }
        for (key, value) in [
            ("rubric", json!(["one"])),
            ("rubric", json!(["one", ""])),
            ("maximum", json!(-1)),
            ("exclusiveMinimum", json!(0)),
        ] {
            let mut field = schema()["properties"]["urgency"].clone();
            field[key] = value;
            incompatible_fields.push(field);
        }
        incompatible_fields.push(json!({"type":"string","description":"n","enum":(0..256).map(|i| i.to_string()).collect::<Vec<_>>()}));
        for field in incompatible_fields {
            assert!(
                questions(&json!({"type":"object","properties":{"field":field}}), true).is_err()
            );
        }
        assert!(questions(&schema(), false).is_err());
        assert!(validate_typesafe_settings(Some(10), None)
            .unwrap_err()
            .to_string()
            .contains("--clear-max-output-tokens"));
        assert!(validate_typesafe_settings(None, Some(0.0))
            .unwrap_err()
            .to_string()
            .contains("--clear-temperature"));
        assert!(state(&[ContentPart::Image {
            data_url: "private".into()
        }])
        .is_err());
        assert!(state(&[ContentPart::File {
            filename: "private".into(),
            file_data: "private".into()
        }])
        .is_err());
    }

    #[tokio::test]
    async fn transport_preserves_text_boundaries_and_only_sends_supported_fields() {
        let mut server = Server::new_async().await;
        let schema = schema();
        let mock = server.mock("POST","/v1/systemone")
            .match_header("authorization","Bearer test-typesafe-key")
            .match_body(Matcher::Json(json!({"model":"jev-1.13.0","state":["context","first","second"],"questions":questions(&schema,true).unwrap()})))
            .with_status(200).with_body(response().to_string()).create_async().await;
        let response = send_request(
            &format!("{}/v1/systemone", server.url()),
            ProviderTextRequest {
                model: "jev-1.13.0",
                content_parts: &[
                    ContentPart::Text("context".into()),
                    ContentPart::Text("first".into()),
                    ContentPart::Text("second".into()),
                ],
                timeout_in_sec: 5,
                token: "test-typesafe-key",
                response_schema: &schema,
                max_output_tokens: None,
                temperature: None,
                rubric_enabled: true,
            },
        )
        .await
        .unwrap();
        assert!(response.text.contains("80.0"));
        mock.assert_async().await;
    }

    #[tokio::test]
    async fn http_failures_are_actionable_without_provider_payloads() {
        for status in [401, 422, 429, 529] {
            let mut server = Server::new_async().await;
            let mock = server.mock("POST","/").with_status(status)
                .with_body(r#"{"detail":"private-request-text Bearer test-typesafe-key","input":"private-content"}"#).create_async().await;
            let error = send_request(
                &server.url(),
                ProviderTextRequest {
                    model: "jev-1.13.0",
                    content_parts: &[],
                    timeout_in_sec: 5,
                    token: "test-typesafe-key",
                    response_schema: &schema(),
                    max_output_tokens: None,
                    temperature: None,
                    rubric_enabled: true,
                },
            )
            .await
            .unwrap_err();
            assert_eq!(error.http_status(), Some(status as u16));
            let diagnostics = super::super::provider_error_messages(&error).join(" ");
            assert!(!diagnostics.contains("private"));
            assert!(!diagnostics.contains("test-typesafe-key"));
            mock.assert_async().await;
        }
    }

    #[tokio::test]
    async fn timeout_preserves_category_without_echoing_endpoint_secrets() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!(
            "http://{}/private-path?key=private-query",
            listener.local_addr().unwrap()
        );
        let error = send_request(
            &endpoint,
            ProviderTextRequest {
                model: "jev-1.13.0",
                content_parts: &[],
                timeout_in_sec: 1,
                token: "private-key",
                response_schema: &schema(),
                max_output_tokens: None,
                temperature: None,
                rubric_enabled: true,
            },
        )
        .await
        .unwrap_err();
        assert_eq!(
            error.kind(),
            super::super::error::ProviderErrorKind::Timeout
        );
        assert!(!error.to_string().contains("private"));
    }

    #[tokio::test]
    async fn credential_echo_cannot_be_exposed_as_returned_model_identity() {
        let mut server = Server::new_async().await;
        let mut body = response();
        body["model"] = json!("test-typesafe-key");
        let mock = server
            .mock("POST", "/")
            .with_status(200)
            .with_body(body.to_string())
            .create_async()
            .await;
        let error = send_request(
            &server.url(),
            ProviderTextRequest {
                model: "jev-1.13.0",
                content_parts: &[],
                timeout_in_sec: 5,
                token: "test-typesafe-key",
                response_schema: &schema(),
                max_output_tokens: None,
                temperature: None,
                rubric_enabled: true,
            },
        )
        .await
        .unwrap_err();
        assert!(error.to_string().contains("$.model"));
        assert!(!error.to_string().contains("test-typesafe-key"));
        mock.assert_async().await;
    }
}
