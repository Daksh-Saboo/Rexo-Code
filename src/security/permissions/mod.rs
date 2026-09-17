//! Permission engine: decides whether a tool call that needs approval is
//! actually allowed to run, by combining static config ("always allow
//! edits"), session grants ("allow terminal for the rest of this run"),
//! and — when running interactively — a `y/N` prompt on stdin.

use std::collections::HashSet;
use std::io::{self, Write};

use anyhow::Result;
use colored::Colorize;

use crate::config::PermissionsConfig;
use crate::security::policies::RiskLevel;

/// The category of permission being requested. Kept coarse-grained on
/// purpose — fine-grained detail (which path, which command) goes in the
/// human-readable summary shown to the user.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PermissionKind {
    Edit,
    CreateFile,
    DeleteFile,
    Terminal,
    GitWrite,
}

impl PermissionKind {
    pub fn label(self) -> &'static str {
        match self {
            PermissionKind::Edit => "edit a file",
            PermissionKind::CreateFile => "create a file",
            PermissionKind::DeleteFile => "delete a file",
            PermissionKind::Terminal => "run a terminal command",
            PermissionKind::GitWrite => "run a state-changing git command",
        }
    }

    pub fn all() -> [PermissionKind; 5] {
        [
            PermissionKind::Edit,
            PermissionKind::CreateFile,
            PermissionKind::DeleteFile,
            PermissionKind::Terminal,
            PermissionKind::GitWrite,
        ]
    }
}

/// The decision returned for a single permission check.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    Allowed,
    Denied,
}

/// A user's raw answer to an interactive "may REXO do X?" prompt — richer
/// than [`Decision`] because "always" needs to be recorded as a session
/// grant by [`PermissionManager::check_interactive`], not just resolved to
/// an allow/deny for this one call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PermResponse {
    Once,
    Always,
    Deny,
}

/// Whatever's driving the actual interactive prompt — a real terminal
/// question, backed by however that terminal happens to be set up right
/// now. Defined here (rather than in `cli::tui`, which is the only real
/// implementer) so this module stays free of any dependency on how the
/// REPL renders itself; `cli::tui::TuiCore` just implements this trait for
/// its own type. `?Send` because the one real implementation holds a
/// `Terminal`/event stream that has no reason to cross threads, and this
/// is only ever awaited in place, never spawned.
#[async_trait::async_trait(?Send)]
pub trait PermissionPrompter {
    async fn ask(&mut self, kind: PermissionKind, summary: &str, risk: RiskLevel) -> Result<PermResponse>;
}

pub struct PermissionManager {
    config: PermissionsConfig,
    /// Kinds the user has approved for the remainder of this session via
    /// the "always" response to a prompt.
    granted_session: HashSet<PermissionKind>,
    /// True when there is no interactive terminal to prompt on (e.g. `-y`
    /// / CI mode). Anything not pre-approved by config is denied outright
    /// instead of hanging on a read from stdin.
    non_interactive: bool,
    /// `-y` / `--yes`: treat every prompt as approved. Distinct from
    /// config's `allow_*` flags so it's an explicit, visible per-run choice
    /// rather than a persisted default.
    auto_approve: bool,
}

impl PermissionManager {
    pub fn new(config: PermissionsConfig, non_interactive: bool, auto_approve: bool) -> Self {
        Self {
            config,
            granted_session: HashSet::new(),
            non_interactive,
            auto_approve,
        }
    }

    fn config_allows(&self, kind: PermissionKind) -> bool {
        match kind {
            PermissionKind::Edit => self.config.allow_edit,
            PermissionKind::CreateFile => self.config.allow_edit,
            PermissionKind::DeleteFile => self.config.allow_edit,
            PermissionKind::Terminal => self.config.allow_terminal,
            PermissionKind::GitWrite => self.config.allow_git_write,
        }
    }

    /// `/permissions <kind> always|ask` — flips the persistent-for-this-run
    /// auto-allow flag for `kind`. This mutates the *live* permission
    /// state (not just the `Config` the caller may also want to keep in
    /// sync for `/status`/`/config` display).
    pub fn set_auto_allow(&mut self, kind: PermissionKind, allowed: bool) {
        match kind {
            PermissionKind::Edit | PermissionKind::CreateFile | PermissionKind::DeleteFile => {
                self.config.allow_edit = allowed;
            }
            PermissionKind::Terminal => self.config.allow_terminal = allowed,
            PermissionKind::GitWrite => self.config.allow_git_write = allowed,
        }
    }

