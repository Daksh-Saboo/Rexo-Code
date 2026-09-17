//! The persistent, full-screen interactive UI.
//!
//! Replaces the old split personality this REPL used to have: the startup
//! trust dialog was a real `ratatui` screen, but everything after that was
//! plain scrollback `println!`s with `rustyline` reading a *second*,
//! separate prompt underneath whatever had just been printed — the "why
//! is there a `rexo>` at the bottom that doesn't do anything" bug. This
//! module is one alternate-screen application for the whole session: a
//! header that updates live as `/model`/`/provider`/etc. change, a
//! scrollable transcript, and a single bordered input box that *is* the
//! input, with nothing else listening for keys underneath it.
//!
//! A few things deliberately still leave this screen rather than trying
//! to make everything work in raw mode:
//! - Commands that need a real, non-raw-mode terminal for a blocking
//!   `stdin`/hidden-password read (`/connect`, `/provider`, `/api set`,
//!   `/workspace`) — see [`run_command`]'s `needs_terminal` branch.
//! - The startup trust dialog, which runs before this screen even opens.
//!
//! Everything else — chat, streaming, tool-call blocks, the permission
//! y/a/N prompt, `/help` — is native to this screen.

mod render;

use std::io::{self, Stdout};
use std::time::{Duration, Instant};

use anyhow::Result;
use crossterm::event::{
    DisableMouseCapture, EnableMouseCapture, Event, EventStream, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseEventKind,
};
use crossterm::execute;
use crossterm::terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen};
use futures_util::StreamExt;
use ratatui::backend::CrosstermBackend;
use ratatui::style::Color;
use ratatui::text::Line;
use ratatui::Terminal;

use crate::agent::{AgentUi, NoteLevel};
use crate::security::permissions::{PermResponse, PermissionKind, PermissionPrompter};
use crate::security::policies::RiskLevel;

use super::commands::{self, Status};
use super::parser::{self, ParsedLine};
use super::{file_ref, picker, CommandOutcome, Session};
use crate::output;

type Backend = CrosstermBackend<Stdout>;

// ---------------------------------------------------------------------
// Transcript
// ---------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EntryKind {
    UserPrompt,
    Reasoning,
    Answer,
    ToolCall,
    ToolResult,
    Note,
    CommandOutput,
}

impl EntryKind {
    /// Whether `/focus` hides entries of this kind.
    fn hidden_in_focus_mode(self) -> bool {
        matches!(self, EntryKind::Reasoning | EntryKind::ToolCall | EntryKind::ToolResult)
    }
}

struct Entry {
    kind: EntryKind,
    lines: Vec<Line<'static>>,
}

/// A streaming reasoning/answer block that hasn't been finalized into the
/// transcript yet — kept separate so `AgentUi::redraw` can cheaply
/// re-render it every ~90ms without re-wrapping the whole transcript.
struct Pending {
    kind: EntryKind,
    raw: String,
}

struct SpinnerState {
    frame: usize,
    elapsed_secs: u64,
}

const SPINNER_FRAMES: [char; 10] = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];

/// The single source of truth for every keybinding this screen actually
/// registers — `/keybindings`, the `/help` General tab, and `{?}` all
/// read from this instead of each keeping their own copy that could (and
/// eventually would) drift out of sync with what the code above actually
/// does.
pub(crate) const KEYBINDINGS: &[(&str, &str)] = &[
    ("Enter", "Send your message / run a command"),
    ("Up / Down", "Browse command history (or move an autocomplete/help selection)"),
    ("Tab", "Accept the highlighted slash-command suggestion"),
    ("@", "Reference a file — opens a searchable picker"),
    ("!", "Toggle shell mode (every line runs in PowerShell/sh, not the model) — or !<cmd> for one command"),
    ("?", "Show shortcuts (as the very first character), or {?} anytime"),
    ("Alt+M", "Quick-switch model (same arrow-key picker as /model)"),
    ("Alt+P", "Quick-switch provider (same arrow-key picker as /provider)"),
    ("Ctrl+Y", "Toggle selection mode — release the mouse so your terminal's own select & copy works"),
    ("Esc", "Close a popup (help, autocomplete, permission prompt) / clear the input line"),
    ("Ctrl+C", "Cancel the current turn, or clear/exit at an idle prompt (press twice)"),
    ("Ctrl+L", "Redraw the screen"),
    ("PageUp / PageDown", "Scroll the conversation by a page"),
    ("Mouse wheel", "Scroll the conversation (off while selection mode is on)"),
];

const TASK_SUGGESTIONS: &[&str] = &[
    "list the files in this project",
    "explain what this project does",
    "find and fix any obvious bugs",
    "add tests for the most important function",
    "review this codebase for security issues",
];

fn pick_suggestion() -> &'static str {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    TASK_SUGGESTIONS[(nanos as usize) % TASK_SUGGESTIONS.len()]
}

/// Sets the terminal window/tab title to "Rexo Code — <folder name>"
/// instead of whatever the shell's own title happens to be (typically
/// just "PowerShell"/"pwsh"), via crossterm's cross-platform `SetTitle` —
/// real Win32 `SetConsoleTitleW` under the hood on Windows, the
/// equivalent OSC 0/2 escape sequence everywhere else. Best-effort: some
/// terminal hosts ignore it entirely, and that's fine — this never fails
/// the caller, just silently no-ops.
///
/// This *is not* the same thing as a custom taskbar/tab **icon** — that
/// needs an icon actually embedded in the compiled `.exe`'s resources
/// (Windows reads it from there for conhost, and Windows Terminal reads
/// it from a profile's `icon` setting instead, which this can't reach at
/// runtime at all). See `build.rs` and `assets/rexo.ico` for the
/// icon-embedding half of this, and `assets/windows-terminal-profile.json`
/// for a snippet Windows Terminal users can add themselves today.
fn set_console_title(session: &Session) {
    let name = session
        .agent
        .workspace()
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| session.agent.workspace().display().to_string());
    let _ = execute!(io::stdout(), crossterm::terminal::SetTitle(format!("Rexo Code — {name}")));
}

/// Header fields, refreshed from `Session` at the top of the loop and
/// after every command — *not* read live during a turn, since nothing
/// that changes them can run while a turn is in flight.
struct HeaderInfo {
    version: String,
    provider_desc: String,
    workspace: String,
    title: Option<String>,
    edit_auto: bool,
    terminal_auto: bool,
    git_auto: bool,
    accent: Color,
}

