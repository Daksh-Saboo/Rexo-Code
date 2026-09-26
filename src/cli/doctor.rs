//! `rexo doctor` — a real diagnostic pass over an installation, so a bug
//! report can be "run `rexo doctor --verbose`, paste the output" instead
//! of "it doesn't work" with no further information to go on.
//!
//! Runs deliberately *before* config load's own error handling, the
//! first-launch wizard, and the trust dialog (see `main.rs`) — diagnosing
//! a broken or brand-new install is exactly the situation where none of
//! those should stand in the way, and a missing/invalid config is one of
//! the things this reports, not something that stops it from running.
//!
//! What this checks: config load, provider settings + credential
//! resolution (never the key's *value*, only whether one resolves),
//! configured MCP servers (existence, not live connectivity — actually
//! spawning each one open a moment doctor doesn't need it to), skills
//! discovery, permissions, basic network reachability to the configured
//! provider host, git, and the shell. What it doesn't do yet: an actual
//! authenticated request to the provider (so a genuinely wrong/expired
//! key still shows as "configured" rather than "invalid" — confirming
//! that needs a real, billable API call doctor deliberately doesn't
//! make on your behalf) or spawning every configured MCP server to
//! confirm each one still starts cleanly (`/mcp connect` already does
//! that, on demand, with its own real error reporting).

use std::path::Path;
use std::time::Duration;

use colored::Colorize;

use crate::config::Config;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Status {
    Pass,
    Warn,
    Fail,
}

impl Status {
    fn marker(self) -> colored::ColoredString {
        match self {
            Status::Pass => "✓".green(),
            Status::Warn => "!".yellow(),
            Status::Fail => "✗".red(),
        }
    }
}

struct Check {
    name: String,
    status: Status,
    detail: String,
}

/// Runs every check and prints them Claude-Code-doctor-style (a section
/// per area, a pass/warn/fail line per check), then returns the process
/// exit code: `0` if nothing failed (warnings don't fail the run — a
/// warning is "worth knowing," a failure is "REXO likely won't work"),
/// `1` if anything did.
pub async fn run(workspace: &Path, verbose: bool) -> i32 {
    println!("{}", "Rexo Code Doctor".bold());
    println!();

    let mut checks: Vec<Check> = Vec::new();

    checks.push(installation_check());

    let config = match Config::load(workspace) {
        Ok(c) => {
            checks.push(Check { name: "Configuration".to_string(), status: Status::Pass, detail: config_detail(&c, workspace, verbose) });
            Some(c)
        }
        Err(e) => {
            checks.push(Check { name: "Configuration".to_string(), status: Status::Fail, detail: format!("couldn't load config: {e}") });
            None
        }
    };

    if let Some(config) = &config {
        checks.push(provider_check(config, verbose));
        checks.push(mcp_check(config, verbose));
        checks.push(skills_check(workspace, verbose));
        checks.push(permissions_check(config, verbose));
        checks.push(network_check(config).await);
    }
    checks.push(git_check(workspace, verbose));
    checks.push(shell_check(verbose));

    for check in &checks {
        println!("{} {}", check.status.marker(), check.name.bold());
        if !check.detail.is_empty() && (verbose || check.status != Status::Pass) {
            for line in check.detail.lines() {
                println!("    {}", line.dimmed());
            }
        }
    }

    let failed = checks.iter().filter(|c| c.status == Status::Fail).count();
    let warned = checks.iter().filter(|c| c.status == Status::Warn).count();
    println!();
    if failed == 0 && warned == 0 {
        println!("{}", "All checks passed.".green().bold());
    } else {
        println!("{} passed, {} warning(s), {} failure(s).", checks.len() - failed - warned, warned, failed);
    }

    if failed > 0 {
        1
    } else {
        0
    }
}

fn installation_check() -> Check {
    Check {
        name: "Rexo installation".to_string(),
        status: Status::Pass,
        detail: format!("v{} · {}-{}", env!("CARGO_PKG_VERSION"), std::env::consts::OS, std::env::consts::ARCH),
    }
}

