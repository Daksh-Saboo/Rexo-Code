//! The agent loop.
//!
//! ```text
//! user prompt
//!      │
//!      ▼
//! send conversation + tool schemas to the model
//!      │
//!      ├── final answer ───────────────────────► done
//!      │
//!      ▼
//! tool call(s)
//!      │
//!      ▼
//! required_permission() ── Denied ──► tool result: "denied", loop continues
//!      │
//!   Automatic / Ask+approved
//!      │
//!      ▼
//! execute tool, append tool result to history, go back to top
//! ```
//!
//! Bounded by `max_iterations` (model turns) and `max_tool_calls` (total
//! tool invocations across the whole run) from `rexo.toml`, so a
//! misbehaving model can't loop forever or hammer the filesystem/network.

pub mod context;
pub mod planner;
pub mod prompts;
pub mod subagent;

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, Mutex};

use anyhow::Result;
use colored::Colorize;

use crate::config::Config;
use crate::providers::{self, parse_tool_arguments, ChatMessage, ModelCapabilities, Provider, StreamEvent, ToolCall};
use crate::security::permissions::{Decision, PermissionKind, PermissionManager, PermissionPrompter};
use crate::security::policies::validate_workspace_path;
use crate::tools::{self, ToolContext, ToolPermission, ToolRegistry};

// Shadows std's `println!`/`print!` within this file with capture-aware
// versions (see `crate::output`) that behave identically unless a capture
// is active — used by `rexo --output json`, which captures this plain
// (non-TUI) streaming/tool-call/result output entirely rather than
// interleaving human-readable text with the final JSON line. Everywhere
// else (normal `rexo "task"` runs, interactive sessions' TUI-flavored
// methods below which never call these anyway) this is a no-op.
use crate::{tprint as print, tprintln as println};

pub use tui_support::{AgentUi, NoteLevel, StreamQueue};
mod tui_support;

// Phases of a single model turn's output, tracked so the streaming printer
// knows when to print a "Thinking…" header / transition out of it.
const PHASE_WAITING: u8 = 0;
const PHASE_REASONING: u8 = 1;
const PHASE_ANSWER: u8 = 2;

pub struct Agent {
    provider: Box<dyn Provider>,
    registry: ToolRegistry,
    permissions: PermissionManager,
    tool_ctx: ToolContext,
    max_iterations: usize,
    max_tool_calls: usize,
    /// The single most recent file change made by `edit_file`,
    /// `create_file`, or `delete_file` this session — see
    /// [`Agent::undo_last_file_change`]. Deliberately one level, not a
    /// stack: this is a quick "oops, revert that" for the file the model
    /// just touched, not the general session/workspace-checkpoint system
    /// (still on the roadmap) that would let you rewind several edits or
    /// survive a restart.
    last_file_change: Option<FileChangeRecord>,
    /// Shared with the `manage_tasks` tool this same list was registered
    /// with at construction — see [`Agent::tasks`] for how the TUI's
    /// Ctrl+T panel reads it.
    tasks: Arc<Mutex<tools::tasks::TaskList>>,
    /// Loaded once at construction from `.rexo/hooks.toml` — `/hooks
    /// add`/`remove` update both this and the file together, so this
    /// never drifts from what's on disk mid-session.
    hooks: Vec<crate::hooks::HookDef>,
    /// Live MCP connections, keyed by server name — populated by
    /// `/mcp connect` (and auto-connected `enabled` servers at
    /// startup), never by direct construction. Each connected server's
    /// discovered tools are already registered into `registry` by the
    /// time they land here; this map exists so `/mcp status`/`disconnect`
    /// have something to report on and act on afterward.
    mcp_clients: std::collections::HashMap<String, crate::mcp::McpClient>,
    /// The running `/ide` server, if `/ide start` has been run this
    /// session — see [`Agent::start_ide`]/[`Agent::stop_ide`].
    ide_server: Option<crate::ide::IdeServer>,
}

/// What a file looked like immediately before a mutating tool call, so
/// [`Agent::undo_last_file_change`] can put it back.
#[derive(Debug, Clone)]
enum PriorFileContent {
    Existed(String),
    DidNotExist,
}

#[derive(Debug, Clone)]
struct FileChangeRecord {
    /// Resolved, validated path on disk.
    resolved: PathBuf,
    /// As the model gave it, for user-facing messages.
    display_path: String,
    prior: PriorFileContent,
}

impl Agent {
    /// Wire together the provider, tool registry, permission manager, and
    /// initial (system-prompt-only) conversation history from config.
    pub fn bootstrap(
        config: &Config,
        api_key: String,
        non_interactive: bool,
        auto_approve: bool,
    ) -> Result<(Self, Vec<ChatMessage>)> {
        let provider = providers::build_provider(config, api_key)?;
        Ok(assemble_agent(config, provider, non_interactive, auto_approve))
    }

    /// Build an Agent with a stand-in provider that fails loudly and
    /// actionably the moment it's actually used, instead of failing to
    /// start at all. Used at startup when no working provider could be
    /// resolved (see `main.rs`) — the same graceful-degradation pattern
    /// `cli::Session::rebuild_provider` already uses for *runtime*
    /// provider changes, just applied to the very first one too, so an
    /// interactive `rexo` always reaches the TUI, where `/connect` can
    /// fix things up. Infallible: the only fallible step in `bootstrap`
    /// is building the real provider, which this skips entirely.
    /// Build an Agent with a stand-in provider that fails loudly and
    /// actionably the moment it's actually used, instead of failing to
    /// start at all. Used at startup when no working provider could be
    /// resolved (see `main.rs`) — the same graceful-degradation pattern
    /// `cli::Session::rebuild_provider` already uses for *runtime*
    /// provider changes, just applied to the very first one too, so an
    /// interactive `rexo` always reaches the TUI, where `/connect` can
    /// fix things up. Infallible: the only fallible step in `bootstrap`
    /// is building the real provider, which this skips entirely.
    pub fn bootstrap_unconfigured(config: &Config, reason: String, non_interactive: bool, auto_approve: bool) -> Result<(Self, Vec<ChatMessage>)> {
        let provider: Box<dyn Provider> = Box::new(providers::UnconfiguredProvider::new(reason));
        Ok(assemble_agent(config, provider, non_interactive, auto_approve))
    }

