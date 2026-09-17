//! Generic, reusable interactive pickers: an arrow-key + type-to-filter
//! selectable list, a text/password input box, and a yes/no confirm —
//! all built directly on raw crossterm key events rather than blocking
//! `stdin`/`rpassword` reads.
//!
//! This is deliberately decoupled from [`super::tui::TuiCore`] and
//! [`super::Session`]: every function here just takes the
//! `Terminal`/`EventStream` pair it draws to and reads from. That's what
//! lets the exact same code run in two different contexts without any
//! duplication:
//!
//! - **Inside the persistent session** (`cli::tui`'s `/connect`, `/model`,
//!   `/models`, `/provider`), where it's handed the *same* `Terminal` and
//!   `EventStream` the outer screen already owns — crucial, because a
//!   second `EventStream` reading the same stdin concurrently would race
//!   the first one for input.
//! - **The first-launch wizard** (`cli::wizard`), which runs *before* any
//!   `TuiCore` exists, so it opens its own short-lived terminal/event
//!   stream, uses these same functions, then tears both down.
//!
//! Reading raw key events ourselves for the masked API-key prompt also
//! means this path never depends on `rpassword`'s termios-based hidden
//! read at all — which is worth calling out, since v0.3 shipped a real
//! fix (`read_hidden_line`'s visible-prompt fallback) for that crate
//! silently eating a keystroke on some terminals. This path doesn't need
//! the fallback because it was never exposed to the bug in the first
//! place: we're already reading key-by-key for the list/filter box, so
//! masking is just "print `•` instead of the character," not a separate
//! hidden-echo syscall that can misbehave.

use std::io::Stdout;

use anyhow::Result;
use crossterm::event::{Event, EventStream, KeyCode, KeyEventKind, KeyModifiers, MouseEventKind};
use futures_util::StreamExt;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear as ClearWidget, Paragraph, Wrap};
use ratatui::Terminal;

pub type Backend = CrosstermBackend<Stdout>;

/// One row in a [`run_select`] list.
#[derive(Debug, Clone)]
pub struct PickerItem {
    pub label: String,
    pub sublabel: Option<String>,
    /// What's actually returned on selection — usually the same as
    /// `label`, but e.g. `/connect`'s provider picker shows a display
    /// name while returning the catalog key.
    pub value: String,
}

impl PickerItem {
    pub fn new(label: impl Into<String>, value: impl Into<String>) -> Self {
        Self { label: label.into(), sublabel: None, value: value.into() }
    }

    pub fn with_sub(label: impl Into<String>, sublabel: impl Into<String>, value: impl Into<String>) -> Self {
        Self { label: label.into(), sublabel: Some(sublabel.into()), value: value.into() }
    }
}

pub enum PickerOutcome {
    Selected(String),
    /// The user typed something that isn't in the list and chose to use
    /// it verbatim (Ctrl+Enter, or Enter when nothing is highlighted and
    /// `allow_custom` is set) — e.g. a model ID discovery didn't return.
    Custom(String),
    Cancelled,
}

pub struct SelectOptions<'a> {
    pub title: String,
    pub items: Vec<PickerItem>,
    pub accent: Color,
    /// Whether typed text with no matching row can be submitted as-is.
    pub allow_custom: bool,
    /// One extra line of guidance shown under the title (e.g. "435 models
    /// from OpenRouter — type to filter").
    pub subtitle: Option<String>,
    pub search_placeholder: &'a str,
}

