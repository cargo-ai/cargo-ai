//! Bounded business JSON shared by action inputs and selected tool results.
use serde::de::{DeserializeSeed, MapAccess, SeqAccess, Visitor};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::BTreeSet;
use std::fmt;

pub const MAX_BUSINESS_BYTES: usize = 65_536;
pub const MAX_PAYLOAD_BYTES: usize = 131_072;
pub const MAX_DEPTH: usize = 8;
pub const MAX_MEMBERS: usize = 64;
pub const MAX_ITEMS: usize = 1_024;
pub const MAX_KEY_BYTES: usize = 128;
pub const MAX_STRING_BYTES: usize = 65_536;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ArtifactNomination {
    pub id: String,
    pub scope: String,
    pub path: String,
    pub mime_type: String,
}

pub fn validate_business_key(key: &str) -> Result<(), String> {
    if key.is_empty()
        || key.len() > MAX_KEY_BYTES
        || key.chars().any(char::is_control)
        || matches!(key, "__proto__" | "prototype" | "constructor")
    {
        Err("Business property name is unsupported.".into())
    } else {
        Ok(())
    }
}

pub fn strict_json(bytes: &[u8]) -> Result<Value, String> {
    strict_json_bounded(bytes, MAX_PAYLOAD_BYTES, 32)
}
pub fn strict_json_bounded(
    bytes: &[u8],
    maximum: usize,
    max_depth: usize,
) -> Result<Value, String> {
    if bytes.len() > maximum {
        return Err("JSON byte limit exceeded.".into());
    }
    strict_json_with_nodes(bytes, maximum, max_depth, 65_536)
}
pub fn strict_definition_json(bytes: &[u8]) -> Result<Value, String> {
    strict_json_with_nodes(bytes, 8 * 1024 * 1024, 64, 131_072)
}
fn strict_json_with_nodes(
    bytes: &[u8],
    maximum: usize,
    max_depth: usize,
    max_nodes: usize,
) -> Result<Value, String> {
    if bytes.len() > maximum {
        return Err("JSON byte limit exceeded.".into());
    }
    let mut nodes = 0;
    let mut decoder = serde_json::Deserializer::from_slice(bytes);
    let value = StrictSeed {
        depth: 0,
        max_depth,
        max_nodes,
        nodes: &mut nodes,
    }
    .deserialize(&mut decoder)
    .map_err(|_| "JSON is invalid, duplicated, or exceeds structural limits.".to_string())?;
    decoder
        .end()
        .map_err(|_| "JSON contains trailing content.".to_string())?;
    Ok(value)
}
struct StrictSeed<'a> {
    depth: usize,
    max_depth: usize,
    max_nodes: usize,
    nodes: &'a mut usize,
}
impl<'de> DeserializeSeed<'de> for StrictSeed<'_> {
    type Value = Value;
    fn deserialize<D: serde::Deserializer<'de>>(self, decoder: D) -> Result<Value, D::Error> {
        *self.nodes += 1;
        if self.depth > self.max_depth || *self.nodes > self.max_nodes {
            return Err(serde::de::Error::custom("JSON structural limit"));
        }
        decoder.deserialize_any(self)
    }
}
impl<'de> Visitor<'de> for StrictSeed<'_> {
    type Value = Value;
    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("strict bounded JSON")
    }
    fn visit_bool<E: serde::de::Error>(self, v: bool) -> Result<Value, E> {
        Ok(Value::Bool(v))
    }
    fn visit_i64<E: serde::de::Error>(self, v: i64) -> Result<Value, E> {
        Ok(v.into())
    }
    fn visit_u64<E: serde::de::Error>(self, v: u64) -> Result<Value, E> {
        Ok(v.into())
    }
    fn visit_f64<E: serde::de::Error>(self, v: f64) -> Result<Value, E> {
        serde_json::Number::from_f64(v)
            .map(Value::Number)
            .ok_or_else(|| E::custom("nonfinite number"))
    }
    fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<Value, E> {
        Ok(v.into())
    }
    fn visit_string<E: serde::de::Error>(self, v: String) -> Result<Value, E> {
        Ok(v.into())
    }
    fn visit_unit<E: serde::de::Error>(self) -> Result<Value, E> {
        Ok(Value::Null)
    }
    fn visit_none<E: serde::de::Error>(self) -> Result<Value, E> {
        Ok(Value::Null)
    }
    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Value, A::Error> {
        let mut items = Vec::new();
        while let Some(item) = seq.next_element_seed(StrictSeed {
            depth: self.depth + 1,
            max_depth: self.max_depth,
            max_nodes: self.max_nodes,
            nodes: self.nodes,
        })? {
            items.push(item);
            if items.len() > 65_536 {
                return Err(serde::de::Error::custom("item limit"));
            }
        }
        Ok(Value::Array(items))
    }
    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Value, A::Error> {
        let mut object = Map::new();
        while let Some(key) = map.next_key::<String>()? {
            if object.contains_key(&key) {
                return Err(serde::de::Error::custom("duplicate member"));
            }
            let value = map.next_value_seed(StrictSeed {
                depth: self.depth + 1,
                max_depth: self.max_depth,
                max_nodes: self.max_nodes,
                nodes: self.nodes,
            })?;
            object.insert(key, value);
            if object.len() > 65_536 {
                return Err(serde::de::Error::custom("member limit"));
            }
        }
        Ok(Value::Object(object))
    }
}

