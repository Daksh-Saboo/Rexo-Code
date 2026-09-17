//! `@file` references in the input box.
//!
//! Typing `@src/main.rs` and sending a message resolves that reference to
//! the file's actual content, appended to what's sent to the model — the
//! model doesn't have to spend a tool call reading something the user
//! already pointed at directly. Respects `.gitignore`/`.ignore` (via the
//! [`ignore`] crate — the same one ripgrep uses, rather than a hand-rolled
//! walker that would inevitably miss cases), skips binaries, and caps how
//! much any single reference can pull in so one `@src/` doesn't blow the
//! context budget.

use std::path::Path;

const MAX_FILE_BYTES: usize = 60_000; // generous but bounded — a few thousand lines of code
const MAX_SUGGESTIONS: usize = 20;
const MAX_WALK_ENTRIES: usize = 4000; // don't walk forever on a huge repo just for autocomplete

/// Live suggestions for the `@` picker: paths under `workspace`
/// containing `fragment` (case-insensitive), respecting ignore rules.
/// Directories are suffixed with `/` so it's obvious more typing narrows
/// further rather than selects a file.
pub fn suggest(workspace: &Path, fragment: &str) -> Vec<String> {
    let frag_lower = fragment.to_lowercase();
    let mut results: Vec<String> = Vec::new();

    let walker = ignore::WalkBuilder::new(workspace).hidden(false).max_depth(Some(12)).require_git(false).build();
    for (count, entry) in walker.flatten().enumerate() {
        if count >= MAX_WALK_ENTRIES {
            break;
        }
        if entry.depth() == 0 {
            continue; // the workspace root itself
        }
        let Ok(rel) = entry.path().strip_prefix(workspace) else { continue };
        let rel_str = rel.to_string_lossy().replace('\\', "/");
        if !frag_lower.is_empty() && !rel_str.to_lowercase().contains(&frag_lower) {
            continue;
        }
        let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
        results.push(if is_dir { format!("{rel_str}/") } else { rel_str });
    }

    results.sort_by_key(|s| (s.len(), s.clone()));
    results.truncate(MAX_SUGGESTIONS);
    results
}

pub struct ResolvedRef {
    pub reference: String,
    pub content: Option<String>,
    /// Why `content` is `None`, when it is.
    pub note: Option<String>,
}

/// Find every `@path` token in `text` and resolve each one safely:
/// path-traversal-checked against `workspace` (an `@../../etc/passwd`
/// resolves to nothing, same as any other tool here would refuse it),
/// size-capped, binary files skipped rather than dumped as garbage.
/// Directories list their (ignore-aware) immediate contents rather than
/// being read as one blob.
pub fn resolve_references(text: &str, workspace: &Path) -> Vec<ResolvedRef> {
    extract_refs(text).into_iter().map(|r| resolve_one(&r, workspace)).collect()
}

fn extract_refs(text: &str) -> Vec<String> {
    let mut refs = Vec::new();
    for token in text.split_whitespace() {
        if let Some(rest) = token.strip_prefix('@') {
            let cleaned = rest.trim_end_matches(['.', ',', ';', ':', '!', '?', ')', ']', '}']);
            if !cleaned.is_empty() {
                refs.push(cleaned.to_string());
            }
        }
    }
    refs
}

fn resolve_one(reference: &str, workspace: &Path) -> ResolvedRef {
    let candidate = workspace.join(reference);
    let inside = match (workspace.canonicalize(), candidate.canonicalize()) {
        (Ok(ws), Ok(c)) => c.starts_with(&ws),
        _ => false,
    };
    if !inside {
        return ResolvedRef {
            reference: reference.to_string(),
            content: None,
            note: Some("not found in this workspace".to_string()),
        };
    }
    let path = candidate.canonicalize().expect("just checked above");

    if path.is_dir() {
        match list_dir(&path, workspace) {
            Ok(listing) => ResolvedRef { reference: reference.to_string(), content: Some(listing), note: None },
            Err(e) => ResolvedRef { reference: reference.to_string(), content: None, note: Some(e.to_string()) },
        }
    } else {
        match std::fs::read(&path) {
            Ok(bytes) => {
                if bytes.iter().take(8000).any(|&b| b == 0) {
                    ResolvedRef { reference: reference.to_string(), content: None, note: Some("binary file, skipped".to_string()) }
                } else {
                    let truncated = bytes.len() > MAX_FILE_BYTES;
                    let slice = &bytes[..bytes.len().min(MAX_FILE_BYTES)];
                    let mut text = String::from_utf8_lossy(slice).into_owned();
                    if truncated {
                        text.push_str("\n... (truncated — file is larger than REXO will inline)");
                    }
                    ResolvedRef { reference: reference.to_string(), content: Some(text), note: None }
                }
            }
            Err(e) => ResolvedRef { reference: reference.to_string(), content: None, note: Some(e.to_string()) },
        }
    }
}

