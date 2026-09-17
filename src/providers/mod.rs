//! Provider abstraction.
//!
//! REXO talks to models through a small, provider-agnostic interface
//! ([`Provider`]) so the agent loop never has to know whether it's talking
//! to NVIDIA NIM, a generic OpenAI-compatible endpoint, or a local model
//! server. All three ship in this crate and, since they all speak the same
//! OpenAI-style `/chat/completions` protocol, share one HTTP client
//! implementation in [`protocol`].

pub mod catalog;
pub mod gemini;
pub mod local;
pub mod nvidia;
pub mod openai_compatible;
pub mod protocol;

use anyhow::{anyhow, Result};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::config::Config;
use crate::tools::ToolDefinition;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    System,
    User,
    Assistant,
    Tool,
}

/// A single tool call the model asked to make, as part of an assistant
/// message. `arguments` is kept as a raw JSON string (as providers send
/// it over the wire) and parsed lazily by the agent loop.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: String,
}

/// One message in the conversation, in our own provider-agnostic shape.
/// Providers translate to/from their wire format at the edges.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: Role,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    /// The model's chain-of-thought for this turn, if the provider streamed
    /// one (e.g. Kimi K3's `reasoning_content`). Kept separate from
    /// `content` so it can be displayed differently and echoed back to
    /// providers that expect it on follow-up turns.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<ToolCall>,
    /// Present only on `Role::Tool` messages: which call this is a result for.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    /// Present only on `Role::Tool` messages: the tool's name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

impl ChatMessage {
    pub fn system(text: impl Into<String>) -> Self {
        Self {
            role: Role::System,
            content: Some(text.into()),
            reasoning: None,
            tool_calls: Vec::new(),
            tool_call_id: None,
            name: None,
        }
    }

    pub fn user(text: impl Into<String>) -> Self {
        Self {
            role: Role::User,
            content: Some(text.into()),
            reasoning: None,
            tool_calls: Vec::new(),
            tool_call_id: None,
            name: None,
        }
    }

    pub fn assistant(content: Option<String>, reasoning: Option<String>, tool_calls: Vec<ToolCall>) -> Self {
        Self {
            role: Role::Assistant,
            content,
            reasoning,
            tool_calls,
            tool_call_id: None,
            name: None,
        }
    }

    pub fn tool_result(tool_call_id: impl Into<String>, name: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            role: Role::Tool,
            content: Some(content.into()),
            reasoning: None,
            tool_calls: Vec::new(),
            tool_call_id: Some(tool_call_id.into()),
            name: Some(name.into()),
        }
    }
}

/// The final, fully-assembled result of one model turn (after a stream, if
/// any, has finished).
#[derive(Debug, Clone, Default)]
pub struct ChatResult {
    pub content: Option<String>,
    pub reasoning: Option<String>,
    pub tool_calls: Vec<ToolCall>,
}

/// Fired incrementally while a streaming response comes in, purely for
/// display purposes — the authoritative result is still the final
/// [`ChatResult`] returned by [`Provider::chat`].
#[derive(Debug, Clone)]
pub enum StreamEvent {
    /// Chain-of-thought text from an always-reasoning model. Not part of
    /// the final answer — display it distinctly (e.g. dimmed) if at all.
    ReasoningDelta(String),
    TextDelta(String),
    ToolCallStarted { name: String },
}

pub type StreamSink<'a> = dyn Fn(StreamEvent) + Send + Sync + 'a;