/// An arrow-key, type-to-filter selectable list — the "Ctrl+P palette"
/// interaction: everything typed narrows `items` by a case-insensitive
/// match on label/sublabel/value (prefix matches ranked first), Up/Down
/// (or the mouse wheel) move the highlight, Enter accepts it, Esc cancels.
pub async fn run_select(terminal: &mut Terminal<Backend>, events: &mut EventStream, opts: SelectOptions<'_>) -> Result<PickerOutcome> {
    let mut query = String::new();
    let mut selected: usize = 0;

    loop {
        let filtered = filter_items(&opts.items, &query);
        if selected >= filtered.len() {
            selected = filtered.len().saturating_sub(1);
        }

        terminal.draw(|f| draw_select(f, &opts, &query, &filtered, selected))?;

        match events.next().await {
            Some(Ok(Event::Key(key))) if key.kind == KeyEventKind::Press => {
                let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
                match key.code {
                    KeyCode::Esc => return Ok(PickerOutcome::Cancelled),
                    KeyCode::Char('c') | KeyCode::Char('C') if ctrl => return Ok(PickerOutcome::Cancelled),
                    KeyCode::Enter if ctrl && opts.allow_custom && !query.trim().is_empty() => {
                        return Ok(PickerOutcome::Custom(query.trim().to_string()));
                    }
                    KeyCode::Enter => {
                        if let Some(item) = filtered.get(selected) {
                            return Ok(PickerOutcome::Selected(item.value.clone()));
                        } else if opts.allow_custom && !query.trim().is_empty() {
                            return Ok(PickerOutcome::Custom(query.trim().to_string()));
                        }
                    }
                    KeyCode::Up => selected = selected.saturating_sub(1),
                    KeyCode::Down => {
                        if selected + 1 < filtered.len() {
                            selected += 1;
                        }
                    }
                    KeyCode::PageUp => selected = selected.saturating_sub(8),
                    KeyCode::PageDown => selected = (selected + 8).min(filtered.len().saturating_sub(1)),
                    KeyCode::Backspace => {
                        query.pop();
                        selected = 0;
                    }
                    KeyCode::Char('u') if ctrl => {
                        query.clear();
                        selected = 0;
                    }
                    KeyCode::Char(c) if !ctrl => {
                        query.push(c);
                        selected = 0;
                    }
                    _ => {}
                }
            }
            Some(Ok(Event::Resize(_, _))) => {}
            Some(Ok(Event::Mouse(m))) => match m.kind {
                MouseEventKind::ScrollUp => selected = selected.saturating_sub(1),
                MouseEventKind::ScrollDown => {
                    if selected + 1 < filtered.len() {
                        selected += 1;
                    }
                }
                _ => {}
            },
            Some(Ok(_)) => {}
            Some(Err(_)) | None => return Ok(PickerOutcome::Cancelled),
        }
    }
}

fn filter_items<'a>(items: &'a [PickerItem], query: &str) -> Vec<&'a PickerItem> {
    let q = query.trim().to_lowercase();
    if q.is_empty() {
        return items.iter().collect();
    }
    let mut prefix = Vec::new();
    let mut contains = Vec::new();
    for item in items {
        let label_lc = item.label.to_lowercase();
        if label_lc.starts_with(&q) || item.value.to_lowercase().starts_with(&q) {
            prefix.push(item);
        } else if label_lc.contains(&q)
            || item.value.to_lowercase().contains(&q)
            || item.sublabel.as_deref().unwrap_or_default().to_lowercase().contains(&q)
        {
            contains.push(item);
        }
    }
    prefix.extend(contains);
    prefix
}

