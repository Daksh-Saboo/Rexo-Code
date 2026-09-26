//! Native Anthropic provider — talks to the real Messages API
//! (`POST /v1/messages`), not the OpenAI-compatible shape. Same reason
//! `gemini/mod.rs` exists as its own module rather than routing through
//! `openai_compatible`: the auth header, request body, and streaming
//! event shape are all genuinely different from OpenAI's.
//!
//! - **Auth**: `x-api-key: <key>` plus a required `anthropic-version`
//!   header — not `Authorization: Bearer`.
//! - **System prompt**: a top-level `system` string field, not a message
//!   with `role: "system"` — `messages` only ever contains `user`/
//!   `assistant` turns, same shape choice Gemini makes with
//!   `systemInstruction`, different mechanism.
//! - **Content is always a block array**, even for plain text: a user
//!   turn is `{"role": "user", "content": [{"type": "text", "text":
//!   "..."}]}`, not a bare string. Assistant turns mix `text` and
//!   `tool_use` blocks in one array; tool *results* go back as a `user`
//!   turn containing `tool_result` blocks (one per call from the
//!   preceding assistant turn) — same "no separate tool role, roll
//!   results into the next user turn" pattern Gemini uses, different
//!   block shape and, unlike Gemini, Anthropic *does* give tool calls
//!   real IDs (`tool_use.id`), echoed back via `tool_result.tool_use_id`
//!   — REXO doesn't need to synthesize its own the way it does for
//!   Gemini's `call_0`/`call_1`.
//! - **Tool schema**: `input_schema`, not `parameters` — otherwise the
//!   same JSON Schema shape REXO's tools already produce, no stripping
//!   needed (unlike Gemini's restricted subset, which rejects
//!   `additionalProperties`).
//!
//! **Streaming**: SSE (`"stream": true`), one JSON object per `data:`
//! line, each carrying its own `"type"` (`message_start`,
//! `content_block_start`, `content_block_delta`, `content_block_stop`,
//! `message_delta`, `message_stop`) — dispatched purely off that field,
//! same as the `event:` line it duplicates, so [`apply_stream_event`]
//! never needs to track the SSE event name separately.
//! `content_block_delta` carries either a `text_delta` (`text`) or an
//! `input_json_delta` (`partial_json`, a fragment of the tool call's
//! arguments streamed incrementally — REXO buffers these per block index
//! and only parses the assembled JSON once the block closes, since a
//! partial JSON fragment isn't valid JSON on its own).
//!
//! **Genuinely untested against the live API**, same caveat as Gemini's:
//! this build sandbox can't reach `api.anthropic.com` over the network
//! REXO's own HTTP client would use for a real key (it's not on the
//! sandbox's outbound allowlist for arbitrary traffic — only this
//! project's own tooling gets network access, not the binary under
//! test). What's tested here is request-building and chunk-assembly
//! against hand-written payloads matching Anthropic's documented format,
//! not an end-to-end call. Worth confirming against a real key.

use anyhow::{Context, Result};
use async_trait::async_trait;
use futures_util::StreamExt;
use serde_json::{json, Value};
use std::collections::HashMap;

use crate::tools::ToolDefinition;

use super::{classify_http_status, classify_reqwest_error, ChatMessage, ChatResult, ModelCapabilities, Provider, ProviderError, Role, StreamEvent, StreamSink, ToolCall};

pub(crate) const ANTHROPIC_ENDPOINT: &str = "https://api.anthropic.com";
const ANTHROPIC_VERSION: &str = "2023-06-01";

pub struct AnthropicProvider {
    http: reqwest::Client,
    api_key: String,
    model: String,
    display_name: String,
    temperature: f32,
    max_tokens: u32,
    base_url: String,
}