    /// The live permission config, reflecting any `/permissions` changes
    /// made this session — the source of truth for `/status`/`/permissions`.
    pub fn current_config(&self) -> &PermissionsConfig {
        &self.config
    }

    /// Whether `kind` was approved via the "always" response to a runtime
    /// prompt (as opposed to being auto-allowed by config).
    pub fn is_session_granted(&self, kind: PermissionKind) -> bool {
        self.granted_session.contains(&kind)
    }

    /// How this run was launched — preserved by callers (e.g. `/reset`)
    /// that rebuild a fresh `PermissionManager` from a fresh `Config`
    /// without re-parsing CLI flags.
    pub fn is_non_interactive(&self) -> bool {
        self.non_interactive
    }

    pub fn is_auto_approve(&self) -> bool {
        self.auto_approve
    }

    /// Drop all "always this session" grants. Called when the workspace
    /// changes, so trust extended in one project doesn't silently carry
    /// over to a different one opened in the same run.
    pub fn clear_session_grants(&mut self) {
        self.granted_session.clear();
    }

    /// Ask for (or silently resolve) permission to perform `kind`, described
    /// to the user by `summary`. Returns `Decision::Denied` instead of an
    /// `Err` on refusal — a denial is a normal, expected outcome that the
    /// agent loop turns into a tool-result message so the model can adapt.
    pub fn check(&mut self, kind: PermissionKind, summary: &str, risk: RiskLevel) -> Result<Decision> {
        if self.config_allows(kind) || self.granted_session.contains(&kind) {
            return Ok(Decision::Allowed);
        }

        if self.auto_approve {
            return Ok(Decision::Allowed);
        }

        if self.non_interactive {
            return Ok(Decision::Denied);
        }

        self.prompt(kind, summary, risk)
    }

    /// Same short-circuits as [`check`](Self::check), but for callers that
    /// can't do a blocking `stdin` read to ask the human — the TUI, whose
    /// terminal is in raw mode and needs to keep redrawing (spinner, live
    /// header) while it waits. `prompter` is asked only when a real
    /// decision is actually needed; the config/session-grant/auto-approve/
    /// non-interactive fast paths never touch it.
    pub async fn check_interactive<P: PermissionPrompter>(
        &mut self,
        kind: PermissionKind,
        summary: &str,
        risk: RiskLevel,
        prompter: &mut P,
    ) -> Result<Decision> {
        if self.config_allows(kind) || self.granted_session.contains(&kind) {
            return Ok(Decision::Allowed);
        }
        if self.auto_approve {
            return Ok(Decision::Allowed);
        }
        if self.non_interactive {
            return Ok(Decision::Denied);
        }

        match prompter.ask(kind, summary, risk).await? {
            PermResponse::Once => Ok(Decision::Allowed),
            PermResponse::Always => {
                self.granted_session.insert(kind);
                Ok(Decision::Allowed)
            }
            PermResponse::Deny => Ok(Decision::Denied),
        }
    }