impl HeaderInfo {
    fn sync(&mut self, session: &Session) {
        self.provider_desc = session.agent.provider_description();
        self.workspace = session.agent.workspace().display().to_string();
        self.title = session.ui.title.clone();
        let perms = session.agent.permissions().current_config();
        self.edit_auto = perms.allow_edit;
        self.terminal_auto = perms.allow_terminal;
        self.git_auto = perms.allow_git_write;
        self.accent = session.ui.accent.to_ratatui();
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HelpTab {
    General,
    Commands,
    Custom,
}

struct HelpState {
    tab: HelpTab,
    selected: usize,
    workspace: std::path::PathBuf,
}

struct PermissionView {
    action: String,
    summary: String,
    risk_label: &'static str,
}

/// Everything needed to render a frame — deliberately kept in its own
/// struct, separate from [`TuiCore`]'s `terminal`, so `TuiCore::draw` can
/// borrow `&self.state` and `&mut self.terminal` at the same time without
/// the borrow checker treating the whole of `self` as one unit (it can't
/// split disjoint fields *through* a method call the way it can for
/// direct field access — see `TuiCore::draw`).
pub(crate) struct UiState {
    transcript: Vec<Entry>,
    pending: Option<Pending>,
    preparing_hint: Option<String>,
    spinner: Option<SpinnerState>,
    scroll_from_bottom: u16,

    input: String,
    cursor: usize,
    history: Vec<String>,
    history_idx: Option<usize>,
    autocomplete_cache: Vec<String>,
    autocomplete_selected: usize,
    last_ctrl_c: Option<Instant>,

    header: HeaderInfo,
    focus_mode: bool,
    shell_mode: bool,
    scroll_step: u16,
    suggestion: &'static str,

    help: Option<HelpState>,
    permission: Option<PermissionView>,
    /// Drives the header mascot's expression — see `render::mascot_glyph`.
    /// Cleared at the start of each new turn so a stale error/success face
    /// doesn't linger once the user's moved on.
    last_note: Option<NoteLevel>,
    /// When true, the terminal's own mouse capture is released (Ctrl+Y) so
    /// its native click-drag text selection and copy work normally — see
    /// `TuiCore::toggle_selection_mode`.
    selection_mode: bool,
}

/// The whole interactive screen: everything needed to draw a frame, plus
/// the one `EventStream` reading real input for the lifetime of the
/// session (never recreated — see the module docs on why a couple of
/// commands still step outside this screen entirely instead). Derefs to
/// [`UiState`], so every method below reads/writes `self.whatever` exactly
/// as if the fields lived directly on `TuiCore`.
pub(crate) struct TuiCore {
    terminal: Terminal<Backend>,
    events: EventStream,
    state: UiState,
}

impl std::ops::Deref for TuiCore {
    type Target = UiState;
    fn deref(&self) -> &UiState {
        &self.state
    }
}

impl std::ops::DerefMut for TuiCore {
    fn deref_mut(&mut self) -> &mut UiState {
        &mut self.state
    }
}

impl TuiCore {
    pub fn enter(session: &Session) -> Result<Self> {
        enable_raw_mode()?;
        let mut stdout = io::stdout();
        execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
        let mut terminal = Terminal::new(CrosstermBackend::new(stdout))?;
        // Same reasoning as trust_dialog.rs/wizard.rs: this is now
        // routinely the *third* alt-screen session in a row (wizard →
        // trust dialog → here), so don't inherit whatever either of
        // those left behind.
        terminal.clear()?;
        set_console_title(session);

        let mut header = HeaderInfo {
            version: env!("CARGO_PKG_VERSION").to_string(),
            provider_desc: String::new(),
            workspace: String::new(),
            title: None,
            edit_auto: false,
            terminal_auto: false,
            git_auto: false,
            accent: Color::Cyan,
        };
        header.sync(session);

        Ok(Self {
            terminal,
            events: EventStream::new(),
            state: UiState {
                transcript: Vec::new(),
                pending: None,
                preparing_hint: None,
                spinner: None,
                scroll_from_bottom: 0,
                input: String::new(),
                cursor: 0,
                history: Vec::new(),
                history_idx: None,
                autocomplete_cache: Vec::new(),
                autocomplete_selected: 0,
                last_ctrl_c: None,
                header,
                focus_mode: session.ui.focus_mode,
                shell_mode: session.ui.shell_mode,
                scroll_step: session.ui.scroll_step.max(1),
                suggestion: pick_suggestion(),
                help: None,
                permission: None,
                last_note: None,
                selection_mode: false,
            },
        })
    }

    /// Leave the alternate screen / raw mode. Safe to call more than
    /// once. Called both on normal exit and from the panic hook installed
    /// in [`run`], so a crash never leaves the user's terminal stuck in
    /// raw mode with no visible prompt.
    pub fn restore() {
        let _ = disable_raw_mode();
        let _ = execute!(io::stdout(), DisableMouseCapture, LeaveAlternateScreen);
    }

    fn sync_header(&mut self, session: &Session) {
        self.header.sync(session);
        self.focus_mode = session.ui.focus_mode;
        self.shell_mode = session.ui.shell_mode;
        self.scroll_step = session.ui.scroll_step.max(1);
        set_console_title(session);
    }

    pub fn draw(&mut self) -> Result<()> {
        // See `UiState`'s docs for why this goes through a pre-extracted
        // reference rather than `render::draw(f, self)` directly.
        let state = &self.state;
        self.terminal.draw(|f| render::draw(f, state))?;
        Ok(())
    }

    /// The startup intro: three scan-lines sweep across the screen at
    /// once (top-to-right, bottom-to-left, and a faster one through the
    /// middle), then the boxed face (the same `┏┓┃┗┛` + `◉◉` visual
    /// language as `MASCOT` in `main.rs`'s headless banner, just bigger)
    /// scatters in from random positions and converges into place, then
    /// holds for a beat with "REXO CODE" underneath before clearing into
    /// the normal session. `--no-intro` / the `REXO_NO_INTRO` env var
    /// skip this entirely (see `main.rs`); any keypress at any point
    /// skips straight to the fully-assembled final frame instead of
    /// waiting out the rest of it.
    ///
    /// Reads through `self.events` (the *same* `EventStream` the rest of
    /// the session uses, via `tokio::select!`) rather than a second,
    /// separate `crossterm::event::poll`/`read` call — mixing a raw
    /// synchronous read with the async `EventStream` already reading the
    /// same fd is exactly the dual-reader race this codebase root-caused
    /// and fixed in v0.4.1 (see this module's own doc comment history).
    /// One reader, for the lifetime of the session, full stop.
    async fn play_intro_animation(&mut self) -> Result<()> {
        const FACE: &[&str] = &[
            "┏━━━━━━━━━━━━━━━━━┓",
            "┃                 ┃",
            "┃   ◉         ◉   ┃",
            "┃                 ┃",
            "┃       ▬▬▬       ┃",
            "┃                 ┃",
            "┗━━━━━━━━━━━━━━━━━┛",
        ];
        const TITLE: &str = "R E X O   C O D E";

        let area = self.terminal.size()?;
        // Too small to draw the face without wrapping nonsense — just
        // skip straight to the normal session rather than render garbage.
        if area.width < FACE[0].chars().count() as u16 + 4 || area.height < FACE.len() as u16 + 8 {
            return Ok(());
        }

        let face_w = FACE.iter().map(|l| l.chars().count()).max().unwrap_or(0) as u16;
        let face_h = FACE.len() as u16;
        let off_x = (area.width.saturating_sub(face_w)) / 2;
        let off_y = (area.height.saturating_sub(face_h + 2)) / 2;

        let mut targets: Vec<(u16, u16, char)> = Vec::new();
        for (y, line) in FACE.iter().enumerate() {
            for (x, ch) in line.chars().enumerate() {
                if ch != ' ' {
                    targets.push((off_x + x as u16, off_y + y as u16, ch));
                }
            }
        }
        let title_x = (area.width.saturating_sub(TITLE.chars().count() as u16)) / 2;
        let title_y = off_y + face_h + 1;
        for (x, ch) in TITLE.chars().enumerate() {
            if ch != ' ' {
                targets.push((title_x + x as u16, title_y, ch));
            }
        }

        let accent = self.header.accent;
        // Set once a keypress arrives, and honored by every remaining
        // phase below — one keypress skips the *whole* intro straight to
        // its fully-assembled final frame, not just the current phase.
        let mut skip = false;

        // --- Phase 1: three scan-lines sweeping at once ---------------
        // Top sweeps left→right, bottom sweeps right→left (both ~1.7s),
        // and a third, faster one (~1.1s) sweeps left→right straight
        // through the face's eye-row — cleared away again right before
        // the face itself starts landing there in Phase 2, so it reads
        // as "the scan finds where the face is about to appear," not
        // leftover clutter once the face is actually sitting on top of
        // it.
        let top_row = off_y.saturating_sub(2).max(1);
        let bottom_row = (off_y + face_h + 3).min(area.height.saturating_sub(1));
        let mid_row = off_y + 2;

        const SWEEP_FRAMES: u32 = 42;
        const SWEEP_FRAME_TIME: Duration = Duration::from_millis(40); // ~1.7s for the full phase
        for frame_i in 0..=SWEEP_FRAMES {
            if skip {
                break;
            }
            let t = frame_i as f32 / SWEEP_FRAMES as f32;
            let ease = |x: f32| 1.0 - (1.0 - x).powi(3); // ease-out cubic
            let e_top = ease((t / 0.95).min(1.0));
            let e_bottom = ease((t / 0.95).min(1.0));
            let e_mid = ease((t / 0.62).min(1.0)); // finishes first, well before the top/bottom pair

            self.terminal.draw(|f| {
                let draw_area = f.size();
                let buf = f.buffer_mut();
                let style = ratatui::style::Style::default().fg(accent);
                let width = draw_area.width;

                let len_top = (width as f32 * e_top).round() as u16;
                for x in 0..len_top.min(width) {
                    if top_row < draw_area.height {
                        buf.get_mut(x, top_row).set_char('━').set_style(style);
                    }
                }
                let len_bottom = (width as f32 * e_bottom).round() as u16;
                for i in 0..len_bottom.min(width) {
                    let x = width.saturating_sub(1).saturating_sub(i);
                    if bottom_row < draw_area.height {
                        buf.get_mut(x, bottom_row).set_char('━').set_style(style);
                    }
                }
                let len_mid = (width as f32 * e_mid).round() as u16;
                for x in 0..len_mid.min(width) {
                    if mid_row < draw_area.height {
                        buf.get_mut(x, mid_row).set_char('─').set_style(style);
                    }
                }
            })?;

            tokio::select! {
                _ = tokio::time::sleep(SWEEP_FRAME_TIME) => {}
                event = self.events.next() => {
                    if let Some(Ok(Event::Key(k))) = event {
                        if k.kind == KeyEventKind::Press {
                            skip = true;
                        }
                    }
                }
            }
        }
        // Land on the full-width top/bottom lines regardless of whether
        // the loop above ran to completion or was skipped early — they
        // stay on screen as a frame around the face through Phase 2.
        self.terminal.draw(|f| {
            let draw_area = f.size();
            let buf = f.buffer_mut();
            let style = ratatui::style::Style::default().fg(accent);
            for x in 0..draw_area.width {
                if top_row < draw_area.height {
                    buf.get_mut(x, top_row).set_char('━').set_style(style);
                }
                if bottom_row < draw_area.height {
                    buf.get_mut(x, bottom_row).set_char('━').set_style(style);
                }
            }
        })?;

        // A tiny xorshift64 PRNG seeded from the clock — scatter start
        // positions don't need to be cryptographically random, just
        // different-looking each run, and this avoids pulling in a `rand`
        // dependency for one cosmetic effect.
        let mut seed = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.subsec_nanos() as u64).unwrap_or(0x2545F4914F6CDD1D) | 1;
        let mut next_rand = move || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        let starts: Vec<(f32, f32)> = targets
            .iter()
            .map(|_| {
                let rx = (next_rand() % area.width.max(1) as u64) as f32;
                let ry = (next_rand() % area.height.max(1) as u64) as f32;
                (rx, ry)
            })
            .collect();

        // --- Phase 2: the face scatters in and converges ---------------
        // Slower and with more frames than the original cut of this
        // animation — meant to read as a deliberate arrival, not a blip.
        const FRAMES: u32 = 40;
        const FRAME_TIME: Duration = Duration::from_millis(32); // ~1.3s for the full phase

        for frame_i in 0..=FRAMES {
            if skip {
                break;
            }
            let t = frame_i as f32 / FRAMES as f32;
            let eased = 1.0 - (1.0 - t).powi(3); // ease-out cubic: fast start, settles gently into place

            self.terminal.draw(|f| {
                let draw_area = f.size();
                let buf = f.buffer_mut();
                // Clear the mid-row scan line first — the face's own eye
                // row lands here, and without this its unclaimed columns
                // (outside the face's width) would keep showing stray
                // '─' clutter from Phase 1 on either side of the face.
                if mid_row < draw_area.height {
                    for x in 0..draw_area.width {
                        buf.get_mut(x, mid_row).set_char(' ');
                    }
                }
                let style = ratatui::style::Style::default().fg(accent);
                for (i, (tx, ty, ch)) in targets.iter().enumerate() {
                    let (sx, sy) = starts[i];
                    let cx = (sx + (*tx as f32 - sx) * eased).round();
                    let cy = (sy + (*ty as f32 - sy) * eased).round();
                    if cx < 0.0 || cy < 0.0 {
                        continue;
                    }
                    let (cx, cy) = (cx as u16, cy as u16);
                    if cx < draw_area.width && cy < draw_area.height {
                        buf.get_mut(cx, cy).set_char(*ch).set_style(style);
                    }
                }
            })?;

            tokio::select! {
                _ = tokio::time::sleep(FRAME_TIME) => {}
                event = self.events.next() => {
                    if let Some(Ok(Event::Key(k))) = event {
                        if k.kind == KeyEventKind::Press {
                            skip = true;
                        }
                    }
                }
            }
        }

        // Hold the assembled face for a beat so it reads as an arrival,
        // not a flicker — same skip-on-keypress behavior, and the
        // top/bottom scan-lines from Phase 1 are still framing it.
        self.terminal.draw(|f| {
            let draw_area = f.size();
            let buf = f.buffer_mut();
            if mid_row < draw_area.height {
                for x in 0..draw_area.width {
                    buf.get_mut(x, mid_row).set_char(' ');
                }
            }
            let style = ratatui::style::Style::default().fg(accent);
            for (tx, ty, ch) in &targets {
                if *tx < draw_area.width && *ty < draw_area.height {
                    buf.get_mut(*tx, *ty).set_char(*ch).set_style(style);
                }
            }
        })?;
        if !skip {
            tokio::select! {
                _ = tokio::time::sleep(Duration::from_millis(700)) => {}
                _ = self.events.next() => {}
            }
        }

        Ok(())
    }

    /// Flip "selection mode" (Ctrl+Y): releases (or re-enables) the
    /// terminal's mouse capture. Enabling mouse capture — which this
    /// screen does for its own scroll-wheel support — is exactly what
    /// stops a terminal emulator's *own* click-drag text selection from
    /// working, since every mouse event goes to the application instead
    /// of the terminal. Rather than reimplementing selection/clipboard
    /// integration inside the TUI, this just gets out of the way for a
    /// moment so the terminal's native selection (and whatever copy
    /// shortcut it uses — Ctrl+Shift+C, a right-click menu, etc.) works
    /// exactly like it does in any other program. Scrolling by mouse
    /// wheel is unavailable while this is on; PageUp/PageDown still work.
    fn toggle_selection_mode(&mut self) -> Result<()> {
        self.state.selection_mode = !self.state.selection_mode;
        if self.state.selection_mode {
            execute!(io::stdout(), DisableMouseCapture)?;
            self.note(
                "Selection mode on — use your terminal's own click-drag select & copy. Ctrl+Y again to resume mouse-wheel scrolling.",
                NoteLevel::Info,
            );
        } else {
            execute!(io::stdout(), EnableMouseCapture)?;
            self.note("Selection mode off — mouse-wheel scrolling resumed.", NoteLevel::Info);
        }
        Ok(())
    }

    fn push_finalized(&mut self, kind: EntryKind, lines: Vec<Line<'static>>) {
        if lines.is_empty() {
            return;
        }
        self.transcript.push(Entry { kind, lines });
        self.scroll_from_bottom = 0;
    }

    fn finalize_pending(&mut self) {
        if let Some(pending) = self.pending.take() {
            let lines = render::wrap_plain(&pending.raw, pending.kind);
            self.push_finalized(pending.kind, lines);
        }
    }

    /// Run a command that can print through `println!`/`print!`
    /// (captured — see `crate::output`) and turn whatever it produced into
    /// one transcript entry. Commands needing a real terminal never reach
    /// here; see [`run_command`].
    fn run_captured(&mut self, session: &mut Session, name: &str, args: &[String]) -> CommandOutcome {
        output::begin_capture();
        let result = commands::dispatch(session, name, args);
        let captured = output::end_capture();

        let outcome = match result {
            Ok(outcome) => outcome,
            Err(e) => {
                self.note(&format!("/{name} failed: {e}"), NoteLevel::Error);
                CommandOutcome::Continue
            }
        };

        if !captured.trim().is_empty() {
            let lines = render::wrap_ansi(&captured);
            self.push_finalized(EntryKind::CommandOutput, lines);
        }
        outcome
    }

    fn push_user_prompt(&mut self, text: &str) {
        self.last_note = None;
        let lines = render::wrap_plain(text, EntryKind::UserPrompt);
        self.push_finalized(EntryKind::UserPrompt, lines);
    }

    // ---- input editing -------------------------------------------------

    fn insert_char(&mut self, c: char) {
        let at = self.cursor;
        self.input.insert(at, c);
        self.cursor += c.len_utf8();
        self.autocomplete_selected = 0;
    }

    fn backspace(&mut self) {
        if self.cursor == 0 {
            return;
        }
        let mut idx = self.cursor - 1;
        while !self.input.is_char_boundary(idx) {
            idx -= 1;
        }
        let end = self.cursor;
        self.input.drain(idx..end);
        self.cursor = idx;
        self.autocomplete_selected = 0;
    }

    fn delete_forward(&mut self) {
        if self.cursor >= self.input.len() {
            return;
        }
        let mut end = self.cursor + 1;
        while end < self.input.len() && !self.input.is_char_boundary(end) {
            end += 1;
        }
        let start = self.cursor;
        self.input.drain(start..end);
    }

    fn move_left(&mut self) {
        if self.cursor == 0 {
            return;
        }
        let mut idx = self.cursor - 1;
        while !self.input.is_char_boundary(idx) {
            idx -= 1;
        }
        self.cursor = idx;
    }

    fn move_right(&mut self) {
        if self.cursor >= self.input.len() {
            return;
        }
        let mut idx = self.cursor + 1;
        while idx < self.input.len() && !self.input.is_char_boundary(idx) {
            idx += 1;
        }
        self.cursor = idx;
    }

    fn clear_input(&mut self) {
        self.input.clear();
        self.cursor = 0;
        self.history_idx = None;
        self.autocomplete_selected = 0;
    }

    fn take_input(&mut self) -> String {
        let line = self.input.clone();
        self.clear_input();
        line
    }

    /// Live slash-command suggestions for the input box's popup, or an
    /// empty list when there's nothing to suggest (not typing a command,
    /// or already past the first token). Cached into `autocomplete_cache`
    /// by [`refresh_autocomplete`](Self::refresh_autocomplete) since
    /// rendering (`draw`, called from contexts with no `Session` on hand,
    /// like mid-turn redraws) needs the *result* without needing a
    /// `Session` reference itself.
    fn compute_autocomplete(&self, session: &Session) -> Vec<String> {
        if let Some(fragment) = at_fragment(&self.input, self.cursor) {
            return super::file_ref::suggest(session.agent.workspace(), fragment);
        }
        let Some(fragment) = self.input.strip_prefix('/') else {
            return Vec::new();
        };
        if fragment.contains(' ') {
            // Argument position: only offer path completion for the two
            // commands that take one, and only once a space follows.
            for prefix in ["workspace ", "cd "] {
                if let Some(rest) = fragment.strip_prefix(prefix) {
                    return super::completion::suggest_paths(session.agent.workspace(), rest);
                }
            }
            return Vec::new();
        }
        super::completion::suggest_commands(fragment)
    }

    fn refresh_autocomplete(&mut self, session: &Session) {
        self.autocomplete_cache = self.compute_autocomplete(session);
        if self.autocomplete_selected >= self.autocomplete_cache.len() {
            self.autocomplete_selected = 0;
        }
    }

    fn history_up(&mut self) {
        if self.history.is_empty() {
            return;
        }
        let next = match self.history_idx {
            None => self.history.len() - 1,
            Some(0) => 0,
            Some(i) => i - 1,
        };
        self.history_idx = Some(next);
        self.input = self.history[next].clone();
        self.cursor = self.input.len();
    }

    fn history_down(&mut self) {
        match self.history_idx {
            None => {}
            Some(i) if i + 1 < self.history.len() => {
                self.history_idx = Some(i + 1);
                self.input = self.history[i + 1].clone();
                self.cursor = self.input.len();
            }
            Some(_) => {
                self.history_idx = None;
                self.clear_input();
            }
        }
    }

    fn scroll_up(&mut self) {
        self.scroll_from_bottom = self.scroll_from_bottom.saturating_add(self.scroll_step);
    }

    fn scroll_down(&mut self) {
        self.scroll_from_bottom = self.scroll_from_bottom.saturating_sub(self.scroll_step);
    }
}

// ---------------------------------------------------------------------
// AgentUi: where a turn's output goes while this screen owns the terminal
// ---------------------------------------------------------------------

#[async_trait::async_trait(?Send)]
impl AgentUi for TuiCore {
    fn spinner_start(&mut self) {
        self.spinner = Some(SpinnerState { frame: 0, elapsed_secs: 0 });
    }

