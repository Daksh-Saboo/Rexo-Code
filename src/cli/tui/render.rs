//! Pure drawing functions. Nothing here mutates [`UiState`] — every
//! function just reads it and a target [`Rect`], so this module doubles
//! as documentation of exactly what's on screen at any given moment.

use ansi_to_tui::IntoText;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear as ClearWidget, Paragraph, Wrap};
use ratatui::Frame;

use super::{EntryKind, HelpState, HelpTab, PermissionView, UiState, SPINNER_FRAMES};
use crate::agent::NoteLevel;

const HEADER_HEIGHT: u16 = 6;
const INPUT_HEIGHT: u16 = 3;
const HINT_HEIGHT: u16 = 1;

/// The header mascot's two-character "eyes," reacting to whatever the
/// session is actually doing right now rather than sitting static:
/// thinking (waiting on the first token), reasoning/writing (streaming
/// text in), a permission prompt waiting on the user, a recent
/// success/warning/error note, or — most of the time — idle, where a
/// wall-clock-driven blink cycle gives it a little life on every redraw
/// that already happens (typing, streaming, tool calls) without needing
/// a dedicated animation timer. A genuinely continuous idle animation
/// while the user isn't touching anything at all would need a periodic
/// redraw the event loop doesn't currently have — this is real
/// expressiveness tied to real activity, not a claim of that.
fn mascot_glyph(core: &UiState) -> (&'static str, Color) {
    if core.permission.is_some() {
        return ("??", Color::Yellow);
    }
    if core.spinner.is_some() {
        const THINK_FRAMES: [&str; 4] = ["◔◔", "◑◑", "◕◕", "●●"];
        let frame = core.spinner.as_ref().map(|s| s.frame).unwrap_or(0);
        return (THINK_FRAMES[frame % THINK_FRAMES.len()], core.header.accent);
    }
    if let Some(pending) = &core.pending {
        let tick = wall_clock_tick(2);
        return match pending.kind {
            EntryKind::Reasoning => (["∴∴", "∵∵"][tick % 2], core.header.accent),
            _ => ([" ✎", "✎ "][tick % 2], core.header.accent),
        };
    }
    match core.last_note {
        Some(NoteLevel::Error) => ("××", Color::Red),
        Some(NoteLevel::Warning) => ("!!", Color::Yellow),
        Some(NoteLevel::Info) => ("^^", Color::Green),
        None => {
            const IDLE_FRAMES: [&str; 4] = ["◉◉", "◉◉", "◉◉", "──"];
            (IDLE_FRAMES[wall_clock_tick(3) % IDLE_FRAMES.len()], core.header.accent)
        }
    }
}

/// A coarse, monotonically-advancing "tick" derived from the real clock
/// (one step every `secs_per_tick` seconds) — enough to cycle an
/// animation frame across separate `draw()` calls without threading a
/// frame counter through every call site.
fn wall_clock_tick(secs_per_tick: u64) -> usize {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    (secs / secs_per_tick.max(1)) as usize
}

pub(super) fn draw(f: &mut Frame<'_>, core: &UiState) {
    if let Some(help) = &core.help {
        draw_help(f, core, help);
        return;
    }

    let area = f.size();
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(HEADER_HEIGHT),
            Constraint::Min(3),
            Constraint::Length(INPUT_HEIGHT),
            Constraint::Length(HINT_HEIGHT),
        ])
        .split(area);

    draw_header(f, chunks[0], core);
    draw_transcript(f, chunks[1], core);
    draw_input(f, chunks[2], core);
    draw_hint(f, chunks[3], core);
    draw_autocomplete(f, chunks[1], core);

    if let Some(p) = &core.permission {
        draw_permission_modal(f, area, p);
    }
}

// ---------------------------------------------------------------------
// header
// ---------------------------------------------------------------------