    pub fn provider_description(&self) -> String {
        self.provider.describe()
    }

    /// What the active provider/model is known to support — see
    /// [`ModelCapabilities`] for why most fields default to "unknown"
    /// rather than a guessed `false`.
    pub fn provider_capabilities(&self) -> ModelCapabilities {
        self.provider.capabilities()
    }

    /// Swap the active provider in place (e.g. after `/provider`, `/model`,
    /// `/base-url`, or `/api set` changes the effective configuration).
    /// Takes effect starting with the next request; conversation history is
    /// untouched.
    pub fn set_provider(&mut self, provider: Box<dyn Provider>) {
        self.provider = provider;
    }

    /// Change the workspace boundary every filesystem/git tool enforces.
    /// Does *not* touch conversation history — callers that want the
    /// system prompt's project context refreshed should also call
    /// [`Agent::build_system_message`] and replace it themselves.
    pub fn set_workspace(&mut self, workspace: std::path::PathBuf) {
        self.tool_ctx.workspace = workspace;
        self.tool_ctx.extra_read_roots.clear();
    }

    pub fn workspace(&self) -> &Path {
        &self.tool_ctx.workspace
    }

    /// Read-only additional roots added via `/add-dir` — see
    /// [`ToolContext::extra_read_roots`] for exactly what these do and
    /// don't grant. Switching workspaces (`/workspace`, `/cd`) clears
    /// these, same as it clears "always this session" permission grants,
    /// so trust in one project's extra directories doesn't silently
    /// follow you into an unrelated one opened in the same run.
    pub fn read_roots(&self) -> &[std::path::PathBuf] {
        &self.tool_ctx.extra_read_roots
    }

    /// Add a directory read-only tools may also resolve *absolute* paths
    /// into. Rejects a directory that doesn't exist, isn't a directory,
    /// or is already the workspace/an existing root.
    pub fn add_read_root(&mut self, path: std::path::PathBuf) -> Result<std::path::PathBuf> {
        let canonical = path
            .canonicalize()
            .map_err(|e| anyhow::anyhow!("Can't add '{}': {e}", path.display()))?;
        if !canonical.is_dir() {
            anyhow::bail!("'{}' isn't a directory.", canonical.display());
        }
        if canonical == self.tool_ctx.workspace {
            anyhow::bail!("'{}' is already the current workspace.", canonical.display());
        }
        if self.tool_ctx.extra_read_roots.contains(&canonical) {
            anyhow::bail!("'{}' is already added.", canonical.display());
        }
        self.tool_ctx.extra_read_roots.push(canonical.clone());
        Ok(canonical)
    }

    pub fn remove_read_root(&mut self, path: &std::path::Path) -> bool {
        let canonical = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
        let before = self.tool_ctx.extra_read_roots.len();
        self.tool_ctx.extra_read_roots.retain(|p| p != &canonical);
        self.tool_ctx.extra_read_roots.len() != before
    }

    pub fn clear_read_roots(&mut self) {
        self.tool_ctx.extra_read_roots.clear();
    }

    pub fn permissions(&self) -> &PermissionManager {
        &self.permissions
    }

    pub fn permissions_mut(&mut self) -> &mut PermissionManager {
        &mut self.permissions
    }

    /// Replace the permission manager wholesale — used by `/reset` to
    /// return session-level permission overrides to configuration defaults.
    pub fn set_permissions(&mut self, permissions: PermissionManager) {
        self.permissions = permissions;
    }

    pub fn max_iterations(&self) -> usize {
        self.max_iterations
    }

    pub fn max_tool_calls(&self) -> usize {
        self.max_tool_calls
    }

    /// Build a fresh system message for the agent's *current* workspace —
    /// used to reset/refresh conversation history (`/clear`, `/reset`, or
    /// after a `/workspace` change) without re-running full bootstrap.
    pub fn build_system_message(&self) -> ChatMessage {
        ChatMessage::system(system_prompt(&self.tool_ctx.workspace))
    }

    /// Push a user message and run the loop until the model gives a final
    /// answer or a safety limit is hit. Returns that final answer.
    pub async fn respond(&mut self, history: &mut Vec<ChatMessage>, user_message: &str) -> Result<String> {
        history.push(ChatMessage::user(user_message));
        self.agent_loop(history).await
    }