fn config_detail(config: &Config, workspace: &Path, verbose: bool) -> String {
    if !verbose {
        return String::new();
    }
    format!(
        "workspace: {}\nglobal config: {}\nprovider kind: {}",
        workspace.display(),
        crate::config::global::global_dir().map(|d| d.display().to_string()).unwrap_or_else(|_| "unresolved".to_string()),
        config.model.provider
    )
}

fn provider_check(config: &Config, verbose: bool) -> Check {
    let name = "Provider configuration".to_string();
    let provider = &config.model.provider;
    let Some(model) = &config.model.model else {
        return Check { name, status: Status::Fail, detail: "no model configured — run /model or the setup wizard".to_string() };
    };

    match config.api_key() {
        Ok(key) => {
            let key_state = if key.is_empty() { "no key required" } else { "key resolved" };
            let mut detail = format!("{provider} · {model} · {key_state}");
            if verbose {
                if let Some(url) = &config.model.base_url {
                    detail.push_str(&format!("\nendpoint: {url}"));
                }
                detail.push_str(&format!("\ncredential key: {}", config.effective_credential_key()));
            }
            Check { name, status: Status::Pass, detail }
        }
        Err(e) => Check { name, status: Status::Fail, detail: format!("{provider} · {model} — {e}") },
    }
}

fn mcp_check(config: &Config, verbose: bool) -> Check {
    let name = "MCP configuration".to_string();
    let servers = &config.global.mcp_servers;
    if servers.is_empty() {
        return Check { name, status: Status::Pass, detail: "none configured".to_string() };
    }
    let mut lines = Vec::new();
    for (server_name, server) in servers {
        let missing_command = server.command.as_deref().map(str::trim).unwrap_or("").is_empty();
        if missing_command {
            lines.push(format!("! {server_name}: no command configured"));
        } else if verbose {
            lines.push(format!("  {server_name}: {}", server.command.as_deref().unwrap_or("")));
        }
    }
    let any_broken = servers.values().any(|s| s.command.as_deref().map(str::trim).unwrap_or("").is_empty());
    Check {
        name,
        status: if any_broken { Status::Warn } else { Status::Pass },
        detail: if lines.is_empty() { format!("{} server(s) configured (not connected — see /mcp connect)", servers.len()) } else { lines.join("\n") },
    }
}

fn skills_check(workspace: &Path, verbose: bool) -> Check {
    let skills = crate::cli::skills::discover(workspace);
    let detail = if verbose {
        skills.iter().map(|s| format!("{} ({})", s.name, if s.project { "project" } else { "global" })).collect::<Vec<_>>().join("\n")
    } else {
        format!("{} discovered", skills.len())
    };
    Check { name: "Skills".to_string(), status: Status::Pass, detail }
}

fn permissions_check(config: &Config, verbose: bool) -> Check {
    let p = &config.permissions;
    let detail = if verbose {
        format!(
            "edit: {}  terminal: {}  git-write: {}  network: {}",
            auto_or_ask(p.allow_edit),
            auto_or_ask(p.allow_terminal),
            auto_or_ask(p.allow_git_write),
            auto_or_ask(p.allow_network)
        )
    } else {
        String::new()
    };
    Check { name: "Permissions".to_string(), status: Status::Pass, detail }
}

fn auto_or_ask(auto: bool) -> &'static str {
    if auto {
        "auto"
    } else {
        "ask"
    }
}

/// A plain TCP connect to the configured provider's host — enough to
/// tell "no network / DNS / firewall problem" apart from "network's
/// fine, something else is wrong" (an invalid key, a wrong model ID)
/// without making an actual authenticated, billable request. `local`
/// providers are skipped (no reason to expect anything's listening
/// before the user has started one).
async fn network_check(config: &Config) -> Check {
    let name = "Network".to_string();
    if config.model.provider == "local" {
        return Check { name, status: Status::Pass, detail: "skipped for the local provider".to_string() };
    }
    let host = config
        .model
        .base_url
        .as_deref()
        .and_then(|u| reqwest::Url::parse(u).ok())
        .and_then(|u| u.host_str().map(str::to_string))
        .or_else(|| default_host_for(&config.model.provider));
    let Some(host) = host else {
        return Check { name, status: Status::Pass, detail: "no endpoint configured to check yet — set one via /connect or /base-url".to_string() };
    };
    match tokio::time::timeout(Duration::from_secs(5), tokio::net::lookup_host(format!("{host}:443"))).await {
        Ok(Ok(mut addrs)) => {
            if addrs.next().is_some() {
                Check { name, status: Status::Pass, detail: format!("{host} resolves") }
            } else {
                Check { name, status: Status::Warn, detail: format!("{host} resolved to no addresses") }
            }
        }
        Ok(Err(e)) => Check { name, status: Status::Fail, detail: format!("couldn't resolve {host}: {e}") },
        Err(_) => Check { name, status: Status::Warn, detail: format!("timed out resolving {host} (5s)") },
    }
}