    fn spinner_tick(&mut self, elapsed_secs: u64) {
        if let Some(s) = &mut self.spinner {
            s.frame = s.frame.wrapping_add(1);
            s.elapsed_secs = elapsed_secs;
        }
    }

    fn spinner_stop(&mut self) {
        self.spinner = None;
    }

    fn reasoning_started(&mut self) {
        self.finalize_pending();
        self.pending = Some(Pending { kind: EntryKind::Reasoning, raw: String::new() });
    }

    fn reasoning_delta(&mut self, text: &str) {
        match &mut self.pending {
            Some(p) if p.kind == EntryKind::Reasoning => p.raw.push_str(text),
            _ => {
                self.finalize_pending();
                self.pending = Some(Pending { kind: EntryKind::Reasoning, raw: text.to_string() });
            }
        }
    }

    fn text_delta(&mut self, text: &str) {
        match &mut self.pending {
            Some(p) if p.kind == EntryKind::Answer => p.raw.push_str(text),
            _ => {
                self.finalize_pending();
                self.pending = Some(Pending { kind: EntryKind::Answer, raw: text.to_string() });
            }
        }
        self.preparing_hint = None;
    }

    fn tool_preparing(&mut self, tool_name: &str) {
        self.finalize_pending();
        self.preparing_hint = Some(format!("preparing {tool_name}…"));
    }

