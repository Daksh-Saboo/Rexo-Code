//! User-defined slash commands: drop a markdown file in
//! `<workspace>/.rexo/commands/<name>.md` (project) or `<global config
//! dir>/commands/<name>.md` (personal, every workspace) and `/<name>`
//! sends its contents to the model as a prompt, with `$ARGUMENTS`
//! replaced by everything typed after the command name and `$1`.."$9"
//! bound to individual whitespace-separated arguments. A project command
//! shadows a global one of the same name.
//!
//! ```markdown
//! ---
//! description: Review a diff against our style guide
//! ---
//! Review this diff for style-guide violations. Focus area: $1
//!
//! $ARGUMENTS
//! ```
//!
//! `/review security @src/auth.rs` then expands `$1` to `security` and
//! `$ARGUMENTS` to `security @src/auth.rs` before the prompt (including
//! any `@file` reference in it) goes through the normal turn.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::cli::frontmatter;

#[derive(Debug, Clone)]
pub struct CustomCommand {
    pub name: String,
    pub description: String,
    pub template: String,
    pub project: bool,
    pub path: PathBuf,
}

pub fn discover(workspace: &Path) -> Vec<CustomCommand> {
    let mut found = Vec::new();
    if let Ok(dir) = crate::config::global::global_dir() {
        collect_from(&dir.join("commands"), false, &mut found);
    }
    collect_from(&workspace.join(".rexo").join("commands"), true, &mut found);

    let mut by_name: BTreeMap<String, CustomCommand> = BTreeMap::new();
    for cmd in found {
        by_name.insert(cmd.name.clone(), cmd);
    }
    let mut out: Vec<CustomCommand> = by_name.into_values().collect();
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

fn collect_from(dir: &Path, project: bool, out: &mut Vec<CustomCommand>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("md") {
            continue;
        }
        let Ok(raw) = std::fs::read_to_string(&path) else { continue };
        let fm = frontmatter::parse(&raw);
        let name = path.file_stem().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default();
        if name.is_empty() {
            continue;
        }
        let description = fm
            .get("description")
            .map(str::to_string)
            .unwrap_or_else(|| fm.body.lines().find(|l| !l.trim().is_empty()).unwrap_or("(custom command)").trim().to_string());
        out.push(CustomCommand { name, description, template: fm.body, project, path });
    }
}

pub fn find<'a>(commands: &'a [CustomCommand], name: &str) -> Option<&'a CustomCommand> {
    let name = name.to_lowercase();
    commands.iter().find(|c| c.name == name)
}

/// Substitute `$ARGUMENTS` and `$1`..`$9` into a command's template.
/// Unused positional placeholders are left as literal text (`$3` with
/// only two args typed stays `$3`) rather than silently becoming empty —
/// that's a more obvious sign something's missing than a blank spot.
pub fn expand(cmd: &CustomCommand, args: &[String]) -> String {
    let joined = args.join(" ");
    let mut out = cmd.template.replace("$ARGUMENTS", &joined);
    for (i, arg) in args.iter().enumerate().take(9) {
        out = out.replace(&format!("${}", i + 1), arg);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn tempdir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("rexo-cmds-test-{}", nanos()));
        fs::create_dir_all(&dir).unwrap();
        dir
    }
    fn nanos() -> u128 {
        use std::time::{SystemTime, UNIX_EPOCH};
        SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos()
    }

    #[test]
    fn discovers_and_expands() {
        let tmp = tempdir();
        let cmds_dir = tmp.join(".rexo").join("commands");
        fs::create_dir_all(&cmds_dir).unwrap();
        fs::write(cmds_dir.join("review.md"), "---\ndescription: Review a diff\n---\nFocus on $1.\n\n$ARGUMENTS\n").unwrap();

        let found = discover(&tmp);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].name, "review");
        assert_eq!(found[0].description, "Review a diff");

        let expanded = expand(&found[0], &["security".to_string(), "@src/auth.rs".to_string()]);
        assert!(expanded.contains("Focus on security."));
        assert!(expanded.contains("security @src/auth.rs"));
        fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn unused_placeholders_left_literal() {
        let cmd = CustomCommand { name: "x".into(), description: String::new(), template: "$1 and $2".into(), project: true, path: PathBuf::new() };
        let out = expand(&cmd, &["only-one".to_string()]);
        assert_eq!(out, "only-one and $2");
    }

    #[test]
    fn project_command_shadows_none_when_only_one_source() {
        let tmp = tempdir();
        assert!(discover(&tmp).is_empty());
    }
}
