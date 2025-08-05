use serde::{Deserialize, Serialize}; // Data format (e.g.,JSON, TOML) (de)serialization
use serde_json;

#[derive(Debug)]
pub struct Cargo<T: for<'de> Deserialize<'de> + Serialize + Clone> {
    prompt: String,
    context: String,
    samples: Vec<T>,
    response: Option<T>,
} // TODO: Hints

// We use `for<'de>` to tell the compiler how long any borrowed data inside T
// must stay valid during deserialization. This annotation only guides the compiler;
// it does not tie that lifetime to the entire struct. 
impl<T: for<'de> Deserialize<'de> + Serialize + Clone> Cargo<T> {
    pub fn new(prompt: String, context: String, samples: Vec<T>) -> Self {
        Cargo {
            prompt,
            context,
            samples,
            response: None,
        }
    }

    pub fn prompt(&self) -> String {
        let context = format!("For context {} \n", self.context);
        let prompt = format!("User Prompt: {} \n", self.prompt);

        let mut sample_jsons: Vec<String> = Vec::new();
        for sample in &self.samples {
            let sample_json = serde_json::to_string(&sample);
            if let Ok(sample_json) = sample_json {
                sample_jsons.push(sample_json);
            }
        }

        let sample_jsons = {
            let mut sample_jsons_string = 
                String::from("Ensure your JSON has all of the key and value fields specified in the example(s) below. Return a percision point if a number even if a whole number, i.e. 4.0\n Samples:\n");
            for (i, sample_json) in sample_jsons.iter().enumerate() {
                let header = format!("Sample {i} JSON\n");
                let body = format!("{sample_json}\n");
                sample_jsons_string.push_str(&header);
                sample_jsons_string.push_str(&body);
            }
            sample_jsons_string
        };

        let prompt = format!("{context}{prompt}{sample_jsons}");
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
