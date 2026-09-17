//! Output capture for slash-command handlers.
//!
//! `src/cli/commands.rs` has ~50 handlers, every one of them built around
//! plain `println!`/`print!` calls (many through `colored`, so the actual
//! bytes include ANSI SGR escape codes). That code is well-tested and
//! reads well; rewriting every call site against a ratatui-specific
//! output API — just so the new persistent TUI (see `cli::tui`) has
//! somewhere other than raw stdout to put it — would touch most of that
//! file for no behavioral gain and a real chance of introducing bugs.
//!
//! Instead: `tprintln!`/`tprint!` are drop-in replacements for
//! `println!`/`print!` (identical `format!`-style syntax, so converting a
//! file is a mechanical rename, not a rewrite) that check a thread-local
//! flag. Outside a capture, they behave exactly like the real macros.
//! Inside one (`begin_capture`/`end_capture`, used by the TUI around a
//! `commands::dispatch` call), they append to an in-memory buffer instead
//! — including the ANSI codes `colored` already produced — which the TUI
//! then hands to `ansi_to_tui` to render as styled lines in the
//! transcript pane. See `cli::tui::run_command` for the call site.

use std::cell::RefCell;

thread_local! {
    static CAPTURE: RefCell<Option<String>> = const { RefCell::new(None) };
}

/// Start capturing everything written through `tprintln!`/`tprint!` on
/// this thread into an in-memory buffer instead of stdout. Must be paired
/// with [`end_capture`] — nesting is not supported (a second
/// `begin_capture` before an `end_capture` silently replaces the buffer).
pub fn begin_capture() {
    CAPTURE.with(|c| *c.borrow_mut() = Some(String::new()));
}

/// Stop capturing and return everything written since [`begin_capture`].
/// Returns an empty string if no capture was active.
pub fn end_capture() -> String {
    CAPTURE.with(|c| c.borrow_mut().take().unwrap_or_default())
}

/// Whether a capture is currently active on this thread — used by
/// handlers that need to know whether it's safe to do something
/// stdout/stdin-specific (see `prompt_line` in `commands.rs`, which
/// refuses to run mid-capture rather than silently hanging).
pub fn is_capturing() -> bool {
    CAPTURE.with(|c| c.borrow().is_some())
}

#[doc(hidden)]
pub fn write_str(s: &str) {
    CAPTURE.with(|c| {
        let mut guard = c.borrow_mut();
        match guard.as_mut() {
            Some(buf) => buf.push_str(s),
            None => {
                print!("{s}");
                use std::io::Write;
                std::io::stdout().flush().ok();
            }
        }
    });
}

#[doc(hidden)]
pub fn write_line(s: &str) {
    CAPTURE.with(|c| {
        let mut guard = c.borrow_mut();
        match guard.as_mut() {
            Some(buf) => {
                buf.push_str(s);
                buf.push('\n');
            }
            None => println!("{s}"),
        }
    });
}

/// Drop-in replacement for `println!`. See the module docs.
#[macro_export]
macro_rules! tprintln {
    () => { $crate::output::write_line("") };
    ($($arg:tt)*) => { $crate::output::write_line(&format!($($arg)*)) };
}

/// Drop-in replacement for `print!`. See the module docs.
#[macro_export]
macro_rules! tprint {
    ($($arg:tt)*) => { $crate::output::write_str(&format!($($arg)*)) };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capture_collects_lines_instead_of_printing() {
        begin_capture();
        tprintln!("hello {}", "world");
        tprintln!();
        tprint!("no newline");
        let out = end_capture();
        assert_eq!(out, "hello world\n\nno newline");
    }

    #[test]
    fn capture_is_off_by_default() {
        assert!(!is_capturing());
    }

    #[test]
    fn end_capture_without_begin_is_empty_not_a_panic() {
        assert_eq!(end_capture(), "");
    }
}
