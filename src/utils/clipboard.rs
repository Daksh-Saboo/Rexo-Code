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

/// Read an image off the system clipboard (Alt+V paste) and re-encode it
/// as PNG bytes. arboard's `image-data` feature only ever hands back raw
/// RGBA8 pixels — it deliberately doesn't do any encoding of its own —
/// so turning that into the PNG bytes a vision-capable model's wire
/// format actually wants is this crate's job, via the separate `image`
/// dependency (PNG-encoder feature only; see `Cargo.toml`'s comment on
/// why that's a second, smaller dependency rather than folding encoding
/// into arboard's own optional feature).
pub fn paste_image_png() -> Result<Vec<u8>> {
    let mut clipboard = arboard::Clipboard::new().context("Couldn't reach the system clipboard")?;
    let img = clipboard.get_image().context("No image on the clipboard (or the clipboard holds something else)")?;
    let (width, height) = (img.width as u32, img.height as u32);
    let buffer = image::RgbaImage::from_raw(width, height, img.bytes.into_owned())
        .context("Clipboard image data didn't match its own reported dimensions")?;
    let mut png_bytes = Vec::new();
    image::DynamicImage::ImageRgba8(buffer)
        .write_to(&mut std::io::Cursor::new(&mut png_bytes), image::ImageFormat::Png)
        .context("Couldn't encode the clipboard image as PNG")?;
    Ok(png_bytes)
}
