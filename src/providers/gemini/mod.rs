//! Native Google Gemini provider.
//!
//! Gemini's `generateContent` API is **not** OpenAI-compatible — pointing
//! `openai_compatible` at it (as REXO's catalog briefly did) 401s no
//! matter what key you give it, because both the auth mechanism and the
//! request/response JSON shape are genuinely different:
//!
//! - **Auth**: `x-goog-api-key: <key>` header, not `Authorization: Bearer`.
//! - **Roles**: only `user` and `model` exist in `contents`; there's no
//!   `system` or `tool` role. A system prompt is its own top-level
//!   `systemInstruction` field, and tool *results* are sent back as a
//!   `user` turn containing a `functionResponse` part — an
//!   OpenAI-style separate "tool" role doesn't exist here at all.
//! - **Tool calls**: the model's function calls arrive as `functionCall`
//!   parts inside a normal `model`-role turn, not a separate top-level
//!   field, and (unlike OpenAI) Gemini never gives them an ID — REXO
//!   assigns its own (`call_0`, `call_1`, ...) purely to satisfy the
//!   rest of the agent loop's provider-agnostic `tool_call_id` pairing;
//!   Gemini itself never sees that ID.
//!
//! **Streaming**: real, as of v0.6 — `POST .../streamGenerateContent?alt=sse`
//! returns a `text/event-stream` of `data: {...}` lines, each one a partial
//! `GenerateContentResponse` with the *same shape* as the non-streaming
//! response (`candidates[].content.parts[]`), just carrying an incremental
//! slice of the answer per chunk rather than the whole thing — a different
//! chunk shape from the OpenAI-style `choices[].delta` this codebase's
//! other providers share via `protocol::OpenAiProtocolClient`, which is
//! why Gemini needed its own SSE loop instead of reusing that one. Tool
//! calls typically arrive whole in a single chunk (Gemini doesn't stream a
//! `functionCall` token-by-token the way it does text), so
//! [`apply_stream_chunk`] just appends any that show up in a chunk as-is.
//! v0.4.1 shipped the non-streaming `generateContent` fallback, which
//! [`Provider::chat`]'s own docs call out as legitimate on its own
//! ("providers that can't stream may simply call it once at the end") —
//! but Gemini *can* stream, so that was always meant to be temporary.
//!
//! **This is genuinely untested against the live API** — this project's
//! build sandbox can't reach `generativelanguage.googleapis.com` (it's
//! not on the sandbox's network allowlist), so what's tested here is the
//! request-building and chunk-assembly logic in isolation against
//! hand-written example payloads matching Google's documented format
//! (including a multi-chunk SSE stream strung together byte-by-byte to
//! exercise the same buffering logic the real network path uses), not an
//! end-to-end call. Worth confirming against a real key and filing an
//! issue if the live shape has drifted from what's here.

use anyhow::{Context, Result};
use async_trait::async_trait;
use futures_util::StreamExt;
use serde_json::{json, Value};

use crate::tools::ToolDefinition;

use super::{classify_http_status, classify_reqwest_error, ChatMessage, ChatResult, ModelCapabilities, Provider, ProviderError, Role, StreamEvent, StreamSink, ToolCall};

pub(crate) const GEMINI_ENDPOINT: &str = "https://generativelanguage.googleapis.com/v1beta";

pub struct GeminiProvider {
    http: reqwest::Client,
    api_key: String,
    model: String,
    display_name: String,
    temperature: f32,
    max_tokens: u32,
    base_url: String,
}

impl GeminiProvider {
    pub fn new(display_name: String, api_key: String, model: String, temperature: f32, max_tokens: u32, base_url: Option<String>) -> Self {
        Self {
            http: reqwest::Client::new(),
            api_key,
            model,
            display_name,
            temperature,
            max_tokens,
            base_url: base_url.unwrap_or_else(|| GEMINI_ENDPOINT.to_string()),
        }
    }

    fn stream_url(&self) -> String {
        format!("{}/models/{}:streamGenerateContent?alt=sse", self.base_url.trim_end_matches('/'), self.model)
    }
}

