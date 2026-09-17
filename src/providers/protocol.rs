//! Shared implementation of the OpenAI-compatible `/chat/completions`
//! protocol (used as-is by NVIDIA NIM, and by most other inference
//! providers and local runners like Ollama/vLLM/LM Studio). Individual
//! providers are thin wrappers around [`OpenAiProtocolClient`] that just
//! supply a base URL, auth header, and model name — this keeps the
//! request-building and streaming-response parsing logic in exactly one
//! place instead of duplicated per provider.

use std::time::Duration;

use anyhow::{Context, Result};
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};

use crate::tools::ToolDefinition;

use super::{classify_http_status, classify_reqwest_error, ChatMessage, ChatResult, ProviderError, Role, StreamEvent, StreamSink, ToolCall};

pub struct OpenAiProtocolClient {
    client: reqwest::Client,
    /// Full URL to the chat completions endpoint, e.g.
    /// `https://integrate.api.nvidia.com/v1/chat/completions`.
    endpoint: String,
    api_key: String,
    model: String,
    temperature: f32,
    max_tokens: u32,
    reasoning_effort: Option<String>,
}

impl OpenAiProtocolClient {
    pub fn new(
        endpoint: String,
        api_key: String,
        model: String,
        temperature: f32,
        max_tokens: u32,
        reasoning_effort: Option<String>,
    ) -> Self {
        let client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(20))
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());

        Self {
            client,
            endpoint,
            api_key,
            model,
            temperature,
            max_tokens,
            reasoning_effort,
        }
    }

    pub async fn chat(
        &self,
        messages: &[ChatMessage],
        tools: &[ToolDefinition],
        on_event: Option<&StreamSink<'_>>,
    ) -> Result<ChatResult> {
        let wire_messages: Vec<WireMessage> = messages.iter().map(WireMessage::from).collect();

        let request = WireRequest {
            model: &self.model,
            messages: wire_messages,
            temperature: self.temperature,
            max_tokens: self.max_tokens,
            stream: true,
            tools: if tools.is_empty() { None } else { Some(tools) },
            tool_choice: if tools.is_empty() { None } else { Some("auto") },
            reasoning_effort: self.reasoning_effort.clone(),
        };

        let mut req = self
            .client
            .post(&self.endpoint)
            .header("Accept", "text/event-stream")
            .json(&request);

        if !self.api_key.trim().is_empty() {
            req = req.bearer_auth(&self.api_key);
        }

        let response = req.send().await.map_err(|e| {
            let kind = classify_reqwest_error(&e);
            ProviderError::new(kind, format!("Failed to reach model endpoint at {}: {e}", self.endpoint))
        })?;

        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            let truncated: String = body.chars().take(2000).collect();
            return Err(self.request_error(status, &truncated));
        }

        self.consume_stream(response, on_event).await
    }

    /// Best-effort model listing via the near-universal OpenAI-compatible
    /// `GET {base}/models` endpoint. Works for most providers in
    /// `providers::catalog` (OpenRouter, Groq, Together, Fireworks,
    /// Mistral, DeepSeek, Cerebras, DeepInfra, xAI, OpenAI itself, and
    /// most local OpenAI-compatible servers) since they all implement it;
    /// callers should treat failure as "discovery isn't available here",
    /// not a hard error — `/model` falls back to manual entry either way.
    pub async fn list_models(&self) -> Result<Vec<ModelListing>> {
        let url = self.models_endpoint();
        let mut req = self.client.get(&url);
        if !self.api_key.trim().is_empty() {
            req = req.bearer_auth(&self.api_key);
        }
        let response = req.send().await.map_err(|e| {
            let kind = classify_reqwest_error(&e);
            ProviderError::new(kind, format!("Failed to reach {url}: {e}"))
        })?;
        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(self.request_error(status, &body.chars().take(500).collect::<String>()));
        }
        let parsed: WireModelsResponse = response
            .json()
            .await
            .with_context(|| format!("{url} responded, but not with the model list shape REXO expected"))?;
        let mut models: Vec<ModelListing> =
            parsed.data.into_iter().map(|m| ModelListing { id: m.id, owned_by: m.owned_by }).collect();
        models.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(models)
    }

    fn models_endpoint(&self) -> String {
        for suffix in ["/chat/completions", "chat/completions"] {
            if let Some(base) = self.endpoint.strip_suffix(suffix) {
                return format!("{}/models", base.trim_end_matches('/'));
            }
        }
        format!("{}/models", self.endpoint.trim_end_matches('/'))
    }

    /// A `Provider request failed` error with the endpoint (safe to show —
    /// never the key), status, response body, and a few concrete next
    /// steps, rather than a bare `Model API returned 404 Not Found`.
    /// Classified into a [`super::ProviderErrorKind`] via
    /// [`classify_http_status`] so callers can branch on *why* it failed,
    /// not just display the message — see `provider_error_kind`.
    fn request_error(&self, status: reqwest::StatusCode, body: &str) -> anyhow::Error {
        let kind = classify_http_status(status.as_u16(), body);
        let message = format!(
            "Provider request failed\n\n  Model:    {}\n  Endpoint: {}\n  HTTP {status}{}\n\n\
             Possible causes:\n  - the model ID isn't available on this endpoint\n  \
             - the endpoint/base URL is wrong\n  - the provider needs a different API key or the key has expired\n  \
             - the provider's route is temporarily unavailable\n\n\
             Run /connect to reconnect, or /model to change the model.",
            self.model,
            redact_query(&self.endpoint),
            if body.trim().is_empty() { String::new() } else { format!("\n\n  {}", body.trim()) }
        );
        ProviderError::new(kind, message).into()
    }

    async fn consume_stream(
        &self,
        response: reqwest::Response,
        on_event: Option<&StreamSink<'_>>,
    ) -> Result<ChatResult> {
        let mut byte_stream = response.bytes_stream();
        let mut line_buffer: Vec<u8> = Vec::new();
        let mut content = String::new();
        let mut reasoning = String::new();
        let mut partials: Vec<PartialToolCall> = Vec::new();

        while let Some(chunk) = byte_stream.next().await {
            let chunk = chunk.context("Error while reading model response stream")?;
            line_buffer.extend_from_slice(&chunk);

            // '\n' can never appear as a continuation byte inside a
            // multi-byte UTF-8 sequence, so splitting on it at the byte
            // level before decoding is always safe.
            while let Some(pos) = line_buffer.iter().position(|&b| b == b'\n') {
                let line_bytes: Vec<u8> = line_buffer.drain(..=pos).collect();
                let line = String::from_utf8_lossy(&line_bytes);
                let line = line.trim();

                if line.is_empty() {
                    continue;
                }
                let Some(data) = line.strip_prefix("data:") else {
                    continue;
                };
                let data = data.trim();
                if data == "[DONE]" {
                    continue;
                }

                let parsed: StreamChunk = match serde_json::from_str(data) {
                    Ok(v) => v,
                    Err(_) => continue, // ignore malformed/keep-alive lines
                };

                self.apply_chunk(parsed, &mut content, &mut reasoning, &mut partials, on_event);
            }
        }

        let tool_calls: Vec<ToolCall> = partials
            .into_iter()
            .filter(|p| !p.name.is_empty())
            .map(|p| ToolCall {
                id: if p.id.is_empty() {
                    format!("call_{}", uuid_like())
                } else {
                    p.id
                },
                name: p.name,
                arguments: p.arguments,
            })
            .collect();

        Ok(ChatResult {
            content: if content.is_empty() { None } else { Some(content) },
            reasoning: if reasoning.is_empty() { None } else { Some(reasoning) },
            tool_calls,
        })
    }

    fn apply_chunk(
        &self,
        chunk: StreamChunk,
        content: &mut String,
        reasoning: &mut String,
        partials: &mut Vec<PartialToolCall>,
        on_event: Option<&StreamSink<'_>>,
    ) {
        for choice in chunk.choices {
            // Reasoning models (Kimi K3 always reasons, regardless of
            // reasoning_effort) stream their chain of thought on this
            // separate field before/alongside the real answer. Some
            // OpenAI-compatible backends use `reasoning` instead of
            // `reasoning_content` for the same thing, so accept either.
            let reasoning_delta = choice
                .delta
                .reasoning_content
                .or(choice.delta.reasoning);
            if let Some(text) = reasoning_delta {
                if !text.is_empty() {
                    reasoning.push_str(&text);
                    if let Some(cb) = on_event {
                        cb(StreamEvent::ReasoningDelta(text));
                    }
                }
            }

            if let Some(text) = choice.delta.content {
                if !text.is_empty() {
                    content.push_str(&text);
                    if let Some(cb) = on_event {
                        cb(StreamEvent::TextDelta(text));
                    }
                }
            }

            if let Some(deltas) = choice.delta.tool_calls {
                for delta in deltas {
                    while partials.len() <= delta.index {
                        partials.push(PartialToolCall::default());
                    }
                    let entry = &mut partials[delta.index];

                    if let Some(id) = delta.id {
                        if !id.is_empty() {
                            entry.id = id;
                        }
                    }

                    if let Some(function) = delta.function {
                        if let Some(name) = function.name {
                            if !name.is_empty() && entry.name.is_empty() {
                                entry.name = name.clone();
                                if let Some(cb) = on_event {
                                    cb(StreamEvent::ToolCallStarted { name });
                                }
                            }
                        }
                        if let Some(args) = function.arguments {
                            entry.arguments.push_str(&args);
                        }
                    }
                }
            }
        }
    }
}

