//! Subagents — a specialized persona (its own system prompt) that runs
//! a bounded task to completion using the same provider/config/tools as
//! the parent session, then reports back a result.
//!
//! As of v0.9, `/agents run` always runs as a real background job (see
//! [`crate::agent::background`]) rather than blocking the parent
//! session — [`run_silent`] is genuinely concurrent with whatever the
//! parent session does next, not sequential. What's still not here:
//! git-worktree isolation between jobs, so two jobs (or a job and the
//! parent session) editing overlapping files in the same workspace can
//! race each other exactly like two humans editing the same repo at
//! once would. That isolation layer remains the one clear item carried
//! forward on the roadmap. What's real: defining a persona, running it
//! against a genuine task with its own tool-use loop, its own bounded
//! iteration count, and its own conversation entirely separate from the
//! parent's, actually running concurrently, and getting a real result
//! back — not a stub.

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::config::Config;
use crate::providers::{ChatMessage, Role};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SubagentPersona {
    #[serde(default)]
    pub description: String,
    pub system_prompt: String,
}

fn agents_dir(workspace: &Path) -> PathBuf {
    workspace.join(".rexo").join("agents")
}

pub fn discover(workspace: &Path) -> Vec<(String, SubagentPersona)> {
    let Ok(entries) = std::fs::read_dir(agents_dir(workspace)) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("toml") {
            continue;
        }
        let Some(name) = path.file_stem().and_then(|n| n.to_str()) else { continue };
        let Ok(raw) = std::fs::read_to_string(&path) else { continue };
        let Ok(persona) = toml::from_str::<SubagentPersona>(&raw) else { continue };
        out.push((name.to_string(), persona));
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

pub fn load(workspace: &Path, name: &str) -> Option<SubagentPersona> {
    let raw = std::fs::read_to_string(agents_dir(workspace).join(format!("{name}.toml"))).ok()?;
    toml::from_str(&raw).ok()
}

/// Scaffold a new persona file and return its path (so a caller can
/// offer to open it in `$EDITOR`, same as `cli::tui::edit_in_editor`
/// does for the input box) — the generated `system_prompt` is a
/// reasonable starting point, not meant to be used unedited for
/// anything that actually needs a carefully-scoped persona.
pub fn create(workspace: &Path, name: &str, description: &str) -> Result<PathBuf> {
    let dir = agents_dir(workspace);
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(format!("{name}.toml"));
    if path.exists() {
        anyhow::bail!("a subagent named '{name}' already exists at {}", path.display());
    }
    let escaped_description = description.replace('"', "\\\"");
    let template = format!(
        "description = \"{escaped_description}\"\nsystem_prompt = \"\"\"\nYou are a specialized assistant focused on: {description}.\nStay narrowly scoped to that job and report back a concise result.\n\"\"\"\n"
    );
    std::fs::write(&path, template).with_context(|| format!("writing {}", path.display()))?;
    Ok(path)
}

/// Delete a persona file. Only ever touches `.rexo/agents/<name>.toml`
/// itself — never anything a job that ran under that persona may have
/// created or edited in the workspace — mirroring the precision
/// `/plugin disable` already holds itself to for what it reverses.
pub fn delete(workspace: &Path, name: &str) -> Result<()> {
    let path = agents_dir(workspace).join(format!("{name}.toml"));
    if !path.exists() {
        anyhow::bail!("no subagent named '{name}'");
    }
    std::fs::remove_file(&path).with_context(|| format!("removing {}", path.display()))
}

/// Run `task` through a fresh, persona-configured `Agent` to completion.
/// Reuses the parent's `config` (same provider, model, credentials, tool
/// permissions) but starts an entirely new conversation seeded with the
/// persona's system prompt in place of REXO's default one — the
/// subagent thinks of itself as the persona, not as REXO's usual
/// assistant with a note bolted on. Bounded by the same
/// `max_iterations`/`max_tool_calls` any `Agent` has, so a runaway
/// subagent can't loop forever any more than a runaway top-level turn
/// can.
/// The background-job counterpart of persona-based subagent execution —
/// see [`crate::agent::Agent::respond_silent`] for why this is the only
/// execution path now (foreground and backgrounded both), not one of
/// two: `respond`'s loop (`stream_turn`) writes raw ANSI (spinner
/// frames, streamed tokens) straight to stdout and listens for `Ctrl+C`
/// process-wide — both correct for a genuinely foreground, blocking call
/// with nothing else on the terminal, and both wrong for something
/// running *while the TUI owns the real terminal*. The former was the
/// actual root cause of `/agents run`'s garbled v0.8 output (two
/// independent writers hitting the same terminal, interleaved — the
/// ratatui redraw loop and this call's raw `println!`s); the latter
/// would make every background job die together the instant the user
/// hits Ctrl+C for an unrelated reason in the foreground session.
/// `persona: None` runs with the parent config's own default system
/// prompt (used by plain `/background <task>`, no persona involved)
/// instead of a persona's.
pub async fn run_silent(config: &Config, persona: Option<&SubagentPersona>, task: &str, cancel: Arc<AtomicBool>) -> Result<String> {
    let api_key = config.api_key().unwrap_or_default();
    // Always non-interactive + never auto-approve: a background job has
    // no one to ask, so anything gated behind `ToolPermission::Ask` is
    // refused with an explanation instead of either hanging forever
    // waiting for an answer nobody can give it, or silently doing
    // something the user never actually approved. Automatic-permission
    // tools (read_file, list_files, safe git status, etc.) still run
    // freely — plenty of useful autonomous work (research, planning,
    // read-heavy multi-step tasks) never needs more than that.
    let (mut sub_agent, mut history) = crate::agent::Agent::bootstrap(config, api_key, true, false).context("couldn't start the background job")?;
    if let Some(persona) = persona {
        if let Some(first) = history.first_mut() {
            if first.role == Role::System {
                *first = ChatMessage::system(persona.system_prompt.clone());
            }
        }
    }
    sub_agent.respond_silent(&mut history, task, &cancel).await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_workspace() -> PathBuf {
        let suffix = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().subsec_nanos();
        let dir = std::env::temp_dir().join(format!("rexo_subagent_test_{}_{suffix}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn create_then_discover_finds_the_persona() {
        let ws = temp_workspace();
        create(&ws, "reviewer", "reviewing code for style issues").unwrap();
        let found = discover(&ws);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].0, "reviewer");
        assert!(found[0].1.system_prompt.contains("reviewing code for style issues"));
        let _ = std::fs::remove_dir_all(&ws);
    }

    #[test]
    fn create_refuses_to_overwrite_an_existing_persona() {
        let ws = temp_workspace();
        create(&ws, "reviewer", "first").unwrap();
        let err = create(&ws, "reviewer", "second").unwrap_err();
        assert!(err.to_string().contains("already exists"));
        let _ = std::fs::remove_dir_all(&ws);
    }

    #[test]
    fn load_reads_back_a_created_persona() {
        let ws = temp_workspace();
        create(&ws, "tester", "writing unit tests").unwrap();
        let persona = load(&ws, "tester").unwrap();
        assert_eq!(persona.description, "writing unit tests");
        let _ = std::fs::remove_dir_all(&ws);
    }

    #[test]
    fn delete_removes_a_persona_so_discover_no_longer_finds_it() {
        let ws = temp_workspace();
        create(&ws, "reviewer", "reviewing code").unwrap();
        assert_eq!(discover(&ws).len(), 1);
        delete(&ws, "reviewer").unwrap();
        assert!(discover(&ws).is_empty());
        let _ = std::fs::remove_dir_all(&ws);
    }

    #[test]
    fn delete_reports_a_clear_error_for_an_unknown_persona() {
        let ws = temp_workspace();
        let err = delete(&ws, "nobody").unwrap_err();
        assert!(err.to_string().contains("no subagent named"));
        let _ = std::fs::remove_dir_all(&ws);
    }
}
