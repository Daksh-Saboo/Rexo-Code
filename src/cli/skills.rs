//! A skills system in the same spirit as Claude's: a directory named for
//! the skill, containing a `SKILL.md` with a little frontmatter (a
//! `description`, and optionally comma-separated `triggers`) followed by
//! the actual instructions — reusable, portable know-how the model can
//! draw on, not code.
//!
//! Two sources, merged, project wins on a name collision:
//! - **Project**: `<workspace>/.rexo/skills/<name>/SKILL.md` — checked
//!   into the repo, travels with the project.
//! - **Global**: `<global config dir>/skills/<name>/SKILL.md` — personal
//!   skills available in every workspace, the same directory
//!   `install.ps1`/`config::global` already use for everything else
//!   workspace-independent.
//!
//! A skill reaches the model two ways: automatically, when the words in a
//! prompt match one of its `triggers` (see [`find_triggered`], wired up
//! in `cli::tui::run_turn`), or explicitly via `/skill <name>`. Either
//! way it's injected as its own message in the conversation, not silently
//! folded into a system prompt the user can't see — `/skills` always
//! shows exactly what's discovered and where it came from.
//!
//! This is deliberately *not* a claim of protocol compatibility with any
//! specific product's skill format — just a genuinely useful, portable
//! shape (a folder + a markdown file) that's easy to author by hand and
//! easy to share.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::cli::frontmatter;

#[derive(Debug, Clone)]
pub struct Skill {
    pub name: String,
    pub description: String,
    pub triggers: Vec<String>,
    pub body: String,
    pub project: bool,
    pub path: PathBuf,
}

/// Discover every skill visible to `workspace` — global skills first,
/// then project skills layered on top (a project skill with the same
/// `name:` replaces the global one of that name, letting a repo pin its
/// own version of a personal skill).
pub fn discover(workspace: &Path) -> Vec<Skill> {
    let mut found = Vec::new();
    if let Ok(dir) = crate::config::global::global_dir() {
        collect_from(&dir.join("skills"), false, &mut found);
    }
    collect_from(&workspace.join(".rexo").join("skills"), true, &mut found);

    let mut by_name: BTreeMap<String, Skill> = BTreeMap::new();
    for skill in found {
        by_name.insert(skill.name.clone(), skill);
    }
    let mut out: Vec<Skill> = by_name.into_values().collect();
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

fn collect_from(dir: &Path, project: bool, out: &mut Vec<Skill>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let skill_md = path.join("SKILL.md");
        let Ok(raw) = std::fs::read_to_string(&skill_md) else { continue };
        let fm = frontmatter::parse(&raw);
        let default_name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        let name = fm.get("name").map(str::to_string).unwrap_or(default_name);
        let description = fm
            .get("description")
            .map(str::to_string)
            .unwrap_or_else(|| fm.body.lines().find(|l| !l.trim().is_empty()).unwrap_or("").trim().to_string());
        out.push(Skill {
            name,
            description,
            triggers: fm.list("triggers"),
            body: fm.body,
            project,
            path: skill_md,
        });
    }
}

/// Skills whose `triggers` list contains a word/phrase that appears
/// (case-insensitively) in `text`. Simple substring matching, not
/// semantic search — honest about what it is: a keyword hook, not RAG.
pub fn find_triggered<'a>(skills: &'a [Skill], text: &str) -> Vec<&'a Skill> {
    let lower = text.to_lowercase();
    skills.iter().filter(|s| s.triggers.iter().any(|t| !t.is_empty() && lower.contains(t.as_str()))).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn write_skill(dir: &Path, folder: &str, content: &str) {
        let skill_dir = dir.join(folder);
        fs::create_dir_all(&skill_dir).unwrap();
        fs::write(skill_dir.join("SKILL.md"), content).unwrap();
    }

    #[test]
    fn discovers_project_skill() {
        let tmp = tempfile();
        let skills_dir = tmp.join(".rexo").join("skills");
        write_skill(&skills_dir, "commit-style", "---\nname: commit-style\ndescription: How this repo writes commit messages\ntriggers: commit, changelog\n---\nUse imperative mood, 72-char subject line.\n");

        let found = super::discover(&tmp);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].name, "commit-style");
        assert!(found[0].project);
        assert_eq!(found[0].triggers, vec!["commit", "changelog"]);
        fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn triggers_match_case_insensitively() {
        let skill = Skill {
            name: "x".into(),
            description: "d".into(),
            triggers: vec!["dockerfile".into()],
            body: "b".into(),
            project: true,
            path: PathBuf::new(),
        };
        let hits = find_triggered(std::slice::from_ref(&skill), "please fix my Dockerfile");
        assert_eq!(hits.len(), 1);
    }

    #[test]
    fn no_skill_dir_is_not_an_error() {
        let tmp = tempfile();
        assert!(super::discover(&tmp).is_empty());
    }

    fn tempfile() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("rexo-skills-test-{}", uuid()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn uuid() -> u128 {
        use std::time::{SystemTime, UNIX_EPOCH};
        SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos()
    }
}