/// Strip any query string before showing an endpoint in an error message —
/// some providers pass credentials as a query parameter rather than a
/// header, and this keeps that impossible to leak here even if one does.
fn redact_query(url: &str) -> String {
    match url.split_once('?') {
        Some((base, _)) => format!("{base}?<redacted>"),
        None => url.to_string(),
    }
}

/// One entry from a provider's `GET /models` response.
pub struct ModelListing {
    pub id: String,
    pub owned_by: Option<String>,
}

#[derive(Deserialize)]
struct WireModelsResponse {
    #[serde(default)]
    data: Vec<WireModelListing>,
}

#[derive(Deserialize)]
struct WireModelListing {
    id: String,
    #[serde(default)]
    owned_by: Option<String>,
}

/// Cheap, dependency-free unique-enough id for tool calls whose provider
/// didn't send one. Not cryptographically anything — just needs to be
/// distinct within a single turn.
fn uuid_like() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("{nanos:x}")
}

#[derive(Default)]
struct PartialToolCall {
    id: String,
    name: String,
    arguments: String,
}

// ---- wire (request) types ----------------------------------------------

#[derive(Serialize)]
struct WireRequest<'a> {
    model: &'a str,
    messages: Vec<WireMessage>,
    temperature: f32,
    max_tokens: u32,
    stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<&'a [ToolDefinition]>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_choice: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    reasoning_effort: Option<String>,
}