fn list_dir(dir: &Path, workspace: &Path) -> std::io::Result<String> {
    let mut lines = Vec::new();
    let walker = ignore::WalkBuilder::new(dir).max_depth(Some(1)).require_git(false).build();
    for entry in walker.flatten() {
        if entry.path() == dir {
            continue;
        }
        let rel = entry.path().strip_prefix(workspace).unwrap_or(entry.path());
        lines.push(rel.display().to_string());
    }
    lines.sort();
    Ok(lines.join("\n"))
}

/// Append every resolved reference's content to `text` as a clearly
/// delimited block, for what's actually sent to the model — the
/// transcript still shows the user's original message unmodified (see
/// `cli::tui::run_turn`), only the wire message gets the expansion.
pub fn augment_with_references(text: &str, workspace: &Path) -> String {
    let refs = resolve_references(text, workspace);
    if refs.is_empty() {
        return text.to_string();
    }
    let mut out = text.to_string();
    out.push_str("\n\n---\nReferenced files:\n");
    for r in refs {
        match r.content {
            Some(content) => {
                out.push_str(&format!("\n@{}:\n```\n{content}\n```\n", r.reference));
            }
            None => {
                out.push_str(&format!("\n@{} — couldn't be included ({})\n", r.reference, r.note.unwrap_or_default()));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn workspace(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("rexo_test_file_ref_{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn extracts_multiple_references_and_strips_trailing_punctuation() {
        let refs = extract_refs("explain @src/main.rs and @Cargo.toml.");
        assert_eq!(refs, vec!["src/main.rs".to_string(), "Cargo.toml".to_string()]);
    }

    #[test]
    fn resolves_a_real_file() {
        let ws = workspace("resolve_file");
        std::fs::write(ws.join("main.rs"), "fn main() {}").unwrap();
        let resolved = resolve_references("@main.rs", &ws);
        assert_eq!(resolved.len(), 1);
        assert_eq!(resolved[0].content.as_deref(), Some("fn main() {}"));
    }

    #[test]
    fn refuses_to_escape_the_workspace() {
        let ws = workspace("escape_attempt");
        let resolved = resolve_references("@../../../etc/passwd", &ws);
        assert_eq!(resolved.len(), 1);
        assert!(resolved[0].content.is_none());
    }

    #[test]
    fn missing_file_is_reported_not_panicked() {
        let ws = workspace("missing_file");
        let resolved = resolve_references("@does/not/exist.rs", &ws);
        assert!(resolved[0].content.is_none());
        assert!(resolved[0].note.is_some());
    }

    #[test]
    fn directory_reference_lists_contents() {
        let ws = workspace("dir_ref");
        std::fs::create_dir_all(ws.join("src")).unwrap();
        std::fs::write(ws.join("src/a.rs"), "a").unwrap();
        std::fs::write(ws.join("src/b.rs"), "b").unwrap();
        let resolved = resolve_references("@src/", &ws);
        let content = resolved[0].content.as_ref().unwrap();
        assert!(content.contains("a.rs"));
        assert!(content.contains("b.rs"));
    }

    #[test]
    fn binary_looking_file_is_skipped_with_a_note() {
        let ws = workspace("binary_file");
        std::fs::write(ws.join("data.bin"), [0u8, 1, 2, 0, 3]).unwrap();
        let resolved = resolve_references("@data.bin", &ws);
        assert!(resolved[0].content.is_none());
        assert!(resolved[0].note.as_deref().unwrap_or("").contains("binary"));
    }

    #[test]
    fn text_without_references_is_returned_unmodified_by_augment() {
        let ws = workspace("no_refs");
        let augmented = augment_with_references("just a normal message", &ws);
        assert_eq!(augmented, "just a normal message");
    }

    #[test]
    fn augment_appends_content_for_a_real_reference() {
        let ws = workspace("augment_real");
        std::fs::write(ws.join("x.txt"), "hello world").unwrap();
        let augmented = augment_with_references("look at @x.txt", &ws);
        assert!(augmented.contains("look at @x.txt"));
        assert!(augmented.contains("hello world"));
    }

    #[test]
    fn suggest_finds_real_paths_and_marks_directories() {
        let ws = workspace("suggest");
        std::fs::create_dir_all(ws.join("src")).unwrap();
        std::fs::write(ws.join("src/main.rs"), "x").unwrap();
        let suggestions = suggest(&ws, "main");
        assert!(suggestions.iter().any(|s| s.contains("main.rs")));
        let dir_suggestions = suggest(&ws, "src");
        assert!(dir_suggestions.iter().any(|s| s == "src/"));
    }

    #[test]
    fn suggest_respects_gitignore() {
        let ws = workspace("suggest_gitignore");
        std::fs::write(ws.join(".gitignore"), "ignored_dir/\n").unwrap();
        std::fs::create_dir_all(ws.join("ignored_dir")).unwrap();
        std::fs::write(ws.join("ignored_dir/secret.rs"), "x").unwrap();
        std::fs::write(ws.join("visible.rs"), "x").unwrap();

        let suggestions = suggest(&ws, "");
        assert!(suggestions.iter().any(|s| s.contains("visible.rs")));
        assert!(!suggestions.iter().any(|s| s.contains("secret.rs")));
    }
}