    async fn agent_loop(&mut self, history: &mut Vec<ChatMessage>) -> Result<String> {
        let tool_defs = self.registry.tool_definitions();
        let mut tool_call_count = 0usize;

        for _iteration in 0..self.max_iterations {
            let result = match self.stream_turn(history, &tool_defs).await? {
                Some(r) => r,
                None => {
                    // Ctrl+C during this turn.
                    let msg = "Cancelled by user.".to_string();
                    return Ok(msg);
                }
            };

            if result.tool_calls.is_empty() {
                let content = result.content.clone().unwrap_or_default();
                history.push(ChatMessage::assistant(result.content, result.reasoning, Vec::new()));
                println!();
                return Ok(content);
            }

            history.push(ChatMessage::assistant(
                result.content.clone(),
                result.reasoning.clone(),
                result.tool_calls.clone(),
            ));

            for call in &result.tool_calls {
                tool_call_count += 1;
                if tool_call_count > self.max_tool_calls {
                    let msg = format!(
                        "Tool call budget exceeded ({} calls this turn). Stopping for safety — \
                         narrow the task or raise [agent].max_tool_calls in rexo.toml.",
                        self.max_tool_calls
                    );
                    println!("{}", msg.red().bold());
                    history.push(ChatMessage::tool_result(call.id.clone(), call.name.clone(), msg.clone()));
                    return Ok(msg);
                }

                let outcome = self.execute_tool_call(call).await;
                history.push(ChatMessage::tool_result(call.id.clone(), call.name.clone(), outcome));
            }
        }

        let msg = format!(
            "Reached the maximum of {} model turns without a final answer. Stopping for safety — \
             the task may be too large for one run, or raise [agent].max_iterations in rexo.toml.",
            self.max_iterations
        );
        println!("{}", msg.yellow().bold());
        Ok(msg)
    }

    /// Send one turn to the model and stream the result to the console,
    /// racing it against Ctrl+C so an impatient/mistaken interrupt exits
    /// cleanly instead of the OS killing the whole process. Shows an
    /// animated spinner with an elapsed timer until the first token
    /// arrives, since reasoning models can take a real while to produce
    /// one. Returns `Ok(None)` if the user cancelled.
    async fn stream_turn(
        &mut self,
        history: &[ChatMessage],
        tool_defs: &[crate::tools::ToolDefinition],
    ) -> Result<Option<providers::ChatResult>> {
        // Shared, atomically-updated phase so the `Fn` streaming callback
        // (which can't capture `&mut` state) can still track whether it's
        // printing the first token of the turn, mid-"thinking", or mid-answer.
        let phase = Arc::new(AtomicU8::new(PHASE_WAITING));

        let printer_phase = phase.clone();
        let printer = move |event: StreamEvent| {
            let mut out = std::io::stdout();
            match event {
                StreamEvent::ReasoningDelta(text) => {
                    let prev = printer_phase.swap(PHASE_REASONING, Ordering::Relaxed);
                    if prev == PHASE_WAITING {
                        clear_line();
                        println!("{}", "✻ Thinking…".italic().truecolor(140, 140, 140));
                    }
                    print!("{}", text.truecolor(140, 140, 140));
                    let _ = out.flush();
                }
                StreamEvent::TextDelta(text) => {
                    let prev = printer_phase.swap(PHASE_ANSWER, Ordering::Relaxed);
                    if prev == PHASE_WAITING {
                        clear_line();
                    } else if prev == PHASE_REASONING {
                        println!();
                        println!();
                    }
                    print!("{text}");
                    let _ = out.flush();
                }
                StreamEvent::ToolCallStarted { .. } => {
                    let prev = printer_phase.swap(PHASE_ANSWER, Ordering::Relaxed);
                    if prev == PHASE_WAITING {
                        clear_line();
                    } else if prev == PHASE_REASONING {
                        println!();
                        println!();
                    }
                }
            }
        };

        // Everything below runs on this one task, polled by the same
        // select! loop, so the spinner tick and the streaming printer above
        // can never interleave/race — only one branch's body ever runs at
        // a time.
        tokio::pin! {
            let chat_future = self.provider.chat(history, tool_defs, Some(&printer));
        }
        let mut ticker = tokio::time::interval(std::time::Duration::from_millis(90));
        let start = std::time::Instant::now();
        const SPINNER_FRAMES: [char; 10] = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];
        let mut frame = 0usize;

