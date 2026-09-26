//! Layered configuration.
//!
//! Three layers, most specific wins — see `global::GlobalConfig`'s module
//! docs for the full resolution order and why this exists (it's the fix
//! for "REXO forgets its provider when I `cd` somewhere else"):
//!
//! 1. **Session** — runtime-only changes from `/provider`, `/model`, etc.
//!    when the user picks "this session only". Never touches disk; lives
//!    entirely in `cli::Session` for the process's lifetime.
//! 2. **Workspace** — `rexo.toml` next to wherever `rexo` was started
//!    from. Optional; only overrides what it explicitly sets.
//! 3. **Global** — `config.toml` in a per-user OS directory
//!    (`global::global_dir()`), loaded identically no matter which
//!    directory `rexo` runs from. What `/connect`'s "Permanently" choice
//!    and the first-launch wizard write to.
//!
//! Secrets (API keys) resolve separately from everything else — see
//! [`Config::api_key`] — and are **never** read from `rexo.toml`, so
//! workspace config files stay safe to commit.

pub mod credentials;
pub mod global;

use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context, Result};
use serde::Deserialize;

pub use global::GlobalConfig;

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct AgentSettings {
    pub max_iterations: usize,
    pub max_tool_calls: usize,
}

impl Default for AgentSettings {
    fn default() -> Self {
        Self { max_iterations: 50, max_tool_calls: 100 }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct ModelSettings {
    pub provider: String,
    pub model: Option<String>,
    pub base_url: Option<String>,
    pub temperature: f32,
    pub max_tokens: u32,
    /// Passed through to providers that support it (e.g. Kimi K3's
    /// `reasoning_effort`). Ignored by providers that don't.
    pub reasoning_effort: Option<String>,
}

impl Default for ModelSettings {
    fn default() -> Self {
        Self {
            provider: "nvidia".to_string(),
            model: None,
            base_url: None,
            temperature: 0.3,
            max_tokens: 4096,
            reasoning_effort: None,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct PermissionsConfig {
    pub allow_read: bool,
    pub allow_search: bool,
    pub allow_edit: bool,
    pub allow_terminal: bool,
    pub allow_git_write: bool,
    pub allow_network: bool,
}

impl Default for PermissionsConfig {
    fn default() -> Self {
        Self {
            allow_read: true,
            allow_search: true,
            allow_edit: false,
            allow_terminal: false,
            allow_git_write: false,
            allow_network: false,
        }
    }
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
struct FileConfig {
    agent: AgentSettings,
    model: ModelSettings,
    permissions: PermissionsConfig,
}

#[derive(Debug, Clone)]
pub struct Config {
    pub workspace: PathBuf,
    pub agent: AgentSettings,
    pub model: ModelSettings,
    pub permissions: PermissionsConfig,
    /// Human-readable label for the active provider ("OpenRouter"), when
    /// it was resolved from a named global profile rather than a bare
    /// `rexo.toml [model] provider = "..."` kind string. `None` falls
    /// back to `model.provider` itself for display.
    pub display_name: Option<String>,
    /// Which name to look the API key up under — in
    /// `REXO_<KEY>_API_KEY` and in the global credential store. `None`
    /// falls back to `model.provider` (the old, v0.2 behavior).
    pub credential_key: Option<String>,
    /// Loaded once at startup; mutated and re-saved by commands that
    /// change global settings (`/connect` "Permanently", `/model` with a
    /// permanent save, the first-launch wizard, `/mcp`).
    pub global: GlobalConfig,
}

impl Config {
    /// Load configuration for `workspace`: global settings first, then a
    /// workspace `rexo.toml` if present (only for the sections it
    /// explicitly sets — see [`workspace_sets_model`]), then a handful of
    /// `REXO_*` environment overrides on top of both.
    pub fn load(workspace: &Path) -> Result<Self> {
        let toml_path = workspace.join("rexo.toml");

        let (file_config, workspace_sets_model) = if toml_path.exists() {
            let raw = std::fs::read_to_string(&toml_path).with_context(|| format!("Failed to read {}", toml_path.display()))?;
            let parsed: FileConfig =
                toml::from_str(&raw).with_context(|| format!("Failed to parse {}", toml_path.display()))?;
            let has_model_table = raw.parse::<toml::Value>().ok().and_then(|v| v.get("model").cloned()).is_some();
            (parsed, has_model_table)
        } else {
            (FileConfig::default(), false)
        };

        let global = GlobalConfig::load()?;

        let mut model = file_config.model;
        let mut display_name = None;
        let mut credential_key = None;

        if !workspace_sets_model {
            if let Some(name) = &global.default_provider {
                if let Some(profile) = global.providers.get(name) {
                    model.provider = profile.kind.clone();
                    if let Some(m) = &profile.model {
                        model.model = Some(m.clone());
                    }
                    if let Some(b) = &profile.base_url {
                        model.base_url = Some(b.clone());
                    }
                    if let Some(t) = profile.temperature {
                        model.temperature = t;
                    }
                    if let Some(mt) = profile.max_tokens {
                        model.max_tokens = mt;
                    }
                    if profile.reasoning_effort.is_some() {
                        model.reasoning_effort = profile.reasoning_effort.clone();
                    }
                    display_name = Some(profile.display_name.clone());
                    credential_key = Some(profile.resolved_credential_key(name).to_string());
                }
            }
        }

        let mut config = Config {
            workspace: workspace.to_path_buf(),
            agent: file_config.agent,
            model,
            permissions: file_config.permissions,
            display_name,
            credential_key,
            global,
        };

        // Environment overrides win over both layers above (matches v0.2
        // behavior, now just documented as its place in the stack). An
        // explicit REXO_PROVIDER invalidates whatever named profile we
        // resolved above, since it may point somewhere else entirely.
        if let Ok(provider) = std::env::var("REXO_PROVIDER") {
            config.model.provider = provider;
            config.display_name = None;
            config.credential_key = None;
        }
        if let Ok(model) = std::env::var("REXO_MODEL") {
            config.model.model = Some(model);
        }
        if let Ok(base_url) = std::env::var("REXO_BASE_URL") {
            config.model.base_url = Some(base_url);
        }

        Ok(config)
    }

    /// Name this provider's credential resolves under, whether or not it
    /// came from a named global profile — falls back to the provider
    /// *kind* string (`"nvidia"`, `"openai_compatible"`, ...) so every
    /// code path has *something* to key credential lookups on.
    pub fn effective_credential_key(&self) -> String {
        self.credential_key.clone().unwrap_or_else(|| self.model.provider.clone())
    }

    /// The label to show the user: the resolved profile's display name if
    /// there is one, otherwise the raw provider kind.
    pub fn display_name(&self) -> String {
        self.display_name.clone().unwrap_or_else(|| self.model.provider.clone())
    }

    /// Legacy, unnamespaced environment variable name for this provider
    /// *kind* — kept for backward compatibility with v0.2 setups that
    /// predate named profiles/credential keys.
    pub fn api_key_env_var(&self) -> &'static str {
        match self.model.provider.as_str() {
            "nvidia" => "NVIDIA_API_KEY",
            "openai_compatible" => "OPENAI_API_KEY",
            "local" => "LOCAL_API_KEY",
            // Not a REXO-invented name — the same env var Anthropic's own
            // SDKs and CLI tools already read, so a key someone has set
            // for other Anthropic tooling just works here too, no
            // REXO_-namespaced var required.
            "anthropic" => "ANTHROPIC_API_KEY",
            _ => "REXO_API_KEY",
        }
    }

    /// Resolve the API key for the configured provider:
    /// 1. `REXO_<CREDENTIAL_KEY>_API_KEY` (namespaced, works for any named profile)
    /// 2. the legacy unnamespaced var for built-in kinds (v0.2 compat)
    /// 3. the global credential store (`/connect`'s "Permanently" choice)
    ///
    /// Local providers typically need no key, so an empty value is
    /// tolerated there and rejected everywhere else. Never logs or
    /// includes the key itself in any error.
    pub fn api_key(&self) -> Result<String> {
        let key_name = self.effective_credential_key();
        let namespaced_var = format!("REXO_{}_API_KEY", env_safe(&key_name));

        if let Ok(v) = std::env::var(&namespaced_var) {
            if !v.trim().is_empty() {
                return Ok(v);
            }
        }
        if let Ok(v) = std::env::var(self.api_key_env_var()) {
            if !v.trim().is_empty() {
                return Ok(v);
            }
        }
        if let Ok(store) = credentials::CredentialStore::open() {
            if let Ok(Some(v)) = store.get(&key_name) {
                if !v.trim().is_empty() {
                    return Ok(v);
                }
            }
        }

        if self.model.provider == "local" {
            return Ok(String::new());
        }

        Err(anyhow!(
            "No API key found for {}. Set {namespaced_var} in your environment, run /connect, \
             or run /api set.",
            self.display_name()
        ))
    }

    pub fn resolved_model(&self) -> Result<String> {
        self.model
            .model
            .clone()
            .ok_or_else(|| anyhow!("No model configured. Set [model].model in rexo.toml, REXO_MODEL, or run /model."))
    }
}

/// Upper-case and replace anything that isn't `[A-Z0-9_]` with `_`, so an
/// arbitrary credential key (a profile name the user typed, possibly with
/// spaces or hyphens) turns into a valid, predictable environment
/// variable name — `"my work key"` -> `REXO_MY_WORK_KEY_API_KEY`.
fn env_safe(key: &str) -> String {
    key.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_uppercase() } else { '_' })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use global::ProviderProfile;

    #[test]
    fn defaults_are_conservative_on_permissions() {
        let perms = PermissionsConfig::default();
        assert!(perms.allow_read);
        assert!(perms.allow_search);
        assert!(!perms.allow_edit);
        assert!(!perms.allow_terminal);
        assert!(!perms.allow_git_write);
    }

    #[test]
    fn missing_rexo_toml_falls_back_to_defaults() {
        global::with_temp_global_dir("config_missing", || {
            let dir = std::env::temp_dir().join("rexo_test_config_missing");
            std::fs::create_dir_all(&dir).unwrap();
            let _ = std::fs::remove_file(dir.join("rexo.toml"));
            let config = Config::load(&dir).unwrap();
            assert_eq!(config.agent.max_iterations, 50);
            // No global profile in this isolated test dir, no workspace
            // override -> the hard-coded fallback default.
            assert_eq!(config.model.provider, "nvidia");
        });
    }

    #[test]
    fn parses_partial_rexo_toml() {
        global::with_temp_global_dir("config_partial", || {
            let dir = std::env::temp_dir().join("rexo_test_config_partial");
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join("rexo.toml"), "[model]\nprovider = \"nvidia\"\nmodel = \"moonshotai/kimi-k3\"\n").unwrap();
            let config = Config::load(&dir).unwrap();
            assert_eq!(config.model.model.as_deref(), Some("moonshotai/kimi-k3"));
            assert_eq!(config.agent.max_iterations, 50);
            assert!(!config.permissions.allow_edit);
        });
    }

    #[test]
    fn workspace_rexo_toml_model_section_overrides_any_global_profile() {
        global::with_temp_global_dir("config_workspace_wins", || {
            let dir = std::env::temp_dir().join("rexo_test_config_workspace_wins");
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join("rexo.toml"), "[model]\nprovider = \"local\"\nmodel = \"qwen\"\n").unwrap();

            // Write a real global profile first, so this genuinely proves
            // workspace beats global rather than global just being empty.
            let mut global_cfg = GlobalConfig::default();
            global_cfg.default_provider = Some("openrouter".to_string());
            global_cfg.providers.insert(
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
            global_cfg.save().unwrap();

            let config = Config::load(&dir).unwrap();
            assert_eq!(config.model.provider, "local");
            assert_eq!(config.model.model.as_deref(), Some("qwen"));
        });
    }

    #[test]
    fn effective_credential_key_falls_back_to_provider_kind() {
        global::with_temp_global_dir("config_credkey_fallback", || {
            let dir = std::env::temp_dir().join("rexo_test_config_credkey_fallback");
            std::fs::create_dir_all(&dir).unwrap();
            let _ = std::fs::remove_file(dir.join("rexo.toml"));
            let config = Config::load(&dir).unwrap();
            assert_eq!(config.effective_credential_key(), "nvidia");
            assert_eq!(config.display_name(), "nvidia");
        });
    }

    #[test]
    fn env_safe_produces_a_valid_variable_name() {
        assert_eq!(env_safe("openrouter"), "OPENROUTER");
        assert_eq!(env_safe("my work key"), "MY_WORK_KEY");
        assert_eq!(env_safe("work-account-2"), "WORK_ACCOUNT_2");
    }

    #[test]
    fn api_key_prefers_namespaced_env_var_over_legacy() {
        global::with_temp_global_dir("config_namespaced_key", || {
            let dir = std::env::temp_dir().join("rexo_test_config_namespaced_key");
            std::fs::create_dir_all(&dir).unwrap();
            let _ = std::fs::remove_file(dir.join("rexo.toml"));
            let mut config = Config::load(&dir).unwrap();
            config.credential_key = Some("myprofile".to_string());

            std::env::set_var("REXO_MYPROFILE_API_KEY", "sk-namespaced");
            std::env::set_var("NVIDIA_API_KEY", "nv-legacy");
            let key = config.api_key().unwrap();
            std::env::remove_var("REXO_MYPROFILE_API_KEY");
            std::env::remove_var("NVIDIA_API_KEY");
            assert_eq!(key, "sk-namespaced");
        });
    }
}
