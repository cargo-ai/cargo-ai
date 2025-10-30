use reqwest;
use futures::future::join_all;
use crate::ResourceUrl;

/// Fetch multiple web resources concurrently.
/// Returns a vector of response bodies as strings.
pub async fn fetch_resources_parallel(urls: &[&str]) -> Result<Vec<String>, reqwest::Error> {
    let client = reqwest::Client::new();

    let futures = urls.iter().map(|&url| {
        let client = client.clone();
        async move {
            for attempt in 1..=3 {
                match client.get(url).send().await {
                    Ok(res) => {
                        let res = res.error_for_status()?;
                        return res.text().await;
                    }
                    Err(e) => {
                        if attempt == 3 {
                            eprintln!("WARNING: Failed to fetch URL {} after 3 attempts: {}", url, e);
                            return Err(e);
                        } else {
                            tokio::time::sleep(std::time::Duration::from_millis(500 * attempt)).await;
                        }
                    }
                }
            }
            unreachable!()
        }
    });

    let results = join_all(futures).await;
    results.into_iter().collect()
}

/// Build a formatted data block containing descriptions, URLs, and fetched content.
/// If fetching fails, this function currently skips injection (same as before).
pub async fn build_data_block(resources: &[ResourceUrl]) -> Result<String, reqwest::Error> {
    let mut data_block = String::from("Here are some resources to aid in your response:\n");

    if !resources.is_empty() {
        let urls: Vec<&str> = resources.iter().map(|r| r.url).collect();
        let results = fetch_resources_parallel(&urls).await?;
        for (res, content) in resources.iter().zip(results.iter()) {
            data_block.push_str(&format!(
                "- {} ({}): {}\n",
                res.description, res.url, content.trim()
            ));
        }
    }

    // println!("DEBUG: web resource data block:\n{}", data_block);
    Ok(data_block)
}