pub fn validate_value_bounds(value: &Value, depth: usize) -> Result<(), String> {
    if depth > MAX_DEPTH {
        return Err("Business JSON depth limit exceeded.".into());
    }
    match value {
        Value::String(s) if s.len() > MAX_STRING_BYTES => {
            return Err("Business string limit exceeded.".into())
        }
        Value::Array(items) => {
            if items.len() > MAX_ITEMS {
                return Err("Business array limit exceeded.".into());
            }
            for item in items {
                validate_value_bounds(item, depth + 1)?;
            }
        }
        Value::Object(object) => {
            if object.len() > MAX_MEMBERS {
                return Err("Business object limit exceeded.".into());
            }
            for (key, item) in object {
                validate_business_key(key)?;
                validate_value_bounds(item, depth + 1)?;
            }
        }
        _ => {}
    }
    Ok(())
}
fn schema_kind(schema: &Map<String, Value>) -> Result<(&str, bool), String> {
    match schema.get("type") {
        Some(Value::String(kind)) => Ok((kind, false)),
        Some(Value::Array(kinds)) if kinds.len() == 2 && kinds[1] == "null" => {
            let kind = kinds[0].as_str().ok_or("Invalid nullable business type.")?;
            if kind == "null" {
                return Err("Nullable type must include one non-null type.".into());
            }
            Ok((kind, true))
        }
        _ => Err("Business schema requires a type or [type, null].".into()),
    }
}
pub fn validate_schema(schema: &Value, depth: usize) -> Result<(), String> {
    if depth > MAX_DEPTH {
        return Err("Business schema depth limit exceeded.".into());
    }
    let object = schema
        .as_object()
        .ok_or("Business schema must be an object.")?;
    let (kind, _) = schema_kind(object)?;
    let specific: &[&str] = match kind {
        "object" => &["properties", "required", "additionalProperties"],
        "array" => &["items", "minItems", "maxItems"],
        "string" => &["minLength", "maxLength"],
        "number" | "integer" => &["minimum", "maximum", "exclusiveMinimum", "exclusiveMaximum"],
        "boolean" | "null" => &[],
        _ => return Err("Unsupported business schema type.".into()),
    };
    if object.keys().any(|key| {
        !matches!(key.as_str(), "type" | "description" | "enum")
            && !specific.contains(&key.as_str())
    }) {
        return Err("Unsupported business schema keyword.".into());
    }
    if let Some(description) = object.get("description") {
        if !description.is_string() || description.as_str().unwrap().len() > MAX_STRING_BYTES {
            return Err("Invalid business schema description.".into());
        }
    }
    match kind {
        "object" => {
            if object.get("additionalProperties") != Some(&Value::Bool(false)) {
                return Err("Business objects must be closed.".into());
            }
            let props = object
                .get("properties")
                .and_then(Value::as_object)
                .ok_or("Business object requires properties.")?;
            if props.len() > MAX_MEMBERS {
                return Err("Business schema member limit exceeded.".into());
            }
            for (key, prop) in props {
                validate_business_key(key)?;
                validate_schema(prop, depth + 1)?;
            }
            let required = object
                .get("required")
                .and_then(Value::as_array)
                .ok_or("Business object requires explicit required members.")?;
            let mut seen = BTreeSet::new();
            for value in required {
                let key = value
                    .as_str()
                    .ok_or("Required business member must be text.")?;
                if !props.contains_key(key) || !seen.insert(key) {
                    return Err("Invalid or duplicate required business member.".into());
                }
            }
        }
        "array" => validate_schema(
            object
                .get("items")
                .ok_or("Business array requires typed items.")?,
            depth + 1,
        )?,
        _ => {}
    }
    for (low, high, maximum) in [
        ("minLength", "maxLength", MAX_STRING_BYTES),
        ("minItems", "maxItems", MAX_ITEMS),
    ] {
        for name in [low, high] {
            if let Some(value) = object.get(name) {
                if value.as_u64().is_none_or(|n| n > maximum as u64) {
                    return Err("Invalid business schema length bound.".into());
                }
            }
        }
        if let (Some(a), Some(b)) = (
            object.get(low).and_then(Value::as_u64),
            object.get(high).and_then(Value::as_u64),
        ) {
            if a > b {
                return Err("Business schema minimum exceeds maximum.".into());
            }
        }
    }
    for name in ["minimum", "maximum", "exclusiveMinimum", "exclusiveMaximum"] {
        if let Some(value) = object.get(name) {
            if !value.is_number() || (kind == "integer" && !is_integer(value)) {
                return Err("Invalid business numeric bound.".into());
            }
        }
    }
    if object.contains_key("minimum") && object.contains_key("exclusiveMinimum")
        || object.contains_key("maximum") && object.contains_key("exclusiveMaximum")
    {
        return Err("Conflicting business numeric bounds.".into());
    }
    let lower = object
        .get("minimum")
        .or_else(|| object.get("exclusiveMinimum"));
    let upper = object
        .get("maximum")
        .or_else(|| object.get("exclusiveMaximum"));
    if let (Some(lower), Some(upper)) = (lower, upper) {
        let cmp = exact_number_order(lower, upper).ok_or("Invalid business numeric bounds.")?;
        if cmp == std::cmp::Ordering::Greater
            || (cmp == std::cmp::Ordering::Equal
                && (object.contains_key("exclusiveMinimum")
                    || object.contains_key("exclusiveMaximum")))
        {
            return Err("Business numeric lower bound exceeds upper bound.".into());
        }
    }
    if let Some(values) = object.get("enum") {
        let values = values.as_array().ok_or("Business enum must be an array.")?;
        if values.is_empty() || values.len() > MAX_ITEMS {
            return Err("Business enum limit exceeded.".into());
        }
        let mut without_enum = object.clone();
        without_enum.remove("enum");
        for (index, value) in values.iter().enumerate() {
            validate_value(&Value::Object(without_enum.clone()), value, depth)?;
            if values[..index].contains(value) {
                return Err("Business enum values must be unique.".into());
            }
        }
    }
    Ok(())
}
fn is_integer(value: &Value) -> bool {
    value.as_i64().is_some() || value.as_u64().is_some()
}
pub fn exact_number_order(value: &Value, bound: &Value) -> Option<std::cmp::Ordering> {
    fn integer(value: &Value) -> Option<i128> {
        value
            .as_i64()
            .map(i128::from)
            .or_else(|| value.as_u64().map(i128::from))
    }
    fn finite(value: &Value) -> Option<f64> {
        value.as_f64().filter(|number| number.is_finite())
    }
    fn integer_float(integer: i128, float: f64) -> std::cmp::Ordering {
        // Every JSON integer fits i128. Truncation preserves the float's whole
        // part without rounding the integer to f64; saturation can only occur
        // beyond the JSON integer range. The fractional sign breaks a tie.
        integer
            .cmp(&(float as i128))
            .then_with(|| 0.0_f64.partial_cmp(&float.fract()).unwrap())
    }
    match (integer(value), integer(bound)) {
        (Some(value), Some(bound)) => Some(value.cmp(&bound)),
        (Some(value), None) => Some(integer_float(value, finite(bound)?)),
        (None, Some(bound)) => Some(integer_float(bound, finite(value)?).reverse()),
        (None, None) => finite(value)?.partial_cmp(&finite(bound)?),
    }
}

