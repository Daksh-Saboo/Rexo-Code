//! Interactive REPL command layer.
//!
//! This module is the only place that knows about `/slash-commands` — it
//! sits *on top of* the existing agent/provider/security/config
//! architecture and drives it, rather than duplicating any of it. A
//! command handler only ever gets there through [`Session`], which bundles
//! the few pieces of runtime state a command might need to read or change:
//! the resolved [`Config`], the live [`Agent`] (whose provider/workspace it
//! can swap in place), and the conversation history.
//!
//! See [`commands`] for the command table, [`parser`] for how a raw input
//! line is classified as a command vs. a plain prompt for the model, and
//! [`tui`] for the persistent full-screen interface that drives all of
//! the above interactively.

pub mod commands;
pub mod completion;
pub mod custom_commands;
pub mod doctor;
pub mod file_ref;
pub mod frontmatter;
pub mod parser;
pub mod picker;
pub mod sessions;
pub mod skills;
pub mod trust_dialog;
pub mod tui;
pub mod wizard;

use std::path::PathBuf;
use std::time::Instant;

use anyhow::Result;
use ratatui::style::Color;

use crate::agent::Agent;
use crate::config::Config;
use crate::providers::{self, ChatMessage};

/// What the REPL loop should do after a command finishes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandOutcome {
    Continue,
    Exit,
}

/// UI accent color, set with `/color`. Kept as a small enum (rather than
/// a raw `ratatui::style::Color`) so `cli::commands` — which otherwise
/// knows nothing about ratatui — can offer it as a plain named choice.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccentColor {
    Cyan,
    Green,
    Magenta,
    Yellow,
    Blue,
}

impl AccentColor {
    pub fn name(self) -> &'static str {
        match self {
            AccentColor::Cyan => "cyan",
            AccentColor::Green => "green",
            AccentColor::Magenta => "magenta",
            AccentColor::Yellow => "yellow",
            AccentColor::Blue => "blue",
        }
    }

    pub fn to_ratatui(self) -> Color {
        match self {
            AccentColor::Cyan => Color::Cyan,
            AccentColor::Green => Color::Green,
            AccentColor::Magenta => Color::Magenta,
            AccentColor::Yellow => Color::Yellow,
            AccentColor::Blue => Color::Blue,
        }
    }
}

impl Default for AccentColor {
    fn default() -> Self {
        AccentColor::Cyan
    }
}

/// A `/loop` job: repeat `text` as if the user had typed it, every
/// `interval_secs`. Checked by the TUI's idle loop, not by any command
/// handler directly — a handler can't "wait around" on its own.
#[derive(Debug, Clone)]
pub struct LoopJob {
    pub interval_secs: u64,
    pub text: String,
    pub next_at: Instant,
}

/// Small, purely cosmetic/behavioral preferences slash commands toggle
/// that the TUI reads every frame — kept on `Session` (not inside
/// `cli::tui::TuiCore`) so `cli::commands` handlers, which only ever see
/// `&mut Session`, can reach them without depending on the TUI at all.
pub struct UiPreferences {
    /// Set by `/rename`; shown in the header in place of the default title.
    pub title: Option<String>,
    /// Set by `/focus`; hides tool-call/tool-result lines in the
    /// transcript, showing just prompts and final answers.
    pub focus_mode: bool,
    /// Set by `/plan`; also nudges the model via a system reminder.
    pub plan_mode: bool,
    /// Set by `/goal`; also nudges the model via a system reminder.
    pub goal: Option<String>,
    /// Set by `/color`.
    pub accent: AccentColor,
    /// Set by `/scroll-speed`; lines moved per scroll key press.
    pub scroll_step: u16,
    /// Set by `/autocompact`; auto-run `/compact`'s logic once this many
    /// tool results have accumulated. `None` = off.
    pub autocompact_threshold: Option<usize>,
    /// Set by `/loop`; `None` = no loop active.
    pub loop_job: Option<LoopJob>,
    /// Toggled by typing a bare `!`. While `true`, `run_inner` sends every
    /// non-slash-command line straight to the OS shell instead of the
    /// model — see `cli::tui::run_shell_command`.
    pub shell_mode: bool,
    /// Toggled by Ctrl+O. Adds turn-level diagnostic detail (elapsed
    /// time, context size) to the transcript — see `cli::tui::run_turn`.
    pub verbose: bool,
    /// Toggled by Ctrl+T. Whether the task-list panel (backed by the
    /// `manage_tasks` tool, see `agent::Agent::tasks`) is currently
    /// shown — see `cli::tui::render::draw`.
    pub tasks_visible: bool,
    /// Toggled by Ctrl+B (or bare `/agents`/`/background`). Whether the
    /// background-jobs panel (defined personas plus everything spawned
    /// via `/agents run` or `/background` — see `agent::background`) is
    /// currently shown, and which row is selected for Enter/delete —
    /// see `cli::tui::render::draw_agents_panel`.
    pub agents_panel_visible: bool,
    pub agents_panel_selected: usize,
}

