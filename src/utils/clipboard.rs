//! System clipboard access for `/copy`, wrapping `arboard` behind a
//! narrow interface with no panics on a headless/no-display environment
//! (CI, this project's own sandboxed test runs) — a failure to reach the
//! OS clipboard is reported back as a normal `Result`, same as any other
//! tool failure, never a crash.

use anyhow::{Context, Result};

/// Copy `text` to the system clipboard. On Windows this is the real
/// Win32 clipboard via `arboard`'s `clipboard-win` backend; on Linux it
/// talks X11 (falling back cleanly to an error, not a panic, if no
/// display server is reachable — exactly the case in this project's own
/// CI/sandbox test runs, which is why no test here actually calls this).
pub fn copy(text: &str) -> Result<()> {
    let mut clipboard = arboard::Clipboard::new().context("Couldn't reach the system clipboard")?;
    clipboard.set_text(text.to_string()).context("Couldn't write to the system clipboard")?;
    Ok(())
}
