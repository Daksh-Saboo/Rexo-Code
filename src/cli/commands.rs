//! The command table itself: one `CommandSpec` per slash command, backed
//! by a plain `fn` handler rather than a full trait object per command.
//! Given how many commands this REPL has, a data table keeps the whole
//! set easy to scan and extend — adding a command later means adding one
//! handler fn and one line to `COMMANDS`, not a new file.
//!
//! Every handler prints its own output (this is a terminal REPL, not a
//! library), but the *content* that could plausibly leak a secret is
//! always built through `render_status`/`render_config`, which only ever
//! see a `configured: bool`, never the key itself — see the `redaction`
//! tests at the bottom of this file.

use std::io::{self, Write};
use std::path::{Path, PathBuf};

use anyhow::Result;
use colored::Colorize;

use crate::config::Config;
use crate::security::permissions::{PermissionKind, PermissionManager};

use super::{AccentColor, CommandOutcome, LoopJob, Session};

// Shadow the std `println!`/`print!` macros within this file with ones
// that write to the TUI's output capture when it's active (see
// `crate::output`) and behave exactly like the real thing otherwise. Every
// existing `println!`/`print!` call below already reads correctly against
// either — this import is the only change needed to make slash-command
// output land in the transcript pane instead of racing a redraw on real
// stdout.
use crate::{tprint as print, tprintln as println};

type Handler = fn(&mut Session, &[String]) -> Result<CommandOutcome>;

/// Whether a command is fully working, or registered purely so `/help` and
/// autocomplete can give an honest "not yet" instead of "unknown command".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Ready,
    Planned,
}

pub struct CommandSpec {
    pub name: &'static str,
    pub aliases: &'static [&'static str],
    pub description: &'static str,
    pub usage: &'static str,
    /// Kept out of `/help`'s command table — reachable only via the `>`
    /// shortcut the parser rewrites to this name, not `/workspace-select`.
    pub hidden: bool,
    /// Whether this command needs a real, non-raw-mode terminal (it does a
    /// blocking `stdin` line/password read via `prompt_line`/`rpassword`).
    /// The TUI leaves the alternate screen for the duration of the call
    /// rather than trying to make blocking stdin reads work while raw
    /// mode owns the terminal — see `cli::tui::run_command`.
    pub needs_terminal: bool,
    pub status: Status,
    pub handler: Handler,
}

pub const COMMANDS: &[CommandSpec] = &[
    CommandSpec {
        name: "help",
        aliases: &["?", "h"],
        description: "Show this help",
        usage: "/help",
        hidden: false,
        needs_terminal: false,
        status: Status::Ready,
        handler: help_cmd,
    },
    CommandSpec {
        name: "status",
        aliases: &[],
        description: "Show current session status",
        usage: "/status",
        hidden: false,
        needs_terminal: false,
        status: Status::Ready,
        handler: status_cmd,
    },
    CommandSpec {
        name: "model",
        aliases: &[],
        description: "Show/change the current model",
        usage: "/model [model-id]",
        hidden: false,
        needs_terminal: false,
        status: Status::Ready,
        handler: model_cmd,
    },
    CommandSpec {
        name: "models",
        aliases: &[],
        description: "List example models for the current provider",
        usage: "/models",
        hidden: false,
        needs_terminal: false,
        status: Status::Ready,
        handler: models_cmd,
    },
    CommandSpec {
        name: "provider",
        aliases: &[],
        description: "Show/change the current provider ('add' for a custom one)",
        usage: "/provider [nvidia|openai_compatible|local|add]",
        hidden: false,
        needs_terminal: true,
        status: Status::Ready,
        handler: provider_cmd,
    },
    CommandSpec {
        name: "providers",
        aliases: &[],
        description: "List known providers and their configuration status",
        usage: "/providers",
        hidden: false,
        needs_terminal: false,
        status: Status::Ready,
        handler: providers_cmd,
    },
    CommandSpec {
        name: "base-url",
        aliases: &["baseurl"],
        description: "Show/change the API base URL",
        usage: "/base-url [url]",
        hidden: false,
        needs_terminal: false,
        status: Status::Ready,
        handler: base_url_cmd,
    },
    CommandSpec {
        name: "workspace",
        aliases: &["cwd"],
        description: "Show/change the workspace directory",
        usage: "/workspace [path]",
        hidden: false,
        needs_terminal: true,
        status: Status::Ready,
        handler: workspace_cmd,
    },
    CommandSpec {
        name: "cd",
        aliases: &[],
        description: "Move this session to a new working directory (alias of /workspace)",
        usage: "/cd [path]",
        hidden: false,
        needs_terminal: true,
        status: Status::Ready,
        handler: workspace_cmd,
    },
    CommandSpec {
        name: "add-dir",
        aliases: &[],
        description: "Add a read-only extra directory (list/remove/clear)",
        usage: "/add-dir [<path>|remove <path>|clear|list]",
        hidden: false,
        needs_terminal: false,
        status: Status::Ready,
        handler: add_dir_cmd,
    },
    CommandSpec {
        name: "workspace-select",
        aliases: &[],
        description: "Quick workspace switch with directory suggestions",
        usage: "> [path-or-fragment]",
        hidden: true,
        needs_terminal: true,
        status: Status::Ready,
        handler: workspace_select_cmd,
    },
    CommandSpec {
        name: "permissions",
        aliases: &["perms"],
        description: "Show/change session permissions",
        usage: "/permissions [edit|create|delete|terminal|git] [always|ask]",
        hidden: false,
        needs_terminal: false,
        status: Status::Ready,
        handler: permissions_cmd,
    },
    CommandSpec {
        name: "tools",
        aliases: &[],
        description: "List available tools",
        usage: "/tools",
        hidden: false,
        needs_terminal: false,
        status: Status::Ready,
        handler: tools_cmd,
    },
    CommandSpec {
        name: "skills",
        aliases: &[],
        description: "List discovered skills (project + global)",
        usage: "/skills",
        hidden: false,
        needs_terminal: false,
        status: Status::Ready,
        handler: skills_cmd,
    },
    CommandSpec {
        name: "skill",
        aliases: &[],
        description: "Force-load a skill into the conversation now",
        usage: "/skill <name>",
        hidden: false,
        needs_terminal: false,
        status: Status::Ready,
        handler: skill_load_cmd,
    },
    CommandSpec {
        name: "commands",
        aliases: &[],
        description: "List your custom (user-defined) slash commands",
        usage: "/commands",
        hidden: false,
        needs_terminal: false,
        status: Status::Ready,
        handler: custom_commands_cmd,
    },
    CommandSpec {
        name: "config",
        aliases: &[],
        description: "Show current (non-secret) configuration",
        usage: "/config",
        hidden: false,
        needs_terminal: false,
        status: Status::Ready,
        handler: config_cmd,
    },
    CommandSpec {
        name: "context",
        aliases: &[],
        description: "Estimate how much of the conversation you're using",
        usage: "/context",
        hidden: false,
        needs_terminal: false,
        status: Status::Ready,
        handler: context_cmd,
    },
    CommandSpec {
        name: "compact",
        aliases: &[],
        description: "Shrink older tool output to free up conversation space",
        usage: "/compact",
        hidden: false,
        needs_terminal: false,
        status: Status::Ready,
        handler: compact_cmd,
    },
    CommandSpec {
        name: "autocompact",
        aliases: &[],
        description: "Run /compact automatically once tool output builds up",
        usage: "/autocompact <count>|off",
        hidden: false,
        needs_terminal: false,
        status: Status::Ready,
        handler: autocompact_cmd,
    },
    CommandSpec {
        name: "api",
        aliases: &[],
        description: "Show/set/clear the API key for the current provider",
        usage: "/api [set|clear]",
        hidden: false,
        needs_terminal: true,
        status: Status::Ready,
        handler: api_cmd,
    },
    CommandSpec {
        name: "connect",
        aliases: &[],
        description: "Guided setup: pick a provider and configure it",
        usage: "/connect",
        hidden: false,
        needs_terminal: true,
        status: Status::Ready,
        handler: connect_cmd,
    },
    CommandSpec {
        name: "doctor",
        aliases: &[],
        description: "Health-check this session (workspace, git, API key, provider)",
        usage: "/doctor",
        hidden: false,
        needs_terminal: false,
        status: Status::Ready,
        handler: doctor_cmd,
    },
    CommandSpec {
        name: "init",
        aliases: &[],
        description: "Create a REXO.md with project notes for the agent",
        usage: "/init",
        hidden: false,
        needs_terminal: false,
        status: Status::Ready,
        handler: init_cmd,
    },
    CommandSpec {
        name: "memory",
        aliases: &[],
        description: "View this workspace's REXO.md project instructions",
        usage: "/memory",
        hidden: false,
        needs_terminal: false,
        status: Status::Ready,
        handler: memory_cmd,
    },
    CommandSpec {
        name: "diff",
        aliases: &[],
        description: "Show uncommitted git changes in the workspace",
        usage: "/diff",
        hidden: false,
        needs_terminal: false,
        status: Status::Ready,
        handler: diff_cmd,
    },
    CommandSpec {
        name: "rename",
        aliases: &[],
        description: "Give this session a title (shown in the header)",
        usage: "/rename <title>",
        hidden: false,
        needs_terminal: false,
        status: Status::Ready,
        handler: rename_cmd,
    },
    CommandSpec {
        name: "export",
        aliases: &[],
        description: "Save the conversation to a Markdown file",
        usage: "/export [filename]",
        hidden: false,
        needs_terminal: false,
        status: Status::Ready,
        handler: export_cmd,
    },
    CommandSpec {
        name: "copy",
        aliases: &[],
        description: "Copy an answer (or the whole conversation) to the clipboard",
        usage: "/copy [all|<n>]",
        hidden: false,
        needs_terminal: false,
        status: Status::Ready,
        handler: copy_cmd,
    },
    CommandSpec {
        name: "goal",
        aliases: &[],
        description: "Set a goal REXO keeps in mind before stopping",
        usage: "/goal <text>",
        hidden: false,
        needs_terminal: false,
        status: Status::Ready,
        handler: goal_cmd,
    },
    CommandSpec {
        name: "plan",
        aliases: &[],
        description: "Toggle plan mode: propose a plan before making changes",
        usage: "/plan",
        hidden: false,
        needs_terminal: false,
        status: Status::Ready,
        handler: plan_cmd,
    },
    CommandSpec {
        name: "focus",
        aliases: &[],
        description: "Toggle focus view: just your prompts and REXO's answers",
        usage: "/focus",
        hidden: false,
        needs_terminal: false,
        status: Status::Ready,
        handler: focus_cmd,
    },
    CommandSpec {
        name: "effort",
        aliases: &[],
        description: "Set reasoning effort (low/medium/high/none)",
        usage: "/effort <low|medium|high|none>",
        hidden: false,
        needs_terminal: false,
        status: Status::Ready,
        handler: effort_cmd,
    },
    CommandSpec {
        name: "color",
        aliases: &[],
        description: "Set the UI accent color for this session",
        usage: "/color <cyan|green|magenta|yellow|blue>",
        hidden: false,
        needs_terminal: false,
        status: Status::Ready,
        handler: color_cmd,
    },
    CommandSpec {
        name: "scroll-speed",
        aliases: &[],
        description: "Adjust how many lines each scroll key press moves",
        usage: "/scroll-speed <1-20>",
        hidden: false,
        needs_terminal: false,
        status: Status::Ready,
        handler: scroll_speed_cmd,
    },
    CommandSpec {
        name: "keybindings",
        aliases: &["keys", "shortcuts"],
        description: "Show keyboard shortcuts for this session",
        usage: "/keybindings",
        hidden: false,
        needs_terminal: false,
        status: Status::Ready,
        handler: keybindings_cmd,
    },
    CommandSpec {
        name: "loop",
        aliases: &[],
        description: "Repeat a prompt on a timer (e.g. /loop 5m check CI)",
        usage: "/loop <Ns|Nm> <prompt> | /loop off",
        hidden: false,
        needs_terminal: false,
        status: Status::Ready,
        handler: loop_cmd,
    },
    CommandSpec {
        name: "release-notes",
        aliases: &[],
        description: "Show what's new in this version",
        usage: "/release-notes",
        hidden: false,
        needs_terminal: false,
        status: Status::Ready,
        handler: release_notes_cmd,
    },
    CommandSpec {
        name: "feedback",
        aliases: &["bug"],
        description: "Save feedback or a bug report to this workspace",
        usage: "/feedback <message>",
        hidden: false,
        needs_terminal: false,
        status: Status::Ready,
        handler: feedback_cmd,
    },
    CommandSpec {
        name: "branch",
        aliases: &[],
        description: "Save this point as a new named session — this one keeps going",
        usage: "/branch <name>",
        hidden: false,
        needs_terminal: false,
        status: Status::Ready,
        handler: branch_cmd,
    },
    CommandSpec {
        name: "fork",
        aliases: &[],
        description: "Save this point as a new session and continue as it (not background)",
        usage: "/fork <name>",
        hidden: false,
        needs_terminal: false,
        status: Status::Ready,
        handler: fork_cmd,
    },
    CommandSpec {
        name: "rewind",
        aliases: &[],
        description: "Roll the conversation back to an earlier point (not file edits)",
        usage: "/rewind [checkpoint#]",
        hidden: false,
        needs_terminal: false,
        status: Status::Ready,
        handler: rewind_cmd,
    },
    CommandSpec {
        name: "undo",
        aliases: &[],
        description: "Revert the single most recent file change (not the conversation)",
        usage: "/undo",
        hidden: false,
        needs_terminal: false,
        status: Status::Ready,
        handler: undo_cmd,
    },
    CommandSpec {
        name: "resume",
        aliases: &[],
        description: "Resume a saved session, or list/save/delete saved sessions",
        usage: "/resume [list|save <name>|delete <name>|<name>]",
        hidden: false,
        needs_terminal: false,
        status: Status::Ready,
        handler: resume_cmd,
    },
    CommandSpec {
        name: "background",
        aliases: &["bg"],
        description: "Send this session to the background",
        usage: "/background",
        hidden: false,
        needs_terminal: false,
        status: Status::Planned,
        handler: background_cmd,
    },
    CommandSpec {
        name: "mcp",
        aliases: &[],
        description: "Manage and connect to MCP servers (stdio transport)",
        usage: "/mcp [list|add|remove|enable|disable|connect <name>|disconnect <name>|status]",
        hidden: false,
        needs_terminal: false,
        status: Status::Ready,
        handler: mcp_cmd,
    },
    CommandSpec {
        name: "hooks",
        aliases: &[],
        description: "Run shell commands around tool calls and session start/end",
        usage: "/hooks [list|add <event> \"<cmd>\" [--matcher <glob>]|remove <i>|test <event>]",
        hidden: false,
        needs_terminal: false,
        status: Status::Ready,
        handler: hooks_cmd,
    },
    CommandSpec {
        name: "ide",
        aliases: &[],
        description: "Start a local server for editor integration (no extension ships yet — see /ide info)",
        usage: "/ide [start|stop|status|info]",
        hidden: false,
        needs_terminal: false,
        status: Status::Ready,
        handler: ide_cmd,
    },
    CommandSpec {
        name: "plugin",
        aliases: &[],
        description: "Install/enable local plugin bundles (skills + commands + hooks)",
        usage: "/plugin [list|enable <name>|disable <name>|info]",
        hidden: false,
        needs_terminal: false,
        status: Status::Ready,
        handler: plugin_cmd,
    },
    CommandSpec {
        name: "agents",
        aliases: &[],
        description: "Define personas and run them against a task (sequential, not concurrent)",
        usage: "/agents [list|create <name> \"<desc>\"|run <name> \"<task>\"]",
        hidden: false,
        needs_terminal: false,
        status: Status::Ready,
        handler: agents_cmd,
    },
    CommandSpec {
        name: "clear",
        aliases: &[],
        description: "Clear the conversation (keeps configuration)",
        usage: "/clear",
        hidden: false,
        needs_terminal: false,
        status: Status::Ready,
        handler: clear_cmd,
    },
    CommandSpec {
        name: "reset",
        aliases: &[],
        description: "Reset provider/model/permissions to config defaults",
        usage: "/reset",
        hidden: false,
        needs_terminal: false,
        status: Status::Ready,
        handler: reset_cmd,
    },
    CommandSpec {
        name: "exit",
        aliases: &["quit", "q"],
        description: "Exit REXO",
        usage: "/exit",
        hidden: false,
        needs_terminal: false,
        status: Status::Ready,
        handler: exit_cmd,
    },
];

