//! Static security policy: workspace boundary enforcement and terminal
//! command risk classification.
//!
//! This module makes no I/O-side decisions on its own beyond reading the
//! filesystem to canonicalize paths — it just answers "is this allowed in
//! principle?" questions. The actual prompt-the-user flow lives in
//! [`crate::security::permissions`].

use std::path::{Component, Path, PathBuf};

use anyhow::{anyhow, Result};

/// How risky a proposed action is judged to be. Used both for terminal
/// commands and, in principle, for other tools in the future.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RiskLevel {
    /// Low risk, but still worth a one-time confirmation (e.g. `npm install`).
    Moderate,
    /// High risk — destructive, irreversible, or system-altering.
    High,
}

/// The outcome of classifying a shell command against the terminal policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandRisk {
    /// Read-only / side-effect-free. Runs without asking.
    Safe,
    /// Mutates the project or environment in a recoverable way. Ask first.
    RequiresApproval,
    /// Never run automatically, no matter what the config says.
    Dangerous,
}

/// Prefixes that are considered safe to run without confirmation. This is a
/// conservative allow-list, matched against the *start* of the (trimmed)
/// command string after normalizing whitespace.
const SAFE_PREFIXES: &[&str] = &[
    "cargo check",
    "cargo test",
    "cargo build",
    "cargo fmt --check",
    "cargo clippy",
    "cargo metadata",
    "cargo tree",
    "git status",
    "git diff",
    "git log",
    "git branch",
    "git show",
    "npm test",
    "npm run test",
    "npm ls",
    "pnpm test",
    "yarn test",
    "pytest",
    "python -m pytest",
    "ls",
    "dir",
    "pwd",
    "echo",
    "cat",
    "type ",
    "Get-ChildItem",
    "Get-Content",
];

/// Substrings that immediately mark a command as dangerous, regardless of
/// anything else in it. Matched case-insensitively.
const DANGEROUS_PATTERNS: &[&str] = &[
    "rm -rf /",
    "rm -rf ~",
    "rm -rf *",
    "remove-item",
    "rd /s",
    "rmdir /s",
    "del /f",
    "del /q",
    "format ",
    "mkfs",
    "diskpart",
    "dd if=",
    "shutdown",
    "restart-computer",
    ":(){ :|:& };:", // fork bomb
    "curl | sh",
    "curl | bash",
    "wget | sh",
    "iex (",
    "invoke-expression",
    "invoke-webrequest",
    "set-executionpolicy",
    "chmod -r 777 /",
    "chown -r",
    "sudo ",
    "reg delete",
    "reg add",
    "net user",
    "credential",
    "passwd",
];

/// Classify a raw shell command string into a risk bucket.
pub fn classify_command(command: &str) -> CommandRisk {
    let normalized = command.trim();
    let lowered = normalized.to_lowercase();

    for pattern in DANGEROUS_PATTERNS {
        if lowered.contains(pattern) {
            return CommandRisk::Dangerous;
        }
    }

    // Command chaining/piping can be used to smuggle a dangerous command
    // behind a safe-looking prefix (e.g. `git status; rm -rf /`). If the
    // command contains a chaining operator, re-classify each segment and
    // take the worst outcome instead of trusting only the first prefix.
    if contains_chaining(normalized) {
        let mut worst = CommandRisk::RequiresApproval;
        for segment in split_on_chaining(normalized) {
            match classify_command(&segment) {
                CommandRisk::Dangerous => return CommandRisk::Dangerous,
                CommandRisk::RequiresApproval => worst = CommandRisk::RequiresApproval,
                CommandRisk::Safe => {}
            }
        }
        return worst;
    }

    if SAFE_PREFIXES
        .iter()
        .any(|prefix| normalized.starts_with(prefix))
    {
        return CommandRisk::Safe;
    }

    CommandRisk::RequiresApproval
}

fn contains_chaining(command: &str) -> bool {
    ["&&", "||", ";", "|", "`", "$("]
        .iter()
        .any(|op| command.contains(op))
}

