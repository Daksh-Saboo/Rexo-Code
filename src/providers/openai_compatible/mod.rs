//! Generic provider for any endpoint that speaks the OpenAI-compatible
//! chat-completions protocol: OpenRouter, Together AI, Groq, a self-hosted
//! vLLM/TGI server, etc. Configure with `[model] provider = "openai_compatible"`
//! and `base_url` in `rexo.toml`, via `/connect`/`/provider add`, or via a
//! saved global provider profile — see `config::global`.

use anyhow::Result;
use async_trait::async_trait;

use crate::tools::ToolDefinition;

use super::protocol::OpenAiProtocolClient;
use super::{ChatMessage, ChatResult, ModelCapabilities, Provider, StreamSink};

pub struct OpenAiCompatibleProvider {
    client: OpenAiProtocolClient,
    model: String,
    display_name: String,
}

impl OpenAiCompatibleProvider {
    pub fn new(display_name: String, api_key: String, base_url: String, model: String, temperature: f32, max_tokens: u32) -> Self {
        Self {
            client: OpenAiProtocolClient::new(base_url, api_key, model.clone(), temperature, max_tokens, None),
            model,
            display_name,
        }
    }
}

#[async_trait]
impl Provider for OpenAiCompatibleProvider {
    async fn chat(
        &self,
        messages: &[ChatMessage],
        tools: &[ToolDefinition],
        on_event: Option<&StreamSink<'_>>,
    ) -> Result<ChatResult> {
        self.client.chat(messages, tools, on_event).await
    }

    fn describe(&self) -> String {
        format!("{} ({})", self.display_name, self.model)
    }

    fn capabilities(&self) -> ModelCapabilities {
        // Most OpenAI-compatible endpoints support tool calling and
        // streaming, but not all models behind them actually honor tool
        // calls well — this is a protocol-level default, not a guarantee
        // about the specific model. See `providers::catalog` for
        // per-preset notes where known.
        ModelCapabilities {
            tool_calling: Some(true),
            streaming: Some(true),
            model_discovery: Some(true), // OpenAiProtocolClient::list_models — most, not all, endpoints implement GET /models
            usage_reporting: Some(false),
            ..ModelCapabilities::unknown()
        }
    }
}