impl Default for UiPreferences {
    fn default() -> Self {
        Self {
            title: None,
            focus_mode: false,
            plan_mode: false,
            goal: None,
            accent: AccentColor::default(),
            scroll_step: 3,
            autocompact_threshold: None,
            loop_job: None,
            shell_mode: false,
            verbose: false,
            tasks_visible: false,
            agents_panel_visible: false,
            agents_panel_selected: 0,
        }
    }
}

/// Bundles the mutable state slash commands read or change. Owned by
/// `main.rs`'s interactive loop for the lifetime of the session.
pub struct Session {
    pub config: Config,
    pub agent: Agent,
    pub history: Vec<ChatMessage>,
    /// Set by `/api set`; overrides the environment-variable-based key
    /// resolution for the *current* provider, for this run only. Cleared
    /// whenever `/provider` switches to a different provider, since a key
    /// typed in for one provider almost never applies to another.
    pub api_key_override: Option<String>,
    /// The workspace directory REXO was started in — `/reset` returns
    /// configuration to defaults but deliberately leaves the workspace
    /// alone (see `/reset`'s own explanation to the user, and `/status`,
    /// which shows this whenever it differs from the current workspace).
    pub startup_workspace: PathBuf,
    pub ui: UiPreferences,
    /// Names of skills already injected into `history` this session
    /// (auto-triggered or via `/skill`) — so a repeated mention of the
    /// same trigger word doesn't re-inject the same skill body every
    /// turn. Cleared on `/clear`/`/reset`, same as history.
    pub loaded_skills: std::collections::HashSet<String>,
    /// Which saved-session file autosaves go to. `None` = the default
    /// `autosave.json`; `Some(name)` after `/fork <name>` redirects
    /// future autosaves to `<name>.json` instead, leaving whatever was in
    /// `autosave.json` at fork time alone as a `/resume`-able snapshot of
    /// "before the fork." See `cli::sessions`.
    pub active_session_name: Option<String>,
}

impl Session {
    pub fn new(config: Config, agent: Agent, history: Vec<ChatMessage>) -> Self {
        let startup_workspace = config.workspace.clone();
        Self {
            config,
            agent,
            history,
            api_key_override: None,
            startup_workspace,
            ui: UiPreferences::default(),
            loaded_skills: std::collections::HashSet::new(),
            active_session_name: None,
        }
    }

    /// Whether *some* key is available for the current provider — an
    /// override from `/api set`, or one resolvable from the environment.
    /// Never returns the key itself.
    pub fn api_key_configured(&self) -> bool {
        self.api_key_override.is_some() || self.config.api_key().is_ok()
    }

    /// Resolve the actual key to use for building a provider: the session
    /// override if `/api set` was used, otherwise whatever `Config::api_key`
    /// resolves from the environment.
    fn resolve_api_key(&self) -> Result<String> {
        if let Some(key) = &self.api_key_override {
            return Ok(key.clone());
        }
        self.config.api_key()
    }

    /// Rebuild the active provider from the session's current `config` +
    /// resolved API key, and swap it into the live agent. Called after any
    /// command that changes provider/model/base-url/api settings.
    ///
    /// On failure, the *old* provider is deliberately not left in place —
    /// that would make `/status` (which reads `config`) lie about what the
    /// agent is actually using. Instead the agent gets a stub provider
    /// that fails loudly, with the real reason, the moment it's next used.
    pub fn rebuild_provider(&mut self) -> Result<()> {
        let api_key = self.resolve_api_key();
        let api_key = match api_key {
            Ok(k) => k,
            Err(e) => {
                self.agent
                    .set_provider(Box::new(providers::UnconfiguredProvider::new(e.to_string())));
                return Err(e);
            }
        };

        match providers::build_provider(&self.config, api_key) {
            Ok(provider) => {
                self.agent.set_provider(provider);
                Ok(())
            }
            Err(e) => {
                self.agent
                    .set_provider(Box::new(providers::UnconfiguredProvider::new(e.to_string())));
                Err(e)
            }
        }
    }

    /// Reset conversation history to a single, freshly-built system
    /// message for the agent's current workspace. Used by `/clear`,
    /// `/reset`, and after a workspace change.
    pub fn reset_history(&mut self) {
        self.history = vec![self.agent.build_system_message()];
        self.loaded_skills.clear();
    }
}