    fn tool_call_started(&mut self, summary: &str) {
        self.finalize_pending();
        self.preparing_hint = None;
        let line = render::styled_line(format!("⏺ {summary}"), Color::Cyan, true);
        self.push_finalized(EntryKind::ToolCall, vec![line]);
    }

    fn tool_result_line(&mut self, line: &str, is_error: bool) {
        let color = if is_error { Color::Red } else { Color::DarkGray };
        let styled = render::styled_line(format!("  ⎿ {line}"), color, false);
        self.push_finalized(EntryKind::ToolResult, vec![styled]);
    }

    fn tool_result_block(&mut self, output: &str) {
        const MAX_LINES: usize = 6;
        const MAX_LINE_CHARS: usize = 140;
        let raw_lines: Vec<&str> = output.lines().collect();
        if raw_lines.is_empty() {
            self.tool_result_line("(no output)", false);
            return;
        }
        let mut lines = Vec::new();
        for (i, line) in raw_lines.iter().take(MAX_LINES).enumerate() {
            let marker = if i == 0 { "⎿ " } else { "  " };
            let capped: String = line.chars().take(MAX_LINE_CHARS).collect();
            let suffix = if line.chars().count() > MAX_LINE_CHARS { "…" } else { "" };
            lines.push(render::styled_line(format!("  {marker}{capped}{suffix}"), Color::DarkGray, false));
        }
        if raw_lines.len() > MAX_LINES {
            lines.push(render::styled_line(
                format!("  … +{} more lines", raw_lines.len() - MAX_LINES),
                Color::DarkGray,
                false,
            ));
        }
        self.push_finalized(EntryKind::ToolResult, lines);
    }

    fn turn_finished(&mut self) {
        self.finalize_pending();
        self.preparing_hint = None;
    }

    fn note(&mut self, text: &str, level: NoteLevel) {
        let color = match level {
            NoteLevel::Info => Color::Gray,
            NoteLevel::Warning => Color::Yellow,
            NoteLevel::Error => Color::Red,
        };
        self.last_note = Some(level);
        let line = render::styled_line(text.to_string(), color, true);
        self.push_finalized(EntryKind::Note, vec![line]);
    }

    fn redraw(&mut self) {
        let _ = self.draw();
    }

    async fn wait_for_cancel(&mut self) {
        loop {
            match self.events.next().await {
                Some(Ok(Event::Key(key))) if key.kind == KeyEventKind::Press => {
                    if is_ctrl_c(&key) {
                        return;
                    }
                    // Anything else typed while a turn is in flight is
                    // dropped — the input box is intentionally disabled
                    // until the turn finishes.
                }
                Some(Ok(Event::Resize(_, _))) => {
                    let _ = self.draw();
                }
                Some(Ok(Event::Mouse(m))) => {
                    match m.kind {
                        MouseEventKind::ScrollUp => self.scroll_up(),
                        MouseEventKind::ScrollDown => self.scroll_down(),
                        _ => {}
                    }
                    let _ = self.draw();
                }
                Some(Ok(_)) => {}
                Some(Err(_)) | None => return,
            }
        }
    }
}

fn is_ctrl_c(key: &KeyEvent) -> bool {
    matches!(key.code, KeyCode::Char('c') | KeyCode::Char('C')) && key.modifiers.contains(KeyModifiers::CONTROL)
}

/// The `@`-prefixed fragment the cursor is currently sitting inside of,
/// if any — e.g. for `explain @src/ma|in.rs` (cursor at `|`) this returns
/// `Some("src/ma")`. Used to decide whether the autocomplete popup is
/// offering file paths (`@...`) or slash commands (`/...`), and where
/// `accept_autocomplete` should splice a chosen suggestion in.
pub(super) fn at_fragment(input: &str, cursor: usize) -> Option<&str> {
    let cursor = cursor.min(input.len());
    let before = &input[..cursor];
    let at_idx = before.rfind('@')?;
    let fragment = &before[at_idx + 1..];
    if fragment.contains(char::is_whitespace) {
        return None;
    }
    Some(fragment)
}

// ---------------------------------------------------------------------
// PermissionPrompter: the y/a/N modal
// ---------------------------------------------------------------------

#[async_trait::async_trait(?Send)]
impl PermissionPrompter for TuiCore {
    async fn ask(&mut self, kind: PermissionKind, summary: &str, risk: RiskLevel) -> Result<PermResponse> {
        self.permission = Some(PermissionView {
            action: kind.label().to_string(),
            summary: summary.to_string(),
            risk_label: match risk {
                RiskLevel::Moderate => "moderate",
                RiskLevel::High => "high",
            },
        });
        self.draw()?;

        loop {
            match self.events.next().await {
                Some(Ok(Event::Key(key))) if key.kind == KeyEventKind::Press => {
                    let response = match key.code {
                        KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::Enter => Some(PermResponse::Once),
                        KeyCode::Char('a') | KeyCode::Char('A') => Some(PermResponse::Always),
                        KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => Some(PermResponse::Deny),
                        _ if is_ctrl_c(&key) => Some(PermResponse::Deny),
                        _ => None,
                    };
                    if let Some(r) = response {
                        self.permission = None;
                        self.draw()?;
                        return Ok(r);
                    }
                }
                Some(Ok(Event::Resize(_, _))) => {
                    self.draw()?;
                }
                Some(Ok(_)) => {}
                Some(Err(_)) | None => {
                    self.permission = None;
                    return Ok(PermResponse::Deny);
                }
            }
        }
    }
}

// ---------------------------------------------------------------------
// The main loop
// ---------------------------------------------------------------------

/// Run the persistent interactive session until the user exits. Installs
/// a panic hook so a crash mid-session still leaves the real terminal
/// usable instead of stuck in raw mode with no visible prompt.
pub async fn run(session: &mut Session, show_intro: bool) -> Result<()> {
    let previous_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        TuiCore::restore();
        previous_hook(info);
    }));

    let result = run_inner(session, show_intro).await;

    TuiCore::restore();
    let _ = std::panic::take_hook(); // drop our hook, restoring the default
    result
}

