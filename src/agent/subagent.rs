//! Subagents — a specialized persona (its own system prompt) that runs
//! a bounded task to completion using the same provider/config/tools as
//! the parent session, then reports back a result.
//!
//! Sequential, not concurrent: `run` reuses [`Agent::bootstrap`] plus the
//! existing headless [`Agent::respond`] loop rather than a second
//! execution thread, so there's no git-worktree isolation and no
//! background execution here — running a subagent blocks the parent
//! session on it, same as any other turn. Concurrent/backgrounded
//! subagents with workspace isolation remain the bigger item on the
//! roadmap (they need the git-worktree layer this doesn't have). What's
//! real here: defining a persona, running it against a genuine task with
//! its own tool-use loop, its own bounded iteration count, and its own
//! conversation entirely separate from the parent's, and getting an
//! actual result back — not a stub.

use std::path::{Path, PathBuf};

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

/// Run `task` through a fresh, persona-configured `Agent` to completion.
/// Reuses the parent's `config` (same provider, model, credentials, tool
/// permissions) but starts an entirely new conversation seeded with the
/// persona's system prompt in place of REXO's default one — the
/// subagent thinks of itself as the persona, not as REXO's usual
/// assistant with a note bolted on. Bounded by the same
/// `max_iterations`/`max_tool_calls` any `Agent` has, so a runaway
/// subagent can't loop forever any more than a runaway top-level turn
/// can.
pub async fn run(config: &Config, persona: &SubagentPersona, task: &str, non_interactive: bool, auto_approve: bool) -> Result<String> {
    let api_key = config.api_key().unwrap_or_default(); // some providers (local servers, etc.) need none at all — same as the top-level bootstrap path
    let (mut sub_agent, mut history) = crate::agent::Agent::bootstrap(config, api_key, non_interactive, auto_approve).context("couldn't start the subagent")?;
    if let Some(first) = history.first_mut() {
        if first.role == Role::System {
            *first = ChatMessage::system(persona.system_prompt.clone());
        }
    }
    sub_agent.respond(&mut history, task).await
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
}
