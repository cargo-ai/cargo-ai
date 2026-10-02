//! Exact Gemini image thinking facts shared by resolution and qualification.
use super::thinking::{ThinkingChoice, ThinkingSupport};

pub(crate) fn gemini_support(raw: &str, model: &str) -> ThinkingSupport {
    let qualified = reqwest::Url::parse(raw).is_ok_and(|url| {
        url.scheme() == "https"
            && url.host_str() == Some("generativelanguage.googleapis.com")
            && url.port_or_known_default() == Some(443)
            && url.path() == "/v1beta/interactions"
            && url.username().is_empty()
            && url.password().is_none()
            && url.query().is_none()
            && url.fragment().is_none()
            && matches!(
                model,
                "gemini-3.1-flash-image" | "gemini-3.1-flash-lite-image"
            )
    });
    if qualified {
        ThinkingSupport::Configurable {
            choices: ["minimal", "high"]
                .into_iter()
                .map(|value| ThinkingChoice {
                    value: value.into(),
                    description: None,
                })
                .collect(),
            default: Some("minimal".into()),
            evidence: Some("https://ai.google.dev/gemini-api/docs/image-generation".into()),
            toggle: None,
        }
    } else {
        ThinkingSupport::Unknown {
            reason: "named thinking support is not qualified for this image endpoint".into(),
            evidence: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn image_facts_require_exact_models_route_and_named_choices() {
        let endpoint = "https://generativelanguage.googleapis.com/v1beta/interactions";
        for model in ["gemini-3.1-flash-image", "gemini-3.1-flash-lite-image"] {
            let ThinkingSupport::Configurable {
                choices, default, ..
            } = gemini_support(endpoint, model)
            else {
                panic!("exact image model must qualify")
            };
            assert_eq!(
                choices.iter().map(|c| c.value.as_str()).collect::<Vec<_>>(),
                ["minimal", "high"]
            );
            assert_eq!(default.as_deref(), Some("minimal"));
        }
        for (url, model) in [
            (endpoint, "gemini-3.1-flash-image-preview"),
            (endpoint, "gemini-3.1-pro-preview"),
            (
                "https://example.org/v1beta/interactions",
                "gemini-3.1-flash-image",
            ),
            (
                "https://generativelanguage.googleapis.com/v1beta/models",
                "gemini-3.1-flash-image",
            ),
            (
                "https://generativelanguage.googleapis.com/v1beta/interactions?key=private",
                "gemini-3.1-flash-image",
            ),
        ] {
            assert!(matches!(
                gemini_support(url, model),
                ThinkingSupport::Unknown { .. }
            ));
        }
    }
}
