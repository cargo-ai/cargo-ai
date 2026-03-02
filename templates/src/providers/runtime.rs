use serde::{Deserialize, Serialize}; // Data format (e.g.,JSON, TOML) (de)serialization
use serde_json;

#[derive(Debug)]
pub struct Cargo<T: for<'de> Deserialize<'de> + Serialize + Clone> {
    prompt: String,
    context: String,
    response: Option<T>,
} // TODO: Hints

// We use `for<'de>` to tell the compiler how long any borrowed data inside T
// must stay valid during deserialization. This annotation only guides the compiler;
// it does not tie that lifetime to the entire struct.
impl<T: for<'de> Deserialize<'de> + Serialize + Clone> Cargo<T> {
    pub fn new(prompt: String, context: String) -> Self {
        Cargo {
            prompt,
            context,
            response: None,
        }
    }

    pub fn prompt(&self) -> String {
        let context = format!("For context {} \n", self.context);
        let prompt = format!("User Prompt: {} \n", self.prompt);

        let prompt = format!("{context}{prompt}");
        prompt
    }

    pub fn set_response(&mut self, response: String) -> bool {
        match serde_json::from_str(&response) {
            Ok(response) => {
                self.response = Some(response);
                true
            }
            Err(_) => {
                // Keep state deterministic: failed parse must not retain stale success output.
                self.response = None;
                false
            }
        }
    }

    pub fn get_response(&self) -> Option<T> {
        self.response.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::Cargo;
    use serde::{Deserialize, Serialize};

    #[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
    struct SampleOutput {
        answer: i32,
    }

    #[test]
    fn set_response_stores_successful_parse() {
        let mut cargo = Cargo::<SampleOutput>::new("prompt".to_string(), "context".to_string());

        assert!(cargo.set_response(r#"{"answer":4}"#.to_string()));
        assert_eq!(cargo.get_response(), Some(SampleOutput { answer: 4 }));
    }

    #[test]
    fn set_response_failure_clears_previous_response() {
        let mut cargo = Cargo::<SampleOutput>::new("prompt".to_string(), "context".to_string());

        assert!(cargo.set_response(r#"{"answer":4}"#.to_string()));
        assert_eq!(cargo.get_response(), Some(SampleOutput { answer: 4 }));

        assert!(!cargo.set_response("not-json".to_string()));
        assert_eq!(cargo.get_response(), None);
    }
}