async fn run_inner(session: &mut Session, show_intro: bool) -> Result<()> {
    let mut ui = TuiCore::enter(session)?;

    if show_intro {
        ui.play_intro_animation().await?;
    }

    ui.note(
        &format!("Welcome to Rexo Code v{}. Type a task, or /help for commands.", ui.header.version),
        NoteLevel::Info,
    );
    // The graceful-startup fallback in main.rs — always reach the TUI,
    // even with nothing configured — means this needs to be said loudly
    // right away, or the first thing a fresh install shows is a
    // confusing provider error on the very first message instead of a
    // clear next step.
    if session.agent.provider_description().starts_with("(not active:") {
        ui.note("No provider is configured yet — run /connect to pick one.", NoteLevel::Warning);
    }
    ui.draw()?;

    loop {
        // A `/loop` job due to fire takes priority over waiting on a real
        // keypress, but only when the user isn't actively typing —
        // otherwise it'd yank away whatever they were composing.
        if ui.input.is_empty() {
            if let Some(job) = &session.ui.loop_job {
                if Instant::now() >= job.next_at {
                    let text = job.text.clone();
                    let interval = job.interval_secs;
                    session.ui.loop_job.as_mut().unwrap().next_at = Instant::now() + Duration::from_secs(interval);
                    if !run_turn(&mut ui, session, &text).await? {
                        break;
                    }
                    continue;
                }
            }
        }

        let Some(line) = read_input(&mut ui, session).await? else {
            break; // Ctrl+C-Ctrl+C or /exit
        };
        if line.trim().is_empty() {
            continue;
        }
        ui.history.push(line.clone());

        // Bare "exit"/"quit"/":q" (no leading slash) are common REPL habits
        // from before slash commands existed — keep honoring them directly
        // rather than sending them to the model as a prompt. While shell
        // mode is on, the same words instead mean "leave the shell" (real
        // shell UX), not "quit REXO" — typing `/exit` still always quits.
        if matches!(line.trim().to_lowercase().as_str(), "exit" | "quit" | ":q") {
            if session.ui.shell_mode {
                session.ui.shell_mode = false;
                ui.sync_header(session);
                ui.note("Shell mode off.", NoteLevel::Info);
                continue;
            }
            break;
        }
        // A dedicated, from-anywhere shortcut-list trigger — see KEYBINDINGS.
        if line.trim() == "{?}" {
            run_help(&mut ui, session, HelpTab::General).await?;
            continue;
        }
        // A bare `!` toggles persistent shell mode — `!<command>` (handled
        // below via `ParsedLine::Shell`) always runs one command
        // immediately regardless of the toggle.
        if line.trim() == "!" {
            session.ui.shell_mode = !session.ui.shell_mode;
            ui.sync_header(session);
            if session.ui.shell_mode {
                ui.note(
                    "Shell mode on — every line now runs directly in PowerShell/sh, not the model. \
                     Type '!' or 'exit' to leave.",
                    NoteLevel::Info,
                );
            } else {
                ui.note("Shell mode off.", NoteLevel::Info);
            }
            continue;
        }

        match parser::parse_line(&line) {
            ParsedLine::Command { name, args } => {
                if !run_command(&mut ui, session, &name, &args).await? {
                    break;
                }
            }
            ParsedLine::Shell(command) => {
                run_shell_command(&mut ui, session, &command).await;
            }
            ParsedLine::Prompt(text) if session.ui.shell_mode => {
                run_shell_command(&mut ui, session, &text).await;
            }
            ParsedLine::Prompt(text) => {
                if !run_turn(&mut ui, session, &text).await? {
                    break;
                }
            }
        }
        maybe_autocompact(&mut ui, session);
    }

    Ok(())
}

/// Run one line as a raw OS shell command instead of an agent turn —
/// invoked either via the one-shot `!<command>` syntax or, while
/// persistent shell mode is on (bare `!` toggles it), for every
/// plain-text line. Deliberately bypasses the agent's permission/security
/// engine entirely: this is the *person* typing a command directly, the
/// same trust boundary as them opening a real terminal window, not the
/// model requesting one — REXO's tool-permission layer exists to gate
/// what the *model* can do, not what the person sitting at the keyboard
/// can type into their own shell.
async fn run_shell_command(ui: &mut TuiCore, session: &mut Session, command: &str) {
    ui.push_user_prompt(&format!("! {command}"));

    let workspace = session.agent.workspace().to_path_buf();
    let command_owned = command.to_string();
    let result = tokio::task::spawn_blocking(move || crate::tools::terminal::spawn_os_shell(&command_owned, &workspace)).await;

    let text = match result {
        Ok(Ok(output)) => crate::tools::terminal::format_command_output(&output, 100_000),
        Ok(Err(e)) => format!("Failed to run '{command}': {e}"),
        Err(e) => format!("Shell command task panicked: {e}"),
    };
    let lines = render::wrap_plain(&text, EntryKind::CommandOutput);
    ui.push_finalized(EntryKind::CommandOutput, lines);
}

/// Run one full agent turn. Returns `Ok(false)` if the session should end
/// (it never actually does from here — kept `bool` for symmetry with
/// [`run_command`] so the caller doesn't need two different loop-exit
/// shapes).
async fn run_turn(ui: &mut TuiCore, session: &mut Session, text: &str) -> Result<bool> {
    ui.sync_header(session);
    ui.push_user_prompt(text);
    ui.draw()?;

    let skills = crate::cli::skills::discover(session.agent.workspace());
    let triggered: Vec<crate::cli::skills::Skill> = crate::cli::skills::find_triggered(&skills, text)
        .into_iter()
        .filter(|s| !session.loaded_skills.contains(&s.name))
        .cloned()
        .collect();
    for skill in &triggered {
        session
            .history
            .push(crate::providers::ChatMessage::system(format!("[Skill: {}]\n{}", skill.name, skill.body)));
        session.loaded_skills.insert(skill.name.clone());
        ui.note(&format!("Loaded skill '{}'", skill.name), NoteLevel::Info);
    }

    let augmented = file_ref::augment_with_references(text, session.agent.workspace());
    if augmented != text {
        ui.note("Expanding @file references…", NoteLevel::Info);
        ui.draw()?;
    }

    match session.agent.respond_tui(&mut session.history, &augmented, ui).await {
        Ok(_answer) => {}
        Err(e) => {
            let kind = crate::providers::provider_error_kind(&e);
            let suffix = if kind == crate::providers::ProviderErrorKind::Unknown {
                String::new()
            } else if kind.is_retryable() {
                format!(" [{}, usually transient — worth trying again]", kind.label())
            } else {
                format!(" [{}]", kind.label())
            };
            ui.note(&format!("Request failed: {e}{suffix}"), NoteLevel::Error);
        }
    }
    ui.sync_header(session);
    ui.draw()?;
    // Even a failed turn's context (what was asked, tool calls made
    // before the failure) is worth keeping for `/resume` — better to
    // autosave regardless of outcome than lose it.
    crate::cli::sessions::autosave(session);
    Ok(true)
}

/// Dispatch a slash command, either natively (captured output lands in
/// the transcript) or by briefly handing the real terminal back for
/// commands that need blocking `stdin`/hidden-password input. Returns
/// `Ok(false)` when the session should end (`/exit`).
async fn run_command(ui: &mut TuiCore, session: &mut Session, name: &str, args: &[String]) -> Result<bool> {
    if name == "help" || name == "?" || name == "h" {
        run_help(ui, session, HelpTab::Commands).await?;
        return Ok(true);
    }

    // Rich, in-TUI pickers for the commands that most benefit from being
    // arrow-key-selectable rather than a wall of printed text. Only takes
    // over when there's actually something to *pick* — `/model gpt-4o`
    // (an argument already given) stays on the fast, plain synchronous
    // path below, same as always.
    match name {
        "connect" if args.is_empty() => return Ok(connect_wizard(ui, session).await?),
        "model" if args.is_empty() => return Ok(model_picker(ui, session).await?),
        "models" => return Ok(models_browser(ui, session).await?),
        "provider" if args.is_empty() => return Ok(provider_picker(ui, session).await?),
        "resume" if args.is_empty() => return Ok(resume_picker(ui, session).await?),
        "rewind" if args.is_empty() => return Ok(rewind_picker(ui, session).await?),
        _ => {}
    }

    let spec = commands::find(name);
    if spec.is_none() {
        let custom = crate::cli::custom_commands::discover(session.agent.workspace());
        if let Some(cmd) = crate::cli::custom_commands::find(&custom, name) {
            let expanded = crate::cli::custom_commands::expand(cmd, args);
            return run_turn(ui, session, &expanded).await;
        }
    }

    let outcome = if spec.map(|s| s.needs_terminal).unwrap_or(false) {
        run_suspended(ui, session, name, args)?
    } else {
        ui.run_captured(session, name, args)
    };

    ui.sync_header(session);
    ui.draw()?;
    Ok(outcome != CommandOutcome::Exit)
}