#[derive(Serialize)]
struct WireMessage {
    role: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    content: Option<String>,
    // Kimi K3 (and similar always-reasoning models) expect prior turns'
    // reasoning to be echoed back for correct multi-turn behavior; harmless
    // to omit for providers that don't use it.
    #[serde(skip_serializing_if = "Option::is_none")]
    reasoning_content: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tool_calls: Vec<WireToolCall>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_call_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    name: Option<String>,
}

#[derive(Serialize)]
struct WireToolCall {
    id: String,
    #[serde(rename = "type")]
    kind: &'static str,
    function: WireFunctionCall,
}

#[derive(Serialize)]
struct WireFunctionCall {
    name: String,
    arguments: String,
}

impl From<&ChatMessage> for WireMessage {
    fn from(msg: &ChatMessage) -> Self {
        let role = match msg.role {
            Role::System => "system",
            Role::User => "user",
            Role::Assistant => "assistant",
            Role::Tool => "tool",
        };

        WireMessage {
            role,
            content: msg.content.clone(),
            reasoning_content: msg.reasoning.clone(),
            tool_calls: msg
                .tool_calls
                .iter()
                .map(|tc| WireToolCall {
                    id: tc.id.clone(),
                    kind: "function",
                    function: WireFunctionCall {
                        name: tc.name.clone(),
                        arguments: tc.arguments.clone(),
                    },
                })
                .collect(),
            tool_call_id: msg.tool_call_id.clone(),
            name: msg.name.clone(),
        }
    }
}

// ---- wire (streaming response) types -----------------------------------

#[derive(Deserialize)]
struct StreamChunk {
    #[serde(default)]
    choices: Vec<StreamChoice>,
}

