//! Local model provider: talks to a local inference server (Ollama, vLLM,
//! LM Studio, llama.cpp's server, or anything else that exposes an
//! OpenAI-compatible `/v1/chat/completions` endpoint) — no API key
//! required by default, though one can be supplied if the server wants
//! one. This is an early, minimal integration; tool-calling quality
//! depends entirely on the local model's own support for it, which varies
//! a lot between models.

use anyhow::Result;
use async_trait::async_trait;

use crate::tools::ToolDefinition;

use super::protocol::OpenAiProtocolClient;
use super::{ChatMessage, ChatResult, ModelCapabilities, Provider, StreamSink};

pub struct LocalProvider {
    client: OpenAiProtocolClient,
    model: String,
    display_name: String,
}

impl LocalProvider {
    pub fn new(display_name: String, mut base_url: String, api_key: String, model: String, temperature: f32, max_tokens: u32) -> Self {
        if !base_url.ends_with("/chat/completions") {
            if !base_url.ends_with('/') {
                base_url.push('/');
            }
            base_url.push_str("v1/chat/completions");
        }

        Self {
            client: OpenAiProtocolClient::new(base_url, api_key, model.clone(), temperature, max_tokens, None),
            model,
            display_name,
        }
    }
}

#[async_trait]
impl Provider for LocalProvider {
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
        // Unknown in general — depends entirely on which local model is
        // actually loaded, which REXO has no way to introspect for most
        // local runtimes. Streaming is nearly universal among
        // OpenAI-compatible local servers, so that much is a safe default.
        ModelCapabilities { streaming: Some(true), ..ModelCapabilities::unknown() }
    }
}
