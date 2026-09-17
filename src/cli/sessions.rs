//! Session persistence for `/resume`: save a conversation's history (plus
//! enough provider context to know what it was talking to) to disk, and
//! load it back later — including automatically, so a crash or an
//! accidental `Ctrl+C` twice doesn't necessarily lose an in-progress
//! conversation.
//!
//! Two kinds of saved file, both under `<workspace>/.rexo/sessions/`:
//! - **`autosave.json`** — overwritten after every turn. Not a history of
//!   past sessions, just "continue where I left off."
//! - **`<name>.json`** — explicit, named saves via `/resume save <name>`
//!   that stick around until deleted.
//!
//! This is deliberately workspace-local (checked into `.gitignore`, not
//! `.rexo/skills`/`.rexo/commands`, which are meant to be shared) since a
//! conversation's tool-call history only makes sense against the
//! workspace it happened in.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::providers::ChatMessage;

const AUTOSAVE_NAME: &str = "autosave";

#[derive(Serialize, Deserialize)]
pub struct SessionSnapshot {
    pub saved_at: String,
    pub provider: String,
    pub model: Option<String>,
    pub history: Vec<ChatMessage>,
}

pub struct SessionSummary {
    pub name: String,
    pub path: PathBuf,
    pub saved_at: String,
    pub turn_count: usize,
    pub preview: String,
    pub is_autosave: bool,
}

fn sessions_dir(workspace: &Path) -> PathBuf {
    workspace.join(".rexo").join("sessions")
}

fn sanitize_name(name: &str) -> String {
    let cleaned: String = name
        .trim()
        .chars()
        .map(|c| if c.is_alphanumeric() || c == '-' || c == '_' { c } else { '-' })
        .collect();
    if cleaned.is_empty() {
        "session".to_string()
    } else {
        cleaned
    }
}

fn now_string() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    // No chrono dependency for this — a plain "seconds since epoch" is
    // enough to sort and display relative recency; see `format_saved_at`.
    let secs = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    secs.to_string()
}

/// Human-readable recency ("just now", "14m ago", "3h ago", "5d ago")
/// from the epoch-seconds string `now_string` produces — avoids pulling
/// in a date/time-formatting crate for something this simple.
pub fn format_saved_at(saved_at: &str) -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let Ok(saved_secs) = saved_at.parse::<u64>() else { return saved_at.to_string() };
    let now_secs = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(saved_secs);
    let delta = now_secs.saturating_sub(saved_secs);
    if delta < 60 {
        "just now".to_string()
    } else if delta < 3600 {
        format!("{}m ago", delta / 60)
    } else if delta < 86400 {
        format!("{}h ago", delta / 3600)
    } else {
        format!("{}d ago", delta / 86400)
    }
}

fn build_snapshot(session: &crate::cli::Session) -> SessionSnapshot {
    SessionSnapshot {
        saved_at: now_string(),
        provider: session.config.model.provider.clone(),
        model: session.config.model.model.clone(),
        history: session.history.clone(),
    }
}

fn write_snapshot(workspace: &Path, name: &str, snapshot: &SessionSnapshot) -> Result<PathBuf> {
    let dir = sessions_dir(workspace);
    std::fs::create_dir_all(&dir).with_context(|| format!("Couldn't create {}", dir.display()))?;
    let path = dir.join(format!("{name}.json"));
    let json = serde_json::to_string_pretty(snapshot)?;
    std::fs::write(&path, json).with_context(|| format!("Couldn't write {}", path.display()))?;
    Ok(path)
}

/// Overwrites `autosave.json` — called after every turn. Silently no-ops
/// on failure (e.g. a read-only workspace): losing autosave shouldn't
/// interrupt an otherwise-working conversation, and there's nowhere
/// useful to report a background write failure to mid-stream anyway.
pub fn autosave(session: &crate::cli::Session) {
    // Nothing worth resuming yet — an autosave of just the initial system
    // message isn't a session, it's noise.
    if session.history.len() <= 1 {
        return;
    }
    let snapshot = build_snapshot(session);
    // After `/fork <name>`, autosaves redirect to `<name>.json` instead
    // of the default slot — see `Session::active_session_name`'s docs.
    let name = session.active_session_name.as_deref().unwrap_or(AUTOSAVE_NAME);
    let _ = write_snapshot(session.agent.workspace(), name, &snapshot);
}