        loop {
            tokio::select! {
                result = &mut chat_future => {
                    if phase.load(Ordering::Relaxed) == PHASE_WAITING {
                        clear_line();
                    }
                    return result.map(Some);
                }
                _ = ticker.tick() => {
                    if phase.load(Ordering::Relaxed) == PHASE_WAITING {
                        let secs = start.elapsed().as_secs();
                        let glyph = SPINNER_FRAMES[frame % SPINNER_FRAMES.len()];
                        frame += 1;
                        print!(
                            "\r{} {}",
                            glyph.to_string().truecolor(180, 140, 255).bold(),
                            format!("Thinking… ({secs}s · Ctrl+C to cancel)").dimmed()
                        );
                        std::io::stdout().flush().ok();
                    }
                }
                _ = tokio::signal::ctrl_c() => {
                    clear_line();
                    println!("{}", "Cancelled.".yellow());
                    return Ok(None);
                }
            }
        }
    }

    async fn execute_tool_call(&mut self, call: &ToolCall) -> String {
        let args = parse_tool_arguments(&call.arguments);

        println!();
        println!("{} {}", "⏺".cyan().bold(), friendly_call(&call.name, &args).bold());

        let tool = match self.registry.require(&call.name) {
            Ok(t) => t,
            Err(e) => {
                print_result_line(&format!("unknown tool: {e}"), true);
                return e.to_string();
            }
        };

        match tool.required_permission(&self.tool_ctx, &args) {
            ToolPermission::Automatic => {}
            ToolPermission::Denied { reason } => {
                print_result_line(&format!("blocked: {reason}"), true);
                return format!("Blocked by policy: {reason}");
            }
            ToolPermission::Ask { summary, risk } => {
                let kind = permission_kind_for(&call.name);
                match self.permissions.check(kind, &summary, risk) {
                    Ok(Decision::Allowed) => {}
                    Ok(Decision::Denied) => {
                        print_result_line("denied by user", true);
                        return format!(
                            "Denied by user: {summary}. Do not retry this exact action; explain \
                             the situation to the user or propose an alternative."
                        );
                    }
                    Err(e) => {
                        print_result_line(&format!("permission check failed: {e}"), true);
                        return format!("Permission check failed: {e}");
                    }
                }
            }
        }

        if let Err(reason) = self.run_pre_tool_hooks(&call.name, &args) {
            print_result_line(&format!("blocked by hook: {reason}"), true);
            return format!("Blocked by a pre_tool hook: {reason}");
        }

        let result = match tool.execute(&self.tool_ctx, args.clone()).await {
            Ok(output) => {
                print_tool_result(&call.name, &output);
                output
            }
            Err(e) => {
                print_result_line(&format!("error: {e}"), true);
                format!("Error: {e}")
            }
        };
        self.run_post_tool_hooks(&call.name, &args, &result);
        result
    }

    // ---- TUI counterparts -------------------------------------------
    //
    // Same control flow as `respond`/`agent_loop`/`stream_turn`/
    // `execute_tool_call` above, kept as separate methods rather than a
    // single generic-output implementation: the plain versions are
    // heavily exercised by the existing test suite and by single-shot
    // (`rexo "..."` piped/scripted) runs that never touch a real
    // terminal, and duplicating a ~150-line loop was a smaller risk than
    // threading a generic sink through code that's this order-sensitive
    // (spinner state, phase transitions, Ctrl+C races).

    /// TUI counterpart of [`respond`](Self::respond): identical control
    /// flow, but everything a human would see goes through `ui` (in
    /// practice, a `cli::tui::TuiCore`) instead of directly to stdout, and
    /// permission prompts are a redraw-safe modal instead of a blocking
    /// `stdin` read that doesn't work while the terminal's in raw mode.
    /// `images` is almost always empty — only non-empty when Alt+V has
    /// pasted something onto `ui.pending_images` since the last message
    /// (see the `KeyCode::Char('v')` (Alt) handler in `read_input`, and
    /// `run_turn`/`run_aside`, which both drain that slot right before
    /// calling this).
    pub async fn respond_tui<U>(
        &mut self,
        history: &mut Vec<ChatMessage>,
        user_message: &str,
        images: Vec<crate::providers::ImageAttachment>,
        ui: &mut U,
    ) -> Result<String>
    where
        U: AgentUi + PermissionPrompter,
    {
        history.push(ChatMessage::user_with_images(user_message, images));
        self.agent_loop_tui(history, ui).await
    }

    async fn agent_loop_tui<U>(&mut self, history: &mut Vec<ChatMessage>, ui: &mut U) -> Result<String>
    where
        U: AgentUi + PermissionPrompter,
    {
        let tool_defs = self.registry.tool_definitions();
        let mut tool_call_count = 0usize;

        for _iteration in 0..self.max_iterations {
            let result = match self.stream_turn_tui(history, &tool_defs, ui).await? {
                Some(r) => r,
                None => return Ok("Cancelled by user.".to_string()),
            };

            if result.tool_calls.is_empty() {
                let content = result.content.clone().unwrap_or_default();
                history.push(ChatMessage::assistant(result.content, result.reasoning, Vec::new()));
                ui.turn_finished();
                ui.redraw();
                return Ok(content);
            }

            history.push(ChatMessage::assistant(
                result.content.clone(),
                result.reasoning.clone(),
                result.tool_calls.clone(),
            ));

            for call in &result.tool_calls {
                tool_call_count += 1;
                if tool_call_count > self.max_tool_calls {
                    let msg = format!(
                        "Tool call budget exceeded ({} calls this turn). Stopping for safety — \
                         narrow the task or raise [agent].max_tool_calls in rexo.toml.",
                        self.max_tool_calls
                    );
                    ui.note(&msg, NoteLevel::Error);
                    ui.redraw();
                    history.push(ChatMessage::tool_result(call.id.clone(), call.name.clone(), msg.clone()));
                    return Ok(msg);
                }

                let outcome = self.execute_tool_call_tui(call, ui).await;
                history.push(ChatMessage::tool_result(call.id.clone(), call.name.clone(), outcome));
            }
        }

        let msg = format!(
            "Reached the maximum of {} model turns without a final answer. Stopping for safety — \
             the task may be too large for one run, or raise [agent].max_iterations in rexo.toml.",
            self.max_iterations
        );
        ui.note(&msg, NoteLevel::Warning);
        ui.redraw();
        Ok(msg)
    }

    /// TUI counterpart of [`stream_turn`](Self::stream_turn). The
    /// streaming callback can't reach `ui` directly (it has to be
    /// `Send + Sync`; see [`StreamQueue`]'s docs) — instead it queues raw
    /// events, and this drains that queue on the same ~90ms tick that
    /// used to just drive the spinner, applying each event to `ui` in
    /// order so phase transitions (waiting → reasoning → answer) still
    /// happen in the right place even though nothing's printed live
    /// token-by-token the way stdout was.
    async fn stream_turn_tui<U>(
        &mut self,
        history: &[ChatMessage],
        tool_defs: &[crate::tools::ToolDefinition],
        ui: &mut U,
    ) -> Result<Option<providers::ChatResult>>
    where
        U: AgentUi + PermissionPrompter,
    {
        let queue: StreamQueue = Arc::new(Mutex::new(Vec::new()));
        let printer_queue = queue.clone();
        let printer = move |event: StreamEvent| {
            if let Ok(mut q) = printer_queue.lock() {
                q.push(event);
            }
        };

        tokio::pin! {
            let chat_future = self.provider.chat(history, tool_defs, Some(&printer));
        }
        let mut ticker = tokio::time::interval(std::time::Duration::from_millis(90));
        let start = std::time::Instant::now();
        let mut phase: u8 = PHASE_WAITING;
        ui.spinner_start();
        ui.redraw();

        loop {
            tokio::select! {
                result = &mut chat_future => {
                    drain_queue_tui(&queue, ui, &mut phase);
                    if phase == PHASE_WAITING {
                        ui.spinner_stop();
                    }
                    ui.redraw();
                    return result.map(Some);
                }
                _ = ticker.tick() => {
                    drain_queue_tui(&queue, ui, &mut phase);
                    if phase == PHASE_WAITING {
                        ui.spinner_tick(start.elapsed().as_secs());
                    }
                    ui.redraw();
                }
                _ = ui.wait_for_cancel() => {
                    ui.spinner_stop();
                    ui.note("Cancelled.", NoteLevel::Warning);
                    ui.redraw();
                    return Ok(None);
                }
            }
        }
    }

    async fn execute_tool_call_tui<U>(&mut self, call: &ToolCall, ui: &mut U) -> String
    where
        U: AgentUi + PermissionPrompter,
    {
        let args = parse_tool_arguments(&call.arguments);
        ui.tool_call_started(&friendly_call(&call.name, &args));
        ui.redraw();

        let tool = match self.registry.require(&call.name) {
            Ok(t) => t,
            Err(e) => {
                ui.tool_result_line(&format!("unknown tool: {e}"), true);
                ui.redraw();
                return e.to_string();
            }
        };

        match tool.required_permission(&self.tool_ctx, &args) {
            ToolPermission::Automatic => {}
            ToolPermission::Denied { reason } => {
                ui.tool_result_line(&format!("blocked: {reason}"), true);
                ui.redraw();
                return format!("Blocked by policy: {reason}");
            }
            ToolPermission::Ask { summary, risk } => {
                let kind = permission_kind_for(&call.name);
                match self.permissions.check_interactive(kind, &summary, risk, ui).await {
                    Ok(Decision::Allowed) => {}
                    Ok(Decision::Denied) => {
                        ui.tool_result_line("denied by user", true);
                        ui.redraw();
                        return format!(
                            "Denied by user: {summary}. Do not retry this exact action; explain \
                             the situation to the user or propose an alternative."
                        );
                    }
                    Err(e) => {
                        ui.tool_result_line(&format!("permission check failed: {e}"), true);
                        ui.redraw();
                        return format!("Permission check failed: {e}");
                    }
                }
            }
        }

        if let Err(reason) = self.run_pre_tool_hooks(&call.name, &args) {
            ui.tool_result_line(&format!("blocked by hook: {reason}"), true);
            ui.redraw();
            return format!("Blocked by a pre_tool hook: {reason}");
        }

        let mutating = matches!(call.name.as_str(), "edit_file" | "create_file" | "delete_file");
        let pre_snapshot = if mutating { self.snapshot_before_mutation(&args) } else { None };

        let outcome = match tool.execute(&self.tool_ctx, args.clone()).await {
            Ok(output) => {
                apply_tool_result_tui(ui, &call.name, &output);
                output
            }
            Err(e) => {
                ui.tool_result_line(&format!("error: {e}"), true);
                format!("Error: {e}")
            }
        };
        if let Some((resolved, display_path, prior)) = pre_snapshot {
            self.record_if_changed(resolved, display_path, prior);
        }
        self.run_post_tool_hooks(&call.name, &args, &outcome);
        ui.redraw();
        outcome
    }

    /// Read a mutating tool call's target file *before* it runs, so
    /// [`Self::record_if_changed`] has something to compare against
    /// afterward. Returns `None` (silently skipping the undo record —
    /// not the tool call itself) when the path is missing/invalid, or
    /// when an existing file can't be read as UTF-8 text: this undo
    /// slot only ever holds text it can faithfully restore, never a
    /// best-effort/binary-unsafe guess.
    fn snapshot_before_mutation(&self, args: &serde_json::Value) -> Option<(PathBuf, String, PriorFileContent)> {
        let display_path = args.get("path").and_then(serde_json::Value::as_str)?.to_string();
        let resolved = validate_workspace_path(&self.tool_ctx.workspace, &display_path).ok()?;
        let prior = if resolved.exists() {
            match std::fs::read_to_string(&resolved) {
                Ok(text) => PriorFileContent::Existed(text),
                Err(_) => return None, // binary or otherwise unreadable as text — don't track it
            }
        } else {
            PriorFileContent::DidNotExist
        };
        Some((resolved, display_path, prior))
    }

    /// Compare a mutating tool call's target file against the snapshot
    /// [`Self::snapshot_before_mutation`] took, and record it as the new
    /// undo slot only if the file actually changed. `edit_file` and
    /// `create_file` fail "softly" (an `Ok(String)` explaining why,
    /// rather than an `Err`) when the model's call doesn't make sense —
    /// comparing before/after filesystem state directly, rather than
    /// trying to parse those messages, means a failed call never
    /// clobbers a real earlier change still waiting to be undone.
    fn record_if_changed(&mut self, resolved: PathBuf, display_path: String, prior: PriorFileContent) {
        let changed = match &prior {
            PriorFileContent::Existed(old) => !resolved.exists() || std::fs::read_to_string(&resolved).map(|new| &new != old).unwrap_or(false),
            PriorFileContent::DidNotExist => resolved.exists(),
        };
        if changed {
            self.last_file_change = Some(FileChangeRecord { resolved, display_path, prior });
        }
    }

    /// Ctrl+Shift+_ (and `/undo`): revert the single most recent file
    /// change this session — the last successful `edit_file`,
    /// `create_file`, or `delete_file` call. One level only, and
    /// conversation-independent (mirrors `/rewind` being file-
    /// independent the other way around): this is a quick "put that
    /// back," not the general workspace-checkpoint/session-snapshot
    /// system still on the roadmap.
    pub fn undo_last_file_change(&mut self) -> std::result::Result<String, String> {
        let Some(change) = self.last_file_change.take() else {
            return Err("Nothing to undo — no file change recorded yet this session.".to_string());
        };
        match change.prior {
            PriorFileContent::Existed(old) => {
                std::fs::write(&change.resolved, old).map_err(|e| format!("Undo failed: couldn't restore {}: {e}", change.display_path))?;
                Ok(format!("Reverted {} to its previous content.", change.display_path))
            }
            PriorFileContent::DidNotExist => {
                std::fs::remove_file(&change.resolved).map_err(|e| format!("Undo failed: couldn't remove {}: {e}", change.display_path))?;
                Ok(format!("Removed {} (undoing its creation).", change.display_path))
            }
        }
    }

    /// Whether there's currently a file change `/undo`/Ctrl+Shift+_
    /// would act on — used by `/status` and the undo key handler's
    /// nothing-to-do case.
    pub fn has_pending_undo(&self) -> bool {
        self.last_file_change.is_some()
    }

    /// A clone of the shared handle the `manage_tasks` tool writes
    /// through — the TUI's Ctrl+T panel reads the same live state the
    /// model sees and edits, not a snapshot that can drift from it.
    pub fn tasks(&self) -> Arc<Mutex<tools::tasks::TaskList>> {
        self.tasks.clone()
    }

    pub fn hooks(&self) -> &[crate::hooks::HookDef] {
        &self.hooks
    }

    /// Actually connect to an MCP server — spawn it, complete the MCP
    /// handshake, discover its tools, and register each one into this
    /// agent's tool registry as `mcp__<name>__<tool>` (see
    /// [`crate::tools::mcp_proxy::McpProxyTool`]). Returns the number of
    /// tools registered. Connecting to a name that's already connected
    /// replaces the old connection (its tools stay registered under the
    /// same names, now backed by the new connection).
    pub async fn connect_mcp_server(&mut self, name: &str, command: &str) -> Result<usize> {
        let client = crate::mcp::McpClient::connect(name, command).await?;
        let tools = client.list_tools().await?;
        let count = tools.len();
        for info in tools {
            self.registry.register(Arc::new(tools::mcp_proxy::McpProxyTool::new(client.clone(), name, info)));
        }
        self.mcp_clients.insert(name.to_string(), client);
        Ok(count)
    }

    pub fn mcp_server_names(&self) -> Vec<String> {
        self.mcp_clients.keys().cloned().collect()
    }

    pub fn is_mcp_connected(&self, name: &str) -> bool {
        self.mcp_clients.contains_key(name)
    }

    /// Removes the connection from tracking *and* kills the child
    /// process directly — relying on `kill_on_drop` alone wouldn't be
    /// enough here, since every already-registered proxy tool for this
    /// server holds its own clone of the same `McpClient` (and so the
    /// same `Arc<Mutex<Child>>`), which would otherwise keep the process
    /// alive for the rest of the session. Those proxy tools stay
    /// registered — calling one after this just gets a connection-closed
    /// error back from `McpClient::call_tool` rather than disappearing
    /// from the model's tool list mid-conversation.
    pub fn disconnect_mcp_server(&mut self, name: &str) -> bool {
        let Some(client) = self.mcp_clients.remove(name) else {
            return false;
        };
        if let Ok(mut child) = client.child_handle().try_lock() {
            let _ = child.start_kill();
        }
        true
    }

    /// `/ide start` — see `crate::ide` for the protocol and what this
    /// server does and doesn't do (no real editor extension exists yet
    /// to connect to it; this is the honestly-tested REXO-side half).
    pub async fn start_ide(&mut self) -> Result<u16> {
        if let Some(existing) = &self.ide_server {
            return Ok(existing.port);
        }
        let server = crate::ide::start(&self.tool_ctx.workspace).await?;
        let port = server.port;
        self.ide_server = Some(server);
        Ok(port)
    }

    pub fn stop_ide(&mut self) -> bool {
        match self.ide_server.take() {
            Some(server) => {
                crate::ide::stop(server);
                true
            }
            None => false,
        }
    }

    pub fn ide_status(&self) -> Option<(u16, crate::ide::IdeContext)> {
        self.ide_server.as_ref().map(|s| (s.port, s.current_context()))
    }

    /// `/hooks add`/`remove` write straight to `.rexo/hooks.toml`
    /// themselves (via `crate::hooks::save`) — this just re-reads it
    /// back so this session's in-memory copy matches immediately,
    /// rather than only taking effect on the next launch.
    pub fn reload_hooks(&mut self) {
        self.hooks = crate::hooks::load(&self.tool_ctx.workspace);
    }

    /// `pre_tool`/`post_tool` around one tool call. Returns `Err(reason)`
    /// when a hook exit-2-blocked it — the caller should skip the tool
    /// call entirely and surface `reason` the same way a denied
    /// permission is surfaced. Any `Warn` outcomes are printed directly
    /// (both call sites already have a `println!`-based note mechanism
    /// in scope) rather than threaded back through the return value —
    /// they never change control flow, only what's visible.
    fn run_pre_tool_hooks(&self, tool_name: &str, args: &serde_json::Value) -> std::result::Result<(), String> {
        for outcome in crate::hooks::run(&self.hooks, &self.tool_ctx.workspace, "pre_tool", Some(tool_name), Some(args), None) {
            match outcome {
                crate::hooks::HookOutcome::Block(reason) => return Err(reason),
                crate::hooks::HookOutcome::Warn(msg) => println!("{} {msg}", "!".yellow()),
                crate::hooks::HookOutcome::Ok(Some(note)) => println!("{} {note}", "hook:".dimmed()),
                crate::hooks::HookOutcome::Ok(None) => {}
            }
        }
        Ok(())
    }

    fn run_post_tool_hooks(&self, tool_name: &str, args: &serde_json::Value, output: &str) {
        for outcome in crate::hooks::run(&self.hooks, &self.tool_ctx.workspace, "post_tool", Some(tool_name), Some(args), Some(output)) {
            match outcome {
                crate::hooks::HookOutcome::Block(_) => {} // post_tool never blocks — see module docs
                crate::hooks::HookOutcome::Warn(msg) => println!("{} {msg}", "!".yellow()),
                crate::hooks::HookOutcome::Ok(Some(note)) => println!("{} {note}", "hook:".dimmed()),
                crate::hooks::HookOutcome::Ok(None) => {}
            }
        }
    }

    /// Run once, right after the TUI opens (or right before a headless
    /// single-shot run starts). No matcher, informational only — stdout
    /// is returned so the TUI can show it as a `note` (its own display
    /// mechanism) instead of this going through the `println!`-based
    /// macro that assumes a plain scrolling terminal.
    pub fn run_session_start_hooks(&self) -> Vec<String> {
        crate::hooks::run(&self.hooks, &self.tool_ctx.workspace, "session_start", None, None, None)
            .into_iter()
            .filter_map(|o| match o {
                crate::hooks::HookOutcome::Ok(Some(note)) => Some(note),
                crate::hooks::HookOutcome::Warn(msg) => Some(format!("(hook warning) {msg}")),
                _ => None,
            })
            .collect()
    }

    /// Same as [`Self::run_session_start_hooks`], for `session_end` —
    /// run right before the TUI exits / right after a headless
    /// single-shot run finishes.
    pub fn run_session_end_hooks(&self) -> Vec<String> {
        crate::hooks::run(&self.hooks, &self.tool_ctx.workspace, "session_end", None, None, None)
            .into_iter()
            .filter_map(|o| match o {
                crate::hooks::HookOutcome::Ok(Some(note)) => Some(note),
                crate::hooks::HookOutcome::Warn(msg) => Some(format!("(hook warning) {msg}")),
                _ => None,
            })
            .collect()
    }
}

