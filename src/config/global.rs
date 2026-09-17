//! Global, per-user, workspace-independent configuration — the layer that
//! fixes "REXO forgets its provider when I `cd` somewhere else".
//!
//! Lives in an OS-appropriate per-user directory
//! (`dirs::data_local_dir()` — `%LOCALAPPDATA%\RexoCode` on Windows,
//! `~/.local/share/rexo-code` on Linux, `~/Library/Application
//! Support/rexo-code` on macOS), never inside a workspace, so it loads
//! identically no matter which directory `rexo` is started from — unlike
//! the old setup, where the *only* place provider/model settings could
//! live was a `rexo.toml` next to wherever you happened to run `rexo`
//! from, and the *only* place credentials could live was a `.env` in that
//! same directory. Neither followed you to a different project.
//!
//! # Resolution order
//!
//! See [`crate::config::Config::load`] for exactly where this fits, but
//! in short, most specific wins:
//!
//! ```text
//! CLI flags (--provider/--model/--base-url)
//!     ↓ (highest)
//! session-only runtime changes (/provider, /model, ... when the user
//! picks "this session only" rather than "permanently")
//!     ↓
//! REXO_PROVIDER / REXO_MODEL / REXO_BASE_URL environment variables
//!     ↓
//! workspace rexo.toml's [model] section, if present
//!     ↓
//! this file's default_provider profile
//!     ↓ (lowest)
//! built-in defaults
//! ```
//!
//! Credentials resolve separately (see [`crate::config::Config::api_key`]):
//! session override → `REXO_<CREDENTIAL_KEY>_API_KEY` env var (plus a
//! couple of legacy unnamespaced vars, for existing v0.2 users) → the
//! [`crate::config::credentials::CredentialStore`] → error.

use std::collections::BTreeMap;
use std::path::PathBuf;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// The one per-user directory everything workspace-independent lives
/// under: `config.toml`, `credentials.toml`. Deliberately the same
/// `%LOCALAPPDATA%\RexoCode` the installer already puts `rexo.exe` in
/// (see `install.ps1`), so there's exactly one "where does REXO keep its
/// stuff" answer, not two.
///
/// Honors `REXO_GLOBAL_DIR` as an override when set — mainly so this
/// module's own tests (and `cli::wizard`'s) never read or write the real
/// per-user directory, but it's a legitimate escape hatch for anyone who
/// wants REXO's global state somewhere else too (a portable install on a
/// USB drive, a container with a mounted config volume, ...).
pub fn global_dir() -> Result<PathBuf> {
    if let Ok(dir) = std::env::var("REXO_GLOBAL_DIR") {
        if !dir.trim().is_empty() {
            return Ok(PathBuf::from(dir));
        }
    }
    let base =
        dirs::data_local_dir().ok_or_else(|| anyhow::anyhow!("Couldn't determine your OS's per-user config directory"))?;
    Ok(base.join("RexoCode"))
}

fn config_path() -> Result<PathBuf> {
    Ok(global_dir()?.join("config.toml"))
}

/// A named, saved provider setup — what `/connect`'s "Permanently" choice
/// (or the first-launch wizard) writes, and what `default_provider` and
/// `/provider <name>` point at by name.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderProfile {
    /// Shown in the header/`/status` — "OpenRouter", not an internal key.
    pub display_name: String,
    /// Dispatch kind: which `Provider` implementation this profile builds.
    /// One of "nvidia" | "openai_compatible" | "local" today.
    pub kind: String,
    pub base_url: Option<String>,
    pub model: Option<String>,
    pub temperature: Option<f32>,
    pub max_tokens: Option<u32>,
    pub reasoning_effort: Option<String>,
    /// Which credential this profile's API key is stored/looked up under
    /// (in the credential store, and as `REXO_<KEY>_API_KEY` uppercased).
    /// Defaults to the profile's own name if not set.
    #[serde(default)]
    pub credential_key: Option<String>,
    #[serde(default)]
    pub requires_key: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct McpServerConfig {
    /// A local command to launch and speak MCP over stdio to, e.g.
    /// `"npx -y @modelcontextprotocol/server-filesystem /path"`.
    pub command: Option<String>,
    /// Or a remote MCP server URL (SSE/HTTP transport), if `command` isn't set.
    pub url: Option<String>,
    pub enabled: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct GlobalUiPrefs {
    pub accent: Option<String>,
    pub scroll_step: Option<u16>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct GlobalConfig {
    pub default_provider: Option<String>,
    #[serde(default)]
    pub providers: BTreeMap<String, ProviderProfile>,
    #[serde(default)]
    pub mcp_servers: BTreeMap<String, McpServerConfig>,
    #[serde(default)]
    pub ui: GlobalUiPrefs,
    /// Set once the first-launch setup wizard has run (or been explicitly
    /// skipped), so it never nags a second time. See `cli::wizard`.
    #[serde(default)]
    pub setup_completed: bool,
}

impl GlobalConfig {
    /// Load `config.toml` from the global directory, or an empty
    /// (`setup_completed: false`) config if it doesn't exist yet — that's
    /// the normal, expected state on a fresh install, not an error.
    pub fn load() -> Result<Self> {
        let path = config_path()?;
        if !path.exists() {
            return Ok(Self::default());
        }
        let raw = std::fs::read_to_string(&path).with_context(|| format!("Failed to read {}", path.display()))?;
        toml::from_str(&raw).with_context(|| {
            format!(
                "Failed to parse {} — it looks corrupted. Fix or delete it to reset global \
                 settings (this won't touch your saved credentials in credentials.toml).",
                path.display()
            )
        })
    }

    pub fn save(&self) -> Result<()> {
        let path = config_path()?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).with_context(|| format!("Failed to create {}", parent.display()))?;
        }
        let raw = toml::to_string_pretty(self)?;
        std::fs::write(&path, raw).with_context(|| format!("Failed to write {}", path.display()))
    }

    pub fn default_profile(&self) -> Option<&ProviderProfile> {
        self.default_provider.as_ref().and_then(|name| self.providers.get(name))
    }

    pub fn path() -> Result<PathBuf> {
        config_path()
    }
}