fn draw_select(f: &mut ratatui::Frame<'_>, opts: &SelectOptions<'_>, query: &str, filtered: &[&PickerItem], selected: usize) {
    let area = f.size();
    f.render_widget(ClearWidget, area);
    let block = Block::default().borders(Borders::ALL).title(format!(" {} ", opts.title)).border_style(Style::default().fg(opts.accent));
    let inner = block.inner(area);
    f.render_widget(block, area);

    let header_h: u16 = if opts.subtitle.is_some() { 3 } else { 2 };
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(header_h), Constraint::Min(3), Constraint::Length(1)])
        .split(inner);

    let mut header_lines = Vec::new();
    if let Some(sub) = &opts.subtitle {
        header_lines.push(Line::from(Span::styled(sub.clone(), Style::default().fg(Color::DarkGray))));
    }
    let search_display = if query.is_empty() { opts.search_placeholder.to_string() } else { query.to_string() };
    let search_style = if query.is_empty() { Style::default().fg(Color::DarkGray) } else { Style::default().fg(Color::White) };
    header_lines.push(Line::from(vec![
        Span::styled("Search: ", Style::default().fg(opts.accent).add_modifier(Modifier::BOLD)),
        Span::styled(search_display, search_style),
        Span::styled("▏", Style::default().fg(opts.accent)),
    ]));
    header_lines.push(Line::from(Span::styled(
        format!("{} match{}", filtered.len(), if filtered.len() == 1 { "" } else { "es" }),
        Style::default().fg(Color::DarkGray),
    )));
    f.render_widget(Paragraph::new(header_lines), chunks[0]);

    let visible = chunks[1].height as usize;
    let scroll = selected.saturating_sub(visible.saturating_sub(1));
    let rows: Vec<Line> = filtered
        .iter()
        .enumerate()
        .skip(scroll)
        .take(visible)
        .map(|(i, item)| {
            let is_sel = i == selected;
            let style = if is_sel {
                Style::default().fg(Color::Black).bg(opts.accent).add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };
            let marker = if is_sel { "› " } else { "  " };
            match &item.sublabel {
                Some(sub) => Line::from(vec![
                    Span::styled(format!("{marker}{}", item.label), style),
                    Span::styled(format!("  {sub}"), if is_sel { style } else { Style::default().fg(Color::DarkGray) }),
                ]),
                None => Line::from(Span::styled(format!("{marker}{}", item.label), style)),
            }
        })
        .collect();
    f.render_widget(Paragraph::new(rows), chunks[1]);

    let hint = if opts.allow_custom {
        "\u{2191}\u{2193} move · Enter select · Ctrl+Enter use typed text as-is · Esc cancel"
    } else {
        "\u{2191}\u{2193} move · Enter select · Esc cancel"
    };
    f.render_widget(Paragraph::new(Line::from(Span::styled(hint, Style::default().fg(Color::DarkGray)))), chunks[2]);
}

/// A single-line text (or masked) input, used for base URLs, API keys,
/// custom provider names, model IDs, etc.
pub async fn run_text_input(
    terminal: &mut Terminal<Backend>,
    events: &mut EventStream,
    title: &str,
    prompt_label: &str,
    default: Option<&str>,
    masked: bool,
    accent: Color,
) -> Result<Option<String>> {
    let mut value = String::new();
    loop {
        terminal.draw(|f| draw_text_input(f, title, prompt_label, default, &value, masked, accent))?;
        match events.next().await {
            Some(Ok(Event::Key(key))) if key.kind == KeyEventKind::Press => {
                let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
                match key.code {
                    KeyCode::Esc => return Ok(None),
                    KeyCode::Char('c') | KeyCode::Char('C') if ctrl => return Ok(None),
                    KeyCode::Enter => {
                        if value.trim().is_empty() {
                            return Ok(default.map(|d| d.to_string()));
                        }
                        return Ok(Some(value.trim().to_string()));
                    }
                    KeyCode::Backspace => {
                        value.pop();
                    }
                    KeyCode::Char('u') if ctrl => value.clear(),
                    KeyCode::Char(c) if !ctrl => value.push(c),
                    _ => {}
                }
            }
            Some(Ok(Event::Resize(_, _))) => {}
            Some(Ok(_)) => {}
            Some(Err(_)) | None => return Ok(None),
        }
    }
}

fn draw_text_input(f: &mut ratatui::Frame<'_>, title: &str, prompt_label: &str, default: Option<&str>, value: &str, masked: bool, accent: Color) {
    let area = f.size();
    f.render_widget(ClearWidget, area);
    let popup = centered_rect(70, 7, area);
    f.render_widget(ClearWidget, popup);
    let block = Block::default().borders(Borders::ALL).title(format!(" {title} ")).border_style(Style::default().fg(accent));
    let inner = block.inner(popup);
    f.render_widget(block, popup);

    let shown: String = if masked { "•".repeat(value.chars().count()) } else { value.to_string() };
    let mut lines = vec![
        Line::from(Span::styled(prompt_label, Style::default().add_modifier(Modifier::BOLD))),
        Line::default(),
        Line::from(vec![Span::raw("> "), Span::raw(shown), Span::styled("▏", Style::default().fg(accent))]),
    ];
    if let Some(d) = default {
        lines.push(Line::default());
        lines.push(Line::from(Span::styled(format!("(blank = {d})"), Style::default().fg(Color::DarkGray))));
    }
    if masked {
        lines.push(Line::from(Span::styled("input hidden", Style::default().fg(Color::DarkGray))));
    }
    f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
}

