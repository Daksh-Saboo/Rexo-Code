//! Project context gathering.
//!
//! Builds a small, bounded-size summary of the workspace (project type,
//! top-level layout, a README excerpt) that gets appended to the system
//! prompt. Deliberately avoids dumping the whole repository into every
//! request — the model has `list_files`/`read_file`/`search_files` for
//! anything more it needs.

use std::path::Path;

const MAX_CONTEXT_CHARS: usize = 3000;
const MAX_TREE_ENTRIES: usize = 60;
const MAX_README_CHARS: usize = 1200;
const MAX_REXO_MD_CHARS: usize = 2000;

const PROJECT_MARKERS: &[(&str, &str)] = &[
    ("Cargo.toml", "Rust (Cargo)"),
    ("package.json", "Node.js"),
    ("pyproject.toml", "Python (pyproject)"),
    ("requirements.txt", "Python (pip)"),
    ("go.mod", "Go"),
    ("pom.xml", "Java (Maven)"),
    ("build.gradle", "Java/Kotlin (Gradle)"),
    ("Gemfile", "Ruby"),
    ("composer.json", "PHP"),
    ("CMakeLists.txt", "C/C++ (CMake)"),
];

/// Build the project-context block. Never fails — any I/O error just
/// results in a shorter summary rather than aborting startup.
pub fn build(workspace: &Path) -> String {
    let mut sections = Vec::new();

    // User-authored project instructions (created by `/init`, or written
    // by hand) take priority — they're explicit guidance, not a guess.
    if let Some(project_md) = project_instructions_excerpt(workspace) {
        sections.push(format!("Project instructions (from REXO.md):\n{project_md}"));
    }

    let project_types = detect_project_types(workspace);
    if !project_types.is_empty() {
        sections.push(format!("Detected project type(s): {}", project_types.join(", ")));
    }

    if let Some(tree) = top_level_tree(workspace) {
        sections.push(format!("Top-level layout:\n{tree}"));
    }

    if let Some(readme) = readme_excerpt(workspace) {
        sections.push(format!("README excerpt:\n{readme}"));
    }

    if sections.is_empty() {
        return String::new();
    }

    let mut context = format!("## Project context\n{}", sections.join("\n\n"));
    if context.len() > MAX_CONTEXT_CHARS {
        context.truncate(MAX_CONTEXT_CHARS);
        context.push_str("\n... (truncated; use list_files/read_file for more)");
    }
    context
}

fn project_instructions_excerpt(workspace: &Path) -> Option<String> {
    let content = std::fs::read_to_string(workspace.join("REXO.md")).ok()?;
    let trimmed = content.trim();
    if trimmed.is_empty() {
        return None;
    }
    let excerpt: String = trimmed.chars().take(MAX_REXO_MD_CHARS).collect();
    if trimmed.chars().count() > MAX_REXO_MD_CHARS {
        Some(format!("{excerpt}\n... (truncated — see REXO.md directly for the rest)"))
    } else {
        Some(excerpt)
    }
}

fn detect_project_types(workspace: &Path) -> Vec<&'static str> {
    PROJECT_MARKERS
        .iter()
        .filter(|(file, _)| workspace.join(file).exists())
        .map(|(_, label)| *label)
        .collect()
}

fn top_level_tree(workspace: &Path) -> Option<String> {
    let entries = std::fs::read_dir(workspace).ok()?;
    let mut names: Vec<String> = entries
        .filter_map(|e| e.ok())
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().to_string();
            if name.starts_with('.') && name != ".env.example" {
                return None;
            }
            if crate::tools::filesystem::IGNORED_DIR_NAMES.contains(&name.as_str()) {
                return None;
            }
            let suffix = if e.path().is_dir() { "/" } else { "" };
            Some(format!("{name}{suffix}"))
        })
        .collect();

    if names.is_empty() {
        return None;
    }

    names.sort();
    names.truncate(MAX_TREE_ENTRIES);
    Some(names.join("\n"))
}

fn readme_excerpt(workspace: &Path) -> Option<String> {
    for candidate in ["README.md", "README.txt", "README"] {
        let path = workspace.join(candidate);
        if let Ok(content) = std::fs::read_to_string(&path) {
            let excerpt: String = content.chars().take(MAX_README_CHARS).collect();
            return Some(excerpt);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_rust_project() {
        let dir = std::env::temp_dir().join("rexo_test_context_rust");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("Cargo.toml"), "[package]\nname=\"x\"").unwrap();

        let ctx = build(&dir);
        assert!(ctx.contains("Rust (Cargo)"));
    }

    #[test]
    fn empty_directory_yields_minimal_context() {
        let dir = std::env::temp_dir().join("rexo_test_context_empty");
        std::fs::create_dir_all(&dir).unwrap();
        // May legitimately be non-empty from a prior run's tree section,
        // so just check it doesn't panic and stays within the size bound.
        let ctx = build(&dir);
        assert!(ctx.len() <= MAX_CONTEXT_CHARS + 64);
    }

    #[test]
    fn project_md_is_included_and_takes_priority_over_readme() {
        let dir = std::env::temp_dir().join("rexo_test_context_project_md");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("REXO.md"), "Always run `cargo fmt` before finishing.").unwrap();
        std::fs::write(dir.join("README.md"), "# Some project\nUnrelated readme text.").unwrap();

        let ctx = build(&dir);
        assert!(ctx.contains("Always run `cargo fmt` before finishing."));
        let instructions_pos = ctx.find("REXO.md").unwrap();
        let readme_pos = ctx.find("README excerpt").unwrap();
        assert!(instructions_pos < readme_pos, "REXO.md guidance should appear before the README excerpt");
    }

    #[test]
    fn missing_project_md_is_not_an_error() {
        let dir = std::env::temp_dir().join("rexo_test_context_no_project_md");
        std::fs::create_dir_all(&dir).unwrap();
        // Just must not panic/fail when REXO.md doesn't exist.
        let _ctx = build(&dir);
    }
}
