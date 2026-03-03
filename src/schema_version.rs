//! Canonical local schema-version helpers for agent definitions.
//!
//! Format contract: `YYYY-MM-DD.rN` (example: `2026-03-03.r1`).

use serde_json::Value;

const ROOT_AGENTCFG: &str = include_str!("../.agentcfg");

pub const SCHEMA_VERSION_EXAMPLE: &str = "2026-03-03.r1";

pub fn current_schema_version() -> String {
    extract_schema_version_from_agentcfg(ROOT_AGENTCFG)
        .unwrap_or_else(|| SCHEMA_VERSION_EXAMPLE.to_string())
}

pub fn extract_schema_version_from_agentcfg(agentcfg_contents: &str) -> Option<String> {
    serde_json::from_str::<Value>(agentcfg_contents)
        .ok()
        .and_then(|json| {
            json.get("version")
                .and_then(|value| value.as_str())
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToString::to_string)
        })
        .filter(|value| is_valid_schema_version(value))
}

pub fn is_valid_schema_version(value: &str) -> bool {
    let Some((date, revision)) = value.split_once(".r") else {
        return false;
    };

    if !is_valid_date_prefix(date) {
        return false;
    }

    if revision.is_empty() || !revision.chars().all(|ch| ch.is_ascii_digit()) {
        return false;
    }

    revision.parse::<u32>().ok().filter(|n| *n > 0).is_some()
}

fn is_valid_date_prefix(value: &str) -> bool {
    let bytes = value.as_bytes();
    if bytes.len() != 10 {
        return false;
    }

    if bytes[4] != b'-' || bytes[7] != b'-' {
        return false;
    }

    let year = match value[0..4].parse::<u32>() {
        Ok(year) => year,
        Err(_) => return false,
    };
    let month = match value[5..7].parse::<u32>() {
        Ok(month) => month,
        Err(_) => return false,
    };
    let day = match value[8..10].parse::<u32>() {
        Ok(day) => day,
        Err(_) => return false,
    };

    if !(1..=12).contains(&month) {
        return false;
    }

    let max_day = match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => {
            if is_leap_year(year) {
                29
            } else {
                28
            }
        }
        _ => return false,
    };

    (1..=max_day).contains(&day)
}

fn is_leap_year(year: u32) -> bool {
    (year % 4 == 0 && year % 100 != 0) || year % 400 == 0
}

#[cfg(test)]
mod tests {
    use super::{
        current_schema_version, extract_schema_version_from_agentcfg, is_valid_schema_version,
        SCHEMA_VERSION_EXAMPLE,
    };

    #[test]
    fn accepts_expected_schema_version_format() {
        assert!(is_valid_schema_version("2026-03-03.r1"));
        assert!(is_valid_schema_version("2024-02-29.r3"));
        assert!(is_valid_schema_version("2030-12-31.r12"));
    }

    #[test]
    fn rejects_legacy_or_invalid_schema_version_values() {
        assert!(!is_valid_schema_version("0.0.10"));
        assert!(!is_valid_schema_version("2026-03-03"));
        assert!(!is_valid_schema_version("2026-13-03.r1"));
        assert!(!is_valid_schema_version("2025-02-29.r1"));
        assert!(!is_valid_schema_version("2026-03-03.r0"));
        assert!(!is_valid_schema_version("2026-03-03.rX"));
    }

    #[test]
    fn extracts_only_valid_schema_versions_from_agentcfg() {
        let valid = extract_schema_version_from_agentcfg(
            r#"{"version":"2026-03-03.r2","prompt":"x","agent_schema":{"type":"object","properties":{}},"resource_urls":[],"actions":[]}"#,
        );
        assert_eq!(valid.as_deref(), Some("2026-03-03.r2"));

        let invalid = extract_schema_version_from_agentcfg(
            r#"{"version":"0.0.10","prompt":"x","agent_schema":{"type":"object","properties":{}},"resource_urls":[],"actions":[]}"#,
        );
        assert!(invalid.is_none());
    }

    #[test]
    fn current_schema_version_is_valid_date_revision_value() {
        let current = current_schema_version();
        assert!(is_valid_schema_version(&current));
        assert_eq!(current, SCHEMA_VERSION_EXAMPLE);
    }
}
