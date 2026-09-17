//! NVIDIA NIM provider (`https://integrate.api.nvidia.com/v1/chat/completions`).
//!
//! NIM speaks the standard OpenAI-compatible chat-completions protocol, so
//! this is a thin configuration wrapper around
//! [`crate::providers::protocol::OpenAiProtocolClient`]. The API key is
//! passed in by the caller (resolved via `Config::api_key` — see
//! `config/mod.rs`) and is never hard-coded here.

use anyhow::Result;
use async_trait::async_trait;

use crate::tools::ToolDefinition;

use super::protocol::OpenAiProtocolClient;
use super::{ChatMessage, ChatResult, ModelCapabilities, Provider, StreamSink};

pub(crate) const NVIDIA_ENDPOINT: &str = "https://integrate.api.nvidia.com/v1/chat/completions";

pub struct NvidiaProvider {
    client: OpenAiProtocolClient,
    model: String,
    display_name: String,
}

impl NvidiaProvider {
    pub fn new(
        display_name: String,
        api_key: String,
        model: String,
        temperature: f32,
        max_tokens: u32,
        reasoning_effort: Option<String>,
    ) -> Self {
        Self {
            client: OpenAiProtocolClient::new(
                NVIDIA_ENDPOINT.to_string(),
                api_key,
                model.clone(),
                temperature,
                max_tokens,
                reasoning_effort,
            ),
            model,
            display_name,
        }
    }
}

#[async_trait]
impl Provider for NvidiaProvider {
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
        ModelCapabilities {
            tool_calling: Some(true),
            streaming: Some(true),
            model_discovery: Some(true), // OpenAiProtocolClient::list_models — backs /model's live picker
            usage_reporting: Some(false), // REXO doesn't read the `usage` field back yet — see roadmap
            ..ModelCapabilities::unknown()
        }
    }
}