fn draw_header(f: &mut Frame<'_>, area: Rect, core: &UiState) {
    let block = Block::default().borders(Borders::ALL).border_style(Style::default().fg(core.header.accent));
    let inner = block.inner(area);
    f.render_widget(block, area);

    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(6), Constraint::Min(0)])
        .split(inner);

    let (eyes, mascot_color) = mascot_glyph(core);
    let mascot_style = Style::default().fg(mascot_color).add_modifier(Modifier::BOLD);
    let mascot = Paragraph::new(vec![
        Line::from(Span::styled("┏━━┓", mascot_style)),
        Line::from(Span::styled(format!("┃{eyes}┃"), mascot_style)),
        Line::from(Span::styled("┗━━┛", mascot_style)),
    ]);
    f.render_widget(mascot, cols[0]);

    let title_line = match &core.header.title {
        Some(t) => format!("Rexo Code v{}  —  {t}", core.header.version),
        None => format!("Rexo Code v{}", core.header.version),
    };
    let text = vec![
        Line::from(Span::styled(title_line, Style::default().add_modifier(Modifier::BOLD))),
        Line::from(Span::raw(core.header.provider_desc.clone())),
        Line::from(Span::styled(core.header.workspace.clone(), Style::default().fg(Color::DarkGray))),
        Line::from(vec![
            Span::raw("Edits: "),
            perm_span(core.header.edit_auto),
            Span::raw("  Terminal: "),
            perm_span(core.header.terminal_auto),
            Span::raw("  Git writes: "),
            perm_span(core.header.git_auto),
        ]),
    ];
    f.render_widget(Paragraph::new(text), cols[1]);
}

fn perm_span(auto: bool) -> Span<'static> {
    if auto {
        Span::styled("auto", Style::default().fg(Color::Green))
    } else {
        Span::styled("ask", Style::default().fg(Color::Yellow))
    }
}

// ---------------------------------------------------------------------
// transcript
// ---------------------------------------------------------------------

fn draw_transcript(f: &mut Frame<'_>, area: Rect, core: &UiState) {
    let mut lines: Vec<Line<'static>> = Vec::new();
    for entry in &core.transcript {
        if core.focus_mode && entry.kind.hidden_in_focus_mode() {
            continue;
        }
        lines.extend(entry.lines.iter().cloned());
        lines.push(Line::default());
    }
    if let Some(pending) = &core.pending {
        if !(core.focus_mode && pending.kind.hidden_in_focus_mode()) {
            lines.extend(wrap_plain(&pending.raw, pending.kind));
        }
    }
    if let Some(hint) = &core.preparing_hint {
        lines.push(Line::from(Span::styled(
            hint.clone(),
            Style::default().fg(Color::DarkGray).add_modifier(Modifier::ITALIC),
        )));
    }
    if let Some(spinner) = &core.spinner {
        let glyph = SPINNER_FRAMES[spinner.frame % SPINNER_FRAMES.len()];
        lines.push(Line::from(Span::styled(
            format!("{glyph} Thinking… ({}s · Ctrl+C to cancel)", spinner.elapsed_secs),
            Style::default().fg(core.header.accent),
        )));
    }
    if lines.is_empty() {
        lines.push(Line::from(Span::styled(
            "Nothing here yet — type a task below, or /help for commands.",
            Style::default().fg(Color::DarkGray),
        )));
    }

    let total_rows = wrapped_row_count(&lines, area.width);
    let height = area.height;
    let max_scroll = total_rows.saturating_sub(height);
    let scroll = core.scroll_from_bottom.min(max_scroll);
    let top = max_scroll.saturating_sub(scroll);

    let paragraph = Paragraph::new(lines).wrap(Wrap { trim: false }).scroll((top, 0));
    f.render_widget(paragraph, area);
}

