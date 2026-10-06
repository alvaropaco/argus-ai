//! LLM provider abstraction and built-in provider catalog (ADR-014).

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelProviderConfig {
    #[serde(default)]
    pub provider: String,
    #[serde(default)]
    pub model: String,
    #[serde(default)]
    pub fallback_models: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    /// Where the provider credential lives — a pointer into the `0600` secret
    /// store (e.g. `argus.secrets.toml`), never the credential itself (FR-010).
    /// Secrets are kept out of the main configuration by construction.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credential_ref: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelInfo {
    pub id: String,
    pub display_name: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProviderDefinition {
    pub id: &'static str,
    pub name: &'static str,
    pub description: &'static str,
    pub default_base_url: Option<&'static str>,
    pub default_model: &'static str,
}

pub struct ProviderCatalog;

impl ProviderCatalog {
    pub const OPENAI: ProviderDefinition = ProviderDefinition {
        id: "openai",
        name: "OpenAI",
        description: "OpenAI API",
        default_base_url: Some("https://api.openai.com/v1"),
        default_model: "gpt-5.6",
    };

    pub const ANTHROPIC: ProviderDefinition = ProviderDefinition {
        id: "anthropic",
        name: "Anthropic",
        description: "Claude API",
        default_base_url: Some("https://api.anthropic.com"),
        default_model: "claude-sonnet",
    };

    pub const OLLAMA: ProviderDefinition = ProviderDefinition {
        id: "ollama",
        name: "Ollama",
        description: "Local Ollama server",
        default_base_url: Some("http://localhost:11434"),
        default_model: "llama3.1",
    };

    pub const LITELLM: ProviderDefinition = ProviderDefinition {
        id: "litellm",
        name: "LiteLLM",
        description: "OpenAI-compatible LLM gateway",
        default_base_url: Some("http://localhost:4000"),
        default_model: "gpt-5.6",
    };

    pub const DEEPSEEK: ProviderDefinition = ProviderDefinition {
        id: "deepseek",
        name: "DeepSeek",
        description: "DeepSeek Platform API",
        default_base_url: Some("https://api.deepseek.com"),
        default_model: "deepseek-flash",
    };

    pub fn all() -> &'static [ProviderDefinition] {
        &[
            Self::OPENAI,
            Self::ANTHROPIC,
            Self::OLLAMA,
            Self::LITELLM,
            Self::DEEPSEEK,
        ]
    }
}

pub trait ModelProvider: Send + Sync {
    fn id(&self) -> &str;
    fn name(&self) -> &str;
    fn list_models(&self) -> Vec<ModelInfo>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_catalog_contains_litellm_and_deepseek() {
        let ids: Vec<_> = ProviderCatalog::all().iter().map(|p| p.id).collect();
        assert!(ids.contains(&"litellm"));
        assert!(ids.contains(&"deepseek"));
    }

    #[test]
    fn deepseek_defaults_are_platform_defaults() {
        assert_eq!(
            ProviderCatalog::DEEPSEEK.default_base_url,
            Some("https://api.deepseek.com")
        );
        assert_eq!(ProviderCatalog::DEEPSEEK.default_model, "deepseek-flash");
    }

    #[test]
    fn config_serde_round_trip() {
        let config = ModelProviderConfig {
            provider: "litellm".into(),
            model: "gpt-5.6".into(),
            fallback_models: vec!["deepseek/deepseek-flash".into()],
            base_url: Some("http://localhost:4000".into()),
            credential_ref: Some("argus.secrets.toml".into()),
        };
        let json = serde_json::to_string(&config).unwrap();
        let back: ModelProviderConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(config, back);
    }

    #[test]
    fn config_without_credential_ref_stays_secret_free() {
        // A configuration without a credential must serialize without any
        // credential-shaped field, and an old file without `credential_ref`
        // still deserializes (FR-010).
        let config = ModelProviderConfig {
            provider: "openai".into(),
            model: "gpt-5.6".into(),
            ..ModelProviderConfig::default()
        };
        let json = serde_json::to_string(&config).unwrap();
        assert!(
            !json.contains("token"),
            "no secret material in config: {json}"
        );
        assert!(!json.contains("credential_ref"));

        let back: ModelProviderConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(back.credential_ref, None);
    }
}