impl AnthropicProvider {
    pub fn new(display_name: String, api_key: String, model: String, temperature: f32, max_tokens: u32, base_url: Option<String>) -> Self {
        Self {
            http: reqwest::Client::new(),
            api_key,
            model,
            display_name,
            temperature,
            max_tokens,
            base_url: base_url.unwrap_or_else(|| ANTHROPIC_ENDPOINT.to_string()),
        }
    }

    fn messages_url(&self) -> String {
        format!("{}/v1/messages", self.base_url.trim_end_matches('/'))
    }
}

#[async_trait]
impl Provider for AnthropicProvider {
    async fn chat(&self, messages: &[ChatMessage], tools: &[ToolDefinition], on_event: Option<&StreamSink<'_>>) -> Result<ChatResult> {
        let body = build_request(messages, tools, self.temperature, self.max_tokens, &self.model);

        let response = self
            .http
            .post(self.messages_url())
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", ANTHROPIC_VERSION)
            .header("Content-Type", "application/json")
            .header("Accept", "text/event-stream")
            .json(&body)
            .send()
            .await
            .map_err(|e| {
                let kind = classify_reqwest_error(&e);
                ProviderError::new(kind, format!("Anthropic request failed (network): {e}"))
            })?;

        let status = response.status();
        if !status.is_success() {
            let text = response.text().await.unwrap_or_default();
            return Err(describe_error(status.as_u16(), &text, &self.model, &self.messages_url()));
        }

        consume_anthropic_stream(response, on_event).await
    }

    fn describe(&self) -> String {
        format!("{} ({})", self.display_name, self.model)
    }

    fn capabilities(&self) -> ModelCapabilities {
        ModelCapabilities {
            tool_calling: Some(true),
            streaming: Some(true),
            vision: Some(true),
            reasoning: None, // extended thinking exists but isn't wired up here yet
            model_discovery: Some(false),
            usage_reporting: Some(false), // `usage` is in the wire response but REXO doesn't read it back yet, same gap as Gemini's
            ..ModelCapabilities::unknown()
        }
    }
}

/// Translate REXO's provider-agnostic history into Anthropic's
/// `system` + `messages` shape. Consecutive `Role::Tool` messages are
/// merged into one `user`-role turn with multiple `tool_result` blocks —
/// required by the API (every `tool_use` in an assistant turn needs a
/// matching `tool_result` in the *next* message, and Anthropic rejects a
/// turn that mixes `tool_result` blocks with anything from a different
/// assistant turn), same merge [`super::gemini`]'s `build_request` does
/// for its own, differently-shaped reason.
fn build_request(messages: &[ChatMessage], tools: &[ToolDefinition], temperature: f32, max_tokens: u32, model: &str) -> Value {
    let mut system_text = String::new();
    let mut turns: Vec<Value> = Vec::new();

    for msg in messages {
        match msg.role {
            Role::System => {
                if let Some(text) = &msg.content {
                    if !system_text.is_empty() {
                        system_text.push_str("\n\n");
                    }
                    system_text.push_str(text);
                }
            }
            Role::User => {
                let mut blocks = Vec::new();
                if let Some(text) = &msg.content {
                    if !text.is_empty() {
                        blocks.push(json!({"type": "text", "text": text}));
                    }
                }
                for img in &msg.images {
                    blocks.push(json!({
                        "type": "image",
                        "source": {"type": "base64", "media_type": img.mime, "data": img.base64_data}
                    }));
                }
                if !blocks.is_empty() {
                    turns.push(json!({"role": "user", "content": blocks}));
                }
            }
            Role::Assistant => {
                let mut blocks = Vec::new();
                if let Some(text) = &msg.content {
                    if !text.is_empty() {
                        blocks.push(json!({"type": "text", "text": text}));
                    }
                }
                for call in &msg.tool_calls {
                    let input: Value = serde_json::from_str(&call.arguments).unwrap_or_else(|_| json!({}));
                    blocks.push(json!({"type": "tool_use", "id": call.id, "name": call.name, "input": input}));
                }
                if !blocks.is_empty() {
                    turns.push(json!({"role": "assistant", "content": blocks}));
                }
            }
            Role::Tool => {
                let block = json!({
                    "type": "tool_result",
                    "tool_use_id": msg.tool_call_id.clone().unwrap_or_default(),
                    "content": msg.content.clone().unwrap_or_default(),
                });
                if let Some(last) = turns.last_mut() {
                    if last.get("role").and_then(Value::as_str) == Some("user") && is_tool_result_turn(last) {
                        if let Some(content) = last.get_mut("content").and_then(Value::as_array_mut) {
                            content.push(block);
                            continue;
                        }
                    }
                }
                turns.push(json!({"role": "user", "content": [block]}));
            }
        }
    }

    let mut req = json!({
        "model": model,
        "max_tokens": max_tokens,
        "temperature": temperature,
        "messages": turns,
        "stream": true,
    });
    if !system_text.is_empty() {
        req["system"] = json!(system_text);
    }
    if !tools.is_empty() {
        let tool_defs: Vec<Value> = tools
            .iter()
            .map(|t| json!({"name": t.function.name, "description": t.function.description, "input_schema": t.function.parameters}))
            .collect();
        req["tools"] = json!(tool_defs);
    }
    req
}