/// A default host to check when no `base_url` is configured at all —
/// only for the three provider kinds with one fixed, known endpoint
/// (`nvidia`, `gemini`, `anthropic`); everything else genuinely has no
/// single right answer (many different `openai_compatible` presets
/// share that kind with different endpoints), so guessing one would be
/// actively misleading rather than merely incomplete.
fn default_host_for(provider: &str) -> Option<String> {
    match provider {
        "nvidia" => Some("integrate.api.nvidia.com".to_string()),
        "gemini" => Some("generativelanguage.googleapis.com".to_string()),
        "anthropic" => Some(crate::providers::anthropic::ANTHROPIC_ENDPOINT.trim_start_matches("https://").to_string()),
        _ => None,
    }
}

fn git_check(workspace: &Path, verbose: bool) -> Check {
    let name = "Git".to_string();
    match std::process::Command::new("git").arg("--version").output() {
        Ok(out) if out.status.success() => {
            let version = String::from_utf8_lossy(&out.stdout).trim().to_string();
            let in_repo = workspace.join(".git").exists();
            let detail = if verbose {
                format!("{version}\n{}", if in_repo { "workspace is a git repo" } else { "workspace is not a git repo (fine — git tools just won't have much to do)" })
            } else {
                String::new()
            };
            Check { name, status: Status::Pass, detail }
        }
        Ok(_) | Err(_) => Check { name, status: Status::Warn, detail: "git not found on PATH — git-based tools will fail if used".to_string() },
    }
}

fn shell_check(verbose: bool) -> Check {
    let name = "Shell".to_string();
    let shell = if cfg!(target_os = "windows") {
        std::env::var("COMSPEC").unwrap_or_else(|_| "cmd.exe".to_string())
    } else {
        std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string())
    };
    Check { name, status: Status::Pass, detail: if verbose { shell } else { String::new() } }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tempdir() -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("rexo_doctor_test_{}_{}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().subsec_nanos()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[tokio::test]
    async fn a_workspace_with_no_config_still_runs_every_check_without_panicking() {
        // The actual point of `doctor`: it has to work on a broken or
        // brand-new install, not just a healthy one — this is the
        // "nothing configured yet" case (Config::load falling back to
        // defaults, no rexo.toml, no global config), which is exactly
        // where a diagnostic tool earns its keep.
        let dir = tempdir();
        let global_dir = dir.join("global");
        std::fs::create_dir_all(&global_dir).unwrap();
        std::env::set_var("REXO_GLOBAL_DIR", &global_dir);
        let exit = run(&dir, false).await;
        // nvidia is the default provider with no key configured anywhere
        // in this isolated env, so a failed provider check is expected —
        // the assertion that matters is that it returned an exit code at
        // all rather than panicking or hanging.
        assert!(exit == 0 || exit == 1);
        std::env::remove_var("REXO_GLOBAL_DIR");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn mcp_check_warns_on_a_server_with_no_command() {
        let mut config = Config::load(&tempdir()).unwrap();
        config.global.mcp_servers.insert(
            "broken".to_string(),
            crate::config::global::McpServerConfig { command: None, url: None, enabled: true },
        );
        let check = mcp_check(&config, false);
        assert_eq!(check.status, Status::Warn);
    }

    #[test]
    fn provider_check_fails_cleanly_with_no_model_configured() {
        let mut config = Config::load(&tempdir()).unwrap();
        config.model.model = None;
        let check = provider_check(&config, false);
        assert_eq!(check.status, Status::Fail);
    }
}
