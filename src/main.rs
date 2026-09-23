mod agent;
mod cli;
mod config;
mod hooks;
mod ide;
mod mcp;
#[macro_use]
mod output;
mod plugins;
mod providers;
mod security;
mod tools;
mod utils;

use std::path::PathBuf;

use anyhow::Result;
use clap::Parser;
use colored::Colorize;

use agent::Agent;
use config::Config;

// A small, original glyph — deliberately not a copy of any other tool's
// mascot, just a compact "circuit node" that fits next to a few info
// lines the way the boxed banner below lays them out.
const MASCOT: [&str; 3] = ["┏━━┓", "┃◉◉┃", "┗━━┛"];

const BANNER_WIDTH: usize = 78;

#[derive(Parser, Debug)]
#[command(
    name = "rexo",
    version,
    about = "Rexo Code — open-source AI coding agent"
)]
struct Cli {
    /// Task/prompt to run once and exit. If omitted, starts an interactive session.
    #[arg(trailing_var_arg = true)]
    prompt: Vec<String>,

    /// Alias for "run once and exit" (headless/"print" mode) — present so
    /// `rexo -p "task"` works the way it does in other agent CLIs. Doesn't
    /// change parsing: providing a prompt at all already means headless
    /// mode; this flag is accepted but otherwise a no-op.
    #[arg(short = 'p', long = "print")]
    print: bool,

    /// Automatically approve every permission prompt (edits, commands, git writes).
    /// Use with care — this removes the human-in-the-loop safety net.
    #[arg(short = 'y', long)]
    yes: bool,

    /// Never prompt interactively; deny anything not pre-approved in rexo.toml.
    /// Useful for CI / piping rexo's input from a script.
    #[arg(long)]
    non_interactive: bool,

    /// Workspace directory to operate in (defaults to the current directory).
    #[arg(short = 'C', long)]
    workspace: Option<PathBuf>,

    /// Override the model configured in rexo.toml / REXO_MODEL for this run.
    #[arg(long)]
    model: Option<String>,

    /// Override the provider configured in rexo.toml / REXO_PROVIDER for this
    /// run. One of: nvidia, openai_compatible, local.
    #[arg(long)]
    provider: Option<String>,

    /// Override the API base URL configured in rexo.toml / REXO_BASE_URL for
    /// this run. Ignored by the nvidia provider, which uses a fixed endpoint.
    #[arg(long = "base-url")]
    base_url: Option<String>,

    /// List available tools and exit (no API key required).
    #[arg(long)]
    list_tools: bool,

    /// Output format for single-shot (`rexo "task"`) runs. `text` (default)
    /// prints normally; `json`/`jsonl` print one JSON object with the
    /// outcome instead, for scripts/CI to parse — see README's Headless
    /// mode section. Ignored for interactive sessions.
    #[arg(long, value_enum, default_value_t = OutputFormat::Text)]
    output: OutputFormat,

    /// Trim reasoning effort and the token ceiling for this run — a quick
    /// lever for snappier responses on simple tasks. For a real speedup on
    /// reasoning models, switch [model].model in rexo.toml instead (see
    /// the commented-out fast-model options there).
    #[arg(long)]
    fast: bool,

    /// Skip the animated startup intro (the scattering face). Also honors
    /// the REXO_NO_INTRO env var, for scripts/CI that launch the
    /// interactive TUI without a human watching it draw in.
    #[arg(long)]
    no_intro: bool,
}

#[derive(clap::ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
enum OutputFormat {
    Text,
    Json,
    Jsonl,
}

/// Process exit codes for single-shot/headless runs (`rexo "task"`),
/// meant to be scriptable (CI, git hooks, `cat error.log | rexo -p "..."`
/// style pipelines) — see README's Headless mode section for what each
/// means. Interactive sessions always exit 0 on a normal `/exit`.
mod exit_code {
    pub const SUCCESS: i32 = 0;
    pub const TASK_FAILURE: i32 = 1;
    pub const CONFIG_ERROR: i32 = 2;
    #[allow(dead_code)] // Not yet distinguished from TASK_FAILURE — see README's Known limitations.
    pub const PERMISSION_DENIED: i32 = 3;
}

