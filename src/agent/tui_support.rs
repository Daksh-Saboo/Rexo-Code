//! [`AgentUi`]: where a TUI-driven agent turn sends everything a plain
//! terminal session would otherwise get via `println!`.
//!
//! Defined in `agent` (not `cli::tui`, which is the only real implementer)
//! so this module doesn't need to depend on how the REPL renders itself —
//! `cli` already depends on `agent`, and this keeps that a one-way street.
//! [`crate::security::permissions::PermissionPrompter`] follows the same
//! pattern for permission prompts specifically.

use std::sync::{Arc, Mutex};

use crate::providers::StreamEvent;

/// Queue the `Send + Sync`-bound streaming callback (required by
/// [`crate::providers::StreamSink`]) pushes raw provider events into.
/// `Agent::stream_turn_tui` drains it on a timer, on the very same task —
/// there's no real concurrency here, just a type-system requirement the
/// callback has to satisfy regardless.
pub type StreamQueue = Arc<Mutex<Vec<StreamEvent>>>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoteLevel {
    Info,
    Warning,
    Error,
}

#[async_trait::async_trait(?Send)]
pub trait AgentUi {
    /// A turn has started waiting on the model; show a "thinking" spinner.
    fn spinner_start(&mut self);
    /// `elapsed_secs` into the current spinner; advance its animation.
    fn spinner_tick(&mut self, elapsed_secs: u64);
    /// The first token arrived (or the turn ended before any did) — stop
    /// showing the spinner.
    fn spinner_stop(&mut self);

    /// The model has started emitting reasoning/thinking tokens.
    fn reasoning_started(&mut self);
    fn reasoning_delta(&mut self, text: &str);
    /// The model has started emitting its final-answer tokens.
    fn text_delta(&mut self, text: &str);
    /// The model has committed to calling `tool_name`, but the arguments
    /// are still streaming in — a light "preparing…" hint rather than
    /// leaving the screen looking stalled until the full call resolves.
    fn tool_preparing(&mut self, tool_name: &str);

    /// A tool call is about to run, given its full human-friendly summary
    /// (e.g. `Read(src/main.rs)`).
    fn tool_call_started(&mut self, summary: &str);
    fn tool_result_line(&mut self, line: &str, is_error: bool);
    fn tool_result_block(&mut self, output: &str);

    /// The turn's final answer has finished streaming/rendering.
    fn turn_finished(&mut self);
    /// A one-off status line outside the normal turn flow (budget
    /// exceeded, max iterations reached, cancelled, ...).
    fn note(&mut self, text: &str, level: NoteLevel);

    /// Redraw now, reflecting whatever's changed since the last call.
    /// Implementations that don't own a screen (tests, etc.) can no-op.
    fn redraw(&mut self);

    /// Cooperative cancellation: resolves only once the human asks to
    /// cancel the in-flight turn (Ctrl+C), and never otherwise — callers
    /// race it in a `select!` alongside the real network I/O. Any other
    /// input arriving while a turn is in flight is intentionally dropped;
    /// the input box is disabled until the turn finishes.
    async fn wait_for_cancel(&mut self);
}