impl ProviderProfile {
    /// The name a `REXO_<...>_API_KEY` env var / credential-store entry
    /// should use for this profile: its explicit `credential_key` if set,
    /// otherwise whatever name it's registered under in `providers`. Takes
    /// the profile's map key as a fallback since the profile itself
    /// doesn't know its own map key.
    pub fn resolved_credential_key<'a>(&'a self, map_key: &'a str) -> &'a str {
        self.credential_key.as_deref().unwrap_or(map_key)
    }
}

/// `REXO_GLOBAL_DIR` is process-wide state and `cargo test` runs tests in
/// parallel by default — anything that sets it needs to hold this for the
/// duration, or two tests' writes/reads can interleave. Shared across
/// `config::global`, `config` (top-level `Config::load` tests), and
/// `cli::wizard`'s tests, all of which touch the same env var.
#[cfg(test)]
pub(crate) static TEST_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Point `REXO_GLOBAL_DIR` at a fresh temp directory for the duration of
/// `f`, holding [`TEST_ENV_LOCK`] so no other test's global-dir override
/// can interleave with this one. Every test anywhere in the crate that
/// calls `Config::load`, `GlobalConfig::load/save`, or
/// `cli::wizard::maybe_run` should go through this — otherwise it's
/// silently reading/writing whatever machine happens to run the suite.
#[cfg(test)]
pub(crate) fn with_temp_global_dir<T>(name: &str, f: impl FnOnce() -> T) -> T {
    let _guard = TEST_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = std::env::temp_dir().join(format!("rexo_test_global_{name}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::env::set_var("REXO_GLOBAL_DIR", &dir);
    let result = f();
    std::env::remove_var("REXO_GLOBAL_DIR");
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_empty_and_not_setup_completed() {
        let cfg = GlobalConfig::default();
        assert!(!cfg.setup_completed);
        assert!(cfg.default_profile().is_none());
    }

    #[test]
    fn round_trips_through_toml() {
        let mut cfg = GlobalConfig::default();
        cfg.default_provider = Some("openrouter".to_string());
        cfg.providers.insert(
            "openrouter".to_string(),
            ProviderProfile {
                display_name: "OpenRouter".to_string(),
                kind: "openai_compatible".to_string(),
                base_url: Some("https://openrouter.ai/api/v1/chat/completions".to_string()),
                model: Some("openrouter/auto".to_string()),
                temperature: None,
                max_tokens: None,
                reasoning_effort: None,
                credential_key: None,
                requires_key: true,
            },
        );
        cfg.setup_completed = true;

        let raw = toml::to_string_pretty(&cfg).unwrap();
        let parsed: GlobalConfig = toml::from_str(&raw).unwrap();
        assert_eq!(parsed.default_provider.as_deref(), Some("openrouter"));
        assert!(parsed.setup_completed);
        let profile = parsed.default_profile().unwrap();
        assert_eq!(profile.display_name, "OpenRouter");
        assert_eq!(profile.model.as_deref(), Some("openrouter/auto"));
    }

    #[test]
    fn resolved_credential_key_falls_back_to_map_key() {
        let profile = ProviderProfile {
            display_name: "OpenRouter".to_string(),
            kind: "openai_compatible".to_string(),
            base_url: None,
            model: None,
            temperature: None,
            max_tokens: None,
            reasoning_effort: None,
            credential_key: None,
            requires_key: true,
        };
        assert_eq!(profile.resolved_credential_key("openrouter"), "openrouter");

        let mut named = profile;
        named.credential_key = Some("work-account".to_string());
        assert_eq!(named.resolved_credential_key("openrouter"), "work-account");
    }

    #[test]
    fn global_dir_honors_the_test_override() {
        with_temp_global_dir("dir_override", || {
            let dir = global_dir().unwrap();
            assert!(dir.to_string_lossy().contains("rexo_test_global_dir_override"));
        });
    }

    #[test]
    fn save_then_load_round_trips_on_disk() {
        with_temp_global_dir("save_load", || {
            let mut cfg = GlobalConfig::default();
            cfg.setup_completed = true;
            cfg.default_provider = Some("nvidia".to_string());
            cfg.save().unwrap();

            let loaded = GlobalConfig::load().unwrap();
            assert!(loaded.setup_completed);
            assert_eq!(loaded.default_provider.as_deref(), Some("nvidia"));
        });
    }

    #[test]
    fn load_with_no_file_yet_is_the_default_not_an_error() {
        with_temp_global_dir("no_file_yet", || {
            let loaded = GlobalConfig::load().unwrap();
            assert!(!loaded.setup_completed);
        });
    }
}