pub fn find(name: &str) -> Option<&'static CommandSpec> {
    COMMANDS
        .iter()
        .find(|c| c.name == name || c.aliases.contains(&name))
}

/// Look up and run a command by name. Unknown commands print a hint and
/// return `Continue` rather than erroring the whole REPL.
pub fn dispatch(session: &mut Session, name: &str, args: &[String]) -> Result<CommandOutcome> {
    match find(name) {
        Some(spec) => (spec.handler)(session, args),
        None => {
            println!(
                "{} Unknown command: /{name}. Type {} for the command list.",
                "?".yellow(),
                "/help".bold()
            );
            Ok(CommandOutcome::Continue)
        }
    }
}

// ---------------------------------------------------------------------
// /help
// ---------------------------------------------------------------------

fn help_cmd(_session: &mut Session, _args: &[String]) -> Result<CommandOutcome> {
    println!("{}", "Rexo Code Commands".bold());
    println!("{}", "─".repeat(60).dimmed());
    for spec in COMMANDS.iter().filter(|c| !c.hidden) {
        let planned = if spec.status == Status::Planned {
            " (planned)".dimmed().to_string()
        } else {
            String::new()
        };
        println!("  {:<28} {}{planned}", spec.usage, spec.description.dimmed());
    }
    println!("  {:<28} {}", ">", "Quick workspace switch (suggests real directories)".dimmed());
    println!("  {:<28} {}", "@path", "Reference a file — content is included for the model".dimmed());
    println!("  {:<28} {}", "{?}", "Show the keyboard shortcut list".dimmed());
    println!("{}", "─".repeat(60).dimmed());
    println!("Aliases: /? /h → /help    /q → /exit");
    println!();
    println!("{}", "Type a plain sentence to give REXO a coding task.".dimmed());
    Ok(CommandOutcome::Continue)
}

// ---------------------------------------------------------------------
// /status, /config  (secret-safe rendering)
// ---------------------------------------------------------------------

fn status_cmd(session: &mut Session, _args: &[String]) -> Result<CommandOutcome> {
    println!("{}", render_status(session));
    Ok(CommandOutcome::Continue)
}

/// `/undo` — the typed, terminal-independent equivalent of
/// Ctrl+Shift+_. See [`crate::agent::Agent::undo_last_file_change`] for
/// exactly what it reverts (one level, file-only).
fn undo_cmd(session: &mut Session, _args: &[String]) -> Result<CommandOutcome> {
    match session.agent.undo_last_file_change() {
        Ok(msg) => println!("{} {msg}", "✓".green()),
        Err(msg) => println!("{} {msg}", "?".yellow()),
    }
    Ok(CommandOutcome::Continue)
}

fn config_cmd(session: &mut Session, _args: &[String]) -> Result<CommandOutcome> {
    println!("{}", render_config(session));
    Ok(CommandOutcome::Continue)
}

/// Builds the `/status` text. Structurally can't leak the API key: it only
/// ever reads `session.api_key_configured() -> bool`, never the key value.
fn render_status(session: &Session) -> String {
    let cfg = &session.config;
    let key_line = if cfg.model.provider == "local" {
        "not required".dimmed().to_string()
    } else if session.api_key_configured() {
        "configured".green().to_string()
    } else {
        "not configured".yellow().to_string()
    };
    let perms = session.agent.permissions().current_config();
    let started_elsewhere = if session.startup_workspace.as_path() != session.agent.workspace() {
        format!("\nStarted in: {}", session.startup_workspace.display())
    } else {
        String::new()
    };
    let saved_default = match cfg.global.default_profile() {
        Some(profile) if Some(profile.display_name.as_str()) != Some(cfg.display_name().as_str()) => {
            format!("\nSaved default: {} (this session is using something else)", profile.display_name)
        }
        _ => String::new(),
    };
    let global_dir = crate::config::global::global_dir().map(|p| p.display().to_string()).unwrap_or_else(|_| "(unavailable)".to_string());
    let capabilities = render_capabilities(&session.agent.provider_capabilities());

    format!(
        "{title}\n{rule}\nProvider:   {provider}\nModel:      {model}\nBase URL:   {base_url}\nAPI Key:    {key}\nWorkspace:  {workspace}{started_elsewhere}{saved_default}\n\nCapabilities: {capabilities}\n\nPermissions:\n  File edits:   {edit}\n  Terminal:     {terminal}\n  Git writes:   {git}\n\nAgent:\n  Max iterations: {max_iter}\n  Max tool calls: {max_calls}\n  Undo pending:   {undo}\n\nGlobal config: {global_dir}\n{rule}",
        title = "Rexo Code Status".bold(),
        rule = "─".repeat(48).dimmed(),
        provider = cfg.display_name(),
        model = cfg.model.model.as_deref().unwrap_or("(none set)"),
        base_url = effective_base_url(cfg),
        key = key_line,
        workspace = session.agent.workspace().display(),
        edit = ask_or_auto(perms.allow_edit),
        terminal = ask_or_auto(perms.allow_terminal),
        git = ask_or_auto(perms.allow_git_write),
        max_iter = session.agent.max_iterations(),
        max_calls = session.agent.max_tool_calls(),
        undo = if session.agent.has_pending_undo() { "yes (/undo to revert)".to_string() } else { "no".dimmed().to_string() },
    )
}

/// One line per known-true/known-false capability flag; unknowns are
/// omitted entirely rather than shown as a confident-looking blank —
/// see [`crate::providers::ModelCapabilities`]'s own doc comment on why
/// `None` means "REXO doesn't know," not "no."
fn render_capabilities(caps: &crate::providers::ModelCapabilities) -> String {
    let mut flags: Vec<String> = Vec::new();
    let mut push = |label: &str, value: Option<bool>| {
        if let Some(v) = value {
            flags.push(format!("{label}={}", if v { "yes".green() } else { "no".dimmed() }));
        }
    };
    push("tools", caps.tool_calling);
    push("streaming", caps.streaming);
    push("vision", caps.vision);
    push("reasoning", caps.reasoning);
    push("parallel_tools", caps.parallel_tools);
    push("structured_output", caps.structured_output);
    push("model_discovery", caps.model_discovery);
    push("usage_reporting", caps.usage_reporting);

    if let Some(ctx) = caps.context_window {
        flags.push(format!("context_window={ctx}"));
    }
    if let Some(max_out) = caps.max_output {
        flags.push(format!("max_output={max_out}"));
    }

    if flags.is_empty() {
        "(unknown for this provider/model)".dimmed().to_string()
    } else {
        flags.join("  ")
    }
}

/// Builds the `/config` text — same non-secret data as `/status`, shaped
/// like `rexo.toml` since that's where these values ultimately come from.
/// Never includes an API key line by design; config files never hold one.
fn render_config(session: &Session) -> String {
    let cfg = &session.config;
    let perms = session.agent.permissions().current_config();

    format!(
        "{title}\n{rule}\nProvider:\n  {provider}\n\nModel:\n  {model}\n\nBase URL:\n  {base_url}\n\nModel settings:\n  temperature = {temperature}\n  max_tokens = {max_tokens}\n  reasoning_effort = {reasoning}\n\nPermissions:\n  edits = {edit}\n  terminal = {terminal}\n  git writes = {git}\n\nAgent:\n  max_iterations = {max_iter}\n  max_tool_calls = {max_calls}\n{rule}",
        title = "REXO Configuration".bold(),
        rule = "─".repeat(48).dimmed(),
        provider = cfg.model.provider,
        model = cfg.model.model.as_deref().unwrap_or("(none set)"),
        base_url = effective_base_url(cfg),
        temperature = cfg.model.temperature,
        max_tokens = cfg.model.max_tokens,
        reasoning = cfg.model.reasoning_effort.as_deref().unwrap_or("(none)"),
        edit = ask_or_auto(perms.allow_edit),
        terminal = ask_or_auto(perms.allow_terminal),
        git = ask_or_auto(perms.allow_git_write),
        max_iter = session.agent.max_iterations(),
        max_calls = session.agent.max_tool_calls(),
    )
}

fn ask_or_auto(allowed: bool) -> &'static str {
    if allowed {
        "auto"
    } else {
        "ask"
    }
}

fn effective_base_url(cfg: &Config) -> String {
    match cfg.model.provider.as_str() {
        "nvidia" => "https://integrate.api.nvidia.com/v1/chat/completions (built-in)".to_string(),
        "local" => cfg
            .model
            .base_url
            .clone()
            .unwrap_or_else(|| "http://localhost:11434 (default)".to_string()),
        _ => cfg.model.base_url.clone().unwrap_or_else(|| "(not set)".to_string()),
    }
}

// ---------------------------------------------------------------------
// /model, /models
// ---------------------------------------------------------------------

fn model_cmd(session: &mut Session, args: &[String]) -> Result<CommandOutcome> {
    if !args.is_empty() {
        let new_model = args.join(" ");
        session.config.model.model = Some(new_model.clone());
        match session.rebuild_provider() {
            Ok(()) => println!("{} Model changed to {}", "✓".green(), new_model.bold()),
            Err(e) => println!("{} Model set to {} but couldn't activate it yet: {e}", "!".yellow(), new_model.bold()),
        }
        offer_to_persist_model(session, &new_model)?;
        return Ok(CommandOutcome::Continue);
    }

    println!("Current provider: {}", session.config.display_name().bold());
    println!("Current model:    {}", session.config.model.model.as_deref().unwrap_or("(none set)"));
    println!();
    println!("  [1] Search available models (live discovery, if this endpoint supports it)");
    println!("  [2] Enter a custom model ID");
    println!("  [3] Cancel");
    let choice = prompt_line("Choice: ")?;

    let chosen = match choice.trim() {
        "1" => pick_model_via_discovery(session)?,
        "2" => manual_model_id()?,
        _ => {
            println!("Cancelled.");
            None
        }
    };

    let Some(new_model) = chosen else {
        return Ok(CommandOutcome::Continue);
    };
    session.config.model.model = Some(new_model.clone());
    match session.rebuild_provider() {
        Ok(()) => println!("{} Model changed to {}", "✓".green(), new_model.bold()),
        Err(e) => println!("{} Model set to {} but couldn't activate it yet: {e}", "!".yellow(), new_model.bold()),
    }
    offer_to_persist_model(session, &new_model)?;
    Ok(CommandOutcome::Continue)
}

/// If the active provider came from a saved global profile, offer to
/// update that profile's default model too — otherwise the model change
/// only lasts this session, which is surprising after just picking one
/// from a list. Silent no-op for legacy `rexo.toml`-only setups (nothing
/// to update).
pub(crate) fn offer_to_persist_model(session: &mut Session, model: &str) -> Result<()> {
    let Some(profile_name) = persistable_model_profile(session) else {
        return Ok(());
    };
    let answer = prompt_line("Save as the default model for this provider too? [y/N] ")?;
    if answer.trim().eq_ignore_ascii_case("y") {
        match persist_model_choice(session, &profile_name, model) {
            Ok(()) => println!("{}", "  Saved.".dimmed()),
            Err(e) => println!("{} Couldn't save: {e}", "✗".red()),
        }
    }
    Ok(())
}

/// The non-prompting half of [`offer_to_persist_model`]: is there even a
/// saved profile for the current provider worth asking about? Split out
/// so the in-TUI picker flow (`cli::tui::model_picker`) can ask the same
/// question through `cli::picker::run_confirm` — a real raw-mode-native
/// prompt — instead of `offer_to_persist_model`'s own blocking
/// `prompt_line`, which assumes cooked/canonical terminal mode. Calling
/// that while the screen is still in raw mode (as every in-TUI picker
/// flow is) doesn't hang outright — `prompt_line` has its own guard for
/// the "TUI never handed off" case — but it also doesn't reliably see
/// keystrokes the way canonical mode does, which is exactly the "I press
/// y or n and nothing happens" bug.
pub(crate) fn persistable_model_profile(session: &Session) -> Option<String> {
    let profile_name = session.config.credential_key.clone()?;
    if session.config.global.providers.contains_key(&profile_name) {
        Some(profile_name)
    } else {
        None
    }
}

pub(crate) fn persist_model_choice(session: &mut Session, profile_name: &str, model: &str) -> Result<()> {
    if let Some(profile) = session.config.global.providers.get_mut(profile_name) {
        profile.model = Some(model.to_string());
    }
    session.config.global.save()
}

fn manual_model_id() -> Result<Option<String>> {
    let id = prompt_line("Model ID: ")?;
    if id.trim().is_empty() {
        Ok(None)
    } else {
        Ok(Some(id.trim().to_string()))
    }
}

/// Live model discovery via the current provider's `GET /models` endpoint
/// (see `providers::protocol::OpenAiProtocolClient::list_models`), with a
/// search box over the results. Falls back to manual entry on any
/// failure — discovery not working is not a reason to block picking a model.
fn pick_model_via_discovery(session: &Session) -> Result<Option<String>> {
    let Some(url) = effective_endpoint(session) else {
        println!("{} No endpoint to discover models from for this provider.", "✗".red());
        return manual_model_id();
    };
    let api_key = session.config.api_key().unwrap_or_default();
    println!("{}", "Fetching model list…".dimmed());
    let models = match fetch_models_blocking(&url, &api_key) {
        Ok(m) => m,
        Err(e) => {
            println!("{} Model discovery isn't available here ({e}).", "✗".red());
            return manual_model_id();
        }
    };
    if models.is_empty() {
        println!("{} This endpoint returned no models.", "✗".red());
        return manual_model_id();
    }
    pick_from_model_list(&models)
}

fn pick_from_model_list(models: &[crate::providers::protocol::ModelListing]) -> Result<Option<String>> {
    let query = prompt_line(&format!("Search {} models (blank = show first 20): ", models.len()))?;
    let q = query.trim().to_lowercase();
    let filtered: Vec<&crate::providers::protocol::ModelListing> =
        models.iter().filter(|m| q.is_empty() || m.id.to_lowercase().contains(&q)).take(20).collect();
    if filtered.is_empty() {
        println!("{} No models matched '{query}'.", "✗".red());
        return Ok(None);
    }
    for (i, m) in filtered.iter().enumerate() {
        match &m.owned_by {
            Some(owner) if !owner.is_empty() => println!("  {}. {}  {}", i + 1, m.id, format!("({owner})").dimmed()),
            _ => println!("  {}. {}", i + 1, m.id),
        }
    }
    let idx = prompt_line("Pick a number (blank to cancel): ")?;
    if idx.trim().is_empty() {
        return Ok(None);
    }
    match idx.trim().parse::<usize>().ok().and_then(|i| i.checked_sub(1)).and_then(|i| filtered.get(i)) {
        Some(m) => Ok(Some(m.id.clone())),
        None => {
            println!("{} Out of range.", "✗".red());
            Ok(None)
        }
    }
}

/// Run an async future to completion from inside a synchronous command
/// handler. Deliberately *not* `Runtime::new().block_on(..)` — this runs
/// on a tokio worker thread already (we're inside `#[tokio::main]`, just
/// mid-way through a blocking, non-TUI command like `/connect`), and
/// starting a second nested runtime there panics ("Cannot start a runtime
/// from within a runtime"). `block_in_place` is tokio's sanctioned escape
/// hatch for exactly this: it hands the current worker thread's other
/// work off to another thread for the duration, so blocking here is safe
/// on the multi-threaded runtime `#[tokio::main]` uses by default.
pub(crate) fn fetch_models_blocking(base_url: &str, api_key: &str) -> Result<Vec<crate::providers::protocol::ModelListing>> {
    let client = crate::providers::protocol::OpenAiProtocolClient::new(
        base_url.to_string(),
        api_key.to_string(),
        String::new(),
        0.0,
        1,
        None,
    );
    tokio::task::block_in_place(|| tokio::runtime::Handle::current().block_on(client.list_models()))
}