fn drain_queue_tui<U: AgentUi>(queue: &StreamQueue, ui: &mut U, phase: &mut u8) {
    let drained: Vec<StreamEvent> = match queue.lock() {
        Ok(mut q) => std::mem::take(&mut *q),
        Err(_) => Vec::new(),
    };
    for event in drained {
        match event {
            StreamEvent::ReasoningDelta(text) => {
                let prev = *phase;
                *phase = PHASE_REASONING;
                if prev == PHASE_WAITING {
                    ui.spinner_stop();
                    ui.reasoning_started();
                }
                ui.reasoning_delta(&text);
            }
            StreamEvent::TextDelta(text) => {
                let prev = *phase;
                *phase = PHASE_ANSWER;
                if prev == PHASE_WAITING {
                    ui.spinner_stop();
                }
                ui.text_delta(&text);
            }
            StreamEvent::ToolCallStarted { name } => {
                let prev = *phase;
                *phase = PHASE_ANSWER;
                if prev == PHASE_WAITING {
                    ui.spinner_stop();
                }
                ui.tool_preparing(&name);
            }
        }
    }
}

/// TUI counterpart of [`print_tool_result`]: same dispatch to a compact
/// one-line summary for the read-only tools vs. a truncated block for
/// everything else, just routed through `ui` instead of `println!`.
fn apply_tool_result_tui<U: AgentUi>(ui: &mut U, tool_name: &str, output: &str) {
    match tool_name {
        "read_file" if !output.starts_with("File not found") && !output.contains("is a directory") => {
            ui.tool_result_line(&pluralize(output.lines().count(), "Read", "line"), false);
        }
        "list_files" if output != "(empty directory)" && !output.starts_with("Directory not found") => {
            ui.tool_result_line(&pluralize(output.lines().count(), "Listed", "item"), false);
        }
        "search_files" if output.starts_with("No matches") => {
            ui.tool_result_line("No matches", false);
        }
        "search_files" => {
            let n = output.lines().count();
            let noun = if n == 1 { "match" } else { "matches" };
            ui.tool_result_line(&format!("Found {n} {noun}"), false);
        }
        _ => ui.tool_result_block(output),
    }
}

