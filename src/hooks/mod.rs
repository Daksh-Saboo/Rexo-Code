//! Lifecycle hooks — shell commands the user configures to run around
//! tool calls (and session start/end), the same idea as git hooks or
//! Claude Code's own hooks: real shell-outs with a documented exit-code
//! protocol, not a plugin API.
//!
//! Stored per-workspace at `.rexo/hooks.toml` (not the shared
//! `rexo.toml`, so adding a hook never risks a clumsy merge with
//! whatever's already in the main config file). Four events:
//!
//! - `pre_tool` — before a tool call runs. Exit 0 lets it proceed; exit
//!   2 blocks it and the tool call fails with the hook's stderr as the
//!   reason (surfaced to the model exactly like a permission denial);
//!   any other nonzero exit is treated as "hook broke, not as a
//!   deliberate block" — the tool call still proceeds, but a warning is
//!   shown, so one misconfigured hook can't silently wedge every tool
//!   call.
//! - `post_tool` — after a tool call runs (regardless of outcome). Its
//!   exit code is informational only; stdout, if any, is shown as a
//!   note.
//! - `session_start` / `session_end` — once each, no matcher. stdout
//!   shown as a note.
//!
//! Every hook receives the same payload two ways: as `REXO_*`
//! environment variables (for simple one-liners) and as a single-line
//! JSON object on stdin (for anything that wants structured input
//! without shell-quoting gymnastics).

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::json;

pub const EVENTS: &[&str] = &["pre_tool", "post_tool", "session_start", "session_end"];

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HookDef {
    pub event: String,
    pub command: String,
    /// Exact tool name or a glob (`edit_*`) — `pre_tool`/`post_tool`
    /// only. `None` (or omitted in the TOML) matches every tool.
    #[serde(default)]
    pub matcher: Option<String>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct HooksFile {
    #[serde(default)]
    hooks: Vec<HookDef>,
}

fn hooks_path(workspace: &Path) -> PathBuf {
    workspace.join(".rexo").join("hooks.toml")
}

pub fn load(workspace: &Path) -> Vec<HookDef> {
    let path = hooks_path(workspace);
    let Ok(raw) = std::fs::read_to_string(&path) else {
        return Vec::new();
    };
    toml::from_str::<HooksFile>(&raw).map(|f| f.hooks).unwrap_or_default()
}

pub fn save(workspace: &Path, hooks: &[HookDef]) -> Result<()> {
    let path = hooks_path(workspace);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).with_context(|| format!("Failed to create {}", parent.display()))?;
    }
    let file = HooksFile { hooks: hooks.to_vec() };
    let raw = toml::to_string_pretty(&file)?;
    std::fs::write(&path, raw).with_context(|| format!("Failed to write {}", path.display()))
}

/// Whether `tool_name` matches this hook's matcher — no matcher (or an
/// empty one) matches everything; otherwise exact match or a glob via
/// the same `globset` engine `.rexoignore`-style matching already uses
/// elsewhere in this crate.
fn matches(matcher: &Option<String>, tool_name: &str) -> bool {
    match matcher.as_deref() {
        None => true,
        Some(pat) if pat.is_empty() => true,
        Some(pat) if pat == tool_name => true,
        Some(pat) => globset::Glob::new(pat).map(|g| g.compile_matcher().is_match(tool_name)).unwrap_or(false),
    }
}

pub enum HookOutcome {
    /// Ran fine (or wasn't run — nothing matched); an optional note to
    /// show (stdout, only if non-empty).
    Ok(Option<String>),
    /// A `pre_tool` hook exited 2: block the tool call. Carries the
    /// reason (the hook's stderr, or a default if it printed nothing).
    Block(String),
    /// The hook itself failed to run, or exited with some other nonzero
    /// code — surfaced as a warning, never as a block.
    Warn(String),
}

/// Run every hook configured for `event` whose matcher matches
/// `tool_name` (ignored for `session_start`/`session_end`), in the order
/// they're defined. For `pre_tool`, the first `Block` stops the loop
/// early — later hooks don't run once one has already said no.
pub fn run(hooks: &[HookDef], workspace: &Path, event: &str, tool_name: Option<&str>, tool_args: Option<&serde_json::Value>, tool_output: Option<&str>) -> Vec<HookOutcome> {
    let mut outcomes = Vec::new();
    for def in hooks.iter().filter(|h| h.event == event) {
        if let Some(name) = tool_name {
            if !matches(&def.matcher, name) {
                continue;
            }
        }
        let outcome = run_one(def, workspace, event, tool_name, tool_args, tool_output);
        let is_block = matches!(outcome, HookOutcome::Block(_));
        outcomes.push(outcome);
        if is_block {
            break;
        }
    }
    outcomes
}