fn wrapped_row_count(lines: &[Line<'_>], width: u16) -> u16 {
    let width = width.max(1);
    lines
        .iter()
        .map(|l| {
            let w = (l.width().max(1)) as u16;
            w.div_ceil(width)
        })
        .sum()
}

/// Turn a plain (non-ANSI) string into styled lines for a given transcript
/// entry kind — used both for finalized entries built directly by
/// `AgentUi` methods and for the in-flight streaming `Pending` block.
pub(super) fn wrap_plain(text: &str, kind: EntryKind) -> Vec<Line<'static>> {
    let body_style = match kind {
        EntryKind::UserPrompt => Style::default().fg(Color::White).add_modifier(Modifier::BOLD),
        EntryKind::Reasoning => Style::default().fg(Color::DarkGray).add_modifier(Modifier::ITALIC),
        EntryKind::Answer => Style::default().fg(Color::White),
        _ => Style::default(),
    };
    let raw_lines: Vec<&str> = if text.is_empty() { vec![""] } else { text.lines().collect() };
    raw_lines
        .into_iter()
        .enumerate()
        .map(|(i, l)| {
            if kind == EntryKind::UserPrompt && i == 0 {
                Line::from(vec![
                    Span::styled("> ", Style::default().fg(Color::Green).add_modifier(Modifier::BOLD)),
                    Span::styled(l.to_string(), body_style),
                ])
            } else {
                Line::from(Span::styled(l.to_string(), body_style))
            }
        })
        .collect()
}

/// Parse `colored`-crate ANSI output (from a captured slash-command
/// handler — see `cli::output`) into styled lines. Falls back to plain
/// text if parsing ever fails, rather than dropping the output.
pub(super) fn wrap_ansi(captured: &str) -> Vec<Line<'static>> {
    match captured.into_text() {
        Ok(text) => text.lines,
        Err(_) => captured.lines().map(|l| Line::from(l.to_string())).collect(),
    }
}

pub(super) fn styled_line(text: String, color: Color, bold: bool) -> Line<'static> {
    let mut style = Style::default().fg(color);
    if bold {
        style = style.add_modifier(Modifier::BOLD);
    }
    Line::from(Span::styled(text, style))
}

// ---------------------------------------------------------------------
// input box + hint line
// ---------------------------------------------------------------------

fn draw_input(f: &mut Frame<'_>, area: Rect, core: &UiState) {
    let title = if core.permission.is_some() {
        " permission needed "
    } else if core.shell_mode {
        " SHELL MODE — ! or 'exit' to leave "
    } else {
        ""
    };
    let border_color = if core.shell_mode { Color::Yellow } else { core.header.accent };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(border_color))
        .title(title);
    let inner = block.inner(area);
    f.render_widget(block, area);

    if core.spinner.is_some() || core.pending.is_some() {
        let p = Paragraph::new(Line::from(Span::styled(
            "waiting for the current turn to finish…",
            Style::default().fg(Color::DarkGray),
        )));
        f.render_widget(p, inner);
        return;
    }

    let display = if core.input.is_empty() && core.transcript.is_empty() {
        Line::from(Span::styled(format!("Try \"{}\"", core.suggestion), Style::default().fg(Color::DarkGray)))
    } else {
        Line::from(Span::raw(core.input.clone()))
    };
    f.render_widget(Paragraph::new(display), inner);

    let cursor_col = core.input[..core.cursor].chars().count() as u16;
    let cursor_x = (inner.x + cursor_col).min(inner.x + inner.width.saturating_sub(1));
    f.set_cursor(cursor_x, inner.y);
}

fn draw_hint(f: &mut Frame<'_>, area: Rect, core: &UiState) {
    let text = if core.selection_mode {
        "\u{1f5b1} selection mode on — select & copy with your terminal · Ctrl+Y to resume scrolling".to_string()
    } else {
        "⏸ manual mode · ? for shortcuts · Ctrl+Y to select & copy · Ctrl+C to cancel".to_string()
    };
    let style = if core.selection_mode { Style::default().fg(Color::Yellow) } else { Style::default().fg(Color::DarkGray) };
    f.render_widget(Paragraph::new(Line::from(Span::styled(text, style))), area);
}

// ---------------------------------------------------------------------
// autocomplete popup
// ---------------------------------------------------------------------