fn permission_kind_for(tool_name: &str) -> PermissionKind {
    match tool_name {
        "edit_file" => PermissionKind::Edit,
        "create_file" => PermissionKind::CreateFile,
        "delete_file" => PermissionKind::DeleteFile,
        "git" => PermissionKind::GitWrite,
        _ => PermissionKind::Terminal,
    }
}

/// Clear whatever's on the current line (spinner or waiting text) before
/// real output starts, without leaving stray characters behind.
fn clear_line() {
    print!("\r{}\r", " ".repeat(60));
    std::io::stdout().flush().ok();
}

fn str_arg(args: &serde_json::Value, key: &str) -> String {
    let raw = args.get(key).and_then(serde_json::Value::as_str).unwrap_or("?");
    let capped: String = raw.chars().take(80).collect();
    if raw.chars().count() > 80 {
        format!("{capped}…")
    } else {
        capped
    }
}

/// Claude-Code-style one-line call summary, e.g. `Read(src/main.rs)` or
/// `Bash(cargo test)`, instead of the raw tool name + JSON arguments.
fn friendly_call(tool_name: &str, args: &serde_json::Value) -> String {
    match tool_name {
        "read_file" => format!("Read({})", str_arg(args, "path")),
        "list_files" => {
            let path = str_arg(args, "path");
            let path = if path == "?" { ".".to_string() } else { path };
            let recursive = args.get("recursive").and_then(serde_json::Value::as_bool).unwrap_or(false);
            if recursive {
                format!("List({path}, recursive)")
            } else {
                format!("List({path})")
            }
        }
        "search_files" => format!("Search(\"{}\")", str_arg(args, "query")),
        "edit_file" => format!("Edit({})", str_arg(args, "path")),
        "create_file" => format!("Create({})", str_arg(args, "path")),
        "delete_file" => format!("Delete({})", str_arg(args, "path")),
        "run_command" => format!("Bash({})", str_arg(args, "command")),
        "git" => format!("Git({})", str_arg(args, "subcommand")),
        other => {
            let rendered = serde_json::to_string(args).unwrap_or_default();
            let capped: String = rendered.chars().take(100).collect();
            format!("{other}({capped})")
        }
    }
}

