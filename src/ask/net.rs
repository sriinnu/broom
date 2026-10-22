//! Blocking JSON POST for the ask providers, on a background thread. The API key goes
//! into the Authorization header and nowhere else — never into a log line, never into
//! an error message.

use std::time::Duration;

use super::ProviderError;

/// The wire, as the providers see it. One method, so swapping the real client out in
/// tests is trivial. `key: None` sends no Authorization header at all — Laya's backup
/// route asks for nothing.
pub trait Http: Send + Sync {
    fn post_json(&self, url: &str, key: Option<&str>, body: &serde_json::Value) -> Result<serde_json::Value, ProviderError>;
}

pub struct UreqHttp;

const TIMEOUT: Duration = Duration::from_secs(8);

impl Http for UreqHttp {
    fn post_json(&self, url: &str, key: Option<&str>, body: &serde_json::Value) -> Result<serde_json::Value, ProviderError> {
        // Rate-limit and overloaded answers are worth two quick retries.
        let mut retries = 0;
        loop {
            let mut request = ureq::post(url).timeout(TIMEOUT).set("Content-Type", "application/json");
            if let Some(key) = key {
                request = request.set("Authorization", &format!("Bearer {key}"));
            }
            let attempt = request.send_json(body.clone());
            let error = match attempt {
                Ok(response) => {
                    return response
                        .into_json::<serde_json::Value>()
                        .map_err(|e| ProviderError::BadResponse(short(&e.to_string())))
                }
                Err(error) => error,
            };
            let status = match &error {
                ureq::Error::Status(code, _) => Some(*code),
                ureq::Error::Transport(_) => None,
            };
            if matches!(status, Some(429 | 529)) && retries < 2 {
                std::thread::sleep(Duration::from_millis(if retries == 0 { 300 } else { 900 }));
                retries += 1;
                continue;
            }
            return Err(match status {
                Some(401) => ProviderError::Unauthorized,
                _ => ProviderError::Http(short(&error.to_string())),
            });
        }
    }
}

/// Error strings carry a short excerpt only, so nothing long or sensitive can ride along.
fn short(message: &str) -> String {
    message.chars().take(160).collect()
}