/// What a model/provider combination is known to support. Per the V3
/// architecture goal of not baking provider-specific assumptions into the
/// agent: these are advisory (the agent currently doesn't hard-gate on
/// them — see the `Roadmap` in the README for "capability-aware
/// behavior"), but the fields exist now so that gating can be added
/// without another round of provider-trait churn, and so `/model`/`/status`
/// have something real to display instead of guessing.
///
/// `None` means "unknown", not "false" — REXO doesn't have live capability
/// data for most endpoints, and showing a confident "no" it can't actually
/// back up would be worse than admitting it doesn't know. This is the v0.6
/// "provider capability model" from the roadmap's Phase 1: the four
/// original fields (`tool_calling`/`streaming`/`vision`/`reasoning`) shipped
/// in v0.4; `parallel_tools`/`structured_output`/`model_discovery`/
/// `context_window`/`max_output`/`cancellation`/`usage_reporting` are new
/// this release. As with the original four, adding a field here is
/// additive and doesn't change `Provider::chat`'s signature — the whole
/// point of keeping this advisory rather than gating on it yet.
#[derive(Debug, Clone, Copy, Default)]
pub struct ModelCapabilities {
    pub tool_calling: Option<bool>,
    pub streaming: Option<bool>,
    pub vision: Option<bool>,
    pub reasoning: Option<bool>,
    /// Can the model request more than one tool call in a single turn?
    pub parallel_tools: Option<bool>,
    /// Does the provider support constraining output to a JSON schema
    /// (OpenAI's `response_format`, Gemini's `responseSchema`, etc.)?
    pub structured_output: Option<bool>,
    /// Does `list_models`/an equivalent live model listing actually work
    /// for this provider? (`Provider::list_models` — see below.)
    pub model_discovery: Option<bool>,
    /// The model's total context window, in tokens, if known.
    pub context_window: Option<u32>,
    /// Max output tokens per turn, if the provider documents one
    /// separately from the context window.
    pub max_output: Option<u32>,
    /// Can an in-flight request actually be cancelled server-side (vs.
    /// REXO just dropping the connection and eating the cost/quota)?
    pub cancellation: Option<bool>,
    /// Does the provider report token usage REXO can read back (for a
    /// future cost/usage tracker — see the roadmap)?
    pub usage_reporting: Option<bool>,
}

impl ModelCapabilities {
    pub fn unknown() -> Self {
        Self::default()
    }
}

/// A provider/network failure, normalized into a small closed set of
/// causes instead of an arbitrary provider-specific string. This is the
/// v0.6 "unified provider errors" piece of Phase 1: every provider's HTTP
/// failures now go through [`classify_http_status`] and get wrapped in a
/// [`ProviderError`] so callers (the agent loop, `/status`, retry logic)
/// can `match` on *why* a call failed instead of grepping the message —
/// while the message text itself (what actually gets shown to the person)
/// is untouched, so this is purely additive.
///
/// Kept deliberately small and closed (an enum, not a provider-defined
/// string) per the architecture-quality rule against "stringly-typed
/// runtime state."
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderErrorKind {
    /// Bad/missing/expired credentials (401/403 with an auth-shaped hint).
    Authentication,
    /// Authenticated, but not allowed to do this specific thing.
    PermissionDenied,
    /// 429 or an equivalent "slow down" response.
    RateLimited,
    /// The request was too large for the model's context window.
    ContextExceeded,
    /// 400-class: malformed request, bad params, unsupported feature.
    InvalidRequest,
    /// The endpoint is reachable but the backend/model is down.
    ModelUnavailable,
    /// 404-class: this model ID doesn't exist at this endpoint.
    ModelNotFound,
    /// The request or connection timed out.
    Timeout,
    /// Couldn't reach the endpoint at all (DNS, TCP, TLS).
    Network,
    /// 5xx from the provider.
    Server,
    /// Anything that doesn't fit the above — not every provider's error
    /// shape is known in advance, and guessing wrong is worse than saying
    /// "unknown."
    Unknown,
}

impl ProviderErrorKind {
    /// Whether it's generally sane to retry this kind of failure
    /// automatically (with backoff for `RateLimited`/`Server`). Matches
    /// the README's retry-policy principle: retry transient failures,
    /// never retry an error that will just fail again unchanged.
    pub fn is_retryable(self) -> bool {
        matches!(self, Self::RateLimited | Self::Server | Self::Timeout | Self::Network | Self::ModelUnavailable)
    }