/// Leave the alternate screen, run a command with real blocking
/// stdin/stdout (so `/connect`'s prompts, `/api set`'s hidden password
/// read, etc. work exactly as they would outside a TUI at all), then
/// return. See the module docs for why this exists instead of a fully
/// raw-mode-safe reimplementation of every interactive prompt.
fn run_suspended(ui: &mut TuiCore, session: &mut Session, name: &str, args: &[String]) -> Result<CommandOutcome> {
    use std::io::Write as _;

    use crossterm::terminal::{Clear, ClearType};
    disable_raw_mode()?;
    execute!(io::stdout(), DisableMouseCapture, LeaveAlternateScreen, Clear(ClearType::All), crossterm::cursor::MoveTo(0, 0))?;

    // Drop the live EventStream before the blocking, synchronous phase
    // below. Crossterm's EventStream keeps a background thread that reads
    // raw terminal input whenever the stream is (or was recently) being
    // polled — one still winding down from a turn that was mid-poll right
    // before we got here can race the plain `io::stdin().read_line()`
    // call below for the same incoming bytes. That race is the actual
    // cause of "I pressed y/Enter and nothing happened" during a
    // suspended command: some keystrokes were silently getting consumed
    // by the old stream's reader instead of reaching this read. Dropping
    // it here signals that thread to shut down cleanly (see
    // `EventStream`'s `Drop` impl) before any blocking read starts; a
    // fresh one — safely idle until actually polled again — replaces it
    // both here and once we're back in raw mode below.
    ui.events = EventStream::new();

    let outcome = commands::dispatch(session, name, args);
    print!("\n{}", "── press Enter to return to REXO ──".to_string());
    io::stdout().flush().ok();
    let mut discard = String::new();
    let _ = io::stdin().read_line(&mut discard);

    execute!(io::stdout(), EnterAlternateScreen, EnableMouseCapture)?;
    enable_raw_mode()?;
    ui.events = EventStream::new();
    ui.terminal.clear()?;

    match outcome {
        Ok(o) => Ok(o),
        Err(e) => {
            ui.note(&format!("/{name} failed: {e}"), NoteLevel::Error);
            Ok(CommandOutcome::Continue)
        }
    }
}

// ---------------------------------------------------------------------
// Rich, in-TUI pickers for /connect, /model, /models, /provider — arrow
// keys + type-to-filter, drawn directly over the persistent screen using
// the *same* `Terminal`/`EventStream` the rest of the session already
// owns (see `cli::picker`'s module docs for why that sharing matters).
// None of these ever leave the alternate screen the way `run_suspended`
// commands do — including the API key prompt, which reads its own raw
// key events and masks them locally instead of shelling out to
// `rpassword` at all.
// ---------------------------------------------------------------------

/// `/connect`: pick a catalog provider (or "Custom endpoint"), enter a
/// base URL/key where needed, optionally browse live-discovered models,
/// then confirm permanent-vs-session — all without leaving the screen.
async fn connect_wizard(ui: &mut TuiCore, session: &mut Session) -> Result<bool> {
    let mut items: Vec<picker::PickerItem> = crate::providers::catalog::CATALOG
        .iter()
        .map(|p| {
            let sub = if p.notes.is_empty() { (if p.requires_key { "requires a key" } else { "no key needed" }).to_string() } else { p.notes.to_string() };
            picker::PickerItem::with_sub(p.display_name, sub, p.key)
        })
        .collect();
    items.push(picker::PickerItem::with_sub("Custom endpoint…", "any OpenAI-compatible API you point at yourself", "__custom__"));

    let accent = ui.header.accent;
    let outcome = picker::run_select(
        &mut ui.terminal,
        &mut ui.events,
        picker::SelectOptions {
            title: "Connect a provider".to_string(),
            items,
            accent,
            allow_custom: false,
            subtitle: Some(format!("{} providers in the catalog — type to filter", crate::providers::catalog::CATALOG.len())),
            search_placeholder: "search providers…",
        },
    )
    .await?;

    let key = match outcome {
        picker::PickerOutcome::Selected(key) => key,
        _ => {
            ui.terminal.clear()?;
            return Ok(true);
        }
    };

    let result = if key == "__custom__" {
        connect_custom_interactive(ui, session).await
    } else {
        let preset = crate::providers::catalog::find(&key).expect("picker only offers catalog keys");
        connect_preset_interactive(ui, session, preset).await
    };

    ui.terminal.clear()?;
    match result {
        Ok(Some(msg)) => ui.note(&msg, NoteLevel::Info),
        Ok(None) => {}
        Err(e) => ui.note(&format!("/connect failed: {e}"), NoteLevel::Error),
    }
    ui.sync_header(session);
    ui.draw()?;
    Ok(true)
}

async fn connect_preset_interactive(ui: &mut TuiCore, session: &mut Session, preset: &'static crate::providers::catalog::ProviderPreset) -> Result<Option<String>> {
    let accent = ui.header.accent;
    let kind = preset.kind;

    let base_url = match preset.base_url {
        Some(url) if kind == "nvidia" => Some(url.to_string()),
        Some(url) => {
            match picker::run_text_input(&mut ui.terminal, &mut ui.events, &preset.display_name.to_string(), "Base URL", Some(url), false, accent).await? {
                Some(v) => Some(v),
                None => return Ok(Some("Cancelled.".to_string())),
            }
        }
        None => match picker::run_text_input(&mut ui.terminal, &mut ui.events, &preset.display_name.to_string(), "Base URL (full chat-completions endpoint)", None, false, accent).await? {
            Some(v) if !v.trim().is_empty() => Some(v),
            _ => return Ok(Some("A base URL is required for this provider — cancelled.".to_string())),
        },
    };

    let mut api_key = None;
    if preset.requires_key {
        match picker::run_text_input(&mut ui.terminal, &mut ui.events, &preset.display_name.to_string(), "API key", None, true, accent).await? {
            Some(v) if !v.is_empty() => api_key = Some(v),
            _ => {}
        }
    }

    let model = pick_model_or_manual(ui, session, base_url.as_deref(), api_key.as_deref()).await?;

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
    let permanent = picker::run_confirm(&mut ui.terminal, &mut ui.events, "Save this connection", "Save permanently for every future run, or just this session?", true, accent).await?;
    Ok(Some(commands::apply_connection(session, preset.key, profile, api_key, permanent).message()))
}

async fn connect_custom_interactive(ui: &mut TuiCore, session: &mut Session) -> Result<Option<String>> {
    let accent = ui.header.accent;
    let Some(name) = picker::run_text_input(&mut ui.terminal, &mut ui.events, "Custom endpoint", "Name for this provider", None, false, accent).await? else {
        return Ok(Some("Cancelled.".to_string()));
    };
    if name.trim().is_empty() {
        return Ok(Some("Cancelled.".to_string()));
    }
    let key_slug = commands::slugify(&name);

    let Some(base_url) = picker::run_text_input(&mut ui.terminal, &mut ui.events, &name, "Base URL (full chat-completions endpoint)", None, false, accent).await? else {
        return Ok(Some("Cancelled.".to_string()));
    };
    if base_url.trim().is_empty() {
        return Ok(Some("A base URL is required — cancelled.".to_string()));
    }

    let requires_key = picker::run_confirm(&mut ui.terminal, &mut ui.events, &name, "Does this endpoint require an API key?", true, accent).await?;
    let mut api_key = None;
    if requires_key {
        if let Some(v) = picker::run_text_input(&mut ui.terminal, &mut ui.events, &name, "API key", None, true, accent).await? {
            if !v.is_empty() {
                api_key = Some(v);
            }
        }
    }

    let model = pick_model_or_manual(ui, session, Some(base_url.as_str()), api_key.as_deref()).await?;

    let profile = crate::config::global::ProviderProfile {
        display_name: name.clone(),
        kind: "openai_compatible".to_string(),
        base_url: Some(base_url),
        model,
        temperature: None,
        max_tokens: None,
        reasoning_effort: None,
        credential_key: None,
        requires_key,
    };
    let permanent = picker::run_confirm(&mut ui.terminal, &mut ui.events, "Save this connection", "Save permanently for every future run, or just this session?", true, accent).await?;
    Ok(Some(commands::apply_connection(session, &key_slug, profile, api_key, permanent).message()))
}

/// Offer a live-discovered, arrow-key-selectable model list when
/// `base_url` supports it; otherwise (or if the user backs out) fall
/// through to a free-text model ID box. Manual entry always has the last
/// word — discovery is a convenience, never a requirement.
async fn pick_model_or_manual(ui: &mut TuiCore, _session: &mut Session, base_url: Option<&str>, api_key: Option<&str>) -> Result<Option<String>> {
    let accent = ui.header.accent;
    if let Some(url) = base_url {
        ui.note("Fetching model list…", NoteLevel::Info);
        ui.draw()?;
        let discovered = commands::fetch_models_blocking(url, api_key.unwrap_or(""));
        if let Ok(models) = discovered {
            if !models.is_empty() {
                let items: Vec<picker::PickerItem> = models
                    .iter()
                    .map(|m| match &m.owned_by {
                        Some(owner) if !owner.is_empty() => picker::PickerItem::with_sub(m.id.clone(), format!("owned by {owner}"), m.id.clone()),
                        _ => picker::PickerItem::new(m.id.clone(), m.id.clone()),
                    })
                    .collect();
                let outcome = picker::run_select(
                    &mut ui.terminal,
                    &mut ui.events,
                    picker::SelectOptions {
                        title: "Choose a model".to_string(),
                        subtitle: Some(format!("{} models available — type to filter, or Ctrl+Enter to use typed text as a custom ID", models.len())),
                        items,
                        accent,
                        allow_custom: true,
                        search_placeholder: "search models…",
                    },
                )
                .await?;
                return Ok(match outcome {
                    picker::PickerOutcome::Selected(id) | picker::PickerOutcome::Custom(id) => Some(id),
                    picker::PickerOutcome::Cancelled => None,
                });
            }
        }
        ui.note("Model discovery isn't available here — enter one manually.", NoteLevel::Info);
    }
    picker::run_text_input(&mut ui.terminal, &mut ui.events, "Model", "Model ID (blank to skip for now)", None, false, accent).await
}