#[derive(Deserialize, Default)]
struct StreamChoice {
    #[serde(default)]
    delta: StreamDelta,
}

#[derive(Deserialize, Default)]
struct StreamDelta {
    #[serde(default)]
    content: Option<String>,
    #[serde(default)]
    reasoning_content: Option<String>,
    #[serde(default)]
    reasoning: Option<String>,
    #[serde(default)]
    tool_calls: Option<Vec<StreamToolCallDelta>>,
}

#[derive(Deserialize)]
struct StreamToolCallDelta {
    index: usize,
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    function: Option<StreamFunctionDelta>,
}

#[derive(Deserialize, Default)]
struct StreamFunctionDelta {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    arguments: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn empty_client() -> OpenAiProtocolClient {
        OpenAiProtocolClient::new(
            "http://example.invalid/v1/chat/completions".to_string(),
            String::new(),
            "test-model".to_string(),
            0.3,
            1024,
            None,
        )
    }

    #[test]
    fn assembles_text_deltas() {
        let client = empty_client();
        let mut content = String::new();
        let mut reasoning = String::new();
        let mut partials = Vec::new();

        let chunk: StreamChunk = serde_json::from_str(
            r#"{"choices":[{"delta":{"content":"Hello"}}]}"#,
        )
        .unwrap();
        client.apply_chunk(chunk, &mut content, &mut reasoning, &mut partials, None);

        let chunk2: StreamChunk = serde_json::from_str(
            r#"{"choices":[{"delta":{"content":", world"}}]}"#,
        )
        .unwrap();
        client.apply_chunk(chunk2, &mut content, &mut reasoning, &mut partials, None);

        assert_eq!(content, "Hello, world");
    }

    #[test]
    fn assembles_tool_call_from_fragmented_deltas() {
        let client = empty_client();
        let mut content = String::new();
        let mut reasoning = String::new();
        let mut partials = Vec::new();

        let c1: StreamChunk = serde_json::from_str(
            r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_1","function":{"name":"search_files","arguments":""}}]}}]}"#,
        )
        .unwrap();
        client.apply_chunk(c1, &mut content, &mut reasoning, &mut partials, None);

        let c2: StreamChunk = serde_json::from_str(
            r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"{\"query\""}}]}}]}"#,
        )
        .unwrap();
        client.apply_chunk(c2, &mut content, &mut reasoning, &mut partials, None);

        let c3: StreamChunk = serde_json::from_str(
            r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":":\"auth\"}"}}]}}]}"#,
        )
        .unwrap();
        client.apply_chunk(c3, &mut content, &mut reasoning, &mut partials, None);

        assert_eq!(partials.len(), 1);
        assert_eq!(partials[0].id, "call_1");
        assert_eq!(partials[0].name, "search_files");
        assert_eq!(partials[0].arguments, "{\"query\":\"auth\"}");
    }

    #[test]
    fn assembles_reasoning_deltas_separately_from_content() {
        let client = empty_client();
        let mut content = String::new();
        let mut reasoning = String::new();
        let mut partials = Vec::new();

        let c1: StreamChunk = serde_json::from_str(
            r#"{"choices":[{"delta":{"reasoning_content":"Let me think"}}]}"#,
        )
        .unwrap();
        client.apply_chunk(c1, &mut content, &mut reasoning, &mut partials, None);

        let c2: StreamChunk = serde_json::from_str(
            r#"{"choices":[{"delta":{"reasoning_content":" about this."}}]}"#,
        )
        .unwrap();
        client.apply_chunk(c2, &mut content, &mut reasoning, &mut partials, None);

        let c3: StreamChunk =
            serde_json::from_str(r#"{"choices":[{"delta":{"content":"The answer is 42."}}]}"#)
                .unwrap();
        client.apply_chunk(c3, &mut content, &mut reasoning, &mut partials, None);

        assert_eq!(reasoning, "Let me think about this.");
        assert_eq!(content, "The answer is 42.");
    }

    #[test]
    fn wire_message_role_names_match_openai_protocol() {
        let msg = ChatMessage::system("hi");
        let wire = WireMessage::from(&msg);
        assert_eq!(wire.role, "system");
    }
}