/// Claude-Code-style single-line tool result, e.g. `  ⎿  error: ...`.
fn print_result_line(line: &str, is_error: bool) {
    let marker = "⎿".dimmed();
    if is_error {
        println!("  {marker} {}", line.red());
    } else {
        println!("  {marker} {}", line.dimmed());
    }
}

/// Dispatches to a compact one-line summary for the read-only tools
/// (matching Claude Code's "Read 45 lines" style) and a truncated raw
/// block for everything else, where the actual content/output matters.
fn print_tool_result(tool_name: &str, output: &str) {
    match tool_name {
        "read_file" if !output.starts_with("File not found") && !output.contains("is a directory") => {
            print_result_line(&pluralize(output.lines().count(), "Read", "line"), false);
        }
        "list_files" if output != "(empty directory)" && !output.starts_with("Directory not found") => {
            print_result_line(&pluralize(output.lines().count(), "Listed", "item"), false);
        }
        "search_files" if output.starts_with("No matches") => {
            print_result_line("No matches", false);
        }
        "search_files" => {
            let n = output.lines().count();
            let noun = if n == 1 { "match" } else { "matches" };
            print_result_line(&format!("Found {n} {noun}"), false);
        }
        _ => print_result_block(output),
    }
}

fn pluralize(count: usize, verb: &str, noun: &str) -> String {
    if count == 1 {
        format!("{verb} {count} {noun}")
    } else {
        format!("{verb} {count} {noun}s")
    }
}

