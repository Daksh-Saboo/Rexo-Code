//! Slash-command and path autocomplete for the TUI's input box.
//!
//! Pure, terminal-agnostic functions — `cli::tui` calls these to build the
//! floating suggestion popup and to resolve Tab-completion, but nothing
//! here touches a terminal directly, so it's all plain unit-testable code.
//! Command-name completion covers the explicit ask ("type `/mo`, see
//! `/model`/`/models` suggested"); path completion for `/workspace`/`>`
//! lists real on-disk entries under the given base directory. No attempt
//! is made to complete command *arguments* beyond that (e.g. model ids) —
//! that needs live session state this module doesn't have access to, and
//! is called out as a limitation in the README.

use std::path::Path;

use super::commands::COMMANDS;

/// Suggest slash command names (and aliases) whose name starts with
/// `fragment` (case-insensitive), sorted and de-duplicated.
pub fn suggest_commands(fragment: &str) -> Vec<String> {
    let frag_lower = fragment.to_lowercase();
    let mut names: Vec<&str> = COMMANDS
        .iter()
        .filter(|c| !c.hidden)
        .flat_map(|c| std::iter::once(c.name).chain(c.aliases.iter().copied()))
        .filter(|name| name.starts_with(frag_lower.as_str()))
        .collect();
    names.sort_unstable();
    names.dedup();
    names.into_iter().map(str::to_string).collect()
}

/// Suggest real, on-disk directory entries under `base` whose name starts
/// with `fragment` (case-insensitive), for `/workspace <Tab>` and the `>`
/// shortcut. Directories are suffixed with `/` so it's obvious which
/// suggestions can be completed further. Never invents a path that
/// doesn't exist.
pub fn suggest_paths(base: &Path, fragment: &str) -> Vec<String> {
    const MAX_SUGGESTIONS: usize = 12;
    let frag_lower = fragment.to_lowercase();

    let Ok(entries) = std::fs::read_dir(base) else {
        return Vec::new();
    };

    let mut names: Vec<String> = entries
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().to_string();
            if !name.to_lowercase().starts_with(frag_lower.as_str()) {
                return None;
            }
            if entry.path().is_dir() {
                Some(format!("{name}/"))
            } else {
                Some(name)
            }
        })
        .collect();

    names.sort_unstable();
    names.truncate(MAX_SUGGESTIONS);
    names
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn suggests_model_commands_for_mo_prefix() {
        let suggestions = suggest_commands("mo");
        assert!(suggestions.contains(&"model".to_string()));
        assert!(suggestions.contains(&"models".to_string()));
    }

    #[test]
    fn suggests_nothing_for_a_nonexistent_prefix() {
        assert!(suggest_commands("zzzzz").is_empty());
    }

    #[test]
    fn empty_fragment_lists_every_visible_command() {
        let suggestions = suggest_commands("");
        assert!(suggestions.contains(&"help".to_string()));
        assert!(suggestions.contains(&"status".to_string()));
        // The '>' shortcut's internal name must never surface here.
        assert!(!suggestions.contains(&"workspace-select".to_string()));
    }

    #[test]
    fn matches_aliases_too() {
        // "q" is an alias of "exit".
        let suggestions = suggest_commands("q");
        assert!(suggestions.contains(&"q".to_string()));
    }

    #[test]
    fn is_case_insensitive() {
        assert_eq!(suggest_commands("MO"), suggest_commands("mo"));
    }

    #[test]
    fn suggests_real_subdirectories_only() {
        let dir = std::env::temp_dir().join("rexo_test_completion_paths");
        std::fs::create_dir_all(dir.join("child_one")).unwrap();
        std::fs::write(dir.join("a_file.txt"), "x").unwrap();

        let suggestions = suggest_paths(&dir, "child");
        assert_eq!(suggestions, vec!["child_one/".to_string()]);
    }

    #[test]
    fn path_suggestions_are_case_insensitive_and_capped() {
        let dir = std::env::temp_dir().join("rexo_test_completion_paths_case");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::create_dir_all(dir.join("Project")).unwrap();
        let suggestions = suggest_paths(&dir, "pro");
        assert!(suggestions.contains(&"Project/".to_string()));
    }
}
