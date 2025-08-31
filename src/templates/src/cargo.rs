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
        let conformed_response: Result<T, serde_json::Error> = serde_json::from_str(&response);
        match conformed_response {
            Ok(response) => {
                self.response = Some(response);
                true
            }
            Err(_) => false,
        }
    }

    pub fn get_response(&self) -> Option<T> {
        self.response.clone()
    }
}