#[async_trait]
impl Provider for GeminiProvider {
    async fn chat(&self, messages: &[ChatMessage], tools: &[ToolDefinition], on_event: Option<&StreamSink<'_>>) -> Result<ChatResult> {
        let body = build_request(messages, tools, self.temperature, self.max_tokens);

        let response = self
            .http
            .post(self.stream_url())
            .header("x-goog-api-key", &self.api_key)
            .header("Content-Type", "application/json")
            .header("Accept", "text/event-stream")
            .json(&body)
            .send()
            .await
            .map_err(|e| {
                let kind = classify_reqwest_error(&e);
                ProviderError::new(kind, format!("Gemini request failed (network): {e}"))
            })?;

        let status = response.status();
        if !status.is_success() {
            let text = response.text().await.unwrap_or_default();
            return Err(describe_error(status.as_u16(), &text, &self.model, &self.stream_url()));
        }

        consume_gemini_stream(response, on_event).await
    }

    fn describe(&self) -> String {
        format!("{} ({})", self.display_name, self.model)
    }

    fn capabilities(&self) -> ModelCapabilities {
        ModelCapabilities {
            tool_calling: Some(true),
            streaming: Some(true), // streamGenerateContent, as of v0.6 — see this module's docs
            vision: Some(true),
            reasoning: None,
            model_discovery: Some(false), // no models.list translation implemented
            usage_reporting: Some(false), // usageMetadata is in the wire response but REXO doesn't read it back yet
            ..ModelCapabilities::unknown()
        }
    }
}

/// Read the `streamGenerateContent?alt=sse` response body as it arrives,
/// applying each `data:` chunk via [`apply_stream_chunk`] and firing
/// `on_event` incrementally, exactly mirroring
/// `protocol::OpenAiProtocolClient::consume_stream`'s buffering approach
/// (byte-level line splitting is safe here for the same reason: `\n`
/// never appears as a UTF-8 continuation byte) so a real network hiccup
/// mid-stream behaves the same way across every provider in this crate.
async fn consume_gemini_stream(response: reqwest::Response, on_event: Option<&StreamSink<'_>>) -> Result<ChatResult> {
    let mut byte_stream = response.bytes_stream();
    let mut line_buffer: Vec<u8> = Vec::new();
    let mut content = String::new();
    let mut tool_calls: Vec<ToolCall> = Vec::new();

    while let Some(chunk) = byte_stream.next().await {
        let chunk = chunk.context("Error while reading Gemini response stream")?;
        line_buffer.extend_from_slice(&chunk);

        for data in drain_sse_data_lines(&mut line_buffer) {
            let parsed: Value = match serde_json::from_str(&data) {
                Ok(v) => v,
                Err(_) => continue, // ignore malformed/keep-alive lines, same policy as the OpenAI-compat path
            };
            apply_stream_chunk(&parsed, &mut content, &mut tool_calls, on_event);
        }
    }

    Ok(ChatResult {
        content: if content.is_empty() { None } else { Some(content) },
        reasoning: None,
        tool_calls,
    })
}

/// Drain every complete `\n`-terminated line out of `buffer`, returning
/// the trimmed payload of each `data: ...` line found (SSE comment lines,
/// blank lines, and anything else are dropped). Incomplete trailing bytes
/// (a line split across two network reads) are left in `buffer` for the
/// next call — this is the one piece of stateful buffering logic in the
/// whole streaming path, so it's factored out to be tested directly
/// against byte slices rather than only indirectly through a fake
/// network response.
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