#[tokio::main]
async fn main() -> Result<()> {
    dotenvy::dotenv().ok();
    let cli = Cli::parse();

    let workspace = utils::resolve_workspace(cli.workspace.clone())?;

    if cli.list_tools {
        println!("Available tools:");
        println!();
        for tool in tools::default_registry().list() {
            println!("  {}", tool.name().bold());
            println!("    {}", tool.description());
        }
        return Ok(());
    }

    let is_interactive_session = cli.prompt.is_empty();
    let skip_trust_dialog = cli.yes || cli.non_interactive || !is_interactive_session;

    let mut config = Config::load(&workspace)?;
    cli::wizard::maybe_run(&mut config, cli.non_interactive, cli.yes).await?;

    if !skip_trust_dialog {
        match cli::trust_dialog::confirm_workspace_trust(&workspace) {
            cli::trust_dialog::TrustDecision::Trusted | cli::trust_dialog::TrustDecision::Skipped => {}
            cli::trust_dialog::TrustDecision::Declined => {
                println!("Exited without accessing {}.", workspace.display());
                return Ok(());
            }
        }
    }

    if let Some(model) = cli.model.clone() {
        config.model.model = Some(model);
    }
    if let Some(provider) = cli.provider.clone() {
        config.model.provider = provider;
        config.display_name = None;
        config.credential_key = None;
    }
    if let Some(base_url) = cli.base_url.clone() {
        config.model.base_url = Some(base_url);
    }
    if cli.fast {
        config.model.reasoning_effort = Some("low".to_string());
        config.model.max_tokens = config.model.max_tokens.min(2048);
    }

    let non_interactive = cli.non_interactive;

    // Interactive sessions (no prompt given on the command line) should
    // always reach the TUI, even with nothing configured yet — /connect
    // can fix that from inside, the same as it fixes a *later* config
    // change gone wrong (see `Session::rebuild_provider`, which this
    // mirrors). Only headless/single-shot mode has no interactive
    // fallback to reach, so it keeps failing fast below.
    let (mut agent, mut history) = if is_interactive_session {
        match config.api_key() {
            Ok(api_key) => match Agent::bootstrap(&config, api_key, non_interactive, cli.yes) {
                Ok(v) => v,
                Err(e) => Agent::bootstrap_unconfigured(&config, e.to_string(), non_interactive, cli.yes).expect("bootstrap_unconfigured never actually fails"),
            },
            Err(e) => Agent::bootstrap_unconfigured(&config, e.to_string(), non_interactive, cli.yes).expect("bootstrap_unconfigured never actually fails"),
        }
    } else {
        let api_key = match config.api_key() {
            Ok(k) => k,
            Err(e) => return fail_startup(&cli, &e.to_string(), exit_code::CONFIG_ERROR),
        };
        match Agent::bootstrap(&config, api_key, non_interactive, cli.yes) {
            Ok(v) => v,
            Err(e) => return fail_startup(&cli, &e.to_string(), exit_code::CONFIG_ERROR),
        }
    };

    if !cli.prompt.is_empty() {
        let prompt = cli.prompt.join(" ");
        let json_mode = cli.output != OutputFormat::Text;
        if !json_mode {
            print_banner(&config, &agent, &workspace);
        }
        match run_single_shot(&mut agent, &mut history, &prompt, json_mode).await {
            Ok(answer) => {
                if json_mode {
                    print_json_result(true, Some(&answer), None, None);
                }
                std::process::exit(exit_code::SUCCESS);
            }
            Err(e) => {
                if json_mode {
                    let kind = providers::provider_error_kind(&e);
                    let kind_label = (kind != providers::ProviderErrorKind::Unknown).then(|| kind.label());
                    print_json_result(false, None, Some(&e.to_string()), kind_label);
                } else {
                    eprintln!("{} {}", "error:".red().bold(), e);
                }
                std::process::exit(exit_code::TASK_FAILURE);
            }
        }
    } else {
        if non_interactive {
            anyhow::bail!(
                "Interactive mode requires a terminal. Pass a prompt directly \
                 (rexo \"your task\") when running non-interactively."
            );
        }
        let mut session = cli::Session::new(config, agent, history);
        let show_intro = !cli.no_intro && std::env::var("REXO_NO_INTRO").is_err();
        cli::tui::run(&mut session, show_intro).await?;
    }

    Ok(())
}