    fn prompt(&mut self, kind: PermissionKind, summary: &str, risk: RiskLevel) -> Result<Decision> {
        let risk_tag = match risk {
            RiskLevel::Moderate => "moderate risk".yellow(),
            RiskLevel::High => "high risk".red().bold(),
        };

        println!();
        println!(
            "{} REXO wants to {} ({})",
            "?".bright_yellow().bold(),
            kind.label(),
            risk_tag
        );
        println!("  {}", summary);
        print!(
            "  {} ",
            "[y] once   [a] always this session   [N] deny ›".dimmed()
        );
        io::stdout().flush().ok();

        let mut input = String::new();
        io::stdin().read_line(&mut input)?;
        let answer = input.trim().to_lowercase();

        match answer.as_str() {
            "y" | "yes" => Ok(Decision::Allowed),
            "a" | "always" => {
                self.granted_session.insert(kind);
                Ok(Decision::Allowed)
            }
            _ => Ok(Decision::Denied),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::PermissionsConfig;

    #[test]
    fn config_auto_allow_short_circuits_prompt() {
        let mut cfg = PermissionsConfig::default();
        cfg.allow_terminal = true;
        let mut manager = PermissionManager::new(cfg, /*non_interactive=*/ true, false);
        let decision = manager
            .check(PermissionKind::Terminal, "cargo test", RiskLevel::Moderate)
            .unwrap();
        assert_eq!(decision, Decision::Allowed);
    }

    #[test]
    fn non_interactive_denies_by_default() {
        let cfg = PermissionsConfig::default();
        let mut manager = PermissionManager::new(cfg, /*non_interactive=*/ true, false);
        let decision = manager
            .check(PermissionKind::Terminal, "cargo test", RiskLevel::Moderate)
            .unwrap();
        assert_eq!(decision, Decision::Denied);
    }

    #[test]
    fn auto_approve_flag_allows_everything() {
        let cfg = PermissionsConfig::default();
        let mut manager = PermissionManager::new(cfg, true, true);
        let decision = manager
            .check(PermissionKind::DeleteFile, "delete foo.txt", RiskLevel::High)
            .unwrap();
        assert_eq!(decision, Decision::Allowed);
    }

    #[test]
    fn set_auto_allow_flips_live_config() {
        let cfg = PermissionsConfig::default();
        let mut manager = PermissionManager::new(cfg, true, false);
        assert!(!manager.current_config().allow_edit);

        manager.set_auto_allow(PermissionKind::Edit, true);
        assert!(manager.current_config().allow_edit);
        let decision = manager
            .check(PermissionKind::Edit, "edit foo.rs", RiskLevel::Moderate)
            .unwrap();
        assert_eq!(decision, Decision::Allowed);

        manager.set_auto_allow(PermissionKind::Edit, false);
        assert!(!manager.current_config().allow_edit);
    }

    #[test]
    fn clear_session_grants_revokes_always_approvals() {
        let cfg = PermissionsConfig::default();
        let mut manager = PermissionManager::new(cfg, true, false);
        // Simulate an "always" grant the way prompt() would record it.
        manager.granted_session.insert(PermissionKind::Terminal);
        assert!(manager.is_session_granted(PermissionKind::Terminal));

        manager.clear_session_grants();
        assert!(!manager.is_session_granted(PermissionKind::Terminal));
        let decision = manager
            .check(PermissionKind::Terminal, "rm foo", RiskLevel::Moderate)
            .unwrap();
        // non_interactive=true and nothing pre-approved -> denied, proving
        // the earlier "always" grant no longer applies.
        assert_eq!(decision, Decision::Denied);
    }

    /// A canned-answer prompter for exercising [`check_interactive`]
    /// without a real terminal.
    struct FakePrompter(PermResponse);

    #[async_trait::async_trait(?Send)]
    impl PermissionPrompter for FakePrompter {
        async fn ask(&mut self, _kind: PermissionKind, _summary: &str, _risk: RiskLevel) -> Result<PermResponse> {
            Ok(self.0)
        }
    }

    #[tokio::test]
    async fn check_interactive_short_circuits_without_asking_when_pre_approved() {
        let mut cfg = PermissionsConfig::default();
        cfg.allow_terminal = true;
        let mut manager = PermissionManager::new(cfg, false, false);
        // If this actually asked, it would deny — proving the fast path
        // never touches the prompter.
        let mut prompter = FakePrompter(PermResponse::Deny);
        let decision = manager
            .check_interactive(PermissionKind::Terminal, "cargo test", RiskLevel::Moderate, &mut prompter)
            .await
            .unwrap();
        assert_eq!(decision, Decision::Allowed);
    }

    #[tokio::test]
    async fn check_interactive_always_records_a_session_grant() {
        let cfg = PermissionsConfig::default();
        let mut manager = PermissionManager::new(cfg, false, false);
        let mut prompter = FakePrompter(PermResponse::Always);
        let decision = manager
            .check_interactive(PermissionKind::Edit, "edit foo.rs", RiskLevel::Moderate, &mut prompter)
            .await
            .unwrap();
        assert_eq!(decision, Decision::Allowed);
        assert!(manager.is_session_granted(PermissionKind::Edit));

        // Next call for the same kind shouldn't need to ask again either.
        let mut deny_prompter = FakePrompter(PermResponse::Deny);
        let decision2 = manager
            .check_interactive(PermissionKind::Edit, "edit bar.rs", RiskLevel::Moderate, &mut deny_prompter)
            .await
            .unwrap();
        assert_eq!(decision2, Decision::Allowed);
    }

    #[tokio::test]
    async fn check_interactive_once_does_not_persist() {
        let cfg = PermissionsConfig::default();
        let mut manager = PermissionManager::new(cfg, false, false);
        let mut prompter = FakePrompter(PermResponse::Once);
        manager
            .check_interactive(PermissionKind::Terminal, "ls", RiskLevel::Moderate, &mut prompter)
            .await
            .unwrap();
        assert!(!manager.is_session_granted(PermissionKind::Terminal));
    }
}