/// Apply one parsed SSE chunk (a partial `GenerateContentResponse`) to the
/// accumulated result, firing stream events as new text/tool-call parts
/// show up. Pure and side-effect-free apart from `on_event`, so it's
/// tested directly against hand-built `Value`s below without any network
/// involved — same pattern as `protocol::apply_chunk`.
fn apply_stream_chunk(chunk: &Value, content: &mut String, tool_calls: &mut Vec<ToolCall>, on_event: Option<&StreamSink<'_>>) {
    let Some(candidate) = chunk.get("candidates").and_then(Value::as_array).and_then(|c| c.first()) else {
        return; // e.g. a chunk that's only `usageMetadata` / `promptFeedback`
    };
    let Some(parts) = candidate.get("content").and_then(|c| c.get("parts")).and_then(Value::as_array) else {
        return;
    };

    for part in parts {
        if let Some(text) = part.get("text").and_then(Value::as_str) {
            if !text.is_empty() {
                content.push_str(text);
                if let Some(sink) = on_event {
                    sink(StreamEvent::TextDelta(text.to_string()));
                }
            }
        }
        if let Some(call) = part.get("functionCall") {
            let name = call.get("name").and_then(Value::as_str).unwrap_or_default().to_string();
            let args = call.get("args").cloned().unwrap_or_else(|| json!({}));
            let id = format!("call_{}", tool_calls.len());
            if let Some(sink) = on_event {
                sink(StreamEvent::ToolCallStarted { name: name.clone() });
            }
            tool_calls.push(ToolCall { id, name, arguments: args.to_string() });
        }
    }
}

/// Translate REXO's provider-agnostic history into Gemini's `contents` +
/// `systemInstruction` shape. Consecutive `Role::Tool` messages (multiple
/// results from one multi-tool-call turn) are merged into a single
/// `user`-role turn with multiple `functionResponse` parts, since Gemini
/// expects one turn per "speaker," not one per result — sending them as
/// separate back-to-back `user` turns is a common cause of a 400 from the
/// real API for exactly this translation.
fn build_request(messages: &[ChatMessage], tools: &[ToolDefinition], temperature: f32, max_tokens: u32) -> Value {
    let mut system_text = String::new();
    let mut contents: Vec<Value> = Vec::new();

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
                let mut parts = Vec::new();
                if let Some(text) = &msg.content {
                    if !text.is_empty() {
                        parts.push(json!({"text": text}));
                    }
                }
                // Alt+V-pasted images — Gemini's native format wants each
                // as its own `inlineData` part alongside the text part,
                // rather than the data-URL-in-content-string shape the
                // OpenAI-compatible wire format uses (see
                // `protocol.rs`'s `WireMessage`).
                for img in &msg.images {
                    parts.push(json!({"inlineData": {"mimeType": img.mime, "data": img.base64_data}}));
                }
                if !parts.is_empty() {
                    contents.push(json!({"role": "user", "parts": parts}));
                }
            }
            Role::Assistant => {
                let mut parts = Vec::new();
                if let Some(text) = &msg.content {
                    if !text.is_empty() {
                        parts.push(json!({"text": text}));
                    }
                }
                for call in &msg.tool_calls {
                    let args: Value = serde_json::from_str(&call.arguments).unwrap_or_else(|_| json!({}));
                    parts.push(json!({"functionCall": {"name": call.name, "args": args}}));
                }
                if !parts.is_empty() {
                    contents.push(json!({"role": "model", "parts": parts}));
                }
            }
            Role::Tool => {
                let response_value: Value = msg
                    .content
                    .as_deref()
                    .and_then(|c| serde_json::from_str(c).ok())
                    .unwrap_or_else(|| json!({"result": msg.content.clone().unwrap_or_default()}));
                let part = json!({
                    "functionResponse": {
                        "name": msg.name.clone().unwrap_or_default(),
                        "response": response_value,
                    }
                });
                // Merge into the previous turn if it was also a tool-result
                // turn — see this function's doc comment.
                if let Some(last) = contents.last_mut() {
                    if last.get("role").and_then(Value::as_str) == Some("user") && is_function_response_turn(last) {
                        if let Some(parts) = last.get_mut("parts").and_then(Value::as_array_mut) {
                            parts.push(part);
                            continue;
                        }
                    }
                }
                contents.push(json!({"role": "user", "parts": [part]}));
            }
        }
    }

    let mut body = json!({
        "contents": contents,
        "generationConfig": {
            "temperature": temperature,
            "maxOutputTokens": max_tokens,
        },
    });

    if !system_text.is_empty() {
        body["systemInstruction"] = json!({"parts": [{"text": system_text}]});
    }

    if !tools.is_empty() {
        let declarations: Vec<Value> = tools
            .iter()
            .map(|t| {
                json!({
                    "name": t.function.name,
                    "description": t.function.description,
                    "parameters": sanitize_schema(&t.function.parameters),
                })
            })
            .collect();
        body["tools"] = json!([{"functionDeclarations": declarations}]);
    }

    body
}