/// The current provider's full chat-completions endpoint, regardless of
/// kind — `nvidia`'s is fixed and not stored in `config.model.base_url`,
/// `local` needs the same suffix-normalization `LocalProvider` applies,
/// and `openai_compatible` just is whatever's configured.
pub(crate) fn effective_endpoint(session: &Session) -> Option<String> {
    match session.config.model.provider.as_str() {
        "nvidia" => Some(crate::providers::nvidia::NVIDIA_ENDPOINT.to_string()),
        "local" => {
            let mut url = session.config.model.base_url.clone().unwrap_or_else(|| "http://localhost:11434".to_string());
            if !url.ends_with("/chat/completions") {
                if !url.ends_with('/') {
                    url.push('/');
                }
                url.push_str("v1/chat/completions");
            }
            Some(url)
        }
        _ => session.config.model.base_url.clone(),
    }
}

fn models_cmd(session: &mut Session, _args: &[String]) -> Result<CommandOutcome> {
    let Some(url) = effective_endpoint(session) else {
        println!("{} No endpoint configured to discover models from. Run /connect first.", "✗".red());
        return Ok(CommandOutcome::Continue);
    };
    let api_key = session.config.api_key().unwrap_or_default();
    println!("{}", "Fetching model list…".dimmed());
    match fetch_models_blocking(&url, &api_key) {
        Ok(models) if !models.is_empty() => {
            println!("{} models available from {}:", models.len(), session.config.display_name().bold());
            for m in models.iter().take(60) {
                println!("  {}", m.id);
            }
            if models.len() > 60 {
                println!("  {}", format!("… and {} more — use /model to search.", models.len() - 60).dimmed());
            }
        }
        Ok(_) => println!("{}", "This endpoint returned no models.".dimmed()),
        Err(e) => {
            println!("{} Live model discovery isn't available here: {e}", "○".dimmed());
            println!("{}", "Use /model <id> to set any model id this provider supports directly.".dimmed());
        }
    }
    Ok(CommandOutcome::Continue)
}

// ---------------------------------------------------------------------
// /provider, /providers, /connect  (search-driven, backed by the shared
// catalog in providers::catalog + saved global profiles)
// ---------------------------------------------------------------------

fn normalize_provider_arg(raw: &str) -> String {
    raw.to_lowercase().replace('-', "_")
}

fn provider_cmd(session: &mut Session, args: &[String]) -> Result<CommandOutcome> {
    let Some(arg) = args.first() else {
        println!("Current provider: {}", session.config.display_name().bold());
        println!();
        if session.config.global.providers.is_empty() {
            println!("No saved provider profiles yet. Run {} to add one.", "/connect".bold());
        } else {
            println!("Saved profiles: {}", session.config.global.providers.keys().cloned().collect::<Vec<_>>().join(", "));
            println!("Switch with {}, or add a new one with {}.", "/provider <name>".bold(), "/connect".bold());
        }
        return Ok(CommandOutcome::Continue);
    };

    if arg == "add" {
        return connect_custom(session);
    }

    // Named saved profile takes priority over the raw provider-kind form,
    // since that's what most users will actually type after /connect.
    if let Some(profile) = session.config.global.providers.get(arg).cloned() {
        apply_profile_session_only(session, arg, &profile, None);
        return activate_and_report(session, &format!("Switched to {}", profile.display_name));
    }

    let normalized = normalize_provider_arg(arg);
    if !["nvidia", "openai_compatible", "local"].contains(&normalized.as_str()) {
        println!(
            "{} Unknown provider '{arg}'. Expected a saved profile name, one of: nvidia, \
             openai_compatible, local — or /provider add for a custom endpoint.",
            "✗".red()
        );
        if !session.config.global.providers.is_empty() {
            println!("Saved profiles: {}", session.config.global.providers.keys().cloned().collect::<Vec<_>>().join(", "));
        }
        return Ok(CommandOutcome::Continue);
    }

    session.config.model.provider = normalized.clone();
    session.config.display_name = None;
    session.config.credential_key = None;
    session.api_key_override = None;

    if normalized == "openai_compatible" && session.config.model.base_url.is_none() {
        session.config.model.base_url = Some(prompt_line("Base URL (full chat-completions endpoint): ")?);
    }
    activate_and_report(session, &format!("Provider changed to {normalized}"))
}

fn activate_and_report(session: &mut Session, success_message: &str) -> Result<CommandOutcome> {
    match session.rebuild_provider() {
        Ok(()) => println!("{} {success_message}", "✓".green()),
        Err(e) => println!("{} {success_message}, but couldn't activate it: {e}", "!".yellow()),
    }
    Ok(CommandOutcome::Continue)
}

fn providers_cmd(session: &mut Session, _args: &[String]) -> Result<CommandOutcome> {
    if !session.config.global.providers.is_empty() {
        println!("{}", "Saved profiles".bold());
        println!("{}", "─".repeat(56).dimmed());
        for (name, profile) in &session.config.global.providers {
            let is_default = session.config.global.default_provider.as_deref() == Some(name.as_str());
            let marker = if is_default { "✓".green() } else { "○".dimmed() };
            println!("{marker} {} {}", name.bold(), if is_default { "(default)".dimmed().to_string() } else { String::new() });
            println!("  {} — {}", profile.display_name, profile.model.as_deref().unwrap_or("(no default model)"));
        }
        println!();
    }

    println!("{}", "Provider catalog".bold());
    println!("{}", "─".repeat(56).dimmed());
    println!("{}", "(search with /connect; base URLs shown here are best-effort — verify against the provider's own docs)".dimmed());
    println!();
    for preset in crate::providers::catalog::CATALOG {
        let credential_key = preset.key;
        let has_key = crate::config::credentials::CredentialStore::open()
            .and_then(|s| s.get(credential_key))
            .map(|k| k.is_some())
            .unwrap_or(false)
            || std::env::var(format!("REXO_{}_API_KEY", credential_key.to_uppercase())).is_ok();
        let marker = if !preset.requires_key || has_key { "✓".green() } else { "○".dimmed() };
        println!("{marker} {}", preset.display_name.bold());
        if let Some(url) = preset.base_url {
            println!("  {url}");
        } else {
            println!("  {}", "(base URL entered during /connect)".dimmed());
        }
        if !preset.notes.is_empty() {
            println!("  {}", preset.notes.dimmed());
        }
    }
    println!();
    println!("Connect one interactively with {}", "/connect".bold());
    Ok(CommandOutcome::Continue)
}

fn connect_cmd(session: &mut Session, _args: &[String]) -> Result<CommandOutcome> {
    println!("{}", "Connect a provider".bold());
    println!("{}", "Search by name — blank shows everything, 'custom' for a fully custom endpoint.".dimmed());
    let query = prompt_line("Search provider: ")?;
    if query.trim().eq_ignore_ascii_case("custom") {
        return connect_custom(session);
    }

    let matches = crate::providers::catalog::search(&query);
    if matches.is_empty() {
        println!("{} Nothing matched '{}'. Try again, or 'custom' for a fully custom endpoint.", "✗".red(), query.trim());
        return Ok(CommandOutcome::Continue);
    }
    println!();
    for (i, preset) in matches.iter().enumerate().take(12) {
        let note = if preset.notes.is_empty() { String::new() } else { format!(" — {}", preset.notes) };
        println!("  {}. {}{}", i + 1, preset.display_name.bold(), note.dimmed());
    }
    println!();
    let choice = prompt_line("Pick a number (or blank to cancel): ")?;
    if choice.trim().is_empty() {
        println!("Cancelled.");
        return Ok(CommandOutcome::Continue);
    }
    let Ok(idx) = choice.trim().parse::<usize>() else {
        println!("{} Not a number.", "✗".red());
        return Ok(CommandOutcome::Continue);
    };
    let Some(preset) = idx.checked_sub(1).and_then(|i| matches.get(i)).copied() else {
        println!("{} Out of range.", "✗".red());
        return Ok(CommandOutcome::Continue);
    };

    connect_preset(session, preset)
}

fn connect_preset(session: &mut Session, preset: &crate::providers::catalog::ProviderPreset) -> Result<CommandOutcome> {
    let kind = preset.kind;

    let base_url = match preset.base_url {
        Some(url) if kind == "nvidia" => Some(url.to_string()), // fixed, not user-editable
        Some(url) => {
            let entered = prompt_line(&format!("Base URL [{url}]: "))?;
            Some(if entered.trim().is_empty() { url.to_string() } else { entered.trim().to_string() })
        }
        None => {
            let entered = prompt_line("Base URL (full chat-completions endpoint): ")?;
            if entered.trim().is_empty() {
                println!("{} A base URL is required for this provider.", "✗".red());
                return Ok(CommandOutcome::Continue);
            }
            Some(entered.trim().to_string())
        }
    };

    let mut api_key = None;
    if preset.requires_key {
        let entered =
            read_hidden_line(&format!("API key for {} (input hidden): ", preset.display_name))?;
        if !entered.is_empty() {
            api_key = Some(entered);
        }
    }

    println!();
    println!("Model:");
    println!("  [1] Search available models (live discovery)");
    println!("  [2] Enter a custom model ID");
    println!("  [3] Skip for now");
    let model_choice = prompt_line("Choice [2]: ")?;
    let model = match model_choice.trim() {
        "1" => match &base_url {
            Some(url) => {
                println!("{}", "Fetching model list…".dimmed());
                match fetch_models_blocking(url, api_key.as_deref().unwrap_or("")) {
                    Ok(models) if !models.is_empty() => pick_from_model_list(&models)?,
                    Ok(_) => {
                        println!("{} This endpoint returned no models.", "✗".red());
                        manual_model_id()?
                    }
                    Err(e) => {
                        println!("{} Model discovery isn't available here: {e}", "○".dimmed());
                        manual_model_id()?
                    }
                }
            }
            None => manual_model_id()?,
        },
        "3" => None,
        _ => manual_model_id()?,
    };

    let profile = crate::config::global::ProviderProfile {
        display_name: preset.display_name.to_string(),
        kind: kind.to_string(),
        base_url,
        model,
        temperature: None,
        max_tokens: None,
        reasoning_effort: None,
        credential_key: None,
        requires_key: preset.requires_key,
    };
    finish_connect(session, preset.key, profile, api_key)
}

fn connect_custom(session: &mut Session) -> Result<CommandOutcome> {
    let name = prompt_line("Name for this provider: ")?;
    let name = name.trim().to_string();
    if name.is_empty() {
        println!("Cancelled.");
        return Ok(CommandOutcome::Continue);
    }
    let key_slug = slugify(&name);

    let base_url = prompt_line("Base URL (full chat-completions endpoint): ")?;
    let base_url = base_url.trim().to_string();
    if base_url.is_empty() {
        println!("{} A base URL is required.", "✗".red());
        return Ok(CommandOutcome::Continue);
    }

    let wants_key = prompt_line("Requires an API key? [Y/n] ")?;
    let requires_key = !wants_key.trim().eq_ignore_ascii_case("n");
    let mut api_key = None;
    if requires_key {
        let entered = read_hidden_line("API key (input hidden): ")?;
        if !entered.is_empty() {
            api_key = Some(entered);
        }
    }

    println!();
    println!("Model:");
    println!("  [1] Search available models (live discovery)");
    println!("  [2] Enter a custom model ID");
    println!("  [3] Skip for now");
    let model_choice = prompt_line("Choice [2]: ")?;
    let model = match model_choice.trim() {
        "1" => {
            println!("{}", "Fetching model list…".dimmed());
            match fetch_models_blocking(&base_url, api_key.as_deref().unwrap_or("")) {
                Ok(models) if !models.is_empty() => pick_from_model_list(&models)?,
                Ok(_) => {
                    println!("{} This endpoint returned no models.", "✗".red());
                    manual_model_id()?
                }
                Err(e) => {
                    println!("{} Model discovery isn't available here: {e}", "○".dimmed());
                    manual_model_id()?
                }
            }
        }
        "3" => None,
        _ => manual_model_id()?,
    };

    let profile = crate::config::global::ProviderProfile {
        display_name: name,
        kind: "openai_compatible".to_string(),
        base_url: Some(base_url),
        model,
        temperature: None,
        max_tokens: None,
        reasoning_effort: None,
        credential_key: None,
        requires_key,
    };
    finish_connect(session, &key_slug, profile, api_key)
}

pub(crate) fn slugify(name: &str) -> String {
    let slug: String = name
        .trim()
        .to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    let slug = slug.trim_matches('-').to_string();
    if slug.is_empty() {
        "custom".to_string()
    } else {
        slug
    }
}

fn finish_connect(
    session: &mut Session,
    key_slug: &str,
    profile: crate::config::global::ProviderProfile,
    api_key: Option<String>,
) -> Result<CommandOutcome> {
    println!();
    println!("Save this connection:");
    println!("  [1] Permanently — available every time you run rexo, from any workspace");
    println!("  [2] This session only");
    let choice = prompt_line("Choice [1]: ")?;
    let permanent = choice.trim() != "2";
    let result = apply_connection(session, key_slug, profile, api_key, permanent);
    match &result.activated {
        Ok(()) => println!("{} {}", "✓".green(), result.message()),
        Err(_) => println!("{} {}", "!".yellow(), result.message()),
    }
    Ok(CommandOutcome::Continue)
}

/// The actual "make this connection real" logic — saving to global
/// config/the credential store when `permanent`, or just applying it to
/// the live session otherwise, then rebuilding the active provider. Shared
/// by the plain-terminal `/connect` flow above (which gets `permanent`
/// from a `prompt_line` choice) and the in-TUI picker-driven flow in
/// `cli::tui::connect_wizard` (which gets it from an arrow-key confirm) —
/// one place that actually writes state, two different ways of asking the
/// question.
/// Wires up the connection (credential store / global config / live
/// provider) but deliberately reports nothing itself — the two callers
/// need to surface the result completely differently: the plain-terminal
/// `/connect` flow via a normal `println!` (fine there; `run_suspended`
/// has already left raw mode/the alternate screen by the time this
/// runs), the in-TUI picker flow (`cli::tui::connect_wizard`) via
/// `ui.note()`. A `println!` from inside here reached straight through
/// to real stdout regardless of which caller invoked it, corrupting the
/// screen when the caller was still in raw mode — exactly the kind of
/// bug this split exists to rule out structurally rather than rely on
/// every future caller remembering not to print.
pub(crate) fn apply_connection(
    session: &mut Session,
    key_slug: &str,
    mut profile: crate::config::global::ProviderProfile,
    api_key: Option<String>,
    permanent: bool,
) -> ConnectResult {
    if permanent {
        let credential_key = profile.resolved_credential_key(key_slug).to_string();
        if let Some(key) = &api_key {
            if !key.trim().is_empty() {
                if let Err(e) = crate::config::credentials::CredentialStore::open().and_then(|store| store.set(&credential_key, key)) {
                    return ConnectResult { activated: Err(e), display_name: profile.display_name, permanent };
                }
            }
        }
        profile.credential_key = Some(credential_key.clone());
        session.config.global.providers.insert(key_slug.to_string(), profile.clone());
        session.config.global.default_provider = Some(key_slug.to_string());
        if let Err(e) = session.config.global.save() {
            return ConnectResult { activated: Err(e), display_name: profile.display_name, permanent };
        }

        session.config.model.provider = profile.kind.clone();
        session.config.model.model = profile.model.clone();
        session.config.model.base_url = profile.base_url.clone();
        session.config.display_name = Some(profile.display_name.clone());
        session.config.credential_key = Some(credential_key);
        session.api_key_override = None;
    } else {
        profile.credential_key = Some(key_slug.to_string());
        apply_profile_session_only(session, key_slug, &profile, api_key);
    }

    ConnectResult { activated: session.rebuild_provider(), display_name: profile.display_name, permanent }
}

pub(crate) struct ConnectResult {
    pub activated: Result<()>,
    pub display_name: String,
    pub permanent: bool,
}

impl ConnectResult {
    /// The message text either caller should show — same wording either
    /// way, just delivered through a different channel.
    pub(crate) fn message(&self) -> String {
        let headline = match &self.activated {
            Ok(()) => format!("Connected: {}", self.display_name),
            Err(e) => format!("Saved {}, but couldn't activate it yet: {e}", self.display_name),
        };
        let detail = if self.permanent {
            "this is now your default provider, from any workspace"
        } else {
            "session only — not saved to disk"
        };
        format!("{headline} ({detail})")
    }
}

