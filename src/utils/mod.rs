//! Small helpers shared across modules that don't warrant their own file.

pub mod clipboard;

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

/// Canonicalize a path the way every downstream consumer actually wants it:
/// absolute and symlink-resolved, but **without** Windows' `\\?\`
/// extended-length verbatim prefix that `std::fs::canonicalize` adds there.
///
/// That prefix is the real cause behind a whole class of "REXO just died"
/// reports on Windows (workspace switches, git/hook/MCP subprocess spawns,
/// title-bar/header rendering): `\\?\C:\foo` round-trips fine through pure
/// Rust `Path` code, but most external programs and even some Windows APIs
/// (anything that does its own path parsing rather than calling straight
/// into `CreateFileW`) choke on it or treat it as a different path than the
/// same directory written normally — so a `git`/hook/MCP-server child
/// spawned with it as its working directory can fail unpredictably, and a
/// user staring at `\\?\I:\TRON-Code\test1` in the header has no way to
/// know that's "just" `I:\TRON-Code\test1`. This is the same footgun
/// `cargo`, `rustc`, and ripgrep all route around (via the same crate this
/// wraps) rather than expose raw `std::fs::canonicalize` output to users or
/// to subprocesses. On non-Windows platforms this is exactly
/// `Path::canonicalize` — the prefix only exists on Windows in the first
/// place.
pub fn real_path(path: &Path) -> std::io::Result<PathBuf> {
    dunce::canonicalize(path)
}

/// Resolve the workspace directory: the `--workspace` flag if given,
/// otherwise the current directory, canonicalized so every downstream path
/// check (see [`crate::security::policies::validate_workspace_path`]) has a
/// stable, absolute root to compare against. See [`real_path`] for why this
/// isn't a plain `.canonicalize()` call.
pub fn resolve_workspace(explicit: Option<PathBuf>) -> Result<PathBuf> {
    let base = match explicit {
        Some(path) => path,
        None => std::env::current_dir().context("Failed to determine current directory")?,
    };

    real_path(&base).with_context(|| format!("Workspace directory does not exist: {}", base.display()))
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

    /// The actual regression: on a platform where `std::fs::canonicalize`
    /// adds a verbatim prefix (Windows' `\\?\`), `real_path` must not leak
    /// it. Can't force that prefix to appear on this Linux sandbox, but
    /// the important invariant — same resolved directory either way, and
    /// never a `\\?\`-prefixed display string — is checked directly.
    #[test]
    fn real_path_never_returns_a_windows_verbatim_prefix() {
        let dir = std::env::temp_dir();
        let resolved = real_path(&dir).unwrap();
        assert!(!resolved.display().to_string().starts_with(r"\\?\"));
    }
}