fn is_function_response_turn(turn: &Value) -> bool {
    turn.get("parts")
        .and_then(Value::as_array)
        .map(|parts| !parts.is_empty() && parts.iter().all(|p| p.get("functionResponse").is_some()))
        .unwrap_or(false)
}

/// Gemini's function-calling schema is a real subset of JSON Schema, not
/// the whole thing — in particular it rejects an `additionalProperties`
/// keyword REXO's tool schemas don't rely on anyway, but strip
/// defensively rather than risk a 400 on a schema keyword it doesn't
/// recognize.
fn sanitize_schema(schema: &Value) -> Value {
    let mut cleaned = schema.clone();
    if let Some(obj) = cleaned.as_object_mut() {
        obj.remove("additionalProperties");
        obj.remove("$schema");
        if let Some(props) = obj.get_mut("properties").and_then(Value::as_object_mut) {
            for (_, v) in props.iter_mut() {
                *v = sanitize_schema(v);
            }
        }
    }
    cleaned
}

/// Parse a *whole, non-streaming-shaped* Gemini response into REXO's
/// [`ChatResult`] — kept as a test helper (this module's production path
/// is the SSE one above) because a single SSE chunk and a full
/// `generateContent` response share the exact same
/// `candidates[].content.parts[]` shape, so this doubles as a check that
/// `apply_stream_chunk`'s per-part logic (text/`functionCall` extraction,
/// synthetic `call_N` IDs since Gemini never sends one) agrees with that
/// shape. Not called from `chat()` — real requests go through
/// `consume_gemini_stream`/`apply_stream_chunk` instead.
#[cfg(test)]
fn parse_response(value: &Value) -> Result<ChatResult> {
    let candidate = value
        .get("candidates")
        .and_then(Value::as_array)
        .and_then(|c| c.first())
        .ok_or_else(|| anyhow::anyhow!("Gemini response had no candidates: {value}"))?;

    let parts = candidate.get("content").and_then(|c| c.get("parts")).and_then(Value::as_array).cloned().unwrap_or_default();

    let mut text = String::new();
    let mut tool_calls = Vec::new();
    for (i, part) in parts.iter().enumerate() {
        if let Some(t) = part.get("text").and_then(Value::as_str) {
            text.push_str(t);
        }
        if let Some(call) = part.get("functionCall") {
            let name = call.get("name").and_then(Value::as_str).unwrap_or_default().to_string();
            let args = call.get("args").cloned().unwrap_or_else(|| json!({}));
            tool_calls.push(ToolCall { id: format!("call_{i}"), name, arguments: args.to_string() });
        }
    }

    Ok(ChatResult { content: if text.is_empty() { None } else { Some(text) }, reasoning: None, tool_calls })
}