fn is_tool_result_turn(turn: &Value) -> bool {
    turn.get("content")
        .and_then(Value::as_array)
        .and_then(|blocks| blocks.first())
        .and_then(|b| b.get("type"))
        .and_then(Value::as_str)
        == Some("tool_result")
}

/// Read the SSE response body as it arrives. `content_block_start`
/// records each block's type (and, for `tool_use`, its id/name) by
/// index; `content_block_delta` appends text or partial-JSON fragments
/// into a per-index buffer; `content_block_stop` finalizes a `tool_use`
/// block by parsing its accumulated JSON — only then, since
/// `input_json_delta` fragments aren't individually valid JSON.
async fn consume_anthropic_stream(response: reqwest::Response, on_event: Option<&StreamSink<'_>>) -> Result<ChatResult> {
    let mut byte_stream = response.bytes_stream();
    let mut line_buffer: Vec<u8> = Vec::new();
    let mut content = String::new();
    let mut tool_calls: Vec<ToolCall> = Vec::new();
    let mut block_kinds: HashMap<u64, BlockKind> = HashMap::new();

    while let Some(chunk) = byte_stream.next().await {
        let chunk = chunk.context("Error while reading Anthropic response stream")?;
        line_buffer.extend_from_slice(&chunk);

        for data in drain_sse_data_lines(&mut line_buffer) {
            let parsed: Value = match serde_json::from_str(&data) {
                Ok(v) => v,
                Err(_) => continue, // malformed/keep-alive line — same policy as gemini/protocol
            };
            apply_stream_event(&parsed, &mut content, &mut tool_calls, &mut block_kinds, on_event);
        }
    }

    Ok(ChatResult {
        content: if content.is_empty() { None } else { Some(content) },
        reasoning: None,
        tool_calls,
    })
}

/// What a content block turned out to be, tracked by index between
/// `content_block_start` and `content_block_stop` — `Text` deltas are
/// applied straight to the running `content` string; `ToolUse` deltas
/// accumulate into `partial_json` until the block closes.
enum BlockKind {
    Text,
    ToolUse { id: String, name: String, partial_json: String },
}

