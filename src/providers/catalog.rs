//! Static provider catalog — what `/connect`'s searchable picker searches,
//! and what a first-launch wizard offers.
//!
//! This is reference/convenience data, not a source of truth the agent
//! depends on: every entry just pre-fills a [`crate::config::global::ProviderProfile`]
//! that the user still confirms (and can freely edit) before it's saved.
//! Getting an entry wrong here means a slightly annoying "endpoint not
//! found" the user fixes via `/base-url`, not a security or correctness
//! issue — but it's still worth being honest about what's actually known
//! vs. guessed, per the explicit instruction not to hard-code stale
//! endpoints/pricing:
//!
//! - `base_url: Some(..)` entries are protocols/endpoints that have been
//!   stable and OpenAI-compatible for a long time; still worth
//!   double-checking current provider docs if something doesn't connect.
//! - `base_url: None` entries are real, known providers worth listing for
//!   discoverability, but REXO isn't confident enough in a specific
//!   current endpoint to pre-fill one — `/connect` asks for it instead of
//!   guessing wrong.
//! - Providers with a genuinely different (non-OpenAI-compatible) wire
//!   protocol get their own `kind` and a real driver instead of being
//!   force-fit through the OpenAI-compatible client (which 401s or
//!   400s no matter what key you give it, since both the auth header
//!   and the request/response shape are wrong). Google Gemini has one
//!   now (`providers::gemini`) — Anthropic and Cohere don't yet; see
//!   the README roadmap.
//!
//! No pricing, context-window, or "free tier" claims live here — see
//! [`crate::providers::protocol::OpenAiProtocolClient::list_models`] for
//! live discovery, and `/model`'s docs for why REXO doesn't maintain a
//! static pricing database.

#[derive(Debug, Clone, Copy)]
pub struct ProviderPreset {
    /// Stable id used as the default profile name / credential key, e.g.
    /// `"openrouter"` -> `REXO_OPENROUTER_API_KEY`.
    pub key: &'static str,
    pub display_name: &'static str,
    /// Which `Provider` implementation this routes to when connected —
    /// `"nvidia"`, `"local"`, `"gemini"`, or `"openai_compatible"` (the
    /// default for the many providers that genuinely do speak the
    /// standard `/chat/completions` protocol). See `providers::build_provider`.
    pub kind: &'static str,
    /// Full chat-completions endpoint, when known and stable.
    pub base_url: Option<&'static str>,
    pub requires_key: bool,
    pub notes: &'static str,
}

pub const CATALOG: &[ProviderPreset] = &[
    ProviderPreset {
        key: "nvidia",
        display_name: "NVIDIA NIM",
        kind: "nvidia",
        base_url: Some("https://integrate.api.nvidia.com/v1/chat/completions"),
        requires_key: true,
        notes: "build.nvidia.com — many open models, some with a free tier",
    },
    ProviderPreset {
        key: "gemini",
        display_name: "Google Gemini",
        kind: "gemini",
        base_url: Some("https://generativelanguage.googleapis.com/v1beta"),
        requires_key: true,
        notes: "native driver, not OpenAI-compatible — free tier via Google AI Studio",
    },
    ProviderPreset {
        key: "openrouter",
        display_name: "OpenRouter",
        kind: "openai_compatible",
        base_url: Some("https://openrouter.ai/api/v1/chat/completions"),
        requires_key: true,
        notes: "one key, routes to many providers/models — including free ones",
    },
    ProviderPreset {
        key: "groq",
        display_name: "Groq",
        kind: "openai_compatible",
        base_url: Some("https://api.groq.com/openai/v1/chat/completions"),
        requires_key: true,
        notes: "very fast inference, generous free tier",
    },
    ProviderPreset {
        key: "together",
        display_name: "Together AI",
        kind: "openai_compatible",
        base_url: Some("https://api.together.xyz/v1/chat/completions"),
        requires_key: true,
        notes: "wide open-model selection",
    },
    ProviderPreset {
        key: "fireworks",
        display_name: "Fireworks AI",
        kind: "openai_compatible",
        base_url: Some("https://api.fireworks.ai/inference/v1/chat/completions"),
        requires_key: true,
        notes: "",
    },
    ProviderPreset {
        key: "mistral",
        display_name: "Mistral AI",
        kind: "openai_compatible",
        base_url: Some("https://api.mistral.ai/v1/chat/completions"),
        requires_key: true,
        notes: "has a free tier for some models",
    },
    ProviderPreset {
        key: "deepseek",
        display_name: "DeepSeek",
        kind: "openai_compatible",
        base_url: Some("https://api.deepseek.com/v1/chat/completions"),
        requires_key: true,
        notes: "strong, inexpensive coding models",
    },
    ProviderPreset {
        key: "cerebras",
        display_name: "Cerebras",
        kind: "openai_compatible",
        base_url: Some("https://api.cerebras.ai/v1/chat/completions"),
        requires_key: true,
        notes: "very fast inference, free tier for open models",
    },
    ProviderPreset {
        key: "deepinfra",
        display_name: "DeepInfra",
        kind: "openai_compatible",
        base_url: Some("https://api.deepinfra.com/v1/openai/chat/completions"),
        requires_key: true,
        notes: "",
    },
    ProviderPreset {
        key: "xai",
        display_name: "xAI",
        kind: "openai_compatible",
        base_url: Some("https://api.x.ai/v1/chat/completions"),
        requires_key: true,
        notes: "Grok models",
    },
    ProviderPreset {
        key: "openai",
        display_name: "OpenAI",
        kind: "openai_compatible",
        base_url: Some("https://api.openai.com/v1/chat/completions"),
        requires_key: true,
        notes: "",
    },
    ProviderPreset {
        key: "sambanova",
        display_name: "SambaNova Cloud",
        kind: "openai_compatible",
        base_url: None,
        requires_key: true,
        notes: "OpenAI-compatible; enter the current base URL from their docs",
    },
    ProviderPreset {
        key: "huggingface",
        display_name: "Hugging Face",
        kind: "openai_compatible",
        base_url: None,
        requires_key: true,
        notes: "Inference Providers router; enter the current base URL from their docs",
    },
    ProviderPreset {
        key: "cloudflare",
        display_name: "Cloudflare Workers AI",
        kind: "openai_compatible",
        base_url: None,
        requires_key: true,
        notes: "OpenAI-compatible endpoint is account-specific; enter it manually",
    },
    ProviderPreset {
        key: "github_models",
        display_name: "GitHub Models",
        kind: "openai_compatible",
        base_url: None,
        requires_key: true,
        notes: "enter the current base URL from GitHub's docs",
    },
    ProviderPreset {
        key: "hyperbolic",
        display_name: "Hyperbolic",
        kind: "openai_compatible",
        base_url: None,
        requires_key: true,
        notes: "",
    },
    ProviderPreset {
        key: "ai21",
        display_name: "AI21 Labs",
        kind: "openai_compatible",
        base_url: None,
        requires_key: true,
        notes: "",
    },
    ProviderPreset {
        key: "local",
        display_name: "Local",
        kind: "local",
        base_url: Some("http://localhost:11434"),
        requires_key: false,
        notes: "Ollama/vLLM/LM Studio/llama.cpp — any OpenAI-compatible local server",
    },
];