fn split_on_chaining(command: &str) -> Vec<String> {
    command
        .split(|c| matches!(c, '&' | '|' | ';'))
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

/// Resolve `requested` (a path the model asked to touch, absolute or
/// relative) against `workspace`, and make sure the result stays inside the
/// workspace boundary. Purely lexical for paths that don't exist yet
/// (needed for file *creation*), but canonicalizes when the path already
/// exists so symlink tricks can't escape the sandbox.
pub fn validate_workspace_path(workspace: &Path, requested: &str) -> Result<PathBuf> {
    let workspace = workspace
        .canonicalize()
        .unwrap_or_else(|_| workspace.to_path_buf());

    let candidate = Path::new(requested);
    let joined = if candidate.is_absolute() {
        candidate.to_path_buf()
    } else {
        workspace.join(candidate)
    };

    let normalized = lexically_normalize(&joined);

    let resolved = normalized
        .canonicalize()
        .unwrap_or_else(|_| normalized.clone());

    if !resolved.starts_with(&workspace) {
        return Err(anyhow!(
            "Refusing to access '{}': outside the workspace boundary ({})",
            requested,
            workspace.display()
        ));
    }

    Ok(resolved)
}

/// Like [`validate_workspace_path`], but for the read-only tools
/// (`read_file`, `list_files`) that also accept `/add-dir`-added roots:
/// tries the primary `workspace` first (so every existing relative-path
/// call keeps working exactly as before — this is purely additive), and
/// on failure, tries each of `extra_roots` in turn, but *only* for a path
/// that's already absolute. A relative path is deliberately never
/// resolved against an extra root — there'd be no way to tell which root
/// it meant, and silently guessing is exactly the kind of ambiguity a
/// workspace boundary check exists to avoid. Point at an added directory
/// with its full path (`@`-references and `list_files` both accept one).
pub fn validate_read_path(workspace: &Path, extra_roots: &[PathBuf], requested: &str) -> Result<PathBuf> {
    if let Ok(resolved) = validate_workspace_path(workspace, requested) {
        return Ok(resolved);
    }
    if Path::new(requested).is_absolute() {
        for root in extra_roots {
            if let Ok(resolved) = validate_workspace_path(root, requested) {
                return Ok(resolved);
            }
        }
    }
    // Re-run the primary check purely to get its real error message
    // rather than a generic one — it always fails again here (we already
    // know it does), this just gives the caller an accurate reason.
    validate_workspace_path(workspace, requested)
}

/// Manually resolve `.` and `..` components without touching the
/// filesystem (`Path::canonicalize` requires the path to exist).
fn lexically_normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn safe_commands_are_recognized() {
        assert_eq!(classify_command("cargo check"), CommandRisk::Safe);
        assert_eq!(classify_command("git status"), CommandRisk::Safe);
    }

    #[test]
    fn unknown_commands_require_approval() {
        assert_eq!(classify_command("cargo add serde"), CommandRisk::RequiresApproval);
        assert_eq!(classify_command("npm install"), CommandRisk::RequiresApproval);
    }

    #[test]
    fn dangerous_commands_are_flagged() {
        assert_eq!(classify_command("Remove-Item -Recurse -Force C:\\"), CommandRisk::Dangerous);
        assert_eq!(classify_command("rm -rf /"), CommandRisk::Dangerous);
        assert_eq!(classify_command("sudo rm -rf /var"), CommandRisk::Dangerous);
    }

    #[test]
    fn chained_dangerous_command_is_caught_even_behind_a_safe_prefix() {
        assert_eq!(
            classify_command("git status && rm -rf /"),
            CommandRisk::Dangerous
        );
    }

    #[test]
    fn path_traversal_outside_workspace_is_rejected() {
        let dir = std::env::temp_dir().join("rexo_test_workspace");
        std::fs::create_dir_all(&dir).unwrap();
        let err = validate_workspace_path(&dir, "../../etc/passwd").unwrap_err();
        assert!(err.to_string().contains("outside the workspace"));
    }

    #[test]
    fn path_inside_workspace_is_accepted() {
        let dir = std::env::temp_dir().join("rexo_test_workspace_ok");
        std::fs::create_dir_all(&dir).unwrap();
        let resolved = validate_workspace_path(&dir, "src/main.rs").unwrap();
        assert!(resolved.starts_with(dir.canonicalize().unwrap()));
    }
}