pub fn validate_value(schema: &Value, value: &Value, depth: usize) -> Result<(), String> {
    validate_value_bounds(value, depth)?;
    let schema = schema
        .as_object()
        .ok_or("Business schema must be an object.")?;
    let (kind, nullable) = schema_kind(schema)?;
    if value.is_null() && nullable {
        if schema
            .get("enum")
            .and_then(Value::as_array)
            .is_some_and(|values| !values.contains(value))
        {
            return Err("Business value is outside enum.".into());
        }
        return Ok(());
    }
    let matches = match kind {
        "object" => value.is_object(),
        "array" => value.is_array(),
        "string" => value.is_string(),
        "integer" => is_integer(value),
        "number" => value.is_number(),
        "boolean" => value.is_boolean(),
        "null" => value.is_null(),
        _ => false,
    };
    if !matches {
        return Err("Business value does not match its schema type.".into());
    }
    if schema
        .get("enum")
        .and_then(Value::as_array)
        .is_some_and(|values| !values.contains(value))
    {
        return Err("Business value is outside enum.".into());
    }
    if let Some(object) = value.as_object() {
        let props = schema
            .get("properties")
            .and_then(Value::as_object)
            .ok_or("Missing business properties.")?;
        let required = schema
            .get("required")
            .and_then(Value::as_array)
            .ok_or("Missing required business members.")?;
        if required
            .iter()
            .any(|key| key.as_str().is_none_or(|key| !object.contains_key(key)))
        {
            return Err("Required business member is missing.".into());
        }
        for (key, value) in object {
            validate_value(
                props.get(key).ok_or("Undeclared business member.")?,
                value,
                depth + 1,
            )?;
        }
    }
    if let Some(items) = value.as_array() {
        validate_length(schema, items.len(), "minItems", "maxItems")?;
        let item_schema = schema
            .get("items")
            .ok_or("Missing business array item schema.")?;
        for item in items {
            validate_value(item_schema, item, depth + 1)?;
        }
    }
    if let Some(text) = value.as_str() {
        validate_length(schema, text.chars().count(), "minLength", "maxLength")?;
    }
    if value.is_number() {
        use std::cmp::Ordering;
        for (name, exclusive, lower) in [
            ("minimum", false, true),
            ("maximum", false, false),
            ("exclusiveMinimum", true, true),
            ("exclusiveMaximum", true, false),
        ] {
            if let Some(bound) = schema.get(name) {
                let cmp =
                    exact_number_order(value, bound).ok_or("Invalid business numeric bound.")?;
                if (lower && cmp == Ordering::Less)
                    || (!lower && cmp == Ordering::Greater)
                    || (exclusive && cmp == Ordering::Equal)
                {
                    return Err("Business value violates numeric bound.".into());
                }
            }
        }
    }
    Ok(())
}
fn validate_length(
    schema: &Map<String, Value>,
    length: usize,
    low: &str,
    high: &str,
) -> Result<(), String> {
    if schema
        .get(low)
        .and_then(Value::as_u64)
        .is_some_and(|n| (length as u64) < n)
        || schema
            .get(high)
            .and_then(Value::as_u64)
            .is_some_and(|n| length as u64 > n)
    {
        return Err("Business value violates length bound.".into());
    }
    Ok(())
}