/// A y/n confirm — arrow keys/Tab flip the highlighted choice, `y`/`n` are
/// direct shortcuts, Enter accepts whichever is highlighted.
pub async fn run_confirm(terminal: &mut Terminal<Backend>, events: &mut EventStream, title: &str, question: &str, default_yes: bool, accent: Color) -> Result<bool> {
    let mut yes = default_yes;
    loop {
        terminal.draw(|f| draw_confirm(f, title, question, yes, accent))?;
        match events.next().await {
            Some(Ok(Event::Key(key))) if key.kind == KeyEventKind::Press => match key.code {
                KeyCode::Char('y') | KeyCode::Char('Y') => return Ok(true),
                KeyCode::Char('n') | KeyCode::Char('N') => return Ok(false),
                KeyCode::Left | KeyCode::Right | KeyCode::Tab => yes = !yes,
                KeyCode::Enter => return Ok(yes),
                KeyCode::Esc => return Ok(false),
                _ if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') => return Ok(false),
                _ => {}
            },
            Some(Ok(Event::Resize(_, _))) => {}
            Some(Ok(_)) => {}
            Some(Err(_)) | None => return Ok(false),
        }
    }
}

fn draw_confirm(f: &mut ratatui::Frame<'_>, title: &str, question: &str, yes: bool, accent: Color) {
    let area = f.size();
    let popup = centered_rect(60, 7, area);
    f.render_widget(ClearWidget, popup);
    let block = Block::default().borders(Borders::ALL).title(format!(" {title} ")).border_style(Style::default().fg(accent));
    let inner = block.inner(popup);
    f.render_widget(block, popup);

    let yes_style = if yes { Style::default().fg(Color::Black).bg(Color::Green).add_modifier(Modifier::BOLD) } else { Style::default() };
    let no_style = if !yes { Style::default().fg(Color::Black).bg(Color::Red).add_modifier(Modifier::BOLD) } else { Style::default() };
    let lines = vec![
        Line::from(question.to_string()),
        Line::default(),
        Line::from(vec![
            Span::styled(" Yes ", yes_style),
            Span::raw("    "),
            Span::styled(" No ", no_style),
        ]),
        Line::default(),
        Line::from(Span::styled("\u{2190}/\u{2192} choose · y/n · Enter confirm", Style::default().fg(Color::DarkGray))),
    ];
    f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
}

fn centered_rect(width_pct: u16, height: u16, area: Rect) -> Rect {
    let width = (area.width * width_pct / 100).min(area.width);
    let height = height.min(area.height);
    let x = area.x + (area.width.saturating_sub(width)) / 2;
    let y = area.y + (area.height.saturating_sub(height)) / 2;
    Rect { x, y, width, height }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filter_ranks_prefix_matches_first() {
        let items = vec![
            PickerItem::new("Hugging Face", "huggingface"),
            PickerItem::new("OpenRouter", "openrouter"),
            PickerItem::new("OpenAI", "openai"),
        ];
        let out = filter_items(&items, "open");
        let values: Vec<&str> = out.iter().map(|i| i.value.as_str()).collect();
        assert_eq!(values, vec!["openrouter", "openai"]);
    }

    #[test]
    fn filter_matches_sublabel_too() {
        let items = vec![PickerItem::with_sub("moonshotai/kimi-k3", "owned by moonshotai", "moonshotai/kimi-k3")];
        let out = filter_items(&items, "moonshotai");
        assert_eq!(out.len(), 1);
    }

    #[test]
    fn empty_query_returns_everything_in_order() {
        let items = vec![PickerItem::new("a", "a"), PickerItem::new("b", "b")];
        let out = filter_items(&items, "");
        assert_eq!(out.len(), 2);
    }

    #[test]
    fn no_match_returns_empty() {
        let items = vec![PickerItem::new("OpenRouter", "openrouter")];
        assert!(filter_items(&items, "zzz-nope").is_empty());
    }
}