fn draw_autocomplete(f: &mut Frame<'_>, transcript_area: Rect, core: &UiState) {
    if core.autocomplete_cache.is_empty() || core.permission.is_some() {
        return;
    }
    let visible = core.autocomplete_cache.len().min(6);
    let height = visible as u16 + 2;
    if height >= transcript_area.height || transcript_area.width < 12 {
        return;
    }
    let popup = Rect {
        x: transcript_area.x + 1,
        y: transcript_area.y + transcript_area.height - height,
        width: transcript_area.width.saturating_sub(2),
        height,
    };
    f.render_widget(ClearWidget, popup);

    let lines: Vec<Line> = core
        .autocomplete_cache
        .iter()
        .take(visible)
        .enumerate()
        .map(|(i, name)| {
            let selected = i == core.autocomplete_selected;
            let style = if selected {
                Style::default().fg(Color::Black).bg(core.header.accent).add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };
            let label = if super::at_fragment(&core.input, core.cursor).is_some() || core.input.contains(' ') {
                name.clone()
            } else {
                format!("/{name}")
            };
            Line::from(Span::styled(label, style))
        })
        .collect();
    let block = Block::default().borders(Borders::ALL).border_style(Style::default().fg(core.header.accent));
    f.render_widget(Paragraph::new(lines).block(block), popup);
}

// ---------------------------------------------------------------------
// permission modal
// ---------------------------------------------------------------------

fn draw_permission_modal(f: &mut Frame<'_>, area: Rect, p: &PermissionView) {
    let popup = centered_rect(66, 9, area);
    f.render_widget(ClearWidget, popup);
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" Permission needed ")
        .border_style(Style::default().fg(Color::Yellow));
    let inner = block.inner(popup);
    f.render_widget(block, popup);

    let lines = vec![
        Line::from(Span::styled(format!("REXO wants to {}:", p.action), Style::default().add_modifier(Modifier::BOLD))),
        Line::default(),
        Line::from(Span::styled(p.summary.clone(), Style::default().fg(Color::Cyan))),
        Line::default(),
        Line::from(format!("Risk: {}", p.risk_label)),
        Line::default(),
        Line::from(vec![
            Span::styled("[y]", Style::default().fg(Color::Green).add_modifier(Modifier::BOLD)),
            Span::raw(" once    "),
            Span::styled("[a]", Style::default().fg(Color::Green).add_modifier(Modifier::BOLD)),
            Span::raw(" always this session    "),
            Span::styled("[n]", Style::default().fg(Color::Red).add_modifier(Modifier::BOLD)),
            Span::raw(" deny"),
        ]),
    ];
    f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
}

fn centered_rect(width: u16, height: u16, area: Rect) -> Rect {
    let width = width.min(area.width);
    let height = height.min(area.height);
    let x = area.x + (area.width.saturating_sub(width)) / 2;
    let y = area.y + (area.height.saturating_sub(height)) / 2;
    Rect { x, y, width, height }
}

// ---------------------------------------------------------------------
// /help overlay
// ---------------------------------------------------------------------

fn draw_help(f: &mut Frame<'_>, core: &UiState, help: &HelpState) {
    let area = f.size();
    f.render_widget(ClearWidget, area);
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" Help ")
        .border_style(Style::default().fg(core.header.accent));
    let inner = block.inner(area);
    f.render_widget(block, area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Length(1), Constraint::Min(3), Constraint::Length(2)])
        .split(inner);

    let tabs = [
        ("General", HelpTab::General),
        ("Commands", HelpTab::Commands),
        ("Custom commands", HelpTab::Custom),
    ];
    let mut spans = Vec::new();
    for (label, tab) in tabs {
        let style = if tab == help.tab {
            Style::default().fg(Color::Black).bg(core.header.accent).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::DarkGray)
        };
        spans.push(Span::styled(format!(" {label} "), style));
        spans.push(Span::raw("  "));
    }
    f.render_widget(Paragraph::new(Line::from(spans)), chunks[1]);

    match help.tab {
        HelpTab::General => draw_help_general(f, chunks[2]),
        HelpTab::Commands => draw_help_commands(f, chunks[2], help),
        HelpTab::Custom => draw_help_custom(f, chunks[2], &help.workspace),
    }

    let footer = vec![
        Line::from(Span::styled(
            "\u{2190}\u{2192} switch tabs   \u{2191}\u{2193} scroll   Esc close",
            Style::default().fg(Color::DarkGray),
        )),
        Line::from(Span::styled(
            "See README.md for more. Use /feedback <message> to leave a note.",
            Style::default().fg(Color::DarkGray),
        )),
    ];
    f.render_widget(Paragraph::new(footer), chunks[3]);
}