fn apply_stream_event(event: &Value, content: &mut String, tool_calls: &mut Vec<ToolCall>, blocks: &mut HashMap<u64, BlockKind>, on_event: Option<&StreamSink<'_>>) {
    let Some(event_type) = event.get("type").and_then(Value::as_str) else { return };

    match event_type {
        "content_block_start" => {
            let Some(index) = event.get("index").and_then(Value::as_u64) else { return };
            let Some(block) = event.get("content_block") else { return };
            match block.get("type").and_then(Value::as_str) {
                Some("text") => {
                    blocks.insert(index, BlockKind::Text);
                }
                Some("tool_use") => {
                    let id = block.get("id").and_then(Value::as_str).unwrap_or_default().to_string();
                    let name = block.get("name").and_then(Value::as_str).unwrap_or_default().to_string();
                    if let Some(sink) = on_event {
                        sink(StreamEvent::ToolCallStarted { name: name.clone() });
                    }
                    blocks.insert(index, BlockKind::ToolUse { id, name, partial_json: String::new() });
                }
                _ => {}
            }
        }
        "content_block_delta" => {
            let Some(index) = event.get("index").and_then(Value::as_u64) else { return };
            let Some(delta) = event.get("delta") else { return };
            match delta.get("type").and_then(Value::as_str) {
                Some("text_delta") => {
                    if let Some(text) = delta.get("text").and_then(Value::as_str) {
                        content.push_str(text);
                        if let Some(sink) = on_event {
                            sink(StreamEvent::TextDelta(text.to_string()));
                        }
                    }
                }
                Some("input_json_delta") => {
                    if let Some(BlockKind::ToolUse { partial_json, .. }) = blocks.get_mut(&index) {
                        if let Some(fragment) = delta.get("partial_json").and_then(Value::as_str) {
                            partial_json.push_str(fragment);
                        }
                    }
                }
                _ => {}
            }
        }
        "content_block_stop" => {
            let Some(index) = event.get("index").and_then(Value::as_u64) else { return };
            if let Some(BlockKind::ToolUse { id, name, partial_json }) = blocks.remove(&index) {
                let arguments = if partial_json.trim().is_empty() { "{}".to_string() } else { partial_json };
                tool_calls.push(ToolCall { id, name, arguments });
            }
        }
        _ => {}
    }
}

/// Drain every complete `\n`-terminated line out of `buffer`, returning
/// the trimmed payload of each `data: ...` line — identical logic to
/// `gemini::drain_sse_data_lines` (kept as its own copy rather than
/// shared: the two providers' SSE framing is coincidentally identical
/// today, but nothing guarantees it stays that way, and a shared helper
/// would tempt a future change to one provider to silently affect the
/// other).
fn drain_sse_data_lines(buffer: &mut Vec<u8>) -> Vec<String> {
    let mut out = Vec::new();
    while let Some(pos) = buffer.iter().position(|&b| b == b'\n') {
        let line_bytes: Vec<u8> = buffer.drain(..=pos).collect();
        let line = String::from_utf8_lossy(&line_bytes);
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Some(data) = line.strip_prefix("data:") else {
            continue;
        };
        let data = data.trim();
        if !data.is_empty() {
            out.push(data.to_string());
        }
    }
    out
}