/// Apply a profile to the live session without touching disk — used for
/// "session only" saves and for `/provider <saved-name>` switches.
pub(crate) fn apply_profile_session_only(
    session: &mut Session,
    key_slug: &str,
    profile: &crate::config::global::ProviderProfile,
    api_key: Option<String>,
) {
    session.config.model.provider = profile.kind.clone();
    session.config.model.model = profile.model.clone();
    session.config.model.base_url = profile.base_url.clone();
    if let Some(t) = profile.temperature {
        session.config.model.temperature = t;
    }
    if let Some(mt) = profile.max_tokens {
        session.config.model.max_tokens = mt;
    }
    if profile.reasoning_effort.is_some() {
        session.config.model.reasoning_effort = profile.reasoning_effort.clone();
    }
    session.config.display_name = Some(profile.display_name.clone());
    session.config.credential_key = Some(profile.resolved_credential_key(key_slug).to_string());
    if api_key.is_some() {
        session.api_key_override = api_key;
    }
}

// ---------------------------------------------------------------------
// /base-url
// ---------------------------------------------------------------------

fn base_url_cmd(session: &mut Session, args: &[String]) -> Result<CommandOutcome> {
    if args.is_empty() {
        println!("Current base URL:");
        println!("{}", effective_base_url(&session.config));
        return Ok(CommandOutcome::Continue);
    }

    let url = args.join(" ");
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        println!("{} Base URL must start with http:// or https://", "✗".red());
        return Ok(CommandOutcome::Continue);
    }

    session.config.model.base_url = Some(url.clone());
    if session.config.model.provider == "nvidia" {
        println!(
            "{}",
            "Note: the nvidia provider uses a fixed built-in endpoint; this only takes \
             effect if you switch to openai_compatible or local."
                .dimmed()
        );
    }

    match session.rebuild_provider() {
        Ok(()) => println!("{} Base URL changed to {url}", "✓".green()),
        Err(e) => println!("{} Base URL set but couldn't activate it: {e}", "!".yellow()),
    }
    Ok(CommandOutcome::Continue)
}

// ---------------------------------------------------------------------
// /workspace, `>` workspace selector
// ---------------------------------------------------------------------

fn workspace_cmd(session: &mut Session, args: &[String]) -> Result<CommandOutcome> {
    if args.is_empty() {
        println!("Current workspace:");
        println!("{}", session.agent.workspace().display());
        return Ok(CommandOutcome::Continue);
    }
    let requested = resolve_relative(session.agent.workspace(), &args.join(" "));
    apply_workspace_change(session, requested)?;
    Ok(CommandOutcome::Continue)
}

/// Join a possibly-relative user-typed path against `base` (the agent's
/// *current* workspace) rather than the OS process's cwd, which can
/// legitimately differ (e.g. after `-C`, or a prior `/workspace` change).
fn resolve_relative(base: &Path, input: &str) -> PathBuf {
    let candidate = PathBuf::from(input);
    if candidate.is_absolute() {
        candidate
    } else {
        base.join(candidate)
    }
}

fn workspace_select_cmd(session: &mut Session, args: &[String]) -> Result<CommandOutcome> {
    let current = session.agent.workspace().to_path_buf();

    if let Some(direct) = args.first() {
        let candidate = resolve_relative(&current, direct);
        if candidate.is_dir() {
            apply_workspace_change(session, candidate)?;
            return Ok(CommandOutcome::Continue);
        }
    }

    let filter = args.first().map(|s| s.as_str());
    let suggestions = suggest_workspaces(&current, filter);

    println!("{}", "Select workspace".bold());
    println!();
    println!("Current:\n  {}", current.display());
    println!();
    if suggestions.is_empty() {
        println!("{}", "(no matching directories found nearby)".dimmed());
    } else {
        println!("Suggested:");
        for (i, path) in suggestions.iter().enumerate() {
            println!("  {}. {}", i + 1, path.display());
        }
    }
    println!();
    let answer = prompt_line("Select a number, or type a path (blank to cancel): ")?;
    let answer = answer.trim();
    if answer.is_empty() {
        println!("Cancelled.");
        return Ok(CommandOutcome::Continue);
    }

    let target = match answer.parse::<usize>() {
        Ok(n) => match n.checked_sub(1).and_then(|i| suggestions.get(i)) {
            Some(p) => p.clone(),
            None => {
                println!("{} Out of range.", "✗".red());
                return Ok(CommandOutcome::Continue);
            }
        },
        Err(_) => resolve_relative(&current, answer),
    };

    apply_workspace_change(session, target)?;
    Ok(CommandOutcome::Continue)
}

/// Real, on-disk directories only — never invented. Combines the current
/// workspace's siblings (other projects next to it) and its own immediate
/// subdirectories, optionally filtered by a name prefix, capped to a
/// reasonable number of suggestions.
fn suggest_workspaces(current: &Path, filter: Option<&str>) -> Vec<PathBuf> {
    const MAX_SUGGESTIONS: usize = 9;
    let filter_lower = filter.map(|f| f.to_lowercase());

    let mut candidates = Vec::new();
    if let Some(parent) = current.parent() {
        if let Ok(entries) = std::fs::read_dir(parent) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() && path != current {
                    candidates.push(path);
                }
            }
        }
    }
    if let Ok(entries) = std::fs::read_dir(current) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                candidates.push(path);
            }
        }
    }

    if let Some(f) = &filter_lower {
        candidates.retain(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .map(|n| n.to_lowercase().starts_with(f.as_str()))
                .unwrap_or(false)
        });
    }

    candidates.sort();
    candidates.dedup();
    candidates.truncate(MAX_SUGGESTIONS);
    candidates
}

fn apply_workspace_change(session: &mut Session, requested: PathBuf) -> Result<()> {
    let resolved = if requested.exists() {
        requested.canonicalize().unwrap_or(requested.clone())
    } else {
        println!("{} does not exist.", requested.display());
        let answer = prompt_line("Create it? [y/N] ")?;
        if answer.trim().eq_ignore_ascii_case("y") {
            std::fs::create_dir_all(&requested)?;
            requested.canonicalize().unwrap_or(requested)
        } else {
            println!("Workspace unchanged.");
            return Ok(());
        }
    };

    if !resolved.is_dir() {
        println!("{} '{}' is not a directory.", "✗".red(), resolved.display());
        return Ok(());
    }

    session.agent.set_workspace(resolved.clone());
    session.config.workspace = resolved.clone();
    session.agent.permissions_mut().clear_session_grants();

    if session.history.is_empty() {
        session.history.push(session.agent.build_system_message());
    } else {
        session.history[0] = session.agent.build_system_message();
    }

    println!("{} Workspace changed:\n  {}", "✓".green(), resolved.display());
    println!(
        "{}",
        "  (session 'always' permission grants for the previous workspace were cleared)".dimmed()
    );
    Ok(())
}

// ---------------------------------------------------------------------
// /permissions
// ---------------------------------------------------------------------

fn parse_permission_kind(s: &str) -> Option<PermissionKind> {
    match s.to_lowercase().as_str() {
        "edit" => Some(PermissionKind::Edit),
        "create" => Some(PermissionKind::CreateFile),
        "delete" => Some(PermissionKind::DeleteFile),
        "terminal" | "bash" | "run" => Some(PermissionKind::Terminal),
        "git" => Some(PermissionKind::GitWrite),
        _ => None,
    }
}

fn permissions_cmd(session: &mut Session, args: &[String]) -> Result<CommandOutcome> {
    if args.len() >= 2 {
        let Some(kind) = parse_permission_kind(&args[0]) else {
            println!(
                "{} Unknown permission '{}'. Expected: edit, create, delete, terminal, git.",
                "✗".red(),
                args[0]
            );
            return Ok(CommandOutcome::Continue);
        };
        let allowed = match args[1].to_lowercase().as_str() {
            "always" | "auto" | "allow" => true,
            "ask" | "deny" => false,
            other => {
                println!("{} Unknown value '{other}'. Expected: always, ask.", "✗".red());
                return Ok(CommandOutcome::Continue);
            }
        };
        session.agent.permissions_mut().set_auto_allow(kind, allowed);
        // Mirror into config so /status and /config stay consistent.
        match kind {
            PermissionKind::Edit | PermissionKind::CreateFile | PermissionKind::DeleteFile => {
                session.config.permissions.allow_edit = allowed;
            }
            PermissionKind::Terminal => session.config.permissions.allow_terminal = allowed,
            PermissionKind::GitWrite => session.config.permissions.allow_git_write = allowed,
        }
        println!(
            "{} {} is now {}",
            "✓".green(),
            kind.label(),
            if allowed { "always allowed" } else { "ask every time" }
        );
        return Ok(CommandOutcome::Continue);
    }

    println!("{}", "Permissions".bold());
    println!("{}", "─".repeat(48).dimmed());
    println!("Read files:    automatic (always)");
    println!("Search:        automatic (always)");
    let perms = session.agent.permissions().current_config();
    for kind in PermissionKind::all() {
        let auto = match kind {
            PermissionKind::Edit | PermissionKind::CreateFile | PermissionKind::DeleteFile => perms.allow_edit,
            PermissionKind::Terminal => perms.allow_terminal,
            PermissionKind::GitWrite => perms.allow_git_write,
        };
        let state = ask_or_auto(auto);
        let session_note = if !auto && session.agent.permissions().is_session_granted(kind) {
            " (granted this session via 'a')".dimmed().to_string()
        } else {
            String::new()
        };
        println!("{}  {state}{session_note}", pad_label(&format!("{}:", capitalize(kind.label()))));
    }
    println!("Network:       {}", "not yet enforced by any tool".dimmed());
    println!();
    println!(
        "{}",
        "Change with: /permissions <edit|create|delete|terminal|git> <always|ask>".dimmed()
    );
    Ok(CommandOutcome::Continue)
}