fn draw_help_general(f: &mut Frame<'_>, area: Rect) {
    let bold = Style::default().add_modifier(Modifier::BOLD);
    let mut lines = vec![
        Line::from(Span::styled("Rexo Code", bold)),
        Line::from("Type a plain sentence to give REXO a coding task, or /command — see the Commands tab."),
        Line::default(),
        Line::from(Span::styled("Permissions", bold)),
        Line::from(
            "File edits, terminal commands, and git writes ask before running unless set to \
             auto (/permissions). Reading and searching files never asks.",
        ),
        Line::default(),
        Line::from(Span::styled("Keys", bold)),
    ];
    for (key, what) in super::KEYBINDINGS {
        lines.push(Line::from(vec![
            Span::styled(format!("{key:<16}"), Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(*what),
        ]));
    }
    lines.push(Line::default());
    lines.push(Line::from(Span::styled("Configuration", bold)));
    lines.push(Line::from("rexo.toml in your workspace for project overrides; /connect and /doctor show where global settings live."));
    f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), area);
}

fn draw_help_commands(f: &mut Frame<'_>, area: Rect, help: &HelpState) {
    let rows = super::help_command_rows();
    if rows.is_empty() {
        return;
    }
    let selected = help.selected.min(rows.len() - 1);
    let visible_height = area.height as usize;
    let scroll = selected.saturating_sub(visible_height.saturating_sub(1));

    let lines: Vec<Line> = rows
        .iter()
        .enumerate()
        .skip(scroll)
        .take(visible_height)
        .map(|(i, spec)| {
            let marker = if i == selected { "\u{203a} " } else { "  " };
            let suffix = super::help_status_suffix(spec);
            let style = if i == selected {
                Style::default().fg(Color::Black).bg(Color::Cyan)
            } else if suffix.is_empty() {
                Style::default()
            } else {
                Style::default().fg(Color::DarkGray)
            };
            Line::from(Span::styled(format!("{marker}{:<34} {}{suffix}", spec.usage, spec.description), style))
        })
        .collect();
    f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), area);
}

fn draw_help_custom(f: &mut Frame<'_>, area: Rect, workspace: &std::path::Path) {
    let commands = crate::cli::custom_commands::discover(workspace);
    let mut lines = Vec::new();
    if commands.is_empty() {
        lines.push(Line::from("No custom commands found."));
        lines.push(Line::default());
        lines.push(Line::from(Span::styled(
            "Add one at .rexo/commands/<name>.md (project-only) or in your global commands \
             directory (/doctor shows where) — its contents become the prompt for /<name>, \
             with $ARGUMENTS and $1.. $9 filled in from whatever you type after the command name.",
            Style::default().fg(Color::DarkGray),
        )));
    } else {
        for cmd in &commands {
            let scope = if cmd.project { "project" } else { "global" };
            lines.push(Line::from(vec![
                Span::styled(format!("/{:<20}", cmd.name), Style::default().add_modifier(Modifier::BOLD)),
                Span::styled(format!("[{scope}] "), Style::default().fg(Color::DarkGray)),
                Span::raw(cmd.description.clone()),
            ]));
        }
    }
    f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), area);
}