/// A config/credential-resolution failure before the agent could even
/// start — printed and exited with [`exit_code::CONFIG_ERROR`] rather
/// than propagated as a generic error, so scripts can tell "REXO isn't
/// set up" apart from "the task itself failed". `-> Result<()>` purely so
/// this can be used with an early `return` from `main`; it never actually
/// returns `Ok`.
fn fail_startup(cli: &Cli, message: &str, code: i32) -> Result<()> {
    let json_mode = cli.output != OutputFormat::Text && !cli.prompt.is_empty();
    if json_mode {
        print_json_result(false, None, Some(message), None);
    } else {
        eprintln!("{} {message}", "error:".red().bold());
        // Only reachable from headless/single-shot mode now (a prompt was
        // given on the command line) — interactive sessions no longer
        // fail here at all, see the graceful-fallback branch above.
        eprintln!("{}", "(run `rexo` with no prompt to configure a provider interactively via /connect)".dimmed());
    }
    std::process::exit(code);
}

/// `error_kind` is the normalized [`providers::ProviderErrorKind`] label
/// (`"rate_limited"`, `"authentication"`, etc.) when the failure came from
/// a provider call and was classified as something more specific than
/// `Unknown` — `null` otherwise (config/startup failures, or a provider
/// error REXO couldn't classify). Scripts driving `rexo --output json` in
/// CI can branch on this instead of pattern-matching the message text.
fn print_json_result(success: bool, answer: Option<&str>, error: Option<&str>, error_kind: Option<&str>) {
    let value = serde_json::json!({
        "status": if success { "ok" } else { "error" },
        "answer": answer,
        "error": error,
        "error_kind": error_kind,
    });
    println!("{value}");
}

fn print_banner(config: &Config, agent: &Agent, workspace: &std::path::Path) {
    let info_lines = [
        format!(
            "{} {}",
            "Rexo Code".bold(),
            format!("v{}", env!("CARGO_PKG_VERSION")).dimmed()
        ),
        agent.provider_description(),
        workspace.display().to_string(),
        format!(
            "Edits: {}  Terminal: {}  Git writes: {}",
            permission_tag(config.permissions.allow_edit),
            permission_tag(config.permissions.allow_terminal),
            permission_tag(config.permissions.allow_git_write),
        ),
    ];

    println!("{}", utils::rule(BANNER_WIDTH).dimmed());
    for (i, glyph) in MASCOT.iter().enumerate() {
        let info = info_lines.get(i).cloned().unwrap_or_default();
        println!(" {}  {}", glyph.cyan().bold(), info);
    }
    if let Some(extra) = info_lines.get(MASCOT.len()) {
        println!("      {extra}");
    }
    println!("{}", utils::rule(BANNER_WIDTH).dimmed());
}

fn permission_tag(allowed: bool) -> colored::ColoredString {
    if allowed {
        "auto".green()
    } else {
        "ask".yellow()
    }
}

async fn run_single_shot(
    agent: &mut Agent,
    history: &mut Vec<providers::ChatMessage>,
    prompt: &str,
    json_mode: bool,
) -> Result<String> {
    let show_hook_notes = |notes: Vec<String>| {
        if !json_mode {
            for n in notes {
                println!("{} {n}", "hook:".dimmed());
            }
        }
    };
    show_hook_notes(agent.run_session_start_hooks());

    let result = if json_mode {
        // Machine-readable output must be exactly one JSON line — nothing
        // else on stdout. The plain (non-TUI) agent path's streamed
        // tokens/tool-call lines go through the same capture-aware
        // `println!`/`print!` the TUI uses (see `agent::mod`'s import and
        // `crate::output`), so this collects and discards them rather
        // than letting them print directly.
        output::begin_capture();
        let result = agent.respond(history, prompt).await;
        output::end_capture();
        result
    } else {
        println!("{} {}", ">".bright_black(), prompt);
        println!();
        agent.respond(history, prompt).await
    };

    show_hook_notes(agent.run_session_end_hooks());
    result
}
