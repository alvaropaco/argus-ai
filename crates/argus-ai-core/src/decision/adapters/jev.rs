//! JEV ("System One") HTTP adapter (`simple-jev` sidecar or hosted endpoint).

use async_trait::async_trait;
use reqwest::Client;

use crate::decision::error::DecisionError;
use crate::decision::provider::DecisionProvider;
use crate::decision::types::{DecisionRequest, DecisionResponse};

/// A decision engine reached over the JEV `/v1/classifier` protocol.
pub struct JevHttpProvider {
    client: Client,
    base_url: String,
    default_model: Option<String>,
}

impl JevHttpProvider {
    /// `base_url` is the engine root (e.g. `http://127.0.0.1:8000`).
    pub fn new(base_url: impl Into<String>, default_model: Option<String>) -> Self {
        Self {
            client: Client::new(),
            base_url: base_url.into().trim_end_matches('/').to_string(),
            default_model,
        }
    }
}

#[async_trait]
impl DecisionProvider for JevHttpProvider {
    async fn decide(
        &self,
        mut request: DecisionRequest,
    ) -> Result<DecisionResponse, DecisionError> {
        if request.model.is_none() {
            request.model = self.default_model.clone();
        }

        let url = format!("{}/v1/classifier", self.base_url);
        let response = self
            .client
            .post(&url)
            .json(&request)
            .send()
            .await
            .map_err(|e| DecisionError::Unavailable(e.to_string()))?;

        let status = response.status();
        if !status.is_success() {
            return Err(DecisionError::Unavailable(format!(
                "decision engine returned {status}"
            )));
        }

        response
            .json::<DecisionResponse>()
            .await
            .map_err(|e| DecisionError::Invalid(e.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base_url_trims_trailing_slash() {
        let provider = JevHttpProvider::new("http://127.0.0.1:8000/", None);
        assert_eq!(provider.base_url, "http://127.0.0.1:8000");
    }
}