    /// Short, stable, lowercase label — used in `/status` and log lines,
    /// not meant to be the whole user-facing message.
    pub fn label(self) -> &'static str {
        match self {
            Self::Authentication => "authentication",
            Self::PermissionDenied => "permission_denied",
            Self::RateLimited => "rate_limited",
            Self::ContextExceeded => "context_exceeded",
            Self::InvalidRequest => "invalid_request",
            Self::ModelUnavailable => "model_unavailable",
            Self::ModelNotFound => "model_not_found",
            Self::Timeout => "timeout",
            Self::Network => "network",
            Self::Server => "server",
            Self::Unknown => "unknown",
        }
    }
}

/// Best-effort classification of an HTTP status (plus, for the couple of
/// ambiguous codes, a peek at the body) into a [`ProviderErrorKind`].
/// Shared by every HTTP-based provider so "what does a 429 mean" is
/// answered in exactly one place rather than once per provider.
pub fn classify_http_status(status: u16, body: &str) -> ProviderErrorKind {
    let body_lower = body.to_ascii_lowercase();
    match status {
        401 | 403 => ProviderErrorKind::Authentication,
        404 => ProviderErrorKind::ModelNotFound,
        408 => ProviderErrorKind::Timeout,
        409 | 423 => ProviderErrorKind::PermissionDenied,
        429 => ProviderErrorKind::RateLimited,
        400 | 422 => {
            // A 400 caused by an oversized request is common enough
            // across providers (and distinct enough in what the person
            // should actually do about it — trim context, don't retry)
            // to special-case by sniffing the body for it.
            if body_lower.contains("context") && (body_lower.contains("too long") || body_lower.contains("exceed") || body_lower.contains("maximum"))
                || body_lower.contains("context_length_exceeded")
                || body_lower.contains("token limit")
            {
                ProviderErrorKind::ContextExceeded
            } else {
                ProviderErrorKind::InvalidRequest
            }
        }
        502 | 503 | 504 => ProviderErrorKind::ModelUnavailable,
        500..=599 => ProviderErrorKind::Server,
        _ => ProviderErrorKind::Unknown,
    }
}

/// Classify a `reqwest::Error` that happened *before* any HTTP status was
/// even received — connection refused, DNS failure, TLS failure, or a
/// client-side timeout. Complements [`classify_http_status`], which only
/// applies once a response actually came back.
pub fn classify_reqwest_error(err: &reqwest::Error) -> ProviderErrorKind {
    if err.is_timeout() {
        ProviderErrorKind::Timeout
    } else if err.is_connect() || err.is_request() {
        ProviderErrorKind::Network
    } else {
        ProviderErrorKind::Unknown
    }
}

/// A normalized provider failure. Implements [`std::error::Error`] so it
/// round-trips cleanly through `anyhow::Error` (every call site in this
/// codebase already uses `anyhow::Result` — this doesn't change any
/// function signature) while still being recoverable via
/// [`provider_error_kind`] wherever a caller actually wants to branch on
/// *why* a call failed rather than just display the message.
#[derive(Debug)]
pub struct ProviderError {
    pub kind: ProviderErrorKind,
    pub message: String,
}

impl std::fmt::Display for ProviderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for ProviderError {}

impl ProviderError {
    pub fn new(kind: ProviderErrorKind, message: impl Into<String>) -> Self {
        Self { kind, message: message.into() }
    }
}

/// Recover the [`ProviderErrorKind`] from any `anyhow::Error`, if it
/// originated as a [`ProviderError`] — `Unknown` otherwise (including for
/// plain non-provider errors, which is the correct answer for those too).
pub fn provider_error_kind(err: &anyhow::Error) -> ProviderErrorKind {
    err.downcast_ref::<ProviderError>().map(|e| e.kind).unwrap_or(ProviderErrorKind::Unknown)
}