/// `/model` with no arguments: fetches the live model list for the
/// current provider and opens the same arrow-key picker, applying the
/// choice immediately — "insta change," no confirmation round-trip.
async fn model_picker(ui: &mut TuiCore, session: &mut Session) -> Result<bool> {
    let Some(url) = commands::effective_endpoint(session) else {
        ui.note("No base URL configured for the current provider — run /connect first.", NoteLevel::Error);
        return Ok(true);
    };
    let api_key = session.resolve_api_key().unwrap_or_default();
    let chosen = pick_model_or_manual(ui, session, Some(&url), Some(api_key.as_str())).await?;
    ui.terminal.clear()?;
    if let Some(model) = chosen {
        session.config.model.model = Some(model.clone());
        match session.rebuild_provider() {
            Ok(()) => {
                ui.note(&format!("Model set to {model}"), NoteLevel::Info);
                if let Some(profile_name) = commands::persistable_model_profile(session) {
                    let accent = ui.header.accent;
                    let save = picker::run_confirm(&mut ui.terminal, &mut ui.events, "Save this model?", "Save as the default model for this provider too?", false, accent).await?;
                    ui.terminal.clear()?;
                    if save {
                        match commands::persist_model_choice(session, &profile_name, &model) {
                            Ok(()) => ui.note("Saved as the default model for this provider.", NoteLevel::Info),
                            Err(e) => ui.note(&format!("Couldn't save: {e}"), NoteLevel::Error),
                        }
                    }
                }
            }
            Err(e) => ui.note(&format!("Couldn't switch models: {e}"), NoteLevel::Error),
        }
    }
    ui.sync_header(session);
    ui.draw()?;
    Ok(true)
}

/// `/models`: browse-only version of the same picker (no "custom ID"
/// escape hatch, since here you're looking, not necessarily switching) —
/// but selecting one still switches to it immediately, same as
/// `/model`'s picker, since there's no reason to make the user then go
/// type `/model <id>` themselves after finding it in the list.
async fn models_browser(ui: &mut TuiCore, session: &mut Session) -> Result<bool> {
    model_picker(ui, session).await
}

/// `/provider` with no arguments: pick among saved profiles (global
/// config), applying the choice for this session immediately.
async fn provider_picker(ui: &mut TuiCore, session: &mut Session) -> Result<bool> {
    let profiles: Vec<(String, crate::config::global::ProviderProfile)> = session.config.global.providers.clone().into_iter().collect();
    if profiles.is_empty() {
        ui.note("No saved provider profiles yet — run /connect to add one.", NoteLevel::Info);
        return Ok(true);
    }
    let items: Vec<picker::PickerItem> = profiles
        .iter()
        .map(|(key, p)| {
            let current = if session.config.global.default_provider.as_deref() == Some(key.as_str()) { " (default)" } else { "" };
            picker::PickerItem::with_sub(format!("{}{current}", p.display_name), p.model.clone().unwrap_or_else(|| "no model set".to_string()), key.clone())
        })
        .collect();
    let accent = ui.header.accent;
    let outcome = picker::run_select(
        &mut ui.terminal,
        &mut ui.events,
        picker::SelectOptions {
            title: "Switch provider".to_string(),
            items,
            accent,
            allow_custom: false,
            subtitle: Some("Saved profiles — type to filter".to_string()),
            search_placeholder: "search profiles…",
        },
    )
    .await?;
    ui.terminal.clear()?;
    if let picker::PickerOutcome::Selected(key) = outcome {
        if let Some(profile) = session.config.global.providers.get(&key).cloned() {
            commands::apply_profile_session_only(session, &key, &profile, None);
            match session.rebuild_provider() {
                Ok(()) => ui.note(&format!("Switched to {}", profile.display_name), NoteLevel::Info),
                Err(e) => ui.note(&format!("Couldn't switch providers: {e}"), NoteLevel::Error),
            }
        }
    }
    ui.sync_header(session);
    ui.draw()?;
    Ok(true)
}

/// `/resume` with no arguments: pick a saved session (autosave or named)
/// and load it, replacing the current conversation. `/resume save <name>`,
/// `/resume list`, and `/resume delete <name>` — the argument-taking
/// forms, which don't need a picker — go through the plain synchronous
/// `commands::resume_cmd` instead (see the dispatch table above).
async fn resume_picker(ui: &mut TuiCore, session: &mut Session) -> Result<bool> {
    let saved = crate::cli::sessions::list(session.agent.workspace());
    if saved.is_empty() {
        ui.note("No saved sessions yet — one gets autosaved after your first message.", NoteLevel::Info);
        return Ok(true);
    }
    let items: Vec<picker::PickerItem> = saved
        .iter()
        .map(|s| {
            let label = if s.is_autosave { "autosave (most recent)".to_string() } else { s.name.clone() };
            let sub = format!("{} · {} turn(s) · {}", crate::cli::sessions::format_saved_at(&s.saved_at), s.turn_count, s.preview);
            picker::PickerItem::with_sub(label, sub, s.name.clone())
        })
        .collect();
    let accent = ui.header.accent;
    let outcome = picker::run_select(
        &mut ui.terminal,
        &mut ui.events,
        picker::SelectOptions {
            title: "Resume a session".to_string(),
            items,
            accent,
            allow_custom: false,
            subtitle: Some("Replaces the current conversation — type to filter".to_string()),
            search_placeholder: "search sessions…",
        },
    )
    .await?;
    ui.terminal.clear()?;
    if let picker::PickerOutcome::Selected(name) = outcome {
        match saved.iter().find(|s| s.name == name).map(|s| crate::cli::sessions::load(&s.path)) {
            Some(Ok(snapshot)) => {
                session.history = snapshot.history;
                session.loaded_skills.clear();
                session.active_session_name = if name == "autosave" { None } else { Some(name.clone()) };
                ui.note(&format!("Resumed '{name}' ({} messages).", session.history.len()), NoteLevel::Info);
            }
            Some(Err(e)) => ui.note(&format!("Couldn't load '{name}': {e}"), NoteLevel::Error),
            None => {}
        }
    }
    ui.sync_header(session);
    ui.draw()?;
    Ok(true)
}

/// `/rewind` with no arguments: pick an earlier point in the *current*
/// conversation (each prior message you sent is a checkpoint) and
/// truncate history back to just before it, so you can continue from
/// there with a different message instead. Conversation-only — this does
/// **not** revert any file edits the agent made after that point; doing
/// that too would need workspace snapshots (git-based or otherwise),
/// which this project doesn't have yet (see the README's roadmap notes).
async fn rewind_picker(ui: &mut TuiCore, session: &mut Session) -> Result<bool> {
    let checkpoints: Vec<(usize, String)> = session
        .history
        .iter()
        .enumerate()
        .filter(|(_, m)| m.role == crate::providers::Role::User)
        .map(|(i, m)| {
            let preview = m.content.as_deref().unwrap_or("").trim().replace('\n', " ");
            let preview = if preview.chars().count() > 70 { format!("{}…", preview.chars().take(70).collect::<String>()) } else { preview };
            (i, preview)
        })
        .collect();
    if checkpoints.is_empty() {
        ui.note("Nothing to rewind to yet — send a message first.", NoteLevel::Info);
        return Ok(true);
    }
    let items: Vec<picker::PickerItem> = checkpoints
        .iter()
        .enumerate()
        .map(|(n, (idx, preview))| picker::PickerItem::with_sub(format!("#{}", n + 1), preview.clone(), idx.to_string()))
        .collect();
    let accent = ui.header.accent;
    let outcome = picker::run_select(
        &mut ui.terminal,
        &mut ui.events,
        picker::SelectOptions {
            title: "Rewind the conversation".to_string(),
            items,
            accent,
            allow_custom: false,
            subtitle: Some("Removes everything after the point you pick — file edits aren't reverted".to_string()),
            search_placeholder: "search messages…",
        },
    )
    .await?;
    ui.terminal.clear()?;
    if let picker::PickerOutcome::Selected(idx_str) = outcome {
        if let Ok(idx) = idx_str.parse::<usize>() {
            let removed = session.history.len() - idx;
            session.history.truncate(idx);
            ui.note(&format!("Rewound — removed {removed} message(s). File edits since then are still on disk."), NoteLevel::Info);
            crate::cli::sessions::autosave(session);
        }
    }
    ui.sync_header(session);
    ui.draw()?;
    Ok(true)
}

