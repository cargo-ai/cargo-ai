//! Exact provider choices and provider-default fallback, without configuration or I/O.
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum ThinkingSetting<T = String> {
    ProviderDefault,
    On,
    Off,
    Choice { value: T },
}

impl ThinkingSetting {
    /// Only the generic Boolean aliases are case insensitive; choices stay exact.
    pub fn from_cli(value: &str) -> Self {
        if value.eq_ignore_ascii_case("on") {
            Self::On
        } else if value.eq_ignore_ascii_case("off") {
            Self::Off
        } else {
            Self::Choice {
                value: value.into(),
            }
        }
    }
}

impl<'de, T: Deserialize<'de>> Deserialize<'de> for ThinkingSetting<T> {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        // An empty struct variant rejects fields that Serde ignores on unit variants.
        #[derive(Deserialize)]
        #[serde(tag = "mode", rename_all = "snake_case", deny_unknown_fields)]
        enum Wire<T> {
            ProviderDefault {},
            On {},
            Off {},
            Choice { value: T },
        }
        Ok(match Wire::deserialize(deserializer)? {
            Wire::ProviderDefault {} => Self::ProviderDefault,
            Wire::On {} => Self::On,
            Wire::Off {} => Self::Off,
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
#[serde(deny_unknown_fields)]
pub struct ThinkingToggle {
    pub values: Vec<bool>,
    pub default: Option<bool>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
pub enum ThinkingSupport {
    Configurable {
        choices: Vec<ThinkingChoice>,
        default: Option<String>,
        evidence: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        toggle: Option<ThinkingToggle>,
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
        match &self.effective {
            ThinkingSetting::Choice { value } => Some(value),
            _ => None,
        }
    }
    /// Boolean controls are qualified only for Ollama's existing compatible route.
    /// Its wire efforts encode true/false rather than a named level on that model.
    pub fn provider_value(&self, ollama: bool) -> Option<&str> {
        match (&self.effective, ollama) {
            (ThinkingSetting::On, true) => Some("medium"),
            (ThinkingSetting::Off, true) => Some("none"),
            _ => self.applied_choice(),
        }
    }
    pub fn notice(&self) -> Option<String> {
        let value = match &self.requested {
            Some(ThinkingSetting::Choice { value }) => value.as_str(),
            Some(ThinkingSetting::On) => "on",
            Some(ThinkingSetting::Off) => "off",
            _ => return None,
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
        Some(ThinkingSetting::On | ThinkingSetting::Off) => {
            let enabled = matches!(selection, Some(ThinkingSetting::On));
            match &support {
                ThinkingSupport::Configurable {
                    toggle: Some(toggle),
                    ..
                } if toggle.values.contains(&enabled) => {
                    (Some(if enabled { "on" } else { "off" }.into()), None)
                }
                ThinkingSupport::Configurable { .. } => (
                    None,
                    Some("the selected model does not offer that Boolean thinking control".into()),
                ),
                ThinkingSupport::Unsupported { reason, .. } => (
                    None,
                    Some(format!("thinking control is unsupported ({reason})")),
                ),
                ThinkingSupport::Unknown { reason, .. } => (
                    None,
                    Some(format!(
                        "thinking support could not be determined ({reason})"
                    )),
                ),
            }
        }
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
                Some(format!("thinking control is unsupported ({reason})")),
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
    let effective = if applied.is_some() {
        selection
            .cloned()
            .unwrap_or(ThinkingSetting::ProviderDefault)
    } else {
        ThinkingSetting::ProviderDefault
    };
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

/// Native authority selects an exact wire value; advisory metadata grants nothing.
pub fn resolve_native(
    selection: Option<&ThinkingSetting>,
    source: &str,
    support: ThinkingSupport,
) -> ThinkingOutcome {
    let effective = selection
        .cloned()
        .unwrap_or(ThinkingSetting::ProviderDefault);
    let applied = match &effective {
        ThinkingSetting::ProviderDefault => None,
        ThinkingSetting::Choice { value } => Some(value.clone()),
        ThinkingSetting::On => Some("on".into()),
        ThinkingSetting::Off => Some("off".into()),
    };
    ThinkingOutcome {
        requested: selection.cloned(),
        source: source.to_owned(),
        support,
        applied,
        effective,
        fallback: None,
        reason: None,
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
            toggle: None,
        }
    }
    #[test]
    fn boolean_aliases_are_distinct_from_exact_choices_and_default() {
        for value in ["on", "ON", "On"] {
            assert_eq!(ThinkingSetting::from_cli(value), ThinkingSetting::On);
        }
        for value in ["off", "OFF", "Off"] {
            assert_eq!(ThinkingSetting::from_cli(value), ThinkingSetting::Off);
        }
        assert_eq!(
            ThinkingSetting::from_cli("HIGH"),
            ThinkingSetting::Choice {
                value: "HIGH".into()
            }
        );
        for mode in ["on", "off"] {
            let setting: ThinkingSetting =
                serde_json::from_value(serde_json::json!({"mode":mode})).unwrap();
            assert_eq!(
                serde_json::to_value(setting).unwrap(),
                serde_json::json!({"mode":mode})
            );
            assert!(serde_json::from_value::<ThinkingSetting>(
                serde_json::json!({"mode":mode,"value":true})
            )
            .is_err());
        }
        let named: ThinkingSetting =
            serde_json::from_value(serde_json::json!({"mode":"choice","value":"on"})).unwrap();
        assert_ne!(named, ThinkingSetting::On);
        let support = ThinkingSupport::Configurable {
            choices: vec![],
            default: None,
            evidence: None,
            toggle: Some(ThinkingToggle {
                values: vec![true],
                default: Some(true),
            }),
        };
        let on = resolve(Some(&ThinkingSetting::On), "run", support.clone());
        assert_eq!(on.effective, ThinkingSetting::On);
        assert_eq!(on.applied.as_deref(), Some("on"));
        assert_eq!(on.provider_value(true), Some("medium"));
        assert_eq!(on.provider_value(false), None);
        assert_eq!(on.applied_choice(), None);
        let off = resolve(Some(&ThinkingSetting::Off), "profile", support.clone());
        assert_eq!(off.effective, ThinkingSetting::ProviderDefault);
        assert!(off.notice().unwrap().contains("provider default"));
        assert!(resolve(Some(&named), "step", support).applied.is_none());
        for setting in [ThinkingSetting::On, ThinkingSetting::Off] {
            let result = resolve(Some(&setting), "run", self::support());
            assert!(
                result.applied.is_none(),
                "named choices do not prove a toggle"
            );
            assert!(result.notice().is_some());
        }
    }

    #[test]
    fn native_choices_are_exact_attempts_independent_of_advisory_support() {
        for metadata in [
            support(),
            ThinkingSupport::Unknown {
                reason: "missing".into(),
                evidence: None,
            },
            ThinkingSupport::Unsupported {
                reason: "not listed".into(),
                evidence: Some("advisory".into()),
            },
        ] {
            for value in ["high", "Custom-Attempt", "default", "on"] {
                let selection = ThinkingSetting::Choice {
                    value: value.into(),
                };
                let outcome = resolve_native(Some(&selection), "native_role", metadata.clone());
                assert_eq!(outcome.effective, selection);
                assert_eq!(outcome.applied_choice(), Some(value));
                assert_eq!(outcome.support, metadata);
                assert!(outcome.fallback.is_none());
                assert!(outcome.notice().is_none());
            }
            let default = resolve_native(
                Some(&ThinkingSetting::ProviderDefault),
                "native_role",
                metadata,
            );
            assert_eq!(default.provider_value(false), None);
            assert!(default.fallback.is_none());
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