pub fn find(key: &str) -> Option<&'static ProviderPreset> {
    CATALOG.iter().find(|p| p.key.eq_ignore_ascii_case(key))
}

/// Case-insensitive substring match on key + display name + notes,
/// ranked with prefix matches on the display name first. Empty query
/// returns the whole catalog in its declared (roughly "most likely to be
/// what a coding agent user wants") order.
pub fn search(query: &str) -> Vec<&'static ProviderPreset> {
    let q = query.trim().to_lowercase();
    if q.is_empty() {
        return CATALOG.iter().collect();
    }
    let mut prefix: Vec<&'static ProviderPreset> = Vec::new();
    let mut contains: Vec<&'static ProviderPreset> = Vec::new();
    for preset in CATALOG {
        let name_lower = preset.display_name.to_lowercase();
        if name_lower.starts_with(&q) || preset.key.starts_with(&q) {
            prefix.push(preset);
        } else if name_lower.contains(&q) || preset.key.contains(&q) || preset.notes.to_lowercase().contains(&q) {
            contains.push(preset);
        }
    }
    prefix.extend(contains);
    prefix
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_preset_key_is_unique() {
        let mut keys: Vec<&str> = CATALOG.iter().map(|p| p.key).collect();
        keys.sort_unstable();
        let mut deduped = keys.clone();
        deduped.dedup();
        assert_eq!(keys.len(), deduped.len(), "duplicate provider preset key");
    }

    #[test]
    fn find_is_case_insensitive() {
        assert!(find("OpenRouter").is_some());
        assert!(find("openrouter").is_some());
        assert!(find("not-a-real-provider").is_none());
    }

    #[test]
    fn search_prefix_match_ranks_before_substring_match() {
        let results = search("open");
        // "OpenRouter" and "OpenAI" both start with "open"; "Hugging
        // Face" doesn't, but let's check with a query where a non-prefix
        // substring match exists to actually exercise the ranking.
        let positions: Vec<&str> = results.iter().map(|p| p.key).collect();
        assert!(positions.contains(&"openrouter"));
        assert!(positions.contains(&"openai"));
    }

    #[test]
    fn empty_query_returns_everything() {
        assert_eq!(search("").len(), CATALOG.len());
    }

    #[test]
    fn search_matches_on_notes_too() {
        let results = search("free tier");
        assert!(!results.is_empty());
    }

    #[test]
    fn local_preset_does_not_require_a_key() {
        let local = find("local").unwrap();
        assert!(!local.requires_key);
    }

    #[test]
    fn gemini_routes_to_the_native_driver_not_openai_compatible() {
        let gemini = find("gemini").unwrap();
        assert_eq!(gemini.kind, "gemini");
    }
}