fn describe_error(status: u16, body: &str, model: &str, url: &str) -> anyhow::Error {
    let hint = match status {
        401 => "the API key is missing or invalid",
        403 => "the key doesn't have access to this model/organization",
        404 => "the model ID isn't available at this endpoint — check it against Anthropic's current model list",
        413 => "the request was too large",
        429 => "rate limited",
        529 => "Anthropic's API is temporarily overloaded — safe to retry",
        _ => "see the response body below",
    };
    let kind = classify_http_status(status, body);
    let message = format!("Anthropic request failed: HTTP {status} ({hint})\nModel: {model}  Endpoint: {url}\n{body}");
    ProviderError::new(kind, message).into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::{ChatMessage, ImageAttachment, ToolCall};

    #[test]
    fn system_message_becomes_top_level_system_field_not_a_turn() {
        let messages = vec![ChatMessage::system("You are a helpful coding agent."), ChatMessage::user("hi")];
        let req = build_request(&messages, &[], 0.3, 4096, "claude-sonnet-4-6");
        assert_eq!(req["system"], "You are a helpful coding agent.");
        let turns = req["messages"].as_array().unwrap();
        assert_eq!(turns.len(), 1);
        assert_eq!(turns[0]["role"], "user");
    }

    #[test]
    fn user_text_becomes_a_text_content_block() {
        let req = build_request(&[ChatMessage::user("hi")], &[], 0.3, 4096, "claude-sonnet-4-6");
        let turns = req["messages"].as_array().unwrap();
        assert_eq!(turns[0]["content"][0]["type"], "text");
        assert_eq!(turns[0]["content"][0]["text"], "hi");
    }

    #[test]
    fn images_become_image_blocks_alongside_the_text_block() {
        let messages = vec![ChatMessage::user_with_images(
            "what's this",
            vec![ImageAttachment { mime: "image/png".to_string(), base64_data: "QUJD".to_string() }],
        )];
        let req = build_request(&messages, &[], 0.3, 4096, "claude-sonnet-4-6");
        let blocks = req["messages"][0]["content"].as_array().unwrap();
        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[0]["type"], "text");
        assert_eq!(blocks[1]["type"], "image");
        assert_eq!(blocks[1]["source"]["media_type"], "image/png");
        assert_eq!(blocks[1]["source"]["data"], "QUJD");
    }

    #[test]
    fn tool_call_becomes_tool_use_block_with_a_real_id() {
        let call = ToolCall { id: "toolu_01abc".to_string(), name: "read_file".to_string(), arguments: r#"{"path":"src/main.rs"}"#.to_string() };
        let messages = vec![ChatMessage::user("read main.rs"), ChatMessage::assistant(None, None, vec![call])];
        let req = build_request(&messages, &[], 0.3, 4096, "claude-sonnet-4-6");
        let block = &req["messages"][1]["content"][0];
        assert_eq!(block["type"], "tool_use");
        assert_eq!(block["id"], "toolu_01abc");
        assert_eq!(block["name"], "read_file");
        assert_eq!(block["input"]["path"], "src/main.rs");
    }

    #[test]
    fn consecutive_tool_results_merge_into_one_user_turn() {
        let messages = vec![
            ChatMessage::user("do two things"),
            ChatMessage::assistant(
                None,
                None,
                vec![
                    ToolCall { id: "toolu_0".into(), name: "read_file".into(), arguments: "{}".into() },
                    ToolCall { id: "toolu_1".into(), name: "list_files".into(), arguments: "{}".into() },
                ],
            ),
            ChatMessage::tool_result("toolu_0", "read_file", "file contents here"),
            ChatMessage::tool_result("toolu_1", "list_files", "a.rs\nb.rs"),
        ];
        let req = build_request(&messages, &[], 0.3, 4096, "claude-sonnet-4-6");
        let turns = req["messages"].as_array().unwrap();
        // user, assistant, merged-tool-results = 3 turns, not 4.
        assert_eq!(turns.len(), 3);
        let blocks = turns[2]["content"].as_array().unwrap();
        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[0]["type"], "tool_result");
        assert_eq!(blocks[0]["tool_use_id"], "toolu_0");
        assert_eq!(blocks[1]["tool_use_id"], "toolu_1");
    }

    #[test]
    fn tools_translate_to_input_schema_not_parameters() {
        let tools = vec![ToolDefinition {
            kind: "function",
            function: crate::tools::FunctionDefinition {
                name: "read_file".to_string(),
                description: "Read a file".to_string(),
                parameters: json!({"type": "object", "properties": {"path": {"type": "string"}}}),
            },
        }];
        let req = build_request(&[ChatMessage::user("hi")], &tools, 0.3, 4096, "claude-sonnet-4-6");
        let decls = req["tools"].as_array().unwrap();
        assert_eq!(decls[0]["name"], "read_file");
        assert_eq!(decls[0]["input_schema"]["properties"]["path"]["type"], "string");
        assert!(decls[0].get("parameters").is_none());
    }

    #[test]
    fn streamed_text_deltas_accumulate_in_order() {
        let mut content = String::new();
        let mut tool_calls = Vec::new();
        let mut blocks = HashMap::new();
        for event in [
            json!({"type": "content_block_start", "index": 0, "content_block": {"type": "text", "text": ""}}),
            json!({"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": "Hello"}}),
            json!({"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": " there!"}}),
            json!({"type": "content_block_stop", "index": 0}),
        ] {
            apply_stream_event(&event, &mut content, &mut tool_calls, &mut blocks, None);
        }
        assert_eq!(content, "Hello there!");
        assert!(tool_calls.is_empty());
    }

    #[test]
    fn streamed_tool_use_reassembles_fragmented_input_json() {
        let mut content = String::new();
        let mut tool_calls = Vec::new();
        let mut blocks = HashMap::new();
        for event in [
            json!({"type": "content_block_start", "index": 0, "content_block": {"type": "tool_use", "id": "toolu_01", "name": "read_file", "input": {}}}),
            json!({"type": "content_block_delta", "index": 0, "delta": {"type": "input_json_delta", "partial_json": "{\"path\":"}}),
            json!({"type": "content_block_delta", "index": 0, "delta": {"type": "input_json_delta", "partial_json": "\"src/main.rs\"}"}}),
            json!({"type": "content_block_stop", "index": 0}),
        ] {
            apply_stream_event(&event, &mut content, &mut tool_calls, &mut blocks, None);
        }
        assert_eq!(tool_calls.len(), 1);
        assert_eq!(tool_calls[0].id, "toolu_01");
        assert_eq!(tool_calls[0].name, "read_file");
        let parsed: Value = serde_json::from_str(&tool_calls[0].arguments).unwrap();
        assert_eq!(parsed["path"], "src/main.rs");
    }

    #[test]
    fn text_and_tool_use_blocks_can_interleave_by_index() {
        // A turn that answers in prose AND calls a tool — both content
        // kinds streaming concurrently at different indices, exactly the
        // scenario `HashMap<u64, BlockKind>` (rather than a single
        // "current block" variable) exists to handle correctly.
        let mut content = String::new();
        let mut tool_calls = Vec::new();
        let mut blocks = HashMap::new();
        for event in [
            json!({"type": "content_block_start", "index": 0, "content_block": {"type": "text", "text": ""}}),
            json!({"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": "Let me check that."}}),
            json!({"type": "content_block_stop", "index": 0}),
            json!({"type": "content_block_start", "index": 1, "content_block": {"type": "tool_use", "id": "toolu_02", "name": "list_files", "input": {}}}),
            json!({"type": "content_block_delta", "index": 1, "delta": {"type": "input_json_delta", "partial_json": "{}"}}),
            json!({"type": "content_block_stop", "index": 1}),
        ] {
            apply_stream_event(&event, &mut content, &mut tool_calls, &mut blocks, None);
        }
        assert_eq!(content, "Let me check that.");
        assert_eq!(tool_calls.len(), 1);
        assert_eq!(tool_calls[0].name, "list_files");
    }

    #[test]
    fn parses_a_multi_chunk_sse_stream_split_across_network_reads() {
        // The same "framing survives an arbitrary byte-level split" check
        // gemini's test suite makes — a line cut mid-way across two
        // separate `bytes_stream` chunks must not be dropped or
        // double-parsed.
        let mut buffer: Vec<u8> = Vec::new();
        buffer.extend_from_slice(b"data: {\"type\": \"content_block_delta\", \"in");
        let first = drain_sse_data_lines(&mut buffer);
        assert!(first.is_empty(), "an incomplete line must not be emitted yet");
        buffer.extend_from_slice(b"dex\": 0, \"delta\": {\"type\": \"text_delta\", \"text\": \"hi\"}}\n");
        let second = drain_sse_data_lines(&mut buffer);
        assert_eq!(second.len(), 1);
        let parsed: Value = serde_json::from_str(&second[0]).unwrap();
        assert_eq!(parsed["delta"]["text"], "hi");
    }
}