fn capitalize(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

/// Pad `label` to at least `width` chars, but — unlike `{:<width$}` on a
/// string already longer than `width` — always keeps at least one
/// separating space so long labels don't visually run into the value.
fn pad_label(label: &str) -> String {
    const WIDTH: usize = 24;
    if label.chars().count() >= WIDTH {
        format!("{label} ")
    } else {
        format!("{label:<WIDTH$}")
    }
}

// ---------------------------------------------------------------------
// /tools
// ---------------------------------------------------------------------

fn tools_cmd(_session: &mut Session, _args: &[String]) -> Result<CommandOutcome> {
    println!("{}", "Available Tools".bold());
    println!("{}", "─".repeat(56).dimmed());
    for tool in crate::tools::default_registry().list() {
        println!("  {}", tool.name().bold());
        println!("    {}", tool.description().dimmed());
    }
    Ok(CommandOutcome::Continue)
}

// ---------------------------------------------------------------------
// /skills, /skill, /commands
// ---------------------------------------------------------------------

fn skills_cmd(session: &mut Session, _args: &[String]) -> Result<CommandOutcome> {
    let skills = crate::cli::skills::discover(session.agent.workspace());
    if skills.is_empty() {
        println!("{}", "No skills found.".dimmed());
        println!();
        println!("Add one at .rexo/skills/<name>/SKILL.md (project) or");
        if let Ok(dir) = crate::config::global::global_dir() {
            println!("{} (personal, every workspace).", dir.join("skills").join("<name>").join("SKILL.md").display());
        }
        println!("A SKILL.md starts with '---', a 'description:' and optional comma-separated 'triggers:', then '---' and the instructions.");
        return Ok(CommandOutcome::Continue);
    }
    println!("{}", "Skills".bold());
    println!("{}", "─".repeat(56).dimmed());
    for skill in &skills {
        let scope = if skill.project { "project" } else { "global" };
        let loaded = if session.loaded_skills.contains(&skill.name) { " (loaded)".green().to_string() } else { String::new() };
        println!("  {} {}{}", skill.name.bold(), format!("[{scope}]").dimmed(), loaded);
        println!("    {}", skill.description.dimmed());
        if !skill.triggers.is_empty() {
            println!("    {} {}", "triggers:".dimmed(), skill.triggers.join(", ").dimmed());
        }
    }
    println!();
    println!("{}", "Matching triggers load a skill into the conversation automatically; /skill <name> forces one now.".dimmed());
    Ok(CommandOutcome::Continue)
}

fn skill_load_cmd(session: &mut Session, args: &[String]) -> Result<CommandOutcome> {
    let Some(name) = args.first() else {
        println!("{} Usage: /skill <name>  (see /skills for the list)", "✗".red());
        return Ok(CommandOutcome::Continue);
    };
    let skills = crate::cli::skills::discover(session.agent.workspace());
    let Some(skill) = skills.iter().find(|s| &s.name == name) else {
        println!("{} No skill named '{name}'. Run /skills to see what's available.", "✗".red());
        return Ok(CommandOutcome::Continue);
    };
    session.history.push(crate::providers::ChatMessage::system(format!("[Skill: {}]\n{}", skill.name, skill.body)));
    session.loaded_skills.insert(skill.name.clone());
    println!("{} Loaded skill '{}' into the conversation.", "✓".green(), skill.name);
    Ok(CommandOutcome::Continue)
}

fn custom_commands_cmd(session: &mut Session, _args: &[String]) -> Result<CommandOutcome> {
    let cmds = crate::cli::custom_commands::discover(session.agent.workspace());
    if cmds.is_empty() {
        println!("{}", "No custom commands found.".dimmed());
        println!();
        println!("Add one at .rexo/commands/<name>.md (project) or");
        if let Ok(dir) = crate::config::global::global_dir() {
            println!("{} (personal, every workspace).", dir.join("commands").join("<name>.md").display());
        }
        println!("The file's contents become the prompt for /<name>; $ARGUMENTS and $1.. $9 are substituted from what you type after the command.");
        return Ok(CommandOutcome::Continue);
    }
    println!("{}", "Custom commands".bold());
    println!("{}", "─".repeat(56).dimmed());
    for cmd in &cmds {
        let scope = if cmd.project { "project" } else { "global" };
        println!("  /{} {}", cmd.name.bold(), format!("[{scope}]").dimmed());
        println!("    {}", cmd.description.dimmed());
    }
    Ok(CommandOutcome::Continue)
}

// ---------------------------------------------------------------------
// /api
// ---------------------------------------------------------------------

fn api_cmd(session: &mut Session, args: &[String]) -> Result<CommandOutcome> {
    match args.first().map(String::as_str) {
        None => {
            println!("{}", "API configuration".bold());
            println!();
            println!("Current provider: {}", session.config.display_name());
            println!(
                "API key:          {}",
                if session.config.model.provider == "local" {
                    "not required".dimmed().to_string()
                } else if session.api_key_configured() {
                    "configured".green().to_string()
                } else {
                    "not configured".yellow().to_string()
                }
            );
            let key_name = session.config.effective_credential_key();
            println!("Credential key:   {key_name}  (env var: REXO_{}_API_KEY)", key_name.to_uppercase());
            if let Some(preset) = crate::providers::catalog::find(&key_name) {
                if !preset.notes.is_empty() {
                    println!("{}", format!("  {}", preset.notes).dimmed());
                }
            }
            println!();
            println!("Commands: /api set   /api set --permanent   /api clear");
        }
        Some("set") => {
            let permanent = args.get(1).map(|a| a == "--permanent" || a == "-p").unwrap_or(false);
            set_api_key_interactively(session, permanent)?;
        }
        Some("clear") => {
            session.api_key_override = None;
            let key_name = session.config.effective_credential_key();
            if let Ok(store) = crate::config::credentials::CredentialStore::open() {
                if store.delete(&key_name).unwrap_or(false) {
                    println!("{} Removed the saved key for '{key_name}' too.", "✓".green());
                }
            }
            println!("{} Session API key override cleared.", "✓".green());
            match session.rebuild_provider() {
                Ok(()) => {}
                Err(e) => println!("{}", format!("(no key resolvable from anywhere else either: {e})").dimmed()),
            }
        }
        Some(other) => println!("{} Unknown /api subcommand '{other}'. Try 'set', 'set --permanent', or 'clear'.", "✗".red()),
    }
    Ok(CommandOutcome::Continue)
}

fn set_api_key_interactively(session: &mut Session, permanent: bool) -> Result<()> {
    if crate::output::is_capturing() {
        anyhow::bail!(
            "This command needs a real terminal but the TUI didn't hand off to one first \
             (please report this as a REXO bug)."
        );
    }
    let key = read_hidden_line("API key (input hidden): ")?;
    if key.is_empty() {
        println!("No key entered — unchanged.");
        return Ok(());
    }

    let key_name = session.config.effective_credential_key();
    if permanent {
        let store = crate::config::credentials::CredentialStore::open()?;
        store.set(&key_name, &key)?;
        session.api_key_override = None;
        println!(
            "{}",
            format!("✓ Saved permanently under '{key_name}' — available from any workspace, any time you run rexo.").green()
        );
    } else {
        session.api_key_override = Some(key);
        println!(
            "{}",
            "✓ API key set for this session only — not written to disk. Use \
             /api set --permanent to keep it, or run /connect for the full guided setup."
                .green()
        );
    }
    match session.rebuild_provider() {
        Ok(()) => println!("{} Provider is ready.", "✓".green()),
        Err(e) => println!("{} Key saved but activation failed: {e}", "!".yellow()),
    }
    Ok(())
}

// ---------------------------------------------------------------------
// /clear, /reset, /exit
// ---------------------------------------------------------------------

fn clear_cmd(session: &mut Session, _args: &[String]) -> Result<CommandOutcome> {
    session.reset_history();
    println!("Conversation cleared.");
    Ok(CommandOutcome::Continue)
}

fn reset_cmd(session: &mut Session, _args: &[String]) -> Result<CommandOutcome> {
    let workspace = session.agent.workspace().to_path_buf();
    let fresh_config = Config::load(&workspace)?;

    let non_interactive = session.agent.permissions().is_non_interactive();
    let auto_approve = session.agent.permissions().is_auto_approve();
    session
        .agent
        .set_permissions(PermissionManager::new(fresh_config.permissions.clone(), non_interactive, auto_approve));

    session.config = fresh_config;
    session.api_key_override = None;

    match session.rebuild_provider() {
        Ok(()) => {}
        Err(e) => println!("{} Reset provider config, but couldn't activate it: {e}", "!".yellow()),
    }
    session.reset_history();

    println!("{}", "Session reset.".bold());
    println!(
        "{}",
        "  Provider, model, and permissions reloaded from rexo.toml/environment; \
         conversation cleared. Workspace was left unchanged."
            .dimmed()
    );
    Ok(CommandOutcome::Continue)
}

fn exit_cmd(_session: &mut Session, _args: &[String]) -> Result<CommandOutcome> {
    Ok(CommandOutcome::Exit)
}

// ---------------------------------------------------------------------
// /cd is just workspace_cmd under another name (see COMMANDS) — no
// separate handler needed.
// ---------------------------------------------------------------------

// ---------------------------------------------------------------------
// /context, /compact, /autocompact
// ---------------------------------------------------------------------

fn context_cmd(session: &mut Session, _args: &[String]) -> Result<CommandOutcome> {
    let (mut user_chars, mut user_n) = (0usize, 0usize);
    let (mut assistant_chars, mut assistant_n) = (0usize, 0usize);
    let (mut tool_chars, mut tool_n) = (0usize, 0usize);

    for m in &session.history {
        let len = m.content.as_deref().map(str::len).unwrap_or(0) + m.reasoning.as_deref().map(str::len).unwrap_or(0);
        match m.role {
            crate::providers::Role::User => {
                user_chars += len;
                user_n += 1;
            }
            crate::providers::Role::Assistant => {
                assistant_chars += len;
                assistant_n += 1;
            }
            crate::providers::Role::Tool => {
                tool_chars += len;
                tool_n += 1;
            }
            crate::providers::Role::System => {}
        }
    }
    let total_chars = user_chars + assistant_chars + tool_chars;

    println!("{}", "Context usage (approximate)".bold());
    println!("{}", "─".repeat(48).dimmed());
    println!("  User messages:      {user_n:>4}   (~{} tokens)", user_chars / 4);
    println!("  Assistant messages: {assistant_n:>4}   (~{} tokens)", assistant_chars / 4);
    println!("  Tool results:       {tool_n:>4}   (~{} tokens)", tool_chars / 4);
    println!("  {}", "─".repeat(44).dimmed());
    println!("  Total:                     ~{} tokens", total_chars / 4);
    println!();
    println!(
        "{}",
        "This is a rough estimate (~4 chars/token) from message lengths only — REXO doesn't \
         currently track the provider's real token count or context-window size."
            .dimmed()
    );
    Ok(CommandOutcome::Continue)
}

const KEEP_RECENT_TOOL_RESULTS: usize = 4;

fn compact_cmd(session: &mut Session, _args: &[String]) -> Result<CommandOutcome> {
    let shrunk = run_compact(session);
    if shrunk == 0 {
        println!(
            "{}",
            "Nothing worth compacting yet — not enough tool output in this conversation.".dimmed()
        );
    } else {
        println!("{} Compacted {shrunk} older tool result(s).", "✓".green());
        println!(
            "{}",
            "  (a simple heuristic — replaces older tool output with a placeholder; it \
             doesn't summarize with the model.)"
                .dimmed()
        );
    }
    Ok(CommandOutcome::Continue)
}

/// Replaces tool-result content older than the most recent
/// [`KEEP_RECENT_TOOL_RESULTS`] with a short placeholder. Returns how many
/// messages were actually shrunk. Shared by `/compact` and `/autocompact`.
fn run_compact(session: &mut Session) -> usize {
    let tool_indices: Vec<usize> = session
        .history
        .iter()
        .enumerate()
        .filter(|(_, m)| m.role == crate::providers::Role::Tool)
        .map(|(i, _)| i)
        .collect();
    if tool_indices.len() <= KEEP_RECENT_TOOL_RESULTS {
        return 0;
    }
    let cutoff = tool_indices.len() - KEEP_RECENT_TOOL_RESULTS;
    let mut shrunk = 0;
    for &i in &tool_indices[..cutoff] {
        if let Some(content) = &mut session.history[i].content {
            const PLACEHOLDER_PREFIX: &str = "[tool output omitted for brevity by /compact";
            if content.len() > 200 && !content.starts_with(PLACEHOLDER_PREFIX) {
                *content = format!("{PLACEHOLDER_PREFIX} — {} chars]", content.len());
                shrunk += 1;
            }
        }
    }
    shrunk
}

/// `run_compact`, exposed for the TUI's `/autocompact` idle check (which
/// isn't going through a `/compact` command invocation, so it doesn't want
/// `run_compact`'s own "nothing to do" messaging).
pub fn run_compact_for_autocompact(session: &mut Session) -> usize {
    run_compact(session)
}

fn autocompact_cmd(session: &mut Session, args: &[String]) -> Result<CommandOutcome> {
    let Some(raw) = args.first() else {
        match session.ui.autocompact_threshold {
            Some(n) => println!("Auto-compact is on: triggers once {n} tool results have accumulated."),
            None => println!("Auto-compact is off. Usage: /autocompact <tool-result-count>   or   /autocompact off"),
        }
        return Ok(CommandOutcome::Continue);
    };
    if raw.eq_ignore_ascii_case("off") {
        session.ui.autocompact_threshold = None;
        println!("{} Auto-compact turned off.", "✓".green());
        return Ok(CommandOutcome::Continue);
    }
    match raw.parse::<usize>() {
        Ok(n) if n >= 2 => {
            session.ui.autocompact_threshold = Some(n);
            println!("{} Auto-compact will run /compact once {n} tool results have accumulated.", "✓".green());
        }
        _ => println!("{} Expected a number of at least 2, or 'off'.", "✗".red()),
    }
    Ok(CommandOutcome::Continue)
}

// ---------------------------------------------------------------------
// /doctor, /init, /memory, /diff
// ---------------------------------------------------------------------

fn doctor_cmd(session: &mut Session, _args: &[String]) -> Result<CommandOutcome> {
    println!("{}", "REXO Doctor".bold());
    println!("{}", "─".repeat(48).dimmed());

    let ws = session.agent.workspace();
    check_line(ws.is_dir(), &format!("Workspace exists and is a directory ({})", ws.display()));

    let git_ok = std::process::Command::new("git")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    check_line(git_ok, "git is installed and on PATH");

    let key_ok = session.config.model.provider == "local" || session.api_key_configured();
    check_line(key_ok, "API key resolvable for the current provider");

    let provider_ok = !session.agent.provider_description().to_lowercase().contains("not active");
    check_line(provider_ok, "Provider is active and ready to send requests");

    match crate::config::global::GlobalConfig::path() {
        Ok(path) => check_line(true, &format!("Global config directory reachable ({})", path.display())),
        Err(e) => check_line(false, &format!("Global config directory NOT reachable: {e}")),
    }

    println!();
    println!("{}", "Saved credentials (names only, never values):".dimmed());
    match crate::config::credentials::CredentialStore::open() {
        Ok(store) => {
            match store.configured_keys() {
                Ok(keys) if !keys.is_empty() => println!("  {}", keys.join(", ")),
                Ok(_) => println!("  {}", "(none saved yet — run /connect)".dimmed()),
                Err(e) => println!("  {} {e}", "✗".red()),
            }
            println!("  {}", format!("stored at {}", store.path().display()).dimmed());
        }
        Err(e) => println!("  {} {e}", "✗".red()),
    }

    println!();
    let perms = session.agent.permissions().current_config();
    println!("{}", "Permissions (informational — not a pass/fail check):".dimmed());
    println!(
        "  edit={}  terminal={}  git={}",
        ask_or_auto(perms.allow_edit),
        ask_or_auto(perms.allow_terminal),
        ask_or_auto(perms.allow_git_write)
    );
    Ok(CommandOutcome::Continue)
}

fn check_line(ok: bool, label: &str) {
    if ok {
        println!("  {} {label}", "✓".green());
    } else {
        println!("  {} {label}", "✗".red());
    }
}

fn init_cmd(session: &mut Session, _args: &[String]) -> Result<CommandOutcome> {
    let path = session.agent.workspace().join("REXO.md");
    if path.exists() {
        println!("{} REXO.md already exists — edit it directly, or see {}.", "○".dimmed(), "/memory".bold());
        return Ok(CommandOutcome::Continue);
    }
    let project_name = session
        .agent
        .workspace()
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("this project");
    let scaffold = format!(
        "# {project_name}\n\nNotes for Rexo Code when working in this repository.\n\n## Build & test\n\n- \n\n## Code style\n\n- \n\n## Things to avoid\n\n- \n"
    );
    std::fs::write(&path, scaffold)?;
    println!("{} Created {}", "✓".green(), path.display());
    println!(
        "{}",
        "  REXO includes this file's contents in its system prompt for this workspace going \
         forward — see /memory."
            .dimmed()
    );
    Ok(CommandOutcome::Continue)
}

fn memory_cmd(session: &mut Session, _args: &[String]) -> Result<CommandOutcome> {
    let path = session.agent.workspace().join("REXO.md");
    match std::fs::read_to_string(&path) {
        Ok(content) => {
            println!("{}", format!("REXO.md ({})", path.display()).bold());
            println!("{}", "─".repeat(48).dimmed());
            println!("{content}");
            println!("{}", "─".repeat(48).dimmed());
            println!(
                "{}",
                "Edit this file directly in your editor — REXO includes it in the system \
                 prompt for this workspace."
                    .dimmed()
            );
        }
        Err(_) => {
            println!("{}", "No REXO.md in this workspace yet.".dimmed());
            println!("Create one with {}", "/init".bold());
        }
    }
    Ok(CommandOutcome::Continue)
}

fn diff_cmd(session: &mut Session, _args: &[String]) -> Result<CommandOutcome> {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(session.agent.workspace())
        .arg("diff")
        .output();
    match output {
        Ok(o) if o.status.success() => {
            let text = String::from_utf8_lossy(&o.stdout);
            if text.trim().is_empty() {
                println!("{}", "No uncommitted changes.".dimmed());
            } else {
                println!("{text}");
            }
        }
        Ok(o) => {
            let err = String::from_utf8_lossy(&o.stderr);
            println!("{} git diff failed: {}", "✗".red(), err.trim());
        }
        Err(e) => println!("{} Couldn't run git: {e}", "✗".red()),
    }
    Ok(CommandOutcome::Continue)
}

// ---------------------------------------------------------------------
// /rename, /export, /copy
// ---------------------------------------------------------------------

fn rename_cmd(session: &mut Session, args: &[String]) -> Result<CommandOutcome> {
    if args.is_empty() {
        match &session.ui.title {
            Some(t) => println!("Current session title: {}", t.bold()),
            None => println!("No custom title set. Usage: /rename <title>"),
        }
        return Ok(CommandOutcome::Continue);
    }
    let title = args.join(" ");
    session.ui.title = Some(title.clone());
    println!("{} Session renamed to \"{}\"", "✓".green(), title);
    Ok(CommandOutcome::Continue)
}

fn export_cmd(session: &mut Session, args: &[String]) -> Result<CommandOutcome> {
    let filename = args.first().cloned().unwrap_or_else(|| {
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        format!("rexo-conversation-{ts}.md")
    });
    let path = session.agent.workspace().join(&filename);

    let mut out = String::from("# Rexo Code conversation export\n\n");
    for msg in &session.history {
        let role = match msg.role {
            crate::providers::Role::System => continue,
            crate::providers::Role::User => "User",
            crate::providers::Role::Assistant => "REXO",
            crate::providers::Role::Tool => "Tool result",
        };
        if let Some(content) = &msg.content {
            if !content.is_empty() {
                out.push_str(&format!("**{role}:**\n\n{content}\n\n"));
            }
        }
    }
    std::fs::write(&path, out)?;
    println!("{} Exported conversation to {}", "✓".green(), path.display());
    Ok(CommandOutcome::Continue)
}

/// Copies the most recent assistant answer to the OS clipboard by
/// default; `/copy all` copies the whole conversation (same rendering
/// `/export` uses, minus the markdown headers); `/copy <n>` copies the
/// n-th most recent assistant answer (1 = most recent, same as no
/// argument). Never touches tool-call/tool-result noise — just the
/// model's actual prose.
fn copy_cmd(session: &mut Session, args: &[String]) -> Result<CommandOutcome> {
    let answers: Vec<&str> = session
        .history
        .iter()
        .rev()
        .filter(|m| m.role == crate::providers::Role::Assistant)
        .filter_map(|m| m.content.as_deref())
        .filter(|c| !c.trim().is_empty())
        .collect();

    let text = match args.first().map(String::as_str) {
        Some("all") => {
            let mut out = String::new();
            for msg in &session.history {
                let role = match msg.role {
                    crate::providers::Role::System | crate::providers::Role::Tool => continue,
                    crate::providers::Role::User => "User",
                    crate::providers::Role::Assistant => "REXO",
                };
                if let Some(content) = &msg.content {
                    if !content.is_empty() {
                        out.push_str(&format!("{role}: {content}\n\n"));
                    }
                }
            }
            if out.trim().is_empty() {
                println!("{} Nothing to copy yet.", "○".dimmed());
                return Ok(CommandOutcome::Continue);
            }
            out
        }
        Some(n) => {
            let idx: usize = match n.parse::<usize>() {
                Ok(v) if v >= 1 => v - 1,
                _ => {
                    println!("{} Usage: /copy [all|<n>] — n = 1 for the most recent answer, 2 for the one before, etc.", "✗".red());
                    return Ok(CommandOutcome::Continue);
                }
            };
            match answers.get(idx) {
                Some(text) => text.to_string(),
                None => {
                    println!("{} Only {} assistant answer(s) in this conversation.", "✗".red(), answers.len());
                    return Ok(CommandOutcome::Continue);
                }
            }
        }
        None => match answers.first() {
            Some(text) => text.to_string(),
            None => {
                println!("{} No assistant answer yet to copy.", "○".dimmed());
                return Ok(CommandOutcome::Continue);
            }
        },
    };

    match crate::utils::clipboard::copy(&text) {
        Ok(()) => println!("{} Copied {} character(s) to the clipboard.", "✓".green(), text.chars().count()),
        Err(e) => {
            println!("{} Couldn't reach the system clipboard: {e}", "✗".red());
            println!("{}", "  (headless/no-display environments can't reach a clipboard — /export still works)".dimmed());
        }
    }
    Ok(CommandOutcome::Continue)
}

// ---------------------------------------------------------------------
// /goal, /plan, /focus
// ---------------------------------------------------------------------

fn goal_cmd(session: &mut Session, args: &[String]) -> Result<CommandOutcome> {
    if args.is_empty() {
        match &session.ui.goal {
            Some(g) => println!("Current session goal: {}", g.bold()),
            None => println!("No goal set for this session. Usage: /goal <what REXO should keep in mind>"),
        }
        return Ok(CommandOutcome::Continue);
    }
    let goal = args.join(" ");
    session.ui.goal = Some(goal.clone());
    session.history.push(crate::providers::ChatMessage::system(format!(
        "Session goal set by the user: {goal}\nKeep this in mind for the rest of the session; \
         before giving a final answer, check whether it's actually been met."
    )));
    println!("{} Goal set: {}", "✓".green(), goal);
    println!(
        "{}",
        "  (added as a reminder in the conversation — a prompt-level nudge, not hard enforcement.)".dimmed()
    );
    Ok(CommandOutcome::Continue)
}

fn plan_cmd(session: &mut Session, _args: &[String]) -> Result<CommandOutcome> {
    session.ui.plan_mode = !session.ui.plan_mode;
    if session.ui.plan_mode {
        session.history.push(crate::providers::ChatMessage::system(
            "Plan mode is on: propose a concrete step-by-step plan and ask the user to confirm \
             it before making any file edits, running commands, or making git writes. \
             Read-only exploration (reading/listing/searching files) is fine without asking."
                .to_string(),
        ));
        println!("{} Plan mode on — REXO will propose a plan before making changes.", "✓".green());
    } else {
        println!("{} Plan mode off.", "✓".green());
    }
    println!(
        "{}",
        "  (prompt-level, not hard-enforced — REXO can still deviate if it judges it necessary)".dimmed()
    );
    Ok(CommandOutcome::Continue)
}

fn focus_cmd(session: &mut Session, _args: &[String]) -> Result<CommandOutcome> {
    session.ui.focus_mode = !session.ui.focus_mode;
    if session.ui.focus_mode {
        println!("{} Focus view on — showing just your prompts and REXO's answers.", "✓".green());
    } else {
        println!("{} Focus view off — showing tool calls and details again.", "✓".green());
    }
    Ok(CommandOutcome::Continue)
}

// ---------------------------------------------------------------------
// /effort, /color, /scroll-speed, /keybindings
// ---------------------------------------------------------------------

fn effort_cmd(session: &mut Session, args: &[String]) -> Result<CommandOutcome> {
    let Some(level) = args.first() else {
        println!(
            "Current reasoning effort: {}",
            session.config.model.reasoning_effort.as_deref().unwrap_or("(provider default)")
        );
        println!("Usage: /effort <low|medium|high|none>");
        return Ok(CommandOutcome::Continue);
    };
    let normalized = level.to_lowercase();
    if !["low", "medium", "high", "none"].contains(&normalized.as_str()) {
        println!("{} Unknown effort level '{level}'. Expected: low, medium, high, none.", "✗".red());
        return Ok(CommandOutcome::Continue);
    }
    session.config.model.reasoning_effort = if normalized == "none" { None } else { Some(normalized.clone()) };
    match session.rebuild_provider() {
        Ok(()) => println!("{} Reasoning effort set to {normalized}", "✓".green()),
        Err(e) => println!("{} Effort set to {normalized} but couldn't activate it: {e}", "!".yellow()),
    }
    Ok(CommandOutcome::Continue)
}

const ACCENT_NAMES: &[(&str, AccentColor)] = &[
    ("cyan", AccentColor::Cyan),
    ("green", AccentColor::Green),
    ("magenta", AccentColor::Magenta),
    ("yellow", AccentColor::Yellow),
    ("blue", AccentColor::Blue),
];

fn color_cmd(session: &mut Session, args: &[String]) -> Result<CommandOutcome> {
    let names = || ACCENT_NAMES.iter().map(|(n, _)| *n).collect::<Vec<_>>().join(", ");
    let Some(name) = args.first() else {
        println!("Current accent color: {}", session.ui.accent.name());
        println!("Available: {}", names());
        return Ok(CommandOutcome::Continue);
    };
    match ACCENT_NAMES.iter().find(|(n, _)| n.eq_ignore_ascii_case(name)) {
        Some((_, color)) => {
            session.ui.accent = *color;
            println!("{} Accent color set to {name}", "✓".green());
        }
        None => println!("{} Unknown color '{name}'. Available: {}", "✗".red(), names()),
    }
    Ok(CommandOutcome::Continue)
}

fn scroll_speed_cmd(session: &mut Session, args: &[String]) -> Result<CommandOutcome> {
    let Some(raw) = args.first() else {
        println!("Current scroll speed: {} lines per press", session.ui.scroll_step);
        println!("Usage: /scroll-speed <1-20>");
        return Ok(CommandOutcome::Continue);
    };
    match raw.parse::<u16>() {
        Ok(n) if (1..=20).contains(&n) => {
            session.ui.scroll_step = n;
            println!("{} Scroll speed set to {n} lines per press", "✓".green());
        }
        _ => println!("{} Expected a number from 1 to 20.", "✗".red()),
    }
    Ok(CommandOutcome::Continue)
}

fn keybindings_cmd(_session: &mut Session, _args: &[String]) -> Result<CommandOutcome> {
    println!("{}", "Keybindings".bold());
    println!("{}", "─".repeat(48).dimmed());
    for (key, what) in crate::cli::tui::KEYBINDINGS {
        println!("  {:<22} {}", key.bold(), what.dimmed());
    }
    println!();
    println!("{}", "(In the interactive session, {?} opens the same list as a full-screen overlay.)".dimmed());
    Ok(CommandOutcome::Continue)
}

// ---------------------------------------------------------------------
// /loop
// ---------------------------------------------------------------------

fn loop_cmd(session: &mut Session, args: &[String]) -> Result<CommandOutcome> {
    let Some(first) = args.first() else {
        match &session.ui.loop_job {
            Some(job) => println!("Looping every {}s: \"{}\"", job.interval_secs, job.text),
            None => println!("No loop active. Usage: /loop <Ns|Nm> <message>   or   /loop off"),
        }
        return Ok(CommandOutcome::Continue);
    };
    if first.eq_ignore_ascii_case("off") || first.eq_ignore_ascii_case("stop") {
        session.ui.loop_job = None;
        println!("{} Loop stopped.", "✓".green());
        return Ok(CommandOutcome::Continue);
    }

    let (digits, multiplier) = if let Some(d) = first.strip_suffix('m') {
        (d, 60)
    } else if let Some(d) = first.strip_suffix('s') {
        (d, 1)
    } else {
        (first.as_str(), 1)
    };
    let Ok(n) = digits.parse::<u64>() else {
        println!("{} Expected an interval like '30s' or '5m'.", "✗".red());
        return Ok(CommandOutcome::Continue);
    };
    let interval_secs = n * multiplier;
    if interval_secs == 0 {
        println!("{} Interval must be greater than zero.", "✗".red());
        return Ok(CommandOutcome::Continue);
    }
    if args.len() < 2 {
        println!("{} Give it a prompt to repeat, e.g. /loop 5m check CI status", "✗".red());
        return Ok(CommandOutcome::Continue);
    }
    let text = args[1..].join(" ");
    println!("{} Will run \"{text}\" every {interval_secs}s (starting after the first interval).", "✓".green());
    session.ui.loop_job = Some(LoopJob {
        interval_secs,
        text,
        next_at: std::time::Instant::now() + std::time::Duration::from_secs(interval_secs),
    });
    Ok(CommandOutcome::Continue)
}

// ---------------------------------------------------------------------
// /release-notes, /feedback
// ---------------------------------------------------------------------

fn release_notes_cmd(_session: &mut Session, _args: &[String]) -> Result<CommandOutcome> {
    println!("{}", "Rexo Code v0.8.0".bold());
    println!("{}", "─".repeat(48).dimmed());
    println!("  • Fixed a real startup bug: launching rexo with no provider configured");
    println!("    used to hard-exit before the TUI ever opened, with no way to reach");
    println!("    /connect to fix it. An interactive session now always opens — with a");
    println!("    clear \"No provider is configured yet — run /connect\" note — and only");
    println!("    headless/single-shot mode (a prompt given on the command line) still");
    println!("    fails fast, since there's no session to recover into there.");
    println!("  • Rebrand cleanup pass: internal test scratch-directory names and a few");
    println!("    stale test function names still said \"tron\" (cosmetic — not visible");
    println!("    to users, but worth being thorough about since v0.7.2 already found");
    println!("    one real miss). An exhaustive rescan (including letter-spaced and");
    println!("    binary/asset checks this time) turned up nothing further.");
    println!();
    println!("{}", "Rexo Code v0.7.2".bold());
    println!("{}", "─".repeat(48).dimmed());
    println!("  • Fixed a real leftover-branding bug: the startup intro's title was a");
    println!("    letter-spaced \"T R O N - C O D E\" string, which the v0.7.1 rebrand's");
    println!("    search-and-replace couldn't catch (no contiguous \"TRON\" substring to");
    println!("    match). Now correctly reads \"R E X O   C O D E\".");
    println!("  • The intro animation is slower and more deliberate — three scan-lines");
    println!("    now sweep across the screen at once (top left→right, bottom");
    println!("    right→left, a faster one through the middle) before the face scatters");
    println!("    in, roughly 4s total, up from well under 1s. Any keypress still skips");
    println!("    straight to the end.");
    println!();
    println!("{}", "Rexo Code v0.7.1".bold());
    println!("{}", "─".repeat(48).dimmed());
    println!("  • Renamed from TRON-Code to Rexo Code — a naming clash with an existing");
    println!("    project. Binary, config dir (.rexo/), env var prefix (REXO_*), and");
    println!("    config file (rexo.toml) all renamed to match; behavior is unchanged");
    println!("  • Session persistence wired up for real: a backend module existed since");
    println!("    an earlier pass but was never declared as part of the build — /resume,");
    println!("    /branch, and /rewind now actually work, autosaving after every turn");
    println!("  • /fork: saves the current point as a new session and continues as it —");
    println!("    honestly scoped as a saved divergence point, not background execution");
    println!("    (Rexo has no concurrent/subagent runtime yet — /background, /agents,");
    println!("    and a real /mcp protocol client still don't)");
    println!();
    println!("{}", "Rexo Code v0.7.0".bold());
    println!("{}", "─".repeat(48).dimmed());
    println!("  • Startup intro animation — the boxed face scatters in and assembles on");
    println!("    launch (skip with any keypress, or --no-intro / REXO_NO_INTRO)");
    println!("  • First-launch wizard rewritten on real arrow-key pickers instead of");
    println!("    blocking numbered-list prompts — deferred since v0.4.0, done now");
    println!("  • Two real bugs found via PTY testing of the new wizard and fixed:");
    println!("    stale content bleeding through into the trust dialog's alt-screen,");
    println!("    and a model left blank silently breaking the next launch");
    println!();
    println!("{}", "Rexo Code v0.6.0".bold());
    println!("{}", "─".repeat(48).dimmed());
    println!("  • Shell mode: type '!' to drop into raw PowerShell/sh, '!<cmd>' for a");
    println!("    one-off regardless of mode — bypasses the model entirely");
    println!("  • Provider capability model extended to 11 fields, and — for the first");
    println!("    time — actually displayed in /status instead of sitting unused");
    println!("  • Normalized provider errors (ProviderErrorKind): every provider's");
    println!("    failures classified (rate_limited/authentication/timeout/...), shown");
    println!("    in the TUI and in --output json's new error_kind field");
    println!("  • Real Gemini streaming: streamGenerateContent SSE parsing replaces");
    println!("    the old call-once-at-the-end fallback");
    println!("  • Root-caused the wizard migration credential-rename bug flagged back");
    println!("    in v0.4.1 — migrating a working setup no longer silently orphans it");
    println!();
    println!("{}", "Rexo Code v0.5.0".bold());
    println!("{}", "─".repeat(48).dimmed());
    println!("  • Real, downloadable releases: CI now builds and tests every push on");
    println!("    all five target platforms, and pushing a version tag publishes");
    println!("    prebuilt binaries for each straight to GitHub Releases");
    println!("  • install.sh / uninstall.sh — the Linux/macOS counterpart to");
    println!("    install.ps1, same one-line-PATH-edit philosophy");
    println!("  • A real LICENSE file, now bundled into every release archive");
    println!();
    println!("{}", "v0.4.1".bold());
    println!("{}", "─".repeat(48).dimmed());
    println!("  • Native Google Gemini driver (/connect → Google Gemini). Pointing the old");
    println!("    openai_compatible client at Gemini's own API always 401'd — wrong auth");
    println!("    header, wrong request/response shape entirely. Not streamed yet.");
    println!("  • Fixed: picking a model and answering the \"save as default?\" prompt could");
    println!("    silently eat every keystroke — it was a plain blocking terminal prompt");
    println!("    firing while the screen was still in raw mode. Now a real in-TUI picker.");
    println!("  • Fixed: a rare race between the persistent screen's input reader and a");
    println!("    suspended command's own terminal prompt (e.g. /workspace creating a new");
    println!("    directory) that could eat keystrokes there too.");
    println!("  • Fixed: a couple of connect/model confirmations could print raw text over");
    println!("    the live screen instead of landing in the transcript.");
    println!();
    println!("{}", "v0.4.0".bold());
    println!("{}", "─".repeat(48).dimmed());
    println!("  • /connect, /model, /models, /provider: real arrow-key pickers, in-TUI —");
    println!("    live model discovery is now browsable/filterable, not just numbered");
    println!("  • Masked API-key entry moved fully in-TUI too — no more leaving the");
    println!("    screen for /connect, and no more depending on rpassword for it");
    println!("  • Skills: .rexo/skills/<name>/SKILL.md (+ a global skills dir),");
    println!("    keyword-triggered or /skill <name> — see /skills");
    println!("  • Custom commands: .rexo/commands/<name>.md becomes /<name>,");
    println!("    with $ARGUMENTS/$1.. $9 substitution — see /commands");
    println!("  • Real /copy (OS clipboard) and /add-dir (read-only extra roots)");
    println!("  • Mascot now has actual expressions: thinking, reasoning, writing,");
    println!("    a permission-wait face, and note-reactive idle states");
    println!("  • Ctrl+Y \"selection mode\" — releases the mouse so your terminal's own");
    println!("    click-drag select & copy works; Alt+M/Alt+P jump straight to the");
    println!("    model/provider pickers; bare ? as the first character shows shortcuts");
    println!("  • Console title now reads \"Rexo Code — <folder>\"; a Windows exe icon and");
    println!("    Windows Terminal tab-icon profile are included (see assets/)");
    println!();
    println!("{}", "v0.3.0".bold());
    println!("{}", "─".repeat(48).dimmed());
    println!("  • Provider config is global now — switching workspaces (or restarting)");
    println!("    no longer forgets your provider/model/API key");
    println!("  • Credentials persist in a per-user store, outside any workspace");
    println!("  • First-launch setup wizard");
    println!("  • /connect: searchable provider picker + live model discovery");
    println!("  • /model: search, live discovery, custom IDs, per-provider save");
    println!("  • /mcp: server configuration (protocol execution not wired in yet)");
    println!("  • @file references with a searchable picker");
    println!("  • {{?}} shortcut-list popup; mouse wheel scrolls the transcript");
    println!("  • Headless output: --output json/jsonl, scriptable exit codes");
    println!("  • Clearer provider error messages (endpoint/model/cause, never the key)");
    println!();
    println!("{}", "See README.md for the full history and current limitations.".dimmed());
    Ok(CommandOutcome::Continue)
}

fn feedback_cmd(session: &mut Session, args: &[String]) -> Result<CommandOutcome> {
    if args.is_empty() {
        println!("Usage: /feedback <your message>");
        println!(
            "{}",
            "  Appends to rexo-feedback.log in your workspace — nothing is sent anywhere automatically.".dimmed()
        );
        return Ok(CommandOutcome::Continue);
    }
    let text = args.join(" ");
    let path = session.agent.workspace().join("rexo-feedback.log");
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let line = format!("[{ts}] {text}\n");
    let result = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .and_then(|mut f| f.write_all(line.as_bytes()));
    match result {
        Ok(()) => println!("{} Saved to {}", "✓".green(), path.display()),
        Err(e) => println!("{} Couldn't write feedback: {e}", "✗".red()),
    }
    Ok(CommandOutcome::Continue)
}

// ---------------------------------------------------------------------
// Planned commands — registered so /help and autocomplete give an
// honest "not implemented yet" instead of "unknown command". Tracked in
// README.md's roadmap.
// ---------------------------------------------------------------------

fn not_yet(name: &str, what: &str) -> Result<CommandOutcome> {
    println!("{} {} isn't implemented yet.", "○".dimmed(), name.bold());
    println!("{}", format!("  ({what} — tracked on the roadmap in README.md.)").dimmed());
    Ok(CommandOutcome::Continue)
}

/// Real, but deliberately scoped down from the roadmap's full "multiple
/// simultaneous working directories": added roots are read-only.
/// `read_file`/`list_files` can resolve an *absolute* path into one, but
/// `edit_file`/`create_file`/`delete_file`/`run_command`/git-write stay
/// restricted to the primary workspace — see
/// `tools::ToolContext::extra_read_roots`'s docs for exactly why. That's
/// a genuine, safe version of "look across more than one directory," not
/// the full multi-root workspace this is a first step toward.
fn add_dir_cmd(session: &mut Session, args: &[String]) -> Result<CommandOutcome> {
    match args.first().map(String::as_str) {
        None | Some("list") => {
            let roots = session.agent.read_roots();
            if roots.is_empty() {
                println!("{}", "No additional directories added.".dimmed());
                println!("Usage: /add-dir <path>   (read-only: read_file/list_files can reach it by absolute path)");
            } else {
                println!("{}", "Additional read-only directories".bold());
                for root in roots {
                    println!("  {}", root.display());
                }
                println!();
                println!("{}", "Remove one with /add-dir remove <path>, or all with /add-dir clear.".dimmed());
            }
            Ok(CommandOutcome::Continue)
        }
        Some("remove") => {
            let Some(raw) = args.get(1) else {
                println!("{} Usage: /add-dir remove <path>", "✗".red());
                return Ok(CommandOutcome::Continue);
            };
            let path = resolve_relative(session.agent.workspace(), raw);
            if session.agent.remove_read_root(&path) {
                println!("{} Removed {}", "✓".green(), path.display());
            } else {
                println!("{} '{}' wasn't an added directory.", "✗".red(), path.display());
            }
            Ok(CommandOutcome::Continue)
        }
        Some("clear") => {
            session.agent.clear_read_roots();
            println!("{} Cleared all added directories.", "✓".green());
            Ok(CommandOutcome::Continue)
        }
        Some(raw) => {
            let path = resolve_relative(session.agent.workspace(), raw);
            match session.agent.add_read_root(path) {
                Ok(canonical) => {
                    println!("{} Added {} (read-only: absolute-path reads only).", "✓".green(), canonical.display());
                }
                Err(e) => println!("{} {e}", "✗".red()),
            }
            Ok(CommandOutcome::Continue)
        }
    }
}

/// Save the current conversation as a new named session without
/// switching away from it — a bookmark you can `/resume` later while
/// this session keeps going. See `cli::sessions`' module docs.
fn branch_cmd(session: &mut Session, args: &[String]) -> Result<CommandOutcome> {
    let Some(name) = args.first() else {
        println!("{}", "Usage: /branch <name>".bold());
        println!("Saves the current conversation as a new named session you can /resume");
        println!("later — this session keeps going unchanged.");
        return Ok(CommandOutcome::Continue);
    };
    match crate::cli::sessions::save_named(session, name) {
        Ok(path) => {
            println!("{} Branched — saved as '{}' ({})", "✓".green(), name.bold(), path.display());
            println!("{}", "This session continues unchanged; /resume picks up the branch later.".dimmed());
        }
        Err(e) => println!("{} Couldn't save branch: {e}", "✗".red()),
    }
    Ok(CommandOutcome::Continue)
}

/// Save the current point as a new named session *and* switch this
/// session to continue as that fork going forward — future autosaves go
/// to `<name>.json` instead of the default slot (see
/// `Session::active_session_name`), leaving the prior autosave alone as
/// a `/resume`-able snapshot of "right before the fork."
///
/// This does not run anything concurrently in the background — there's
/// no subagent/background-execution runtime in REXO yet (see the
/// README's roadmap). What it gives you is a real, saved divergence
/// point you can come back to, not autonomous parallel execution.
fn fork_cmd(session: &mut Session, args: &[String]) -> Result<CommandOutcome> {
    let Some(name) = args.first() else {
        println!("{}", "Usage: /fork <name>".bold());
        println!("Saves this point as '<name>' and continues THIS session as that fork.");
        println!("The previous autosave is left alone, so /resume can still get you back");
        println!("to right before the fork.");
        println!();
        println!("{}", "Note: this doesn't run anything in the background — REXO has no".dimmed());
        println!("{}", "concurrent/subagent execution yet. It's a saved divergence point,".dimmed());
        println!("{}", "not autonomous parallel work.".dimmed());
        return Ok(CommandOutcome::Continue);
    };
    match crate::cli::sessions::save_named(session, name) {
        Ok(path) => {
            session.active_session_name = Some(name.clone());
            println!("{} Forked — now continuing as '{}' ({})", "✓".green(), name.bold(), path.display());
            println!("{}", "Future autosaves go here; the previous autosave still has the pre-fork state.".dimmed());
        }
        Err(e) => println!("{} Couldn't fork: {e}", "✗".red()),
    }
    Ok(CommandOutcome::Continue)
}

/// `/rewind <n>` — scripting/non-TUI path straight to checkpoint `n`
/// (1-indexed, matching what the in-TUI picker shows). Bare `/rewind` in
/// the TUI opens that picker instead (see `tui::rewind_picker`); this
/// handler only runs when an argument is given or the TUI isn't active.
fn rewind_cmd(session: &mut Session, args: &[String]) -> Result<CommandOutcome> {
    let Some(n) = args.first().and_then(|s| s.parse::<usize>().ok()) else {
        println!("{}", "Usage: /rewind <checkpoint#>".bold());
        println!("(bare /rewind in the interactive session opens a picker instead)");
        return Ok(CommandOutcome::Continue);
    };
    let checkpoints: Vec<usize> = session.history.iter().enumerate().filter(|(_, m)| m.role == crate::providers::Role::User).map(|(i, _)| i).collect();
    let Some(idx) = n.checked_sub(1).and_then(|i| checkpoints.get(i)).copied() else {
        println!("No checkpoint #{n}. There are {} — run bare /rewind in the TUI to see them.", checkpoints.len());
        return Ok(CommandOutcome::Continue);
    };
    let removed = session.history.len() - idx;
    session.history.truncate(idx);
    println!("{} Rewound — removed {removed} message(s). File edits since then are still on disk (rewind is conversation-only).", "✓".green());
    crate::cli::sessions::autosave(session);
    Ok(CommandOutcome::Continue)
}

/// `/resume list|save <name>|delete <name>|<name>` — the argument-taking
/// forms of session resume/management. Bare `/resume` opens the in-TUI
/// picker instead (see `tui::resume_picker`); this only runs when an
/// argument is given or the picker isn't available.
fn resume_cmd(session: &mut Session, args: &[String]) -> Result<CommandOutcome> {
    match args.first().map(String::as_str) {
        None | Some("list") => list_saved_sessions(session),
        Some("save") => {
            let Some(name) = args.get(1) else {
                println!("Usage: /resume save <name>");
                return Ok(CommandOutcome::Continue);
            };
            match crate::cli::sessions::save_named(session, name) {
                Ok(path) => println!("{} Saved as '{}' ({})", "✓".green(), name.bold(), path.display()),
                Err(e) => println!("{} Couldn't save: {e}", "✗".red()),
            }
        }
        Some("delete") => {
            let Some(name) = args.get(1) else {
                println!("Usage: /resume delete <name>");
                return Ok(CommandOutcome::Continue);
            };
            match crate::cli::sessions::delete(session.agent.workspace(), name) {
                Ok(true) => println!("{} Deleted '{}'.", "✓".green(), name),
                Ok(false) => println!("No saved session named '{name}'."),
                Err(e) => println!("{} Couldn't delete: {e}", "✗".red()),
            }
        }
        Some(name) => {
            let saved = crate::cli::sessions::list(session.agent.workspace());
            match saved.iter().find(|s| s.name == name) {
                Some(s) => match crate::cli::sessions::load(&s.path) {
                    Ok(snapshot) => {
                        session.history = snapshot.history;
                        session.loaded_skills.clear();
                        session.active_session_name = if name == "autosave" { None } else { Some(name.to_string()) };
                        println!("{} Resumed '{}' ({} messages).", "✓".green(), name, session.history.len());
                    }
                    Err(e) => println!("{} Couldn't load '{name}': {e}", "✗".red()),
                },
                None => println!("No saved session named '{name}'. Run /resume list to see what's available."),
            }
        }
    }
    Ok(CommandOutcome::Continue)
}

fn list_saved_sessions(session: &Session) {
    let saved = crate::cli::sessions::list(session.agent.workspace());
    println!("{}", "Saved Sessions".bold());
    println!("{}", "─".repeat(48).dimmed());
    if saved.is_empty() {
        println!("{}", "None yet — one gets autosaved after your first message.".dimmed());
        return;
    }
    for s in &saved {
        let label = if s.is_autosave { "autosave".to_string() } else { s.name.clone() };
        println!("  {:<20} {} · {} turn(s)", label.bold(), crate::cli::sessions::format_saved_at(&s.saved_at), s.turn_count);
        println!("      {}", s.preview.dimmed());
    }
    println!();
    println!("{}", "Resume with /resume <name>, save the current one with /resume save <name>.".dimmed());
}

fn background_cmd(_session: &mut Session, _args: &[String]) -> Result<CommandOutcome> {
    not_yet("/background", "sending this session to the background")
}

fn mcp_cmd(session: &mut Session, args: &[String]) -> Result<CommandOutcome> {
    match args.first().map(String::as_str) {
        None | Some("list") => {
            println!("{}", "MCP Servers".bold());
            println!("{}", "─".repeat(48).dimmed());
            if session.config.global.mcp_servers.is_empty() {
                println!("{}", "None configured yet. Add one with:".dimmed());
                println!("  /mcp add <name> --command \"npx -y @modelcontextprotocol/server-filesystem /path\"");
                println!("  /mcp add <name> --url \"https://example.com/mcp\"");
            } else {
                for (name, server) in &session.config.global.mcp_servers {
                    let marker = if server.enabled { "●".green() } else { "○".dimmed() };
                    let live = if session.agent.is_mcp_connected(name) { " (connected)".green().to_string() } else { String::new() };
                    let source = server
                        .command
                        .as_deref()
                        .map(|c| format!("command: {c}"))
                        .or_else(|| server.url.as_deref().map(|u| format!("url: {u}")))
                        .unwrap_or_else(|| "(no command or url set)".to_string());
                    println!("{marker} {}{live}  [{}]", name.bold(), if server.enabled { "enabled" } else { "disabled" });
                    println!("  {}", source.dimmed());
                }
            }
            println!();
            let live = session.agent.mcp_server_names();
            if live.is_empty() {
                println!("{}", "None currently connected this session. Run /mcp connect <name> to connect one.".dimmed());
            } else {
                println!("{} {}", "Connected now:".dimmed(), live.join(", "));
            }
        }
        Some("add") => {
            let Some(name) = args.get(1).cloned() else {
                println!("{} Usage: /mcp add <name> --command \"...\"   or   /mcp add <name> --url \"...\"", "✗".red());
                return Ok(CommandOutcome::Continue);
            };
            let rest = &args[2..];
            let command = flag_value(rest, "--command");
            let url = flag_value(rest, "--url");
            if command.is_none() && url.is_none() {
                println!("{} Provide --command \"...\" or --url \"...\".", "✗".red());
                return Ok(CommandOutcome::Continue);
            }
            session.config.global.mcp_servers.insert(
                name.clone(),
                crate::config::global::McpServerConfig { command, url, enabled: true },
            );
            session.config.global.save()?;
            println!("{} Saved MCP server '{name}'.", "✓".green());
        }
        Some("remove") => {
            let Some(name) = args.get(1) else {
                println!("{} Usage: /mcp remove <name>", "✗".red());
                return Ok(CommandOutcome::Continue);
            };
            if session.config.global.mcp_servers.remove(name).is_some() {
                session.config.global.save()?;
                println!("{} Removed '{name}'.", "✓".green());
            } else {
                println!("{} No MCP server named '{name}'.", "✗".red());
            }
        }
        Some(sub @ ("enable" | "disable")) => {
            let Some(name) = args.get(1) else {
                println!("{} Usage: /mcp {sub} <name>", "✗".red());
                return Ok(CommandOutcome::Continue);
            };
            match session.config.global.mcp_servers.get_mut(name) {
                Some(server) => {
                    server.enabled = sub == "enable";
                    session.config.global.save()?;
                    println!("{} '{name}' {sub}d.", "✓".green());
                }
                None => println!("{} No MCP server named '{name}'.", "✗".red()),
            }
        }
        Some(sub @ ("connect" | "disconnect" | "status")) => {
            println!("{} /mcp {sub} needs the interactive session (it does real process/network I/O).", "✗".red());
        }
        Some(other) => println!("{} Unknown /mcp subcommand '{other}'. Try: list, add, remove, enable, disable, connect, disconnect, status.", "✗".red()),
    }
    Ok(CommandOutcome::Continue)
}

fn flag_value(args: &[String], flag: &str) -> Option<String> {
    let idx = args.iter().position(|a| a == flag)?;
    args.get(idx + 1).cloned()
}

fn hooks_cmd(session: &mut Session, args: &[String]) -> Result<CommandOutcome> {
    match args.first().map(String::as_str) {
        None | Some("list") => {
            println!("{}", "Hooks".bold());
            println!("{}", "─".repeat(48).dimmed());
            let hooks = session.agent.hooks();
            if hooks.is_empty() {
                println!("{}", "None configured yet. Add one with:".dimmed());
                println!("  /hooks add pre_tool \"echo about to run\" --matcher \"edit_*\"");
                println!("  /hooks add session_start \"echo session starting\"");
            } else {
                for (i, h) in hooks.iter().enumerate() {
                    let matcher = h.matcher.as_deref().map(|m| format!(" [{m}]")).unwrap_or_default();
                    println!("{} {}{matcher}  →  {}", format!("{i}.").dimmed(), h.event.bold(), h.command);
                }
            }
            println!();
            println!("{}", "Events: pre_tool, post_tool, session_start, session_end.".dimmed());
            println!(
                "{}",
                "pre_tool: exit 0 allows, exit 2 blocks (stderr becomes the reason), any other \
                 exit warns but still allows. post_tool/session_*: informational only."
                    .dimmed()
            );
        }
        Some("add") => {
            let Some(event) = args.get(1).cloned() else {
                println!("{} Usage: /hooks add <event> \"<command>\" [--matcher \"<glob>\"]", "✗".red());
                return Ok(CommandOutcome::Continue);
            };
            if !crate::hooks::EVENTS.contains(&event.as_str()) {
                println!("{} Unknown event '{event}'. Try one of: {}", "✗".red(), crate::hooks::EVENTS.join(", "));
                return Ok(CommandOutcome::Continue);
            }
            let Some(command) = args.get(2).cloned() else {
                println!("{} Usage: /hooks add <event> \"<command>\" [--matcher \"<glob>\"]", "✗".red());
                return Ok(CommandOutcome::Continue);
            };
            let matcher = flag_value(&args[3.min(args.len())..], "--matcher");
            let mut hooks = session.agent.hooks().to_vec();
            hooks.push(crate::hooks::HookDef { event: event.clone(), command: command.clone(), matcher });
            crate::hooks::save(session.agent.workspace(), &hooks)?;
            session.agent.reload_hooks();
            println!("{} Added {event} hook: {command}", "✓".green());
        }
        Some("remove") => {
            let Some(idx) = args.get(1).and_then(|s| s.parse::<usize>().ok()) else {
                println!("{} Usage: /hooks remove <index>  (see /hooks list for indices)", "✗".red());
                return Ok(CommandOutcome::Continue);
            };
            let mut hooks = session.agent.hooks().to_vec();
            if idx >= hooks.len() {
                println!("{} No hook at index {idx}.", "✗".red());
                return Ok(CommandOutcome::Continue);
            }
            let removed = hooks.remove(idx);
            crate::hooks::save(session.agent.workspace(), &hooks)?;
            session.agent.reload_hooks();
            println!("{} Removed {} hook: {}", "✓".green(), removed.event, removed.command);
        }
        Some("test") => {
            let Some(event) = args.get(1) else {
                println!("{} Usage: /hooks test <event> [tool_name]", "✗".red());
                return Ok(CommandOutcome::Continue);
            };
            let tool_name = args.get(2).cloned();
            let outcomes = crate::hooks::run(session.agent.hooks(), session.agent.workspace(), event, tool_name.as_deref(), None, None);
            if outcomes.is_empty() {
                println!("{}", "No hooks matched.".dimmed());
            }
            for outcome in outcomes {
                match outcome {
                    crate::hooks::HookOutcome::Ok(note) => println!("{} ok{}", "✓".green(), note.map(|n| format!(": {n}")).unwrap_or_default()),
                    crate::hooks::HookOutcome::Block(reason) => println!("{} would block: {reason}", "✗".red()),
                    crate::hooks::HookOutcome::Warn(msg) => println!("{} {msg}", "!".yellow()),
                }
            }
        }
        Some(other) => println!("{} Unknown /hooks subcommand '{other}'. Try: list, add, remove, test.", "✗".red()),
    }
    Ok(CommandOutcome::Continue)
}

fn ide_cmd(_session: &mut Session, args: &[String]) -> Result<CommandOutcome> {
    match args.first().map(String::as_str) {
        None | Some("info") => {
            println!("{}", "IDE Integration".bold());
            println!("{}", "─".repeat(48).dimmed());
            println!("/ide start   — start a local server an editor extension can connect to");
            println!("/ide stop    — stop it");
            println!("/ide status  — show whether it's running, its port, and the last file/selection it was told about");
            println!();
            println!(
                "{}",
                "Honest caveat: this is the REXO-side server only — no VSCode/JetBrains \
                 extension exists yet to connect to it. The protocol (newline-delimited JSON \
                 over a local TCP port, with a discovery lockfile under the global config dir) \
                 is real and tested against a synthetic client; a real editor extension is a \
                 separate project this release doesn't include."
                    .dimmed()
            );
        }
        Some("start") | Some("stop") | Some("status") => {
            println!("{} /ide {} needs the interactive session.", "✗".red(), args[0]);
        }
        Some(other) => println!("{} Unknown /ide subcommand '{other}'. Try: start, stop, status, info.", "✗".red()),
    }
    Ok(CommandOutcome::Continue)
}

fn plugin_cmd(session: &mut Session, args: &[String]) -> Result<CommandOutcome> {
    match args.first().map(String::as_str) {
        None | Some("list") => {
            println!("{}", "Plugins".bold());
            println!("{}", "─".repeat(48).dimmed());
            let found = crate::plugins::discover(session.agent.workspace());
            if found.is_empty() {
                println!("{}", "None found. Drop one at .rexo/plugins/<name>/plugin.toml — see /plugin info for the layout.".dimmed());
            } else {
                for p in &found {
                    let marker = if p.enabled { "●".green() } else { "○".dimmed() };
                    let ver = if p.version.is_empty() { String::new() } else { format!(" v{}", p.version) };
                    println!("{marker} {}{ver}  [{}]", p.name.bold(), if p.enabled { "enabled" } else { "disabled" });
                    if !p.description.is_empty() {
                        println!("  {}", p.description.dimmed());
                    }
                }
            }
        }
        Some("enable") => {
            let Some(name) = args.get(1) else {
                println!("{} Usage: /plugin enable <name>", "✗".red());
                return Ok(CommandOutcome::Continue);
            };
            match crate::plugins::enable(session.agent.workspace(), name) {
                Ok(summary) => println!("{} Enabled '{name}': {summary}.", "✓".green()),
                Err(e) => println!("{} Couldn't enable '{name}': {e}", "✗".red()),
            }
        }
        Some("disable") => {
            let Some(name) = args.get(1) else {
                println!("{} Usage: /plugin disable <name>", "✗".red());
                return Ok(CommandOutcome::Continue);
            };
            match crate::plugins::disable(session.agent.workspace(), name) {
                Ok(summary) => println!("{} Disabled '{name}': {summary}.", "✓".green()),
                Err(e) => println!("{} Couldn't disable '{name}': {e}", "✗".red()),
            }
        }
        Some("info") => {
            println!("{}", "A plugin is a folder at .rexo/plugins/<name>/ containing:".bold());
            println!("  plugin.toml       description = \"...\", version = \"...\"");
            println!("  skills/<n>/SKILL.md    (optional — same format as /skills)");
            println!("  commands/<n>.md        (optional — same format as custom commands)");
            println!("  hooks.toml              (optional — same format as .rexo/hooks.toml)");
            println!();
            println!("{}", "/plugin enable copies its skills/commands into the standard .rexo/ locations".dimmed());
            println!("{}", "and merges its hooks — /plugin disable removes exactly what enable installed.".dimmed());
        }
        Some(other) => println!("{} Unknown /plugin subcommand '{other}'. Try: list, enable, disable, info.", "✗".red()),
    }
    Ok(CommandOutcome::Continue)
}

fn agents_cmd(session: &mut Session, args: &[String]) -> Result<CommandOutcome> {
    match args.first().map(String::as_str) {
        None | Some("list") => {
            println!("{}", "Subagents".bold());
            println!("{}", "─".repeat(48).dimmed());
            let found = crate::agent::subagent::discover(session.agent.workspace());
            if found.is_empty() {
                println!("{}", "None yet. Create one with /agents create <name> \"<description>\".".dimmed());
            } else {
                for (name, persona) in &found {
                    println!("{} {}", "●".green(), name.bold());
                    if !persona.description.is_empty() {
                        println!("  {}", persona.description.dimmed());
                    }
                }
                println!();
                println!("{}", "Run one with: /agents run <name> \"<task>\"".dimmed());
            }
        }
        Some("create") => {
            let Some(name) = args.get(1).cloned() else {
                println!("{} Usage: /agents create <name> \"<description>\"", "✗".red());
                return Ok(CommandOutcome::Continue);
            };
            let description = args[2.min(args.len())..].join(" ");
            if description.trim().is_empty() {
                println!("{} A description helps the generated system prompt actually be useful — try: /agents create {name} \"<what it's for>\"", "✗".red());
                return Ok(CommandOutcome::Continue);
            }
            match crate::agent::subagent::create(session.agent.workspace(), &name, description.trim()) {
                Ok(path) => println!("{} Created {} — edit its system_prompt directly if the default isn't scoped enough.", "✓".green(), path.display()),
                Err(e) => println!("{} {e}", "✗".red()),
            }
        }
        Some("run") => {
            println!("{} /agents run needs the interactive session (it does real model calls).", "✗".red());
        }
        Some(other) => println!("{} Unknown /agents subcommand '{other}'. Try: list, create, run.", "✗".red()),
    }
    Ok(CommandOutcome::Continue)
}

// ---------------------------------------------------------------------
// shared helpers
// ---------------------------------------------------------------------

fn prompt_line(label: &str) -> Result<String> {
    if crate::output::is_capturing() {
        anyhow::bail!(
            "This command needs a real terminal but the TUI didn't hand off to one first \
             (please report this as a REXO bug)."
        );
    }
    print!("{label}");
    io::stdout().flush().ok();
    let mut line = String::new();
    io::stdin().read_line(&mut line)?;
    Ok(line.trim().to_string())
}

/// Read a key/password with terminal echo disabled. If that fails for any
/// reason (some terminal environments don't support it), falls back to a
/// normal, visible line read rather than silently defaulting to an empty
/// string — a swallowed failure here would otherwise desync every prompt
/// that follows it, since callers always expect exactly one line consumed
/// either way.
pub(crate) fn read_hidden_line(label: &str) -> Result<String> {
    match rpassword::prompt_password(label) {
        Ok(s) => Ok(s.trim().to_string()),
        Err(e) => {
            println!("{} Couldn't read input with echo hidden ({e}) — falling back to visible input.", "!".yellow());
            prompt_line(label)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::Agent;
    use crate::config::Config;

    /// Build a `Session` against the `local` provider (no network, no API
    /// key required) for command tests that don't need a real backend.
    fn test_session() -> Session {
        let dir = std::env::temp_dir().join(format!("rexo_test_cli_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("rexo.toml"),
            "[model]\nprovider = \"local\"\nmodel = \"test-model\"\nbase_url = \"http://127.0.0.1:1\"\n",
        )
        .unwrap();

        let config = Config::load(&dir).unwrap();
        let (agent, history) = Agent::bootstrap(&config, String::new(), true, true).unwrap();
        Session::new(config, agent, history)
    }

    #[test]
    fn dispatch_finds_command_by_alias() {
        let mut session = test_session();
        let outcome = dispatch(&mut session, "q", &[]).unwrap();
        assert_eq!(outcome, CommandOutcome::Exit);
    }

    #[test]
    fn dispatch_unknown_command_continues() {
        let mut session = test_session();
        let outcome = dispatch(&mut session, "nonsense", &[]).unwrap();
        assert_eq!(outcome, CommandOutcome::Continue);
    }

    #[test]
    fn model_command_updates_config_and_provider() {
        let mut session = test_session();
        model_cmd(&mut session, &["a-new-model".to_string()]).unwrap();
        assert_eq!(session.config.model.model.as_deref(), Some("a-new-model"));
        assert!(session.agent.provider_description().contains("a-new-model"));
    }

    #[test]
    fn provider_command_rejects_unknown_kind() {
        let mut session = test_session();
        let before = session.config.model.provider.clone();
        provider_cmd(&mut session, &["not-a-real-provider".to_string()]).unwrap();
        assert_eq!(session.config.model.provider, before);
    }

    #[test]
    fn permissions_command_round_trips() {
        let mut session = test_session();
        assert!(!session.agent.permissions().current_config().allow_edit);
        permissions_cmd(&mut session, &["edit".to_string(), "always".to_string()]).unwrap();
        assert!(session.agent.permissions().current_config().allow_edit);
        assert!(session.config.permissions.allow_edit);

        permissions_cmd(&mut session, &["edit".to_string(), "ask".to_string()]).unwrap();
        assert!(!session.agent.permissions().current_config().allow_edit);
    }

    #[test]
    fn clear_resets_history_to_single_system_message() {
        let mut session = test_session();
        session.history.push(crate::providers::ChatMessage::user("hello"));
        assert!(session.history.len() > 1);
        clear_cmd(&mut session, &[]).unwrap();
        assert_eq!(session.history.len(), 1);
        assert_eq!(session.history[0].role, crate::providers::Role::System);
    }

    #[test]
    fn workspace_change_updates_agent_and_clears_session_grants() {
        let mut session = test_session();
        session.agent.permissions_mut().set_auto_allow(PermissionKind::Terminal, false);
        // Simulate an "always" runtime grant, the kind /workspace should revoke.
        session
            .agent
            .permissions_mut()
            .check(PermissionKind::Terminal, "x", crate::security::policies::RiskLevel::Moderate)
            .ok();

        let new_dir = std::env::temp_dir().join(format!("rexo_test_cli_ws_{}", std::process::id()));
        std::fs::create_dir_all(&new_dir).unwrap();

        apply_workspace_change(&mut session, new_dir.clone()).unwrap();

        assert_eq!(session.agent.workspace(), new_dir.canonicalize().unwrap());
        assert!(!session.agent.permissions().is_session_granted(PermissionKind::Terminal));
    }

    #[test]
    fn workspace_relative_path_resolves_against_agent_workspace_not_process_cwd() {
        let mut session = test_session();
        let workspace = session.agent.workspace().to_path_buf();
        std::fs::create_dir_all(workspace.join("child")).unwrap();

        // A relative path must resolve against the agent's workspace, not
        // whatever directory this test binary happens to be running from.
        workspace_cmd(&mut session, &["child".to_string()]).unwrap();

        assert_eq!(session.agent.workspace(), workspace.join("child").canonicalize().unwrap());
    }

    #[test]
    fn workspace_change_rejects_being_lured_outside_via_a_file() {
        let mut session = test_session();
        let file = std::env::temp_dir().join(format!("rexo_test_cli_not_a_dir_{}", std::process::id()));
        std::fs::write(&file, "not a directory").unwrap();
        let before = session.agent.workspace().to_path_buf();

        apply_workspace_change(&mut session, file).unwrap();

        // Workspace must not have changed to a non-directory path.
        assert_eq!(session.agent.workspace(), before);
    }

    // --- secret redaction -------------------------------------------------

    #[test]
    fn status_never_contains_the_api_key_value() {
        std::env::set_var("LOCAL_API_KEY", "super-secret-value-should-not-leak");
        let mut session = test_session();
        session.api_key_override = Some("another-super-secret-override".to_string());

        let rendered = render_status(&session);
        assert!(!rendered.contains("super-secret-value-should-not-leak"));
        assert!(!rendered.contains("another-super-secret-override"));
        // test_session() uses provider = "local", which never needs a key.
        assert!(rendered.contains("not required"));

        std::env::remove_var("LOCAL_API_KEY");
        let _ = &mut session; // silence unused-mut if assertions above change
    }

    #[test]
    fn status_reports_configured_without_ever_printing_the_key() {
        let dir = std::env::temp_dir().join(format!("rexo_test_cli_openai_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("rexo.toml"),
            "[model]\nprovider = \"openai_compatible\"\nmodel = \"test-model\"\nbase_url = \"http://127.0.0.1:1/v1/chat/completions\"\n",
        )
        .unwrap();
        let config = Config::load(&dir).unwrap();
        let (agent, history) = Agent::bootstrap(&config, "sk-live-secret-do-not-print".to_string(), true, true).unwrap();
        let mut session = Session::new(config, agent, history);
        session.api_key_override = Some("sk-live-secret-do-not-print".to_string());

        let rendered = render_status(&session);
        assert!(rendered.contains("configured"));
        assert!(!rendered.contains("sk-live-secret-do-not-print"));
    }

    #[test]
    fn config_never_contains_the_api_key_value() {
        std::env::set_var("LOCAL_API_KEY", "super-secret-value-should-not-leak");
        let session = test_session();
        let rendered = render_config(&session);
        assert!(!rendered.contains("super-secret-value-should-not-leak"));
        std::env::remove_var("LOCAL_API_KEY");
    }

    #[test]
    fn help_output_never_mentions_hidden_workspace_select_by_name() {
        let mut session = test_session();
        // help_cmd only prints; assert on the spec table it draws from
        // instead of capturing stdout.
        assert!(COMMANDS.iter().find(|c| c.name == "workspace-select").unwrap().hidden);
        help_cmd(&mut session, &[]).unwrap();
    }

    #[test]
    fn provider_switch_failure_leaves_agent_honestly_unconfigured_not_silently_stale() {
        std::env::remove_var("NVIDIA_API_KEY");
        let mut session = test_session();
        let before = session.agent.provider_description();
        assert!(before.contains("local"));

        provider_cmd(&mut session, &["nvidia".to_string()]).unwrap();

        // Config says nvidia now...
        assert_eq!(session.config.model.provider, "nvidia");
        // ...and the *live* agent must agree it's trying nvidia and failing,
        // rather than silently still being the old "local" provider.
        let after = session.agent.provider_description();
        assert!(!after.contains("local"));
        assert!(after.to_lowercase().contains("not active"));
    }
}