#[async_trait]
pub trait Provider: Send + Sync {
    /// Send the conversation so far, with the available tools, and return
    /// the model's next turn. If `on_event` is set, streaming deltas are
    /// reported through it as they arrive (best-effort; providers that
    /// can't stream may simply call it once at the end).
    async fn chat(
        &self,
        messages: &[ChatMessage],
        tools: &[ToolDefinition],
        on_event: Option<&StreamSink<'_>>,
    ) -> Result<ChatResult>;

    /// Human-readable name for logging, e.g. "OpenRouter (openrouter/auto)".
    fn describe(&self) -> String;

    /// What this provider/model is known to support. Default: everything
    /// unknown, rather than falsely confident.
    fn capabilities(&self) -> ModelCapabilities {
        ModelCapabilities::unknown()
    }
}

/// Build the configured provider from [`Config`] + its resolved API key.
/// The provider's `describe()` will use `config.display_name()` — the
/// resolved profile's human label if there is one, otherwise the raw
/// provider kind — so the header/`/status` never show a hard-coded label
/// that doesn't match what's actually configured.
pub fn build_provider(config: &Config, api_key: String) -> Result<Box<dyn Provider>> {
    let model = config.resolved_model()?;
    let display_name = config.display_name();

    match config.model.provider.as_str() {
        "nvidia" => Ok(Box::new(nvidia::NvidiaProvider::new(
            display_name,
            api_key,
            model,
            config.model.temperature,
            config.model.max_tokens,
            config.model.reasoning_effort.clone(),
        ))),
        "openai_compatible" => {
            let base_url = config
                .model
                .base_url
                .clone()
                .ok_or_else(|| anyhow!("A base URL is required for this provider. Run /connect or /base-url."))?;
            Ok(Box::new(openai_compatible::OpenAiCompatibleProvider::new(
                display_name,
                api_key,
                base_url,
                model,
                config.model.temperature,
                config.model.max_tokens,
            )))
        }
        "local" => {
            let base_url = config.model.base_url.clone().unwrap_or_else(|| "http://localhost:11434".to_string());
            Ok(Box::new(local::LocalProvider::new(
                display_name,
                base_url,
                api_key,
                model,
                config.model.temperature,
                config.model.max_tokens,
            )))
        }
        "gemini" => Ok(Box::new(gemini::GeminiProvider::new(
            display_name,
            api_key,
            model,
            config.model.temperature,
            config.model.max_tokens,
            config.model.base_url.clone(),
        ))),
        other => Err(anyhow!(
            "Unknown provider '{other}'. Expected one of: nvidia, openai_compatible, local, gemini."
        )),
    }
}

/// Stand-in used when a *runtime* provider/model/base-url change (via a
/// `/provider`, `/model`, `/base-url`, or `/api` command) fails to produce
/// a working provider — e.g. no API key resolvable yet. Swapping this in
/// (rather than leaving the previous, still-working provider silently
/// active) keeps the agent's actual behavior consistent with what
/// `/status` reports: if config says provider X, the agent really is
/// trying to use provider X, and says exactly why it can't rather than
/// quietly falling back to whatever was configured before.
pub struct UnconfiguredProvider {
    reason: String,
}

impl UnconfiguredProvider {
    pub fn new(reason: String) -> Self {
        Self { reason }
    }
}

#[async_trait]
impl Provider for UnconfiguredProvider {
    async fn chat(
        &self,
        _messages: &[ChatMessage],
        _tools: &[ToolDefinition],
        _on_event: Option<&StreamSink<'_>>,
    ) -> Result<ChatResult> {
        Err(anyhow!(
            "This provider isn't active: {}. Fix it and it'll retry automatically on your next message.",
            self.reason
        ))
    }

    fn describe(&self) -> String {
        format!("(not active: {})", self.reason)
    }
}