/// Explicit, named save — errors are real here, since the user asked for
/// this one directly.
pub fn save_named(session: &crate::cli::Session, name: &str) -> Result<PathBuf> {
    let snapshot = build_snapshot(session);
    write_snapshot(session.agent.workspace(), &sanitize_name(name), &snapshot)
}

pub fn list(workspace: &Path) -> Vec<SessionSummary> {
    let dir = sessions_dir(workspace);
    let Ok(entries) = std::fs::read_dir(&dir) else { return Vec::new() };
    let mut out = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let Ok(raw) = std::fs::read_to_string(&path) else { continue };
        let Ok(snapshot) = serde_json::from_str::<SessionSnapshot>(&raw) else { continue };
        let name = path.file_stem().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        let preview = snapshot
            .history
            .iter()
            .find(|m| m.role == crate::providers::Role::User)
            .and_then(|m| m.content.as_deref())
            .map(|c| {
                let trimmed = c.trim().replace('\n', " ");
                if trimmed.chars().count() > 70 {
                    format!("{}…", trimmed.chars().take(70).collect::<String>())
                } else {
                    trimmed
                }
            })
            .unwrap_or_else(|| "(no user message)".to_string());
        out.push(SessionSummary {
            is_autosave: name == AUTOSAVE_NAME,
            turn_count: snapshot.history.iter().filter(|m| m.role == crate::providers::Role::User).count(),
            saved_at: snapshot.saved_at,
            name,
            path,
            preview,
        });
    }
    out.sort_by(|a, b| b.saved_at.cmp(&a.saved_at));
    out
}

pub fn load(path: &Path) -> Result<SessionSnapshot> {
    let raw = std::fs::read_to_string(path).with_context(|| format!("Couldn't read {}", path.display()))?;
    serde_json::from_str(&raw).with_context(|| format!("'{}' isn't a valid REXO session file", path.display()))
}

pub fn delete(workspace: &Path, name: &str) -> Result<bool> {
    let path = sessions_dir(workspace).join(format!("{}.json", sanitize_name(name)));
    if path.exists() {
        std::fs::remove_file(&path)?;
        Ok(true)
    } else {
        Ok(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tempdir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("rexo-sessions-test-{}", nanos()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }
    fn nanos() -> u128 {
        use std::time::{SystemTime, UNIX_EPOCH};
        SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos()
    }

    #[test]
    fn save_list_load_roundtrip() {
        let tmp = tempdir();
        let snapshot = SessionSnapshot {
            saved_at: now_string(),
            provider: "nvidia".to_string(),
            model: Some("moonshotai/kimi-k3".to_string()),
            history: vec![ChatMessage::system("sys"), ChatMessage::user("hello there, fix my bug please")],
        };
        write_snapshot(&tmp, "my-save", &snapshot).unwrap();

        let found = list(&tmp);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].name, "my-save");
        assert_eq!(found[0].turn_count, 1);
        assert!(found[0].preview.contains("hello there"));

        let loaded = load(&found[0].path).unwrap();
        assert_eq!(loaded.history.len(), 2);
        std::fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn sanitize_name_strips_path_separators() {
        let cleaned = sanitize_name("../../etc/passwd");
        assert!(!cleaned.contains('/'));
        assert!(!cleaned.contains('.'));
        assert_eq!(sanitize_name(""), "session");
    }

    #[test]
    fn delete_reports_whether_it_existed() {
        let tmp = tempdir();
        let snapshot = SessionSnapshot { saved_at: now_string(), provider: "nvidia".into(), model: None, history: vec![] };
        write_snapshot(&tmp, "gone-soon", &snapshot).unwrap();
        assert!(delete(&tmp, "gone-soon").unwrap());
        assert!(!delete(&tmp, "gone-soon").unwrap());
        std::fs::remove_dir_all(&tmp).ok();
    }
}
