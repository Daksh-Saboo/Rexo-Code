// Embeds `assets/rexo.ico` into the compiled `rexo.exe`'s resources on
// Windows — what actually gives the binary a custom taskbar/Alt-Tab icon
// (and the icon `conhost` shows on its tab), as opposed to
// `cli::tui::set_console_title`'s runtime `SetTitle` call, which only
// changes the *text* title, not the icon.
//
// On any other host target this does nothing at all — `winres` invokes
// the Windows resource compiler, which doesn't exist on Linux/macOS, so
// this is skipped outright rather than attempted and failed. That also
// means this can't be verified inside this project's own Linux build
// sandbox; it's real, straightforward-per-`winres`'s-own-docs code, but
// genuinely untested here — worth trying on an actual Windows checkout
// and filing an issue if it doesn't do what it says.
fn main() {
    #[cfg(target_os = "windows")]
    {
        let mut res = winres::WindowsResource::new();
        res.set_icon("assets/rexo.ico");
        if let Err(e) = res.compile() {
            // Never fail the whole build over a missing icon — a
            // console app without a custom icon still works fine.
            println!("cargo:warning=couldn't embed rexo.ico: {e}");
        }
    }
}