/// Helper shared by provider impls: parse a tool call's raw JSON argument
/// string into a `Value`, defaulting to an empty object on parse failure
/// rather than failing the whole turn.
pub fn parse_tool_arguments(raw: &str) -> Value {
    if raw.trim().is_empty() {
        return Value::Object(Default::default());
    }
    serde_json::from_str(raw).unwrap_or_else(|_| Value::Object(Default::default()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn unconfigured_provider_fails_loudly_with_the_real_reason() {
        let provider = UnconfiguredProvider::new("NVIDIA_API_KEY is not set".to_string());
        assert!(provider.describe().contains("NVIDIA_API_KEY is not set"));

        let err = provider.chat(&[], &[], None).await.unwrap_err();
        assert!(err.to_string().contains("NVIDIA_API_KEY is not set"));
    }

    #[test]
    fn build_provider_rejects_unknown_provider_kind() {
        let mut config = Config::load(&std::env::temp_dir()).unwrap();
        config.model.provider = "totally-not-a-provider".to_string();
        config.model.model = Some("x".to_string());
        let err = build_provider(&config, String::new()).err().unwrap();
        assert!(err.to_string().contains("Unknown provider"));
    }

    #[test]
    fn classifies_common_http_statuses() {
        assert_eq!(classify_http_status(401, ""), ProviderErrorKind::Authentication);
        assert_eq!(classify_http_status(403, ""), ProviderErrorKind::Authentication);
        assert_eq!(classify_http_status(404, ""), ProviderErrorKind::ModelNotFound);
        assert_eq!(classify_http_status(429, ""), ProviderErrorKind::RateLimited);
        assert_eq!(classify_http_status(500, ""), ProviderErrorKind::Server);
        assert_eq!(classify_http_status(503, ""), ProviderErrorKind::ModelUnavailable);
        assert_eq!(classify_http_status(400, "bad request"), ProviderErrorKind::InvalidRequest);
        assert_eq!(classify_http_status(299, ""), ProviderErrorKind::Unknown);
    }

    #[test]
    fn classifies_context_overflow_from_body_even_on_a_generic_400() {
        let kind = classify_http_status(400, r#"{"error":"context_length_exceeded: maximum context is 8192 tokens"}"#);
        assert_eq!(kind, ProviderErrorKind::ContextExceeded);
    }

    #[test]
    fn retryable_kinds_are_exactly_the_transient_ones() {
        assert!(ProviderErrorKind::RateLimited.is_retryable());
        assert!(ProviderErrorKind::Server.is_retryable());
        assert!(ProviderErrorKind::Network.is_retryable());
        assert!(!ProviderErrorKind::Authentication.is_retryable());
        assert!(!ProviderErrorKind::InvalidRequest.is_retryable());
        assert!(!ProviderErrorKind::ContextExceeded.is_retryable());
    }

    #[test]
    fn provider_error_kind_round_trips_through_anyhow() {
        let err: anyhow::Error = ProviderError::new(ProviderErrorKind::RateLimited, "slow down").into();
        assert_eq!(provider_error_kind(&err), ProviderErrorKind::RateLimited);
        assert_eq!(err.to_string(), "slow down");
    }

    #[test]
    fn provider_error_kind_is_unknown_for_a_plain_anyhow_error() {
        let err = anyhow!("something else went wrong");
        assert_eq!(provider_error_kind(&err), ProviderErrorKind::Unknown);
    }

    #[test]
    fn build_provider_requires_base_url_for_openai_compatible() {
        let mut config = Config::load(&std::env::temp_dir()).unwrap();
        config.model.provider = "openai_compatible".to_string();
        config.model.model = Some("x".to_string());
        config.model.base_url = None;
        let err = build_provider(&config, "key".to_string()).err().unwrap();
        assert!(err.to_string().contains("base URL") || err.to_string().contains("/connect"));
    }
}