#[derive(Clone, Debug)]
pub struct ProducerResult {
    pub data: Value,
    pub artifacts: Result<Vec<ArtifactNomination>, ResultError>,
}
#[derive(Clone, Copy, Debug)]
pub struct ResultError {
    pub code: &'static str,
    pub message: &'static str,
}
impl fmt::Display for ResultError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}
pub fn decode_producer_result(
    raw: Option<&str>,
    schema: &Value,
) -> Result<ProducerResult, ResultError> {
    let missing = ResultError {
        code: "runtime.result_missing",
        message: "The selected producer returned no result.",
    };
    let invalid = ResultError {
        code: "runtime.result_invalid",
        message: "The selected producer returned invalid result JSON.",
    };
    let limit = ResultError {
        code: "runtime.result_limit",
        message: "The selected producer result exceeds a supported limit.",
    };
    let raw = raw.ok_or(missing)?;
    if raw.len() > MAX_PAYLOAD_BYTES {
        return Err(limit);
    }
    let payload = strict_json(raw.as_bytes()).map_err(|_| invalid)?;
    let object = payload.as_object().ok_or(invalid)?;
    if object
        .keys()
        .any(|key| !matches!(key.as_str(), "data" | "artifacts"))
    {
        return Err(invalid);
    }
    let data = object.get("data").ok_or(invalid)?;
    let encoded = serde_json::to_vec(data).map_err(|_| invalid)?;
    if encoded.len() > MAX_BUSINESS_BYTES {
        return Err(limit);
    }
    if let Err(error) = validate_value_bounds(data, 0) {
        return Err(if error.contains("limit") {
            limit
        } else {
            ResultError {
                code: "runtime.result_schema",
                message: "The selected producer business data uses an unsupported property name.",
            }
        });
    }
    validate_value(schema, data, 0).map_err(|_| ResultError {
        code: "runtime.result_schema",
        message: "The selected producer business data does not match its schema.",
    })?;
    // Export validation follows business validation so invalid nominations cannot
    // erase a successfully validated business result after a mutation.
    let artifacts = object
        .get("artifacts")
        .cloned()
        .unwrap_or_else(|| Value::Array(vec![]));
    let artifacts =
        serde_json::from_value::<Vec<ArtifactNomination>>(artifacts).map_err(|_| ResultError {
            code: "artifact.export_failed",
            message: "The selected producer artifact nominations are invalid.",
        });
    // The caller records the business data before inspecting this separate set.
    Ok(ProducerResult {
        data: data.clone(),
        artifacts,
    })
}
pub fn validate_nominations(nominations: &[ArtifactNomination]) -> Result<(), String> {
    if nominations.len() > 64 {
        return Err("Artifact nomination limit exceeded.".into());
    }
    let mut seen = BTreeSet::new();
    for item in nominations {
        if item.id.is_empty()
            || item.id.len() > 128
            || item.id.chars().any(char::is_control)
            || !seen.insert(&item.id)
            || item.path.is_empty()
            || item.path.len() > 1024
            || item.scope.is_empty()
            || item.scope.len() > 128
            || item.mime_type.len() > 128
        {
            return Err("Artifact nomination is invalid or exceeds supported limits.".into());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn object_schema() -> Value {
        json!({"type":"object","properties":{"settings":{"type":"object","properties":{"model":{"type":"string"},"config":{"type":["null","null"]}},"required":["model"],"additionalProperties":false}},"required":["settings"],"additionalProperties":false})
    }
    #[test]
    fn strict_json_rejects_duplicates_trailing_and_depth_before_conversion() {
        for raw in [
            br#"{"x":1,"x":2}"#.as_slice(),
            br#"{"x":{"n":1,"n":2}}"#,
            b"{} {}",
        ] {
            assert!(strict_json(raw).is_err());
        }
        assert!(strict_json_bounded(b"[[[0]]]", 100, 2).is_err());
        assert!(strict_json_bounded(b"{}", 1, 8).is_err());
    }
    #[test]
    fn strict_json_keeps_large_signed_and_unsigned_integers() {
        let value = strict_json(br#"[-9223372036854775808,18446744073709551615]"#).unwrap();
        assert_eq!(value[0].as_i64(), Some(i64::MIN));
        assert_eq!(value[1].as_u64(), Some(u64::MAX));
    }
    #[test]
    fn business_names_and_null_round_trip_without_authority_semantics() {
        let schema = json!({"type":"object","properties":{"execution_policy":{"type":"object","properties":{"settings":{"type":"object","properties":{"model":{"type":"string"},"config":{"type":["string","null"]}},"required":["model","config"],"additionalProperties":false}},"required":["settings"],"additionalProperties":false}},"required":["execution_policy"],"additionalProperties":false});
        let data = json!({"execution_policy":{"settings":{"model":"editorial","config":null}}});
        validate_schema(&schema, 0).unwrap();
        validate_value(&schema, &data, 0).unwrap();
        let result =
            decode_producer_result(Some(&json!({"data":data}).to_string()), &schema).unwrap();
        assert_eq!(result.data, data);
    }
    #[test]
    fn business_keys_are_bounded_and_prototype_sensitive_names_fail_at_all_depths() {
        for key in ["", "__proto__", "constructor", "prototype", "line\nfeed"] {
            assert!(validate_business_key(key).is_err());
        }
        assert!(validate_business_key(&"é".repeat(65)).is_err());
        assert!(validate_business_key(&"é".repeat(64)).is_ok());
        assert!(validate_value_bounds(&json!({"safe":{"constructor":0}}), 0).is_err());
        assert!(validate_schema(&json!({"type":"object","properties":{"prototype":{"type":"null"}},"required":[],"additionalProperties":false}),0).is_err());
    }
    #[test]
    fn schema_vocabulary_is_closed_and_requires_typed_collections() {
        for schema in [
            json!({"type":"string","pattern":".*"}),
            json!({"$ref":"remote"}),
            json!({"type":["string","integer"]}),
            json!({"type":"array"}),
            json!({"type":"object","properties":{},"required":[]}),
            object_schema(),
        ] {
            assert!(validate_schema(&schema, 0).is_err());
        }
        for kind in ["string", "integer", "number", "boolean", "null"] {
            validate_schema(&json!({"type":kind}), 0).unwrap();
        }
    }
    #[test]
    fn value_schema_checks_required_unknown_enum_and_bounds() {
        let schema = json!({"type":"object","properties":{"n":{"type":"integer","minimum":1,"maximum":u64::MAX},"tags":{"type":"array","items":{"type":"string","enum":["a","b"]},"minItems":1,"maxItems":2}},"required":["n","tags"],"additionalProperties":false});
        validate_schema(&schema, 0).unwrap();
        validate_value(&schema, &json!({"n":u64::MAX,"tags":["a"]}), 0).unwrap();
        for data in [
            json!({"n":1}),
            json!({"n":0,"tags":["a"]}),
            json!({"n":1,"tags":["c"]}),
            json!({"n":1,"tags":["a"],"extra":true}),
        ] {
            assert!(validate_value(&schema, &data, 0).is_err());
        }
        let integer = json!({"type":"integer","minimum":u64::MAX});
        assert!(validate_value(&integer, &json!(u64::MAX - 1), 0).is_err());
        assert!(validate_value(
            &json!({"type":"integer","exclusiveMinimum":i64::MAX}),
            &json!(i64::MAX),
            0
        )
        .is_err());
    }
    #[test]
    fn selected_results_allow_scalars_arrays_and_wrapped_business_null() {
        for (data, schema) in [
            (json!(true), json!({"type":"boolean"})),
            (json!(null), json!({"type":["string","null"]})),
            (
                json!(["a"]),
                json!({"type":"array","items":{"type":"string"}}),
            ),
        ] {
            let result =
                decode_producer_result(Some(&json!({"data":data}).to_string()), &schema).unwrap();
            assert_eq!(result.data, data);
            assert!(result.artifacts.unwrap().is_empty());
        }
        assert_eq!(
            decode_producer_result(None, &json!({"type":"null"}))
                .unwrap_err()
                .code,
            "runtime.result_missing"
        );
        for raw in [
            "null",
            "{}",
            r#"{"data":1,"data":2}"#,
            r#"{"data":1,"extra":2}"#,
        ] {
            assert_eq!(
                decode_producer_result(Some(raw), &json!({"type":"integer"}))
                    .unwrap_err()
                    .code,
                "runtime.result_invalid"
            );
        }
    }
    #[test]
    fn producer_errors_are_safe_and_typed() {
        let error = decode_producer_result(
            Some(r#"{"data":"private-sentinel"}"#),
            &json!({"type":"integer"}),
        )
        .unwrap_err();
        assert_eq!(error.code, "runtime.result_schema");
        assert!(!error.to_string().contains("private-sentinel"));
        assert_eq!(
            decode_producer_result(
                Some(&"x".repeat(MAX_PAYLOAD_BYTES + 1)),
                &json!({"type":"string"})
            )
            .unwrap_err()
            .code,
            "runtime.result_limit"
        );
        let largest_escaped = json!({"data":"\u{1}".repeat(MAX_BUSINESS_BYTES/6)}).to_string();
        decode_producer_result(Some(&largest_escaped), &json!({"type":"string"})).unwrap();
        let protocol =
            serde_json::to_vec(&json!({"protocol_version":1,"result":largest_escaped})).unwrap();
        assert!(protocol.len() < 1024 * 1024);
        let escaped = json!({"data":"\u{1}".repeat(MAX_BUSINESS_BYTES/6+1)}).to_string();
        assert_eq!(
            decode_producer_result(Some(&escaped), &json!({"type":"string"}))
                .unwrap_err()
                .code,
            "runtime.result_limit"
        );
    }
    #[test]
    fn business_result_survives_malformed_export_nominations() {
        let result = decode_producer_result(
            Some(r#"{"data":7,"artifacts":[{"path":"private"}]}"#),
            &json!({"type":"integer"}),
        )
        .unwrap();
        assert_eq!(result.data, json!(7));
        assert_eq!(result.artifacts.unwrap_err().code, "artifact.export_failed");
    }
    #[test]
    fn structural_and_nomination_limits_include_boundaries() {
        assert!(validate_value_bounds(&json!(vec![0; MAX_ITEMS]), 0).is_ok());
        assert!(validate_value_bounds(&json!(vec![0; MAX_ITEMS + 1]), 0).is_err());
        let mut nested = json!(0);
        for _ in 0..MAX_DEPTH {
            nested = json!([nested]);
        }
        assert!(validate_value_bounds(&nested, 0).is_ok());
        nested = json!([nested]);
        assert!(validate_value_bounds(&nested, 0).is_err());
        let mut map = Map::new();
        for i in 0..MAX_MEMBERS {
            map.insert(format!("f{i}"), json!(0));
        }
        assert!(validate_value_bounds(&Value::Object(map.clone()), 0).is_ok());
        map.insert("extra".into(), json!(0));
        assert!(validate_value_bounds(&Value::Object(map), 0).is_err());
        let nomination = ArtifactNomination {
            id: "a".into(),
            scope: "exports".into(),
            path: "summary.txt".into(),
            mime_type: "text/plain".into(),
        };
        assert!(validate_nominations(std::slice::from_ref(&nomination)).is_ok());
        assert!(validate_nominations(&[nomination.clone(), nomination]).is_err());
    }
}