fn run_one(def: &HookDef, workspace: &Path, event: &str, tool_name: Option<&str>, tool_args: Option<&serde_json::Value>, tool_output: Option<&str>) -> HookOutcome {
    let payload = json!({
        "event": event,
        "tool_name": tool_name,
        "tool_args": tool_args,
        "tool_output": tool_output,
    });

    let mut cmd = if cfg!(target_os = "windows") {
        let mut c = Command::new("powershell");
        c.args(["-NoProfile", "-NonInteractive", "-Command", &def.command]);
        c
    } else {
        let mut c = Command::new("sh");
        c.args(["-c", &def.command]);
        c
    };
    cmd.current_dir(workspace)
        .env("REXO_HOOK_EVENT", event)
        .env("REXO_TOOL_NAME", tool_name.unwrap_or(""))
        .env("REXO_TOOL_ARGS", tool_args.map(|v| v.to_string()).unwrap_or_default())
        .env("REXO_TOOL_OUTPUT", tool_output.unwrap_or(""))
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());

    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => return HookOutcome::Warn(format!("hook `{}` failed to start: {e}", def.command)),
    };
    if let Some(mut stdin) = child.stdin.take() {
        use std::io::Write;
        let _ = writeln!(stdin, "{payload}");
    }
    let output = match child.wait_with_output() {
        Ok(o) => o,
        Err(e) => return HookOutcome::Warn(format!("hook `{}` failed: {e}", def.command)),
    };

    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    match output.status.code() {
        Some(0) => HookOutcome::Ok(if stdout.is_empty() { None } else { Some(stdout) }),
        Some(2) if event == "pre_tool" => {
            HookOutcome::Block(if stderr.is_empty() { format!("hook `{}` denied this tool call", def.command) } else { stderr })
        }
        Some(code) => HookOutcome::Warn(format!("hook `{}` exited {code}{}", def.command, if stderr.is_empty() { String::new() } else { format!(": {stderr}") })),
        None => HookOutcome::Warn(format!("hook `{}` terminated by signal", def.command)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hook(event: &str, command: &str, matcher: Option<&str>) -> HookDef {
        HookDef { event: event.to_string(), command: command.to_string(), matcher: matcher.map(str::to_string) }
    }

    #[test]
    fn glob_matcher_matches_prefix_pattern() {
        assert!(matches(&Some("edit_*".to_string()), "edit_file"));
        assert!(!matches(&Some("edit_*".to_string()), "read_file"));
    }

    #[test]
    fn no_matcher_matches_everything() {
        assert!(matches(&None, "anything"));
    }

    #[test]
    fn exit_0_hook_runs_and_reports_stdout() {
        let h = hook("pre_tool", "echo hi", None);
        let out = run(&[h], &std::env::temp_dir(), "pre_tool", Some("edit_file"), None, None);
        assert_eq!(out.len(), 1);
        assert!(matches!(&out[0], HookOutcome::Ok(Some(s)) if s == "hi"));
    }

    #[test]
    fn exit_2_hook_blocks_pre_tool() {
        let h = hook("pre_tool", "echo nope 1>&2; exit 2", None);
        let out = run(&[h], &std::env::temp_dir(), "pre_tool", Some("edit_file"), None, None);
        assert!(matches!(&out[0], HookOutcome::Block(s) if s == "nope"));
    }

    #[test]
    fn non_matching_matcher_skips_the_hook_entirely() {
        let h = hook("pre_tool", "exit 2", Some("delete_file"));
        let out = run(&[h], &std::env::temp_dir(), "pre_tool", Some("edit_file"), None, None);
        assert!(out.is_empty());
    }

    #[test]
    fn other_nonzero_exit_warns_but_does_not_block() {
        let h = hook("pre_tool", "exit 1", None);
        let out = run(&[h], &std::env::temp_dir(), "pre_tool", Some("edit_file"), None, None);
        assert!(matches!(&out[0], HookOutcome::Warn(_)));
    }

    #[test]
    fn save_then_load_round_trips() {
        let dir = std::env::temp_dir().join(format!("rexo_hooks_test_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let hooks = vec![hook("post_tool", "echo done", Some("edit_file"))];
        save(&dir, &hooks).unwrap();
        let loaded = load(&dir);
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].command, "echo done");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