/// Claude-Code-style multi-line tool result block:
/// ```text
///   ⎿  first line
///      second line
///      … +N more lines
/// ```
fn print_result_block(output: &str) {
    const MAX_LINES: usize = 6;
    const MAX_LINE_CHARS: usize = 140;

    let lines: Vec<&str> = output.lines().collect();
    if lines.is_empty() {
        print_result_line("(no output)", false);
        return;
    }

    for (i, line) in lines.iter().take(MAX_LINES).enumerate() {
        let marker = if i == 0 { "⎿ ".dimmed() } else { "  ".dimmed() };
        let capped: String = line.chars().take(MAX_LINE_CHARS).collect();
        let suffix = if line.chars().count() > MAX_LINE_CHARS { "…" } else { "" };
        println!("  {marker}{}{}", capped.dimmed(), suffix.dimmed());
    }
    if lines.len() > MAX_LINES {
        println!(
            "  {}",
            format!("… +{} more lines", lines.len() - MAX_LINES).dimmed()
        );
    }
}

/// Shared by `Agent::bootstrap` and `Agent::bootstrap_unconfigured` — the
/// only thing that differs between "starting with a real, working
/// provider" and "starting with a stand-in that'll fail loudly on first
/// use" is which `Box<dyn Provider>` gets passed in here. Kept as a free
/// function rather than a private associated function: an inherent-impl
/// private helper sitting alongside this struct's `async fn`s (which
/// desugar to hidden opaque return types) tripped a rustc 1.75
/// effective-visibility cycle (E0391) on this exact toolchain — a
/// top-level function sidesteps it entirely and is just as clear to call.
fn assemble_agent(config: &Config, provider: Box<dyn Provider>, non_interactive: bool, auto_approve: bool) -> (Agent, Vec<ChatMessage>) {
    let mut registry = tools::default_registry();
    let tasks = Arc::new(Mutex::new(tools::tasks::TaskList::default()));
    registry.register(Arc::new(tools::tasks::ManageTasksTool::new(tasks.clone())));
    let permissions = PermissionManager::new(config.permissions.clone(), non_interactive, auto_approve);
    let tool_ctx = ToolContext {
        workspace: config.workspace.clone(),
        extra_read_roots: Vec::new(),
    };

    let agent = Agent {
        provider,
        registry,
        permissions,
        tool_ctx,
        max_iterations: config.agent.max_iterations.max(1),
        max_tool_calls: config.agent.max_tool_calls.max(1),
        last_file_change: None,
        tasks,
        hooks: crate::hooks::load(&config.workspace),
        mcp_clients: std::collections::HashMap::new(),
        ide_server: None,
    };

    let history = vec![ChatMessage::system(system_prompt(&config.workspace))];

    (agent, history)
}

fn system_prompt(workspace: &Path) -> String {
    let os_name = if cfg!(target_os = "windows") {
        "Windows"
    } else if cfg!(target_os = "macos") {
        "macOS"
    } else {
        "Linux"
    };
    let project_context = context::build(workspace);
    prompts::system_prompt(workspace, os_name, &project_context)
}
