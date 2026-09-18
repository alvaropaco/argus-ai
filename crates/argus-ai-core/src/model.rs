//! LLM provider abstraction (ADR-014).
//!
//! Model providers are replaceable infrastructure dependencies, never part of
//! the core domain model. This module defines the non-secret provider/model
//! selection and the trait boundary that concrete vendor adapters implement.

use serde::{Deserialize, Serialize};

/// Non-secret configuration for a single model-provider selection.
///
/// Secrets (API tokens/keys) are deliberately **not** part of this type: they
/// live in a separate secret store (see the `argus-secrets` boundary) so they
/// can never leak into config, logs, or the operational state store.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelProviderConfig {
    /// Provider identifier (e.g. `openai`, `anthropic`, `ollama`), resolved
    /// against the registered provider adapters.
    pub provider: String,
    /// Primary model identifier for the provider.
    pub model: String,
    /// Optional fallback models, tried in order.
    #[serde(default)]
    pub fallback_models: Vec<String>,
    /// Optional base-URL override (e.g. a local Ollama endpoint).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
}

/// Metadata about a single model exposed by a provider.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelInfo {
    pub id: String,
    pub display_name: String,
}

/// The boundary implemented by concrete model-provider adapters (ADR-014).
///
/// The core depends only on this trait; a specific vendor (OpenAI, Anthropic,
/// Ollama, …) is an adapter and never part of the core domain.
pub trait ModelProvider: Send + Sync {
    /// Stable provider identifier matching [`ModelProviderConfig::provider`].
    fn id(&self) -> &str;

    /// Human-readable provider name.
    fn name(&self) -> &str;

    /// Models this provider can serve.
    fn list_models(&self) -> Vec<ModelInfo>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_serde_round_trip() {
        let config = ModelProviderConfig {
            provider: "openai".into(),
            model: "gpt-4.1".into(),
            fallback_models: vec!["gpt-4.1-mini".into()],
            base_url: Some("https://api.example.com/v1".into()),
        };
        let json = serde_json::to_string(&config).unwrap();
        let back: ModelProviderConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(config, back);
    }

    #[test]
    fn config_defaults_to_empty_selection() {
        let config = ModelProviderConfig::default();
        assert!(config.provider.is_empty());
        assert!(config.model.is_empty());
        assert!(config.fallback_models.is_empty());
        assert!(config.base_url.is_none());
    }

    #[test]
    fn base_url_is_omitted_from_toml_when_none() {
        let config = ModelProviderConfig {
            provider: "ollama".into(),
            model: "llama3.1".into(),
            fallback_models: vec![],
            base_url: None,
        };
        let toml = toml::to_string(&config).unwrap();
        assert!(!toml.contains("base_url"));
        let back: ModelProviderConfig = toml::from_str(&toml).unwrap();
        assert_eq!(config, back);
    }

    #[test]
    fn trait_is_object_safe_for_adapters() {
        struct Ollama;
        impl ModelProvider for Ollama {
            fn id(&self) -> &str {
                "ollama"
            }
            fn name(&self) -> &str {
                "Ollama (local)"
            }
            fn list_models(&self) -> Vec<ModelInfo> {
                vec![ModelInfo {
                    id: "llama3.1".into(),
                    display_name: "Llama 3.1".into(),
                }]
            }
        }

        let providers: Vec<Box<dyn ModelProvider>> = vec![Box::new(Ollama)];
        assert_eq!(providers[0].id(), "ollama");
        assert_eq!(providers[0].list_models().len(), 1);
    }
}