fn describe_error(status: u16, body: &str, model: &str, url: &str) -> anyhow::Error {
    let hint = match status {
        401 | 403 => "the API key is missing, invalid, or blocked for this API — double check it's a Gemini API key from Google AI Studio, not a Cloud/OAuth credential",
        404 => "the model ID isn't available at this endpoint",
        429 => "rate limited — the free tier has a low requests-per-minute cap",
        _ => "see the response body below",
    };
    let kind = classify_http_status(status, body);
    let message = format!("Gemini request failed: HTTP {status} ({hint})\nModel: {model}  Endpoint: {url}\n{body}");
    ProviderError::new(kind, message).into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::{ChatMessage, ImageAttachment, ToolCall};

    #[test]
    fn system_message_becomes_system_instruction_not_a_content_turn() {
        let messages = vec![ChatMessage::system("You are a helpful coding agent."), ChatMessage::user("hi")];
        let req = build_request(&messages, &[], 0.3, 4096);
        assert_eq!(req["systemInstruction"]["parts"][0]["text"], "You are a helpful coding agent.");
        let contents = req["contents"].as_array().unwrap();
        assert_eq!(contents.len(), 1);
        assert_eq!(contents[0]["role"], "user");
    }

    #[test]
    fn assistant_role_becomes_model() {
        let messages = vec![ChatMessage::user("hi"), ChatMessage::assistant(Some("hello!".to_string()), None, vec![])];
        let req = build_request(&messages, &[], 0.3, 4096);
        let contents = req["contents"].as_array().unwrap();
        assert_eq!(contents[1]["role"], "model");
        assert_eq!(contents[1]["parts"][0]["text"], "hello!");
    }

    #[test]
    fn images_become_inline_data_parts_alongside_the_text_part() {
        let messages = vec![ChatMessage::user_with_images(
            "what's this",
            vec![ImageAttachment { mime: "image/png".to_string(), base64_data: "QUJD".to_string() }],
        )];
        let req = build_request(&messages, &[], 0.3, 4096);
        let parts = req["contents"][0]["parts"].as_array().unwrap();
        assert_eq!(parts.len(), 2);
        assert_eq!(parts[0]["text"], "what's this");
        assert_eq!(parts[1]["inlineData"]["mimeType"], "image/png");
        assert_eq!(parts[1]["inlineData"]["data"], "QUJD");
    }

    #[test]
    fn tool_call_becomes_function_call_part() {
        let call = ToolCall { id: "call_0".to_string(), name: "read_file".to_string(), arguments: r#"{"path":"src/main.rs"}"#.to_string() };
        let messages = vec![ChatMessage::user("read main.rs"), ChatMessage::assistant(None, None, vec![call])];
        let req = build_request(&messages, &[], 0.3, 4096);
        let contents = req["contents"].as_array().unwrap();
        let fc = &contents[1]["parts"][0]["functionCall"];
        assert_eq!(fc["name"], "read_file");
        assert_eq!(fc["args"]["path"], "src/main.rs");
    }

    #[test]
    fn consecutive_tool_results_merge_into_one_user_turn() {
        let messages = vec![
            ChatMessage::user("do two things"),
            ChatMessage::assistant(
                None,
                None,
                vec![
                    ToolCall { id: "call_0".into(), name: "read_file".into(), arguments: "{}".into() },
                    ToolCall { id: "call_1".into(), name: "list_files".into(), arguments: "{}".into() },
                ],
            ),
            ChatMessage::tool_result("call_0", "read_file", "file contents here"),
            ChatMessage::tool_result("call_1", "list_files", "a.rs\nb.rs"),
        ];
        let req = build_request(&messages, &[], 0.3, 4096);
        let contents = req["contents"].as_array().unwrap();
        // user, model, merged-tool-results = 3 turns, not 4.
        assert_eq!(contents.len(), 3);
        let last_parts = contents[2]["parts"].as_array().unwrap();
        assert_eq!(last_parts.len(), 2);
        assert_eq!(last_parts[0]["functionResponse"]["name"], "read_file");
        assert_eq!(last_parts[1]["functionResponse"]["name"], "list_files");
    }

    #[test]
    fn tools_translate_to_function_declarations() {
        let tools = vec![ToolDefinition {
            kind: "function",
            function: crate::tools::FunctionDefinition {
                name: "read_file".to_string(),
                description: "Read a file".to_string(),
                parameters: json!({"type": "object", "properties": {"path": {"type": "string"}}, "additionalProperties": false}),
            },
        }];
        let req = build_request(&[ChatMessage::user("hi")], &tools, 0.3, 4096);
        let decls = req["tools"][0]["functionDeclarations"].as_array().unwrap();
        assert_eq!(decls.len(), 1);
        assert_eq!(decls[0]["name"], "read_file");
        // additionalProperties stripped — Gemini's schema subset rejects it.
        assert!(decls[0]["parameters"].get("additionalProperties").is_none());
    }

    #[test]
    fn parses_plain_text_response() {
        let body = json!({
            "candidates": [{
                "content": {"role": "model", "parts": [{"text": "Hello there!"}]},
                "finishReason": "STOP"
            }]
        });
        let result = parse_response(&body).unwrap();
        assert_eq!(result.content.as_deref(), Some("Hello there!"));
        assert!(result.tool_calls.is_empty());
    }

    #[test]
    fn parses_function_call_response_with_synthetic_id() {
        let body = json!({
            "candidates": [{
                "content": {"role": "model", "parts": [
                    {"functionCall": {"name": "read_file", "args": {"path": "src/main.rs"}}}
                ]},
                "finishReason": "STOP"
            }]
        });
        let result = parse_response(&body).unwrap();
        assert_eq!(result.tool_calls.len(), 1);
        assert_eq!(result.tool_calls[0].name, "read_file");
        assert!(!result.tool_calls[0].id.is_empty());
        assert!(result.tool_calls[0].arguments.contains("src/main.rs"));
    }

    #[test]
    fn missing_candidates_is_a_clear_error_not_a_panic() {
        let body = json!({"promptFeedback": {"blockReason": "SAFETY"}});
        assert!(parse_response(&body).is_err());
    }

    #[test]
    fn error_hint_distinguishes_auth_failures() {
        let err = describe_error(401, "{}", "gemini-flash-latest", "https://example.com").to_string();
        assert!(err.contains("API key"));
        let err = describe_error(429, "{}", "gemini-flash-latest", "https://example.com");
        assert!(err.to_string().contains("rate limited"));
    }

    #[test]
    fn error_kind_is_classified_and_recoverable() {
        use crate::providers::{provider_error_kind, ProviderErrorKind};
        let err = describe_error(429, "{}", "gemini-flash-latest", "https://example.com");
        assert_eq!(provider_error_kind(&err), ProviderErrorKind::RateLimited);
        let err = describe_error(401, "{}", "gemini-flash-latest", "https://example.com");
        assert_eq!(provider_error_kind(&err), ProviderErrorKind::Authentication);
    }

    #[test]
    fn capabilities_report_real_streaming_now() {
        let provider = GeminiProvider::new("Gemini".to_string(), "key".to_string(), "gemini-flash-latest".to_string(), 0.3, 4096, None);
        let caps = provider.capabilities();
        assert_eq!(caps.streaming, Some(true));
        assert_eq!(caps.tool_calling, Some(true));
    }

    // ---- SSE streaming assembly ----------------------------------------

    #[test]
    fn apply_stream_chunk_assembles_text_across_multiple_chunks() {
        let mut content = String::new();
        let mut tool_calls = Vec::new();

        let c1 = json!({"candidates": [{"content": {"role": "model", "parts": [{"text": "Hello"}]}}]});
        apply_stream_chunk(&c1, &mut content, &mut tool_calls, None);
        let c2 = json!({"candidates": [{"content": {"role": "model", "parts": [{"text": ", world!"}]}}]});
        apply_stream_chunk(&c2, &mut content, &mut tool_calls, None);

        assert_eq!(content, "Hello, world!");
        assert!(tool_calls.is_empty());
    }

    #[test]
    fn apply_stream_chunk_captures_function_call_with_synthetic_id() {
        let mut content = String::new();
        let mut tool_calls = Vec::new();

        let c1 = json!({"candidates": [{"content": {"role": "model", "parts": [
            {"functionCall": {"name": "read_file", "args": {"path": "src/main.rs"}}}
        ]}}]});
        apply_stream_chunk(&c1, &mut content, &mut tool_calls, None);

        assert_eq!(tool_calls.len(), 1);
        assert_eq!(tool_calls[0].id, "call_0");
        assert_eq!(tool_calls[0].name, "read_file");
        assert!(tool_calls[0].arguments.contains("src/main.rs"));
    }

    #[test]
    fn apply_stream_chunk_ignores_metadata_only_chunks() {
        // A trailing chunk that's only `usageMetadata` (no `candidates`)
        // shouldn't panic or add bogus content — real Gemini streams end
        // with one of these.
        let mut content = String::new();
        let mut tool_calls = Vec::new();
        let usage_only = json!({"usageMetadata": {"promptTokenCount": 42, "candidatesTokenCount": 7}});
        apply_stream_chunk(&usage_only, &mut content, &mut tool_calls, None);
        assert!(content.is_empty());
        assert!(tool_calls.is_empty());
    }

    #[test]
    fn apply_stream_chunk_fires_stream_events() {
        use std::sync::{Arc, Mutex};
        let events: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let events_clone = events.clone();
        let sink: Box<StreamSink> = Box::new(move |e| {
            events_clone.lock().unwrap().push(format!("{e:?}"));
        });

        let mut content = String::new();
        let mut tool_calls = Vec::new();
        let chunk = json!({"candidates": [{"content": {"role": "model", "parts": [{"text": "hi"}]}}]});
        apply_stream_chunk(&chunk, &mut content, &mut tool_calls, Some(&*sink));

        assert_eq!(events.lock().unwrap().len(), 1);
        assert!(events.lock().unwrap()[0].contains("TextDelta"));
    }

    #[test]
    fn drain_sse_data_lines_extracts_data_payloads_and_skips_noise() {
        let mut buffer = b"data: {\"a\":1}\n\n: this is a comment\n\ndata: {\"b\":2}\n".to_vec();
        let lines = drain_sse_data_lines(&mut buffer);
        assert_eq!(lines, vec!["{\"a\":1}".to_string(), "{\"b\":2}".to_string()]);
        assert!(buffer.is_empty());
    }

    #[test]
    fn drain_sse_data_lines_holds_back_an_incomplete_trailing_line() {
        // Simulates a `data: {...}` line arriving split across two network
        // reads — the second half hasn't arrived yet, so nothing should be
        // extracted until the terminating '\n' shows up.
        let mut buffer = b"data: {\"partial".to_vec();
        let lines = drain_sse_data_lines(&mut buffer);
        assert!(lines.is_empty());
        assert_eq!(buffer, b"data: {\"partial");

        buffer.extend_from_slice(b"\":true}\n");
        let lines = drain_sse_data_lines(&mut buffer);
        assert_eq!(lines, vec!["{\"partial\":true}".to_string()]);
    }

    #[test]
    fn full_sse_stream_with_text_and_tool_call_assembles_correctly() {
        // A realistic multi-chunk stream: two text deltas, then a
        // function call, then a trailing usage-only chunk — strung
        // together as raw SSE bytes and fed through the exact same
        // buffering path `consume_gemini_stream` uses.
        let sse = concat!(
            "data: {\"candidates\":[{\"content\":{\"role\":\"model\",\"parts\":[{\"text\":\"Let me check \"}]}}]}\n\n",
            "data: {\"candidates\":[{\"content\":{\"role\":\"model\",\"parts\":[{\"text\":\"that file.\"}]}}]}\n\n",
            "data: {\"candidates\":[{\"content\":{\"role\":\"model\",\"parts\":[{\"functionCall\":{\"name\":\"read_file\",\"args\":{\"path\":\"a.rs\"}}}]}}]}\n\n",
            "data: {\"usageMetadata\":{\"promptTokenCount\":10}}\n\n",
        );
        let mut buffer = sse.as_bytes().to_vec();
        let mut content = String::new();
        let mut tool_calls = Vec::new();

        for data in drain_sse_data_lines(&mut buffer) {
            let parsed: Value = serde_json::from_str(&data).unwrap();
            apply_stream_chunk(&parsed, &mut content, &mut tool_calls, None);
        }

        assert_eq!(content, "Let me check that file.");
        assert_eq!(tool_calls.len(), 1);
        assert_eq!(tool_calls[0].name, "read_file");
    }
}
