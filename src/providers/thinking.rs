//! Exact provider choices and provider-default fallback, without configuration or I/O.
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum ThinkingSetting<T = String> {
    ProviderDefault,
    Choice { value: T },
}

impl<'de, T: Deserialize<'de>> Deserialize<'de> for ThinkingSetting<T> {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        // An empty struct variant rejects fields that Serde ignores on unit variants.
        #[derive(Deserialize)]
        #[serde(tag = "mode", rename_all = "snake_case", deny_unknown_fields)]
        enum Wire<T> {
            ProviderDefault {},
            Choice { value: T },
        }
        Ok(match Wire::deserialize(deserializer)? {
            Wire::ProviderDefault {} => Self::ProviderDefault,
            Wire::Choice { value } => Self::Choice { value },
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ThinkingChoice {
    pub value: String,
    pub description: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
pub enum ThinkingSupport {
    Configurable {
        choices: Vec<ThinkingChoice>,
        default: Option<String>,
        evidence: Option<String>,
    },
    Unsupported {
        reason: String,
        evidence: Option<String>,
    },
    Unknown {
        reason: String,
        evidence: Option<String>,
    },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ThinkingFallback {
    UnavailableChoice,
    UnsupportedControl,
    UnknownSupport,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ThinkingOutcome {
    pub requested: Option<ThinkingSetting>,
    pub source: String,
    pub support: ThinkingSupport,
    pub applied: Option<String>,
    pub effective: ThinkingSetting,
    pub fallback: Option<ThinkingFallback>,
    pub reason: Option<String>,
}

impl ThinkingOutcome {
    pub fn applied_choice(&self) -> Option<&str> {
        self.applied.as_deref()
    }
    pub fn notice(&self) -> Option<String> {
        let Some(ThinkingSetting::Choice { value }) = &self.requested else {
            return None;
        };
        self.reason.as_ref().map(|reason| format!("Thinking choice {value:?} from {} was not applied: {reason}; using provider default.", self.source))
    }
}

pub fn resolve(
    selection: Option<&ThinkingSetting>,
    source: &str,
    support: ThinkingSupport,
) -> ThinkingOutcome {
    let (applied, reason) = match selection {
        None | Some(ThinkingSetting::ProviderDefault) => (None, None),
        Some(ThinkingSetting::Choice { value }) => match &support {
            ThinkingSupport::Configurable { choices, .. }
                if choices.iter().any(|choice| choice.value == *value) =>
            {
                (Some(value.clone()), None)
            }
            ThinkingSupport::Configurable { .. } => (
                None,
                Some("the selected model does not offer that exact choice".into()),
            ),
            ThinkingSupport::Unsupported { reason, .. } => (
                None,
                Some(format!("named thinking control is unsupported ({reason})")),
            ),
            ThinkingSupport::Unknown { reason, .. } => (
                None,
                Some(format!(
                    "thinking support could not be determined ({reason})"
                )),
            ),
        },
    };
    let fallback = if reason.is_some() {
        Some(match &support {
            ThinkingSupport::Configurable { .. } => ThinkingFallback::UnavailableChoice,
            ThinkingSupport::Unsupported { .. } => ThinkingFallback::UnsupportedControl,
            ThinkingSupport::Unknown { .. } => ThinkingFallback::UnknownSupport,
        })
    } else {
        None
    };
    let effective = applied
        .as_ref()
        .map(|value| ThinkingSetting::Choice {
            value: value.clone(),
        })
        .unwrap_or(ThinkingSetting::ProviderDefault);
    ThinkingOutcome {
        requested: selection.cloned(),
        source: source.to_owned(),
        support,
        applied,
        effective,
        fallback,
        reason,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn support() -> ThinkingSupport {
        ThinkingSupport::Configurable {
            choices: ["low", "medium", "high", "default"]
                .into_iter()
                .map(|value| ThinkingChoice {
                    value: value.into(),
                    description: None,
                })
                .collect(),
            default: None,
            evidence: None,
        }
    }
    #[test]
    fn exact_choices_default_and_fallback_are_distinct() {
        for value in ["max", "HIGH", ""] {
            let setting = ThinkingSetting::Choice {
                value: value.into(),
            };
            let result = resolve(Some(&setting), "profile", support());
            assert_eq!(result.applied_choice(), None);
            assert!(result.notice().unwrap().contains("using provider default"));
        }
        let named = ThinkingSetting::Choice {
            value: "default".into(),
        };
        assert_eq!(
            resolve(Some(&named), "run", support()).applied_choice(),
            Some("default")
        );
        assert_eq!(
            resolve(Some(&ThinkingSetting::ProviderDefault), "run", support()).applied_choice(),
            None
        );
        assert!(resolve(None, "provider", support()).notice().is_none());
    }
    #[test]
    fn tagged_settings_reject_extraneous_fields_and_preserve_generic_choices() {
        for setting in [
            serde_json::json!({"mode":"provider_default","value":"high"}),
            serde_json::json!({"mode":"provider_default","value":null}),
            serde_json::json!({"mode":"provider_default","unknown":true}),
            serde_json::json!({"mode":"choice","value":"high","unknown":true}),
            serde_json::json!({"mode":"choice"}),
            serde_json::json!({"mode":"unknown"}),
        ] {
            assert!(serde_json::from_value::<ThinkingSetting>(setting).is_err());
        }
        let default: ThinkingSetting =
            serde_json::from_value(serde_json::json!({"mode":"provider_default"})).unwrap();
        assert_eq!(default, ThinkingSetting::ProviderDefault);
        let value = serde_json::json!({"mode":"choice","value":{"input":"thinking"}});
        let generic: ThinkingSetting<serde_json::Value> =
            serde_json::from_value(value.clone()).unwrap();
        assert_eq!(serde_json::to_value(generic).unwrap(), value);
    }
    #[test]
    fn unknown_is_not_unsupported_and_settings_are_strict() {
        let setting = ThinkingSetting::Choice {
            value: "high".into(),
        };
        for (support, text) in [
            (
                ThinkingSupport::Unknown {
                    reason: "missing metadata".into(),
                    evidence: None,
                },
                "could not be determined",
            ),
            (
                ThinkingSupport::Unsupported {
                    reason: "boolean control only".into(),
                    evidence: None,
                },
                "unsupported",
            ),
        ] {
            assert!(resolve(Some(&setting), "step", support)
                .notice()
                .unwrap()
                .contains(text));
        }
        assert!(serde_json::from_value::<ThinkingSetting>(
            serde_json::json!({"mode":"provider_default","value":"high"})
        )
        .is_err());
        assert!(serde_json::from_value::<ThinkingSetting>(
            serde_json::json!({"mode":"choice","value":"high","rank":3})
        )
        .is_err());
    }
}