fn maybe_autocompact(ui: &mut TuiCore, session: &mut Session) {
    let Some(threshold) = session.ui.autocompact_threshold else {
        return;
    };
    let tool_results = session
        .history
        .iter()
        .filter(|m| m.role == crate::providers::Role::Tool)
        .count();
    if tool_results < threshold {
        return;
    }
    let shrunk = commands::run_compact_for_autocompact(session);
    if shrunk > 0 {
        ui.note(&format!("Auto-compacted {shrunk} older tool result(s) (see /autocompact).", ), NoteLevel::Info);
    }
}

/// Read one line from the input box: handles typing, history, Tab
/// autocomplete, Ctrl+C, PageUp/PageDown scrolling, and `/help`. Returns
/// `Ok(None)` when the session should end (double Ctrl+C on an empty
/// line).
async fn read_input(ui: &mut TuiCore, session: &mut Session) -> Result<Option<String>> {
    ui.refresh_autocomplete(session);
    loop {
        ui.draw()?;
        match ui.events.next().await {
            Some(Ok(Event::Key(key))) if key.kind == KeyEventKind::Press => {
                if is_ctrl_c(&key) {
                    if ui.input.is_empty() {
                        let now = Instant::now();
                        let recent = ui.last_ctrl_c.map(|t| now.duration_since(t) < Duration::from_secs(2)).unwrap_or(false);
                        if recent {
                            return Ok(None);
                        }
                        ui.last_ctrl_c = Some(now);
                        ui.note("Press Ctrl+C again to exit, or type /exit.", NoteLevel::Info);
                    } else {
                        ui.clear_input();
                        ui.refresh_autocomplete(session);
                    }
                    continue;
                }
                ui.last_ctrl_c = None;

                let candidates = ui.autocomplete_cache.clone();
                match key.code {
                    KeyCode::Enter => {
                        return Ok(Some(ui.take_input()));
                    }
                    // A bare `?` as the very first character — before
                    // anything else has been typed — jumps straight to
                    // the shortcut list, same as typing the full `{?}`
                    // and pressing Enter. Only at the very start of the
                    // line, so `?` still works as an ordinary character
                    // (e.g. "what's this file?") everywhere else.
                    KeyCode::Char('?') if ui.input.is_empty() && !key.modifiers.contains(KeyModifiers::CONTROL) && !key.modifiers.contains(KeyModifiers::ALT) => {
                        return Ok(Some("{?}".to_string()));
                    }
                    // Quick pickers — jump straight to /model or
                    // /provider from anywhere, same arrow-key UI either
                    // command opens on its own.
                    KeyCode::Char('m') if key.modifiers.contains(KeyModifiers::ALT) => {
                        return Ok(Some("/model".to_string()));
                    }
                    KeyCode::Char('p') if key.modifiers.contains(KeyModifiers::ALT) => {
                        return Ok(Some("/provider".to_string()));
                    }
                    // "Selection mode": releases the terminal's mouse
                    // capture so its *own* click-drag text selection and
                    // copy work normally — see `toggle_selection_mode`'s
                    // docs for why that's the real fix for "I can't
                    // select text in the TUI," rather than REXO trying to
                    // reimplement clipboard selection itself.
                    KeyCode::Char('y') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                        ui.toggle_selection_mode()?;
                        continue;
                    }
                    KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                        ui.insert_char(c);
                        ui.refresh_autocomplete(session);
                    }
                    KeyCode::Backspace => {
                        ui.backspace();
                        ui.refresh_autocomplete(session);
                    }
                    KeyCode::Delete => {
                        ui.delete_forward();
                        ui.refresh_autocomplete(session);
                    }
                    KeyCode::Left => ui.move_left(),
                    KeyCode::Right => ui.move_right(),
                    KeyCode::Home => ui.cursor = 0,
                    KeyCode::End => ui.cursor = ui.input.len(),
                    KeyCode::Char('l') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                        ui.terminal.clear()?;
                    }
                    KeyCode::Tab => {
                        if let Some(name) = candidates.get(ui.autocomplete_selected) {
                            accept_autocomplete(ui, name);
                            ui.refresh_autocomplete(session);
                        }
                    }
                    KeyCode::Up if !candidates.is_empty() => {
                        ui.autocomplete_selected = ui.autocomplete_selected.saturating_sub(1);
                    }
                    KeyCode::Down if !candidates.is_empty() => {
                        if ui.autocomplete_selected + 1 < candidates.len() {
                            ui.autocomplete_selected += 1;
                        }
                    }
                    KeyCode::Up => {
                        ui.history_up();
                        ui.refresh_autocomplete(session);
                    }
                    KeyCode::Down => {
                        ui.history_down();
                        ui.refresh_autocomplete(session);
                    }
                    KeyCode::PageUp => ui.scroll_up(),
                    KeyCode::PageDown => ui.scroll_down(),
                    KeyCode::Esc => {
                        ui.clear_input();
                        ui.refresh_autocomplete(session);
                    }
                    _ => {}
                }
            }
            Some(Ok(Event::Resize(_, _))) => {}
            Some(Ok(Event::Mouse(m))) => match m.kind {
                MouseEventKind::ScrollUp => ui.scroll_up(),
                MouseEventKind::ScrollDown => ui.scroll_down(),
                _ => {}
            },
            Some(Ok(_)) => {}
            Some(Err(_)) | None => return Ok(None),
        }
    }
}

fn accept_autocomplete(ui: &mut TuiCore, name: &str) {
    if let Some(at_idx) = ui.input[..ui.cursor.min(ui.input.len())].rfind('@') {
        if at_fragment(&ui.input, ui.cursor).is_some() {
            ui.input.truncate(at_idx + 1);
            if let Some(stripped) = name.strip_suffix('/') {
                ui.input.push_str(stripped);
                ui.input.push('/'); // still inside a directory — keep exploring
            } else {
                ui.input.push_str(name);
                ui.input.push(' ');
            }
            ui.cursor = ui.input.len();
            ui.autocomplete_selected = 0;
            return;
        }
    }
    if ui.input.contains(' ') {
        // Path completion: replace the last path segment being typed
        // (everything after the final space or slash) with `name`.
        let prefix_end = ui.input.rfind([' ', '/']).map(|i| i + 1).unwrap_or(ui.input.len());
        ui.input.truncate(prefix_end);
        ui.input.push_str(name);
    } else {
        // Command-name completion: the whole line so far is `/<fragment>`.
        ui.input = format!("/{name} ");
    }
    ui.cursor = ui.input.len();
    ui.autocomplete_selected = 0;
}

// ---------------------------------------------------------------------
// /help — full-screen overlay
// ---------------------------------------------------------------------

async fn run_help(ui: &mut TuiCore, session: &mut Session, initial_tab: HelpTab) -> Result<()> {
    ui.help = Some(HelpState { tab: initial_tab, selected: 0, workspace: session.agent.workspace().to_path_buf() });
    ui.draw()?;

    loop {
        match ui.events.next().await {
            Some(Ok(Event::Key(key))) if key.kind == KeyEventKind::Press => {
                let Some(help) = &mut ui.help else { break };
                match key.code {
                    KeyCode::Esc | KeyCode::Char('q') => break,
                    KeyCode::Left => {
                        help.tab = match help.tab {
                            HelpTab::General => HelpTab::Custom,
                            HelpTab::Commands => HelpTab::General,
                            HelpTab::Custom => HelpTab::Commands,
                        };
                    }
                    KeyCode::Right | KeyCode::Tab => {
                        help.tab = match help.tab {
                            HelpTab::General => HelpTab::Commands,
                            HelpTab::Commands => HelpTab::Custom,
                            HelpTab::Custom => HelpTab::General,
                        };
                    }
                    KeyCode::Down => {
                        let max = help_command_rows().len().saturating_sub(1);
                        help.selected = (help.selected + 1).min(max);
                    }
                    KeyCode::Up => help.selected = help.selected.saturating_sub(1),
                    _ if is_ctrl_c(&key) => break,
                    _ => {}
                }
                ui.draw()?;
            }
            Some(Ok(Event::Resize(_, _))) => {
                ui.draw()?;
            }
            Some(Ok(Event::Mouse(m))) => {
                if let Some(help) = &mut ui.help {
                    match m.kind {
                        MouseEventKind::ScrollDown => {
                            let max = help_command_rows().len().saturating_sub(1);
                            help.selected = (help.selected + 1).min(max);
                        }
                        MouseEventKind::ScrollUp => help.selected = help.selected.saturating_sub(1),
                        _ => {}
                    }
                }
                ui.draw()?;
            }
            Some(Ok(_)) => {}
            Some(Err(_)) | None => break,
        }
    }

    ui.help = None;
    ui.draw()?;
    Ok(())
}

/// Non-hidden, non-planned-only commands the "Commands" tab lists, in
/// declaration order (roughly grouped by theme already in `COMMANDS`).
fn help_command_rows() -> Vec<&'static commands::CommandSpec> {
    commands::COMMANDS.iter().filter(|c| !c.hidden).collect()
}

fn help_status_suffix(spec: &commands::CommandSpec) -> &'static str {
    if spec.status == Status::Planned {
        " (planned)"
    } else {
        ""
    }
}
