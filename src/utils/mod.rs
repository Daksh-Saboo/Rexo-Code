//! Small helpers shared across modules that don't warrant their own file.

pub mod clipboard;

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

/// Resolve the workspace directory: the `--workspace` flag if given,
/// otherwise the current directory, canonicalized so every downstream path
/// check (see [`crate::security::policies::validate_workspace_path`]) has a
/// stable, absolute root to compare against.
pub fn resolve_workspace(explicit: Option<PathBuf>) -> Result<PathBuf> {
    let base = match explicit {
        Some(path) => path,
        None => std::env::current_dir().context("Failed to determine current directory")?,
    };

    base.canonicalize()
        .with_context(|| format!("Workspace directory does not exist: {}", base.display()))
}

/// Render a dimmed horizontal rule of the given width for CLI banners.
pub fn rule(width: usize) -> String {
    "─".repeat(width)
}

#[allow(dead_code)]
pub fn is_within(root: &Path, candidate: &Path) -> bool {
    candidate.starts_with(root)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_workspace_defaults_to_current_dir() {
        let resolved = resolve_workspace(None).unwrap();
        assert!(resolved.is_absolute());
    }
}
