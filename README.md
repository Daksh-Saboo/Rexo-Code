# Rexo Code

[![CI](../../actions/workflows/ci.yml/badge.svg)](../../actions/workflows/ci.yml)
[![Release](../../actions/workflows/release.yml/badge.svg)](../../actions/workflows/release.yml)

An open-source, provider-agnostic AI coding agent for the terminal — inspired by
tools like Claude Code, built as an independent implementation.

```
 _____  ________   ______     _____ ____  _____  ______
|  __ \|  ____\ \ / / __ \   / ____/ __ \|  __ \|  ____|
| |__) | |__   \ V / |  | | | |   | |  | | |  | | |__
|  _  /|  __|   > <| |  | | | |   | |  | | |  | |  __|
| | \ \| |____ / . \ |__| | | |___| |__| | |__| | |____
|_|  \_\______/_/ \_\____/   \_____\____/|_____/|______|
```

REXO reads and searches your project, proposes and applies edits, runs
commands, and iterates — with a permission system in the loop for anything
that isn't read-only.

## Status

This is v0.8.0: a working agent loop with real tool-calling and streaming
(including native Gemini SSE streaming and multimodal image input via
Alt+V), support for many OpenAI-compatible providers plus local models, a
persistent full-screen terminal UI with an animated startup intro, arrow-key
pickers for `/connect`/`/model`/first-launch setup/`/resume`/`/rewind`,
session persistence (`/resume`, `/branch`, `/fork`), a shell-mode
passthrough (`!`), quick side questions (`btw <question>`), a skills
system, custom commands, installable plugins bundling skills/commands/hooks
(`/plugin`), a real MCP client speaking the actual stdio JSON-RPC protocol
(`/mcp connect`), lifecycle hooks around tool calls and session start/end
(`/hooks`), sequential subagents with their own persona and context
(`/agents run`), a model-driven task list (Ctrl+T), single-level file-edit
undo (`/undo`, Ctrl+Shift+_), a local server for future editor integration
(`/ide` — see [Known limitations](#known-limitations), no editor extension
ships yet), a permission/security layer gating every tool call, a provider
capability model and normalized error classification, and CI/packaged
downloads for all five target platforms instead of source-only delivery.
It has not been run against a large real-world codebase yet — treat it as
an early, working foundation rather than a finished product. See
[Roadmap](#roadmap) for what's next, and [Known limitations](#known-limitations)
for an honest list of what's real vs. still scoped down (background/
concurrent subagents with git-worktree isolation, in particular, aren't —
`/agents run` is real but sequential, blocking the session until it
finishes).

**Renamed from TRON-Code to Rexo Code in v0.7.1** — a naming clash with
an existing, unrelated project. Everything user-facing changed to match:
the `rexo` binary, `rexo.toml`, the `.rexo/` config directory,
`REXO_*` env vars. No functional behavior changed because of the rename
itself — see [What's new in v0.7.1](#whats-new-in-v071) for exactly
what moved.

## Downloads

REXO builds natively — not cross-compiled — for five targets. [CI](.github/workflows/ci.yml)
builds and, everywhere a runner for that OS exists, tests all five on
every push; [Release](.github/workflows/release.yml) publishes them to
this repo's [Releases page](../../releases) whenever a `vX.Y.Z` tag is
pushed.

| Platform             | Target triple              | Archive |
|-----------------------|-----------------------------|---------|
| Linux x64             | `x86_64-unknown-linux-gnu`  | `rexo-code-linux-x86_64.tar.gz` |
| macOS Intel            | `x86_64-apple-darwin`       | `rexo-code-macos-x86_64.tar.gz` |
| macOS Apple Silicon    | `aarch64-apple-darwin`      | `rexo-code-macos-aarch64.tar.gz` |
| Windows x64            | `x86_64-pc-windows-msvc`    | `rexo-code-windows-x86_64.zip` |
| Windows ARM64          | `aarch64-pc-windows-msvc`   | `rexo-code-windows-aarch64.zip` |

Each archive is self-contained: the `rexo` binary, `README.md`,
`LICENSE`, and the matching install script (`install.sh`/`uninstall.sh`
for Linux/macOS, `install.ps1`/`install.bat`/`uninstall.ps1` plus
`rexo.ico` for Windows). A `SHA256SUMS.txt` covering every archive ships
alongside them on the release.

**No Rust toolchain needed** for a downloaded release — that's the whole
point of shipping prebuilt binaries rather than source-only, which is
what every prior version did. If you'd rather build from source (or
you're on a platform not listed above), see [Quickstart](#quickstart)
below; `install.sh`/`install.ps1` both still build locally by default.

A couple of honest caveats, stated plainly rather than glossed over:

- **The `linux-x86_64` leg is the only one actually built and run in
  this project's own development environment**, which is Linux-only —
  every release/`cargo test` verification described in this README for
  that target is real, on the real compiled binary. The other four legs
  (macOS Intel/Apple Silicon, Windows x64/ARM64) are defined correctly in
  `release.yml`/`ci.yml` and follow the same native-runner-per-OS
  approach every major Rust CLI tool uses, but **have not yet been
  observed actually running on GitHub's infrastructure** — no tag has
  been pushed to trigger them yet. Pushing the first `vX.Y.Z` tag to a
  real GitHub repo is what turns that "should work" into "does work";
  until then, treat those four as correctly-specified but unverified.
- **Windows ARM64 is cross-linked, not run.** MSVC's linker can target
  ARM64 from an x64 host without an ARM64 machine, so the binary will be
  real once CI runs — but GitHub doesn't currently offer a hosted
  Windows-on-ARM runner, so CI can't execute the test suite (or this
  binary) on real ARM64 Windows before it ships. Flagged in `ci.yml`'s
  comments too.
- **macOS binaries will be unsigned/unnotarized.** Gatekeeper will refuse
  to open them with a plain double-click; either
  `xattr -d com.apple.quarantine rexo` after downloading, or right-click
  → Open once to approve it. Code-signing needs a paid Apple Developer
  account, which isn't set up for this project — a real limitation, not
  an oversight.

### What's new in v0.7.3

- **Fixed a real startup bug**: launching `rexo` with no provider
  configured used to hard-exit before the TUI ever opened — no way to
  reach `/connect` to fix it, exactly the failure mode a real Windows
  user hit and reported. Root cause: startup called `config.api_key()`
  and `Agent::bootstrap()` unconditionally and exited hard on either
  failing, even for interactive sessions. The codebase already had a
  graceful-degradation pattern for this exact situation —
  `Session::rebuild_provider` swaps in an `UnconfiguredProvider` stand-in
  when a *runtime* `/connect`/`/model` change fails, rather than leaving
  the old provider silently in place — it just wasn't being used at
  startup. Now it is: an interactive `rexo` always reaches the TUI, with
  a clear "No provider is configured yet — run /connect" note. Headless/
  single-shot mode (a prompt given on the command line) still fails
  fast, since there's no session to recover into there, but the message
  now also says to run `rexo` with no prompt to set up interactively.
  Verified with a PTY test that reproduces the exact reported scenario —
  no key, no model, `setup_completed` already true — and confirms the
  process stays alive, shows the warning, and `/status` still works.
- **A bug in fixing the bug, caught before it shipped**: wiring the fix
  above briefly broke the build with a rustc cycle error, traced back to
  an editing slip in v0.7.2 that had deleted the `Provider` trait's own
  `#[async_trait]` attribute while removing an accidental duplicate type.
  Restored; full test suite re-verified green before this was called
  done.
- **Rebrand cleanup pass**: internal test scratch-directory names and a
  few stale test function names still said `tron` — cosmetic, never
  visible to users, but worth being thorough about since v0.7.2 already
  found one real miss (the letter-spaced intro title). An exhaustive
  rescan this time — including letter-spaced patterns and the compiled
  icon's binary strings, not just plain-text search — turned up nothing
  further.

### What's new in v0.7.2

- **Fixed real leftover branding**: the startup intro's title was still
  a letter-spaced `"T R O N - C O D E"` string. The v0.7.1 rebrand's
  search-and-replace was word-boundary-based and looked for a contiguous
  `TRON` substring — which doesn't exist in text with a space between
  every letter, so it walked right past this one. An exhaustive grep
  across the tree came up clean at the time because grep doesn't know
  the letters were meant to spell a word either; the intro animation
  itself just never got the same scrutiny as everything else, since it's
  cosmetic and there was no existing test checking its on-screen text.
  Now reads `"R E X O   C O D E"`, confirmed by a PTY test that
  specifically asserts no `"T R O N"` shows up anywhere across the whole
  animation, not just that the fix looks right in the source.
- **Slower, more deliberate intro**: three scan-lines now sweep across
  the screen at once before the face assembles — one along the top
  (left→right), one along the bottom (right→left), and a faster one
  straight through where the face's eyes are about to land (cleared away
  right as the face itself gets there, so it reads as "the scan finds
  where the face appears" rather than leftover clutter). The whole thing
  now runs about 4 seconds end to end, up from well under 1 — genuinely
  slower, not just padded with a longer final hold. Any keypress at any
  point still skips straight to the fully-assembled final frame.

### What's new in v0.7.1

- **Renamed to Rexo Code.** Binary (`tron` → `rexo`), crate/package
  (`tron-code` → `rexo-code`), config file (`tron.toml` → `rexo.toml`),
  config directory (`.tron/` → `.rexo/`), every `TRON_*` env var
  (`REXO_MODEL`, `REXO_GLOBAL_DIR`, `REXO_NO_INTRO`, ...), the icon, the
  Windows Terminal profile, install/uninstall scripts, CI/release
  workflows — everywhere. Verified with a clean rebuild, the full test
  suite, a real PTY session showing "Rexo Code" in the running TUI, and
  `install.sh`/`uninstall.sh` run end-to-end against the renamed binary;
  a final exhaustive grep across the whole tree turned up zero remaining
  references to the old name.
- **Session persistence wired up for real.** A backend module
  (`cli::sessions` — save/load/list/delete, autosave after every turn,
  path-traversal-safe name sanitizing, its own unit tests) already
  existed in the tree from an earlier pass, but was never declared as
  part of the build — `grep` for its own module declaration turned up
  nothing, meaning it wasn't even being compiled. Declaring it and wiring
  it up is most of what made the rest of this section possible.
- **`/resume`** — bare, opens a picker over every saved session
  (autosave + named) and loads the one you pick, replacing the current
  conversation. `/resume list`/`save <name>`/`delete <name>`/`<name>`
  work without the picker too, for scripting.
- **`/branch <name>`** — bookmarks the current conversation as a new
  named session without switching away from it; `/resume <name>` later
  to explore that path. **`/fork <name>`** does the same save, but also
  switches *this* session to continue as the fork going forward (future
  autosaves redirect to `<name>.json`), leaving the pre-fork autosave
  alone as something you can still `/resume` back to. Honestly scoped:
  neither one runs anything concurrently — Rexo has no
  subagent/background-execution runtime, so "fork" here means a real
  saved divergence point, not autonomous parallel work.
- **`/rewind`** — bare, in the TUI, opens a picker over every earlier
  point in *this* conversation (each message you sent is a checkpoint)
  and truncates history back to it; `/rewind <n>` does the same without
  the picker. Conversation-only, stated plainly in the command's own
  output: it does not revert file edits, since that needs workspace
  snapshots (git-based or otherwise) this project doesn't have yet.
- **Two real bugs found by testing all of the above against the actual
  binary**, not just reviewing the code:
  - Real PTY testing of `/branch`/`/resume`/`/rewind` — nothing further
    to report here; it worked as designed the first time these got a
    proper end-to-end pass.
  - A pre-existing formatting drift in the provider catalog (`base_url:`
    lines missing their indentation on about a dozen presets, unrelated
    to the rename) got noticed and fixed while in that file for the
    rebrand sweep.

### What's new in v0.7

- **Startup intro animation** — typing `rexo` now shows the boxed face
  (the same `┏┓┃┗┛`/`◉◉` visual language as the header mascot, just
  bigger) scatter in from random positions and converge into place, then
  hold for a beat with "REXO-CODE" underneath before clearing into the
  session. Any keypress skips straight through; `--no-intro` or
  `REXO_NO_INTRO=1` skip it entirely (for scripts, or anyone who'd rather
  not see it every time). Verified with a real PTY test against the
  compiled binary, both with and without the flag.
- **First-launch wizard rewritten on real pickers** — provider selection
  is now arrow-key + type-to-filter, and the API key prompt is genuine
  masked raw-mode input, replacing the "type a number from a printed
  list" + plain-text `read_line` flow that had been deferred since
  v0.4.0. It opens its own short-lived terminal session before the main
  TUI exists (the `cli::picker` module's own docs described exactly this
  plan from the start — it just hadn't been wired up until now).
- **Two real bugs found by actually testing the above, not just writing
  it**, both fixed:
  - Leaving the wizard's alt-screen and immediately entering the trust
    dialog's could show stale wizard content bleeding through cells the
    trust dialog never explicitly draws into — alternate-screen buffers
    aren't guaranteed blank on re-entry. Fixed with an explicit
    `terminal.clear()` right after every `EnterAlternateScreen` in the
    wizard, trust dialog, and main TUI.
  - The wizard's model prompt said "blank = pick later with /model", but
    `rexo` can't actually reach the interactive session (where `/model`
    would run) without a model already resolved — leaving it blank
    produced a config that failed on the *next* launch with "No model
    configured". Fixed by defaulting to a sensible per-provider model
    (and requiring one for providers with no safe universal default)
    instead of promising a deferred fix that didn't work.

### What's new in v0.6

- **Shell mode** — type a bare `!` to drop into a mode where every line
  runs directly in PowerShell/`sh` instead of going to the model;
  `!<command>` runs one command immediately regardless of mode. See
  [Shell mode](#shell-mode). Bypasses the agent's permission engine
  entirely and deliberately so — you're the one typing it, not the
  model.
- **Provider capability model, for real this time** — `ModelCapabilities`
  grew from 4 fields to 11 (`parallel_tools`/`structured_output`/
  `model_discovery`/`context_window`/`max_output`/`cancellation`/
  `usage_reporting`, on top of the original `tool_calling`/`streaming`/
  `vision`/`reasoning`), and — the actual gap being closed here — it's
  now displayed in `/status`. It existed since v0.4 but nothing ever
  read it; confirmed via a grep across the codebase before touching it.
- **Normalized provider errors** — a closed `ProviderErrorKind` enum
  classifies every provider failure (`rate_limited`, `authentication`,
  `context_exceeded`, `timeout`, `network`, `server`, ...) on top of the
  existing human-readable message. Surfaced in the TUI (retryable
  failures get tagged) and in `rexo --output json`'s new `error_kind`
  field for scripts. See [Providers](#providers).
- **Real Gemini streaming** — `streamGenerateContent?alt=sse`, parsed
  incrementally, replaces the old "call the plain endpoint once, report
  the whole answer at the end" fallback from v0.4.1. Its own SSE loop
  (Gemini's chunk shape differs from the OpenAI-style one every other
  provider shares), tested against hand-built multi-chunk streams —
  still genuinely untested against the live API, same sandbox-network
  limitation as before.
- **The wizard migration credential-rename bug is fixed** — flagged in
  v0.4.1, left open through v0.5.0. Root cause: migrating a working setup
  to a global profile renamed its credential lookup key to `"migrated"`
  instead of preserving whichever key was already resolving successfully,
  silently orphaning env vars/credential-store entries saved under the
  old name. 3 regression tests reproduce the original failure and confirm
  the fix.

### What's new in v0.5

- **Real CI**: every push builds (and, on every OS with a native GitHub
  runner, tests) all five target platforms — see
  [`.github/workflows/ci.yml`](.github/workflows/ci.yml). Before this,
  "it compiles" only ever meant "it compiles on this project's Linux
  sandbox."
- **Packaged releases**: pushing a `vX.Y.Z` tag builds and publishes
  prebuilt binaries for all five platforms to GitHub Releases — see
  [`.github/workflows/release.yml`](.github/workflows/release.yml) and
  [Downloads](#downloads) above. This closes out the "Packaged releases"
  roadmap item that's been open since v0.1.
- **`install.sh`/`uninstall.sh`** — the Linux/macOS counterpart to
  `install.ps1`/`uninstall.ps1`, same philosophy: build (or reuse a
  downloaded binary), copy it to a per-user bin directory, add exactly
  one PATH line to whichever shell rc file matches `$SHELL`, touch
  nothing else. Tested end-to-end (install → run → uninstall → PATH
  line cleanly removed) against the real script, not just written and
  assumed correct.
- **A real `LICENSE` file** (MIT) — the README claimed one "before
  publishing" since v0.1; it's now actually there, and bundled into
  every release archive.

### What's new in v0.4

- **`/connect`, `/model`, `/models`, `/provider` are real arrow-key,
  type-to-filter pickers now**, drawn directly in the persistent screen —
  including a live-discovered, browsable/filterable model list and a
  masked API-key box that never leaves the TUI (see
  [Console UX](#console-ux)). Selecting a model applies it immediately.
- **A skills system** (`.rexo/skills/<name>/SKILL.md`, project + global,
  keyword-triggered or forced with `/skill <name>` — see
  [Skills](#skills)) and **custom commands**
  (`.rexo/commands/<name>.md` → `/<name>`, with `$ARGUMENTS`/`$1..$9`
  substitution — see [Custom commands](#custom-commands)).
- **Real `/copy`** (OS clipboard) and **real `/add-dir`** (read-only extra
  roots — see [Known limitations](#known-limitations) for exactly how
  that's scoped down from the full roadmap item).
- **The header mascot has actual expressions** now — thinking, reasoning,
  writing, a permission-wait face, and note-reactive idle states — driven
  off real session state, not a canned animation loop.
- **Ctrl+Y "selection mode"** releases the terminal's own mouse capture so
  its native click-drag select-and-copy works normally; **Alt+M**/**Alt+P**
  jump straight to the model/provider pickers; a bare **`?`** as the very
  first character opens the shortcut list.
- The console title now reads "Rexo Code — \<folder\>" instead of your
  shell's own title; a Windows `.exe` icon and a Windows Terminal
  tab-icon profile fragment are included under `assets/` (the icon
  embedding is real `build.rs`/`winres` code, gated to `cfg(windows)` —
  genuinely untested on this project's own Linux build sandbox, worth
  trying on a real Windows checkout).

See `/release-notes` inside REXO for the full list, including v0.3's.

## Quickstart

Already have a [downloaded release](#downloads)? Skip straight to running
`rexo` (or `.\install.ps1 -SkipBuild` / `./install.sh --skip-build` to put
it on PATH) — everything below is the from-source path.

**Windows (PowerShell):**

```powershell
git clone <your-fork-url> rexo-code
cd rexo-code
copy .env.example .env
```

**Linux / macOS:**

```bash
git clone <your-fork-url> rexo-code
cd rexo-code
cp .env.example .env
```

Edit `.env` and set your key:

```env
NVIDIA_API_KEY=nvapi-...
```

Get a free NVIDIA NIM API key at <https://build.nvidia.com>. `rexo.toml`
already points at NVIDIA's `moonshotai/kimi-k3` — change `[model].model` if
you'd rather use a different NIM model.

```bash
cargo build
cargo run -- "list the top-level files in this project"
```

Or run the compiled binary directly:

```powershell
# Windows
cargo run --release
target\release\rexo.exe "find where authentication is implemented"
```

```bash
# Linux / macOS
cargo run --release
target/release/rexo "find where authentication is implemented"
```

### Running `rexo` from anywhere (not just this folder)

`cargo build`/`cargo run` only ever produce a binary inside this checkout
— cargo never puts that on your PATH for you, which is why `rexo` on its
own fails with "command not found" (or "'rexo' is not recognized...")
from any other directory. A few ways to fix that:

```powershell
# Windows — Option A: the usual Rust way, installs into
# %USERPROFILE%\.cargo\bin, which rustup already added to PATH.
cargo install --path .

# Windows — Option B: builds a release binary and copies it to
# %LOCALAPPDATA%\RexoCode\bin, adding that to your PATH.
.\install.ps1
# (or double-click / run install.bat if you're in cmd.exe)
```

```bash
# Linux / macOS — Option A: the usual Rust way, installs into
# ~/.cargo/bin, which rustup already added to PATH.
cargo install --path .

# Linux / macOS — Option B: builds a release binary and copies it to
# ~/.local/bin (or $REXO_INSTALL_DIR if you set one), adding that to PATH.
./install.sh
```

Either way, open a **new** terminal window afterwards — PATH changes never
reach a terminal that's already running. `.\uninstall.ps1` / `./uninstall.sh`
reverses what the matching install script did.

With no prompt argument, REXO starts an interactive session: a full-screen
terminal UI opens (see [Console UX](#console-ux) below for what that looks
like). Type `/exit`, bare `exit`/`quit`, or press Ctrl+C twice to leave.

> **Security note on API keys:** never put a real key in `rexo.toml`,
> source files, commit messages, or anywhere it might get committed or
> pasted somewhere public — `.env` is git-ignored specifically so this
> doesn't happen. If a key is ever exposed (committed, pasted in a chat,
> logged, etc.), treat it as compromised and rotate it immediately.

## Console UX

The first time you start an interactive session, REXO asks whether it
should trust the workspace — a real, keyboard-navigable dialog (arrow keys
or j/k to move, Enter to confirm, Esc to cancel), rendered with
[`ratatui`](https://ratatui.rs)/`crossterm`:

```
┌ Rexo Code ─────────────────────────────────────────────┐
│ Accessing workspace:                                    │
│ I:\Projects\MyApp                                        │
│                                                           │
│ Quick safety check: is this a project you created or     │
│ one you trust (your own code, a well-known open-source   │
│ project, or work from your team)? If not, review what's  │
│ in this folder before continuing.                        │
│                                                           │
│ REXO will be able to read, edit, and execute files here.  │
│                                                           │
│ > No, exit                                                │
│   Yes, I trust this folder                                │
│                                                           │
│ ↑/↓ to choose · Enter to confirm · Esc to cancel           │
└───────────────────────────────────────────────────────────┘
```

Declining exits without touching the workspace. This only runs for
interactive sessions (not `--yes`, `--non-interactive`, or single-shot
`rexo "task"` runs), and degrades gracefully — not an error — if the
terminal can't support it (piped output, some restricted consoles).

After that, the whole session is one persistent, full-screen `ratatui`
application — a live header, a scrollable transcript, and a single bordered
input box that *is* the input (earlier versions had a second, separate
`rexo>` prompt underneath a plain-printed banner that didn't actually take
input where it visually looked like it should; that's gone):

```
┌──────────────────────────────────────────────────────────────────────────┐
│ ┏━━┓  Rexo Code v0.2.0                                                   │
│ ┃◉◉┃  nvidia (moonshotai/kimi-k3)                                        │
│ ┗━━┛  I:\Projects\MyApp                                                  │
│       Edits: ask  Terminal: ask  Git writes: ask                        │
└──────────────────────────────────────────────────────────────────────────┘
 Welcome to Rexo Code v0.2.0. Type a task, or /help for commands.

 > list the files in this project
 ✻ Thinking…                          (dimmed chain-of-thought, if the model streams one)
 Let me check the project structure first...

 ⏺ Read(src/main.rs)
   ⎿ Read 214 lines

 ⏺ Edit(src/main.rs)
   ⎿ Edited src/main.rs (-3 / +5 lines).

 Here's what I changed and why...      (the final answer)

┌──────────────────────────────────────────────────────────────────────────┐
│ Try "list the files in this project"                                     │
└──────────────────────────────────────────────────────────────────────────┘
  ⏸ manual mode · /help for shortcuts · Ctrl+C to cancel
```

The header updates live the moment `/model`, `/provider`, `/permissions`,
`/rename`, or `/color` change anything — no restart, no stale display.

The input box has real line editing: Up/Down for command history, and
**Tab-completion** — type `/mo` and a floating popup lists `/model` and
`/models`; Up/Down narrows the selection, Tab or Enter accepts it.
`/workspace <path>` and `/cd <path>` tab-complete real, on-disk directories
the same way.

`/help` opens as a full-screen overlay (General / Commands / Custom
commands tabs, ←/→ to switch, ↑/↓ to scroll) rather than a wall of text —
run it any time for the complete, current command list; `/background` is
marked `(planned)` — it's registered for discoverability but doesn't do
anything yet (see [Roadmap](#roadmap)) — everything else in the list is real.

Permission prompts (`y` once / `a` always this session / `n` deny) are a
proper modal in this same screen — not a second blocking `stdin` read
racing the render loop. A handful of commands that need old-fashioned
blocking terminal input of their own (`/connect`'s guided setup,
`/provider add`, `/api set`'s hidden password prompt, `/workspace`/`/cd`
when the target doesn't exist yet and REXO asks whether to create it) step
out of this screen for the length of that one prompt and back in
afterwards, rather than reimplementing every one of those flows against
raw-mode key events — see the doc comment on `cli::tui::run_suspended` if
you're curious exactly why.

Some models (Kimi K3 in particular) **always reason** before answering,
streamed separately from the final answer as `reasoning_content`. REXO shows
that "thinking" text dimmed as it arrives, with an animated spinner + elapsed
timer before the first token, so the screen never looks frozen — see
[Speeding it up](#speeding-it-up) below for making that reasoning step
faster.

Press Ctrl+C at any point during a turn to cancel it cleanly — you get a
`Cancelled.` note and a clean return to the input box. At an idle, empty
input box, Ctrl+C once shows a reminder; press it again within two seconds
(or type `/exit`) to actually leave. With text typed in the box, Ctrl+C
just clears the line, the usual shell convention.

## How it works

```
rexo "your task"
      │
      ▼
 conversation + tool schemas ──► model
      │                            │
      │       final answer ◄───────┤
      │                            │
      │                     tool call(s)
      │                            │
      │                     permission check
      │                    (automatic / ask / denied)
      │                            │
      │                        execute tool
      │                            │
      │                     tool result appended
      │                            │
      └────────────────── loop until final answer ─┘
```

Bounded by `[agent].max_iterations` and `[agent].max_tool_calls` in
`rexo.toml`, so a misbehaving model can't loop forever.

### Tools

| Tool | What it does | Default permission |
|---|---|---|
| `read_file` | Read a UTF-8 text file (line-numbered) | automatic |
| `list_files` | List a directory, optionally recursive | automatic |
| `search_files` | Recursive literal-substring search | automatic |
| `edit_file` | Exact find/replace on a file's content | ask |
| `create_file` | Create a new file | ask |
| `delete_file` | Delete a file | ask (high risk) |
| `run_command` | Run a shell command | classified: safe (`cargo check`, `git status`, ...) / ask / **blocked** if it matches the dangerous-command policy |
| `git` | Run a git subcommand | read subcommands (status/diff/log/branch/show/remote) automatic; write subcommands (commit/push/checkout/...) ask |

`edit_file` is deliberately not "overwrite the whole file": the model must
supply the exact text it expects to find (`old_string`) and what to replace
it with (`new_string`). The edit is rejected — no change made — if that text
isn't found, or isn't unique, in the file. This both forces the model to
work from the file's real current content and gives a free, cheap conflict
check.

### Security

- **Workspace boundary**: every filesystem tool resolves paths against the
  workspace root and refuses anything that would escape it (`../../etc/...`,
  absolute paths outside the workspace, etc).
- **Command risk classification**: shell commands are matched against a
  safe-prefix allow-list and a dangerous-pattern deny-list. Dangerous
  commands (`rm -rf /`, `Remove-Item -Recurse -Force`, disk/format
  operations, `sudo`, execution-policy changes, ...) are blocked outright —
  no permission prompt can override this. Chained commands
  (`git status && rm -rf /`) are decomposed and re-checked so a dangerous
  command can't hide behind a safe-looking prefix.
- **Permission engine**: anything that isn't automatic asks for approval —
  `y` (once), `a` (always for the rest of this session), or anything else
  (deny). `--yes` auto-approves everything (use with care); `--non-interactive`
  (or simply not having a terminal attached, e.g. in CI) denies anything not
  pre-approved in `rexo.toml` instead of hanging on a prompt.
- A denied or blocked call is reported back to the model as a normal tool
  result — the agent adapts instead of crashing.

None of this is a full sandbox (REXO runs with your OS-user's permissions,
same as any CLI tool you `cargo run`). It's a deliberate, layered set of
speed bumps and hard stops, not a security boundary against a truly
adversarial model.

### Configuration, providers, and credentials

Three layers, most specific wins — this is the fix for the exact bug of
"REXO forgets its provider when I `cd` somewhere else": earlier versions'
*only* place for provider/model settings was a `rexo.toml` next to
wherever you ran `rexo` from, and the *only* place for credentials was a
`.env` in that same directory — neither followed you anywhere else.

```
CLI flags (--provider/--model/--base-url)        most specific
    ↓
session-only changes (/provider, /model, ... when you pick
"this session only" rather than "permanently")
    ↓
REXO_PROVIDER / REXO_MODEL / REXO_BASE_URL environment variables
    ↓
workspace rexo.toml's [model] section, if present
    ↓
global config.toml's default provider profile
    ↓
built-in defaults                                least specific
```

**Global config** lives in an OS-appropriate per-user directory —
`%LOCALAPPDATA%\RexoCode` on Windows (the same directory `install.ps1`
already puts `rexo.exe` in), `~/.local/share/rexo-code` on Linux,
`~/Library/Application Support/rexo-code` on macOS — never inside a
workspace, so it loads identically no matter which directory `rexo` runs
from. `/connect`'s "Permanently" choice and the first-launch wizard write
to it; `/doctor` shows exactly where it is on your machine.

**Credentials** are stored separately, in `credentials.toml` next to
`config.toml` — file-based, not a true OS-encrypted vault (not Windows
Credential Manager / macOS Keychain / a Secret Service item — see
[Known limitations](#known-limitations)), but structurally outside any
git-tracked project directory (impossible to `git add` by accident, unlike
a workspace `.env`), permission-hardened to owner-only on Unix, and
resolved the same way regardless of workspace. Key resolution for a given
provider profile: session override → `REXO_<CREDENTIAL_KEY>_API_KEY` env
var → the credential store. `/api set --permanent` and `/connect`'s
"Permanently" choice write to it; nothing ever prints a key, logs one, or
sends one anywhere but the provider itself.

### First run

The first time you run `rexo` with no saved global config, a short setup
wizard runs instead of dropping straight into a session: search for a
provider, enter a key, pick a model, choose permanent-vs-session. It never
runs again after that (whether you configured something or explicitly
skipped it) — `/connect` covers the same ground any time later. Piped/CI
runs (`--non-interactive`, `--yes`, or no real terminal attached) skip it
silently and just proceed.

If you already had a working v0.2-style setup (workspace `rexo.toml` +
`.env`) the first time you run the new version, the wizard detects that
and offers a one-question "save this globally too?" instead of the full
flow — your existing settings aren't touched either way.

### Providers

Most providers speak the same OpenAI-compatible `/chat/completions`
protocol (request building + SSE streaming parsing lives once, in
`src/providers/protocol.rs`) and just differ in endpoint/model — `/connect`
searches a built-in catalog of about 18 of these
(`src/providers/catalog.rs`: OpenRouter, Groq, Together, Fireworks,
Mistral, DeepSeek, Cerebras, DeepInfra, xAI, OpenAI, NVIDIA NIM, plus a few
more listed for discoverability where REXO isn't confident enough in a
current exact endpoint to pre-fill one, and asks instead of guessing
wrong) or takes a fully custom endpoint (`/connect` → `custom`, or
`/provider add`). Local servers (Ollama, vLLM, LM Studio, llama.cpp's
server, ...) work the same way via the `local` kind.

**Google Gemini has a real, native driver** (`src/providers/gemini/`,
`/connect` → Google Gemini) — Gemini's `generateContent`/
`streamGenerateContent` APIs genuinely aren't OpenAI-compatible (a
different auth header, and a completely different request/response shape
— no `system`/`tool` roles, function calls and results live inline as
message *parts* instead of separate fields), so pointing the
OpenAI-compatible client at it, as an earlier version of this catalog
briefly implied you could, always 401'd regardless of the key. **As of
v0.6, it streams for real** — `streamGenerateContent?alt=sse`, parsed
incrementally (its own SSE loop, separate from the OpenAI-style
`choices[].delta` one every other provider shares via
`protocol.rs`, since Gemini's chunk shape is different) — replacing the
earlier "call the plain endpoint once, report the whole answer at the
end" fallback. It's still genuinely untested against the live API: this
project's own build/test sandbox can't reach
`generativelanguage.googleapis.com` at all, so what's covered is the
request-building and SSE chunk-assembly logic against hand-written
example payloads, including a full multi-chunk stream strung together
byte-by-byte (`cargo test -- gemini`), not an end-to-end call — please
file an issue if the live shape has drifted from what's implemented.

Anthropic and Cohere use their own genuinely different wire protocols too
and don't have adapters yet — see [Known limitations](#known-limitations)
rather than a catalog entry that would silently fail to connect. A native
Gemini driver exists now specifically because it was the most-requested
gap; the same `kind`-based pattern in `providers::build_provider` is what
the next one would extend.

```toml
[model]
provider = "nvidia"            # or "openai_compatible", "local", or "gemini"
model = "moonshotai/kimi-k3"
# base_url = "..."             # required for openai_compatible; optional
                                # override for local (default: localhost:11434)
```

Setting `[model]` explicitly in a workspace `rexo.toml` overrides whatever
your global default provider is *for that workspace only* — the layered
resolution above.

**Live model discovery**: `/model` and `/models` call the endpoint's
`GET /models` (near-universal among OpenAI-compatible providers) and let
you search the results, rather than a hard-coded, inevitably-stale model
list. Falls back to manual entry when an endpoint doesn't support it —
that's not an error, just not every server implements it.

**Model capabilities**: `Provider::capabilities()` reports what's known
about tool-calling/streaming/vision/reasoning support, and, new in v0.6,
`parallel_tools`/`structured_output`/`model_discovery`/`context_window`/
`max_output`/`cancellation`/`usage_reporting` too (`None` = genuinely
unknown, not "no" — REXO doesn't have live capability data for most
endpoints and won't fake confidence it doesn't have). `/status` now
actually displays this (it didn't before v0.6 — the struct existed but
nothing read it). Still advisory, not yet gating agent behavior — see
[Known limitations](#known-limitations).

**Normalized provider errors**: also new in v0.6 — every provider's HTTP
and network failures get classified into a small closed
`ProviderErrorKind` (`rate_limited`, `authentication`, `context_exceeded`,
`timeout`, `network`, `server`, ...) via `classify_http_status`/
`classify_reqwest_error` in `src/providers/mod.rs`, on top of the
existing human-readable message (which is unchanged). The TUI tags
retryable failures in the transcript (e.g. `[rate_limited, usually
transient — worth trying again]`), and `rexo --output json` includes an
`error_kind` field scripts can branch on instead of grepping message
text. Kept as a genuinely closed enum, not a provider-defined string —
see the architecture-quality notes on avoiding "stringly-typed runtime
state."

### Configuration

`rexo.toml` (safe to commit — never put secrets in it) holds *workspace*
overrides — agent limits, permissions, and an optional `[model]` section
that beats your global default provider for this project specifically:

```toml
[agent]
max_iterations = 50
max_tool_calls = 100

[model]
provider = "nvidia"
model = "moonshotai/kimi-k3"
temperature = 0.3
max_tokens = 4096
reasoning_effort = "low"   # passed through as-is; ignored by providers that don't support it

[permissions]
allow_read = true
allow_search = true
allow_edit = false
allow_terminal = false
allow_git_write = false
allow_network = false
```

(`rexo.toml` also ships with two commented-out fast-model alternatives —
see [Speeding it up](#speeding-it-up).)

`REXO_PROVIDER` / `REXO_MODEL` / `REXO_BASE_URL` environment variables
override both `rexo.toml` and the global default provider, for this run.
`REXO_<CREDENTIAL_KEY>_API_KEY` resolves a specific saved profile's key —
see Providers above. `REXO_GLOBAL_DIR` overrides where the global
directory itself lives (mainly useful for tests/portable installs).

### CLI flags

```
rexo [OPTIONS] [PROMPT]...

  <no prompt>              interactive session
  "task description"       run once and exit (headless — see below)

  -y, --yes                auto-approve every permission prompt
      --non-interactive    never prompt; deny anything not pre-approved
  -C, --workspace <DIR>    operate in DIR instead of the current directory
      --model <MODEL>      override the configured model for this run
      --provider <KIND>    override the provider for this run (nvidia|openai_compatible|local)
      --base-url <URL>     override the API base URL for this run
      --output <FORMAT>    text (default) | json | jsonl — see Headless mode
      --fast                trim reasoning effort + token ceiling for this run
      --no-intro            skip the animated startup intro (also: REXO_NO_INTRO=1)
      --list-tools         print available tools and exit (no API key needed)
```

### Headless mode

`rexo "task"` already ran once and exited (no TUI) — v0.3 adds
scriptable output and exit codes on top of that, for CI/git-hooks/pipes:

```powershell
rexo "fix the failing test" --output json
```

prints exactly one JSON line (`{"status": "ok"|"error", "answer": ..., "error": ...}`)
instead of the normal human-readable stream — `--output jsonl` is
currently identical to `json` for a single-shot run (no per-token
streaming JSONL yet; see [Known limitations](#known-limitations)) rather
than pretending a difference that isn't implemented. Exit codes:

| Code | Meaning |
|---|---|
| 0 | Completed successfully |
| 1 | The task/request itself failed |
| 2 | Configuration/credential error — couldn't even start (no API key, bad provider config, ...) |
| 3 | *(reserved for permission/security rejection — not yet distinguished from code 1; see Known limitations)* |

```powershell
cat error.log | rexo "diagnose and fix this" --non-interactive --output json
```

### `@file` references

Type `@src/main.rs` (or `@src/` for a directory listing) anywhere in your
message and REXO resolves it to real file content before sending — the
model doesn't burn a tool call reading something you already pointed at.
A searchable picker pops up as you type after the `@`, respecting
`.gitignore`, skipping binaries, and capping how much any one reference
pulls in (~60KB) so `@src/` on a big tree doesn't blow your context.
Multiple references in one message work fine: `@src/main.rs @Cargo.toml`.

### Skills

A skill is a folder with a `SKILL.md` inside — reusable, portable
know-how the model can draw on, in the same spirit as Claude Skills, but
not a claim of protocol compatibility with any specific product's format:

```
.rexo/skills/commit-style/SKILL.md      (project — checked into the repo)
<global config dir>/skills/api-conventions/SKILL.md   (personal, every workspace)
```

```markdown
---
name: commit-style
description: How this repo writes commit messages
triggers: commit, changelog
---
Use imperative mood, a 72-character subject line, and reference the
issue number when one exists.
```

`triggers` is an optional, comma-separated list of words — if one shows
up (case-insensitively, plain substring match, not semantic search) in
what you type, REXO loads that skill into the conversation automatically
and says so in the transcript. `/skill <name>` loads one on demand
regardless of triggers; `/skills` lists everything discovered, project
and global, and shows which ones are already loaded this session. A
project skill with the same `name:` as a global one replaces it.

### Shell mode

Type a bare `!` and press Enter to drop into shell mode: the input box
border turns yellow and reads `SHELL MODE — ! or 'exit' to leave`, and
every line you type from then on runs directly in PowerShell (Windows)
or `sh` (Linux/macOS) — not sent to the model at all. Type `!` again, or
`exit`, to leave (leaving shell mode, not REXO — `/exit` always quits
REXO itself, even while shell mode is on).

Don't want to switch modes for one command? `!<command>` runs it
immediately regardless of which mode you're in — `!git status` works
the same whether shell mode is on or off.

```text
> !
Shell mode on — every line now runs directly in PowerShell/sh, not the model.
> git status
On branch main
nothing to commit, working tree clean
> !
Shell mode off.
```

This is deliberately outside the agent's permission/security engine —
you're the one typing the command, the same trust boundary as opening a
real terminal window, not the model requesting one. REXO's permission
prompts exist to gate what the *model* can do; they were never meant to
gate what you type into your own shell.

### Custom commands

Drop a markdown file in `.rexo/commands/<name>.md` (project) or your
global commands directory (personal, every workspace) and `/<name>`
sends its contents to the model as a prompt:

```markdown
---
description: Review a diff against our style guide
---
Review this diff for style-guide violations. Focus area: $1

$ARGUMENTS
```

`/review security @src/auth.rs` expands `$1` to `security` and
`$ARGUMENTS` to everything typed after the command name (including the
`@file` reference, which still resolves normally) before the turn runs.
`/commands` lists what's discovered; an unused `$3` with only two
arguments typed is left as literal text rather than silently blanked, so
it's obvious something's missing.

### Interactive commands

Inside an interactive session, you don't need to edit `rexo.toml` or
restart to change anything — everything below takes effect immediately.
This is the core set; run **`/help`** (or type **`{?}`** for just the
keyboard shortcuts) for the complete, current list — around 55 commands as
of v0.8.0, with `/background` the one command marked `(planned)` and
registered honestly rather than left out or faked (it needs git-worktree
isolation — see [Roadmap](#roadmap)):

```
/help                  Show the full-screen command browser
{?}                    Show the keyboard shortcut list
/status                Provider, model, workspace, permissions, limits
/config                Non-secret configuration (rexo.toml-shaped)
/doctor                Health-check: workspace, git, credentials, global config location

/model [id]            Show/change model — search live, or enter any ID
/models                List models available from the current endpoint

/connect               Searchable provider picker; guided key/model/save setup
/provider [name]       Switch to a saved profile, or a kind: nvidia|openai_compatible|local
/provider add          Add a fully custom OpenAI-compatible endpoint
/providers             List saved profiles + the built-in catalog

/base-url [url]        Show/change the API base URL
/api                   Show API key status (never the key itself)
/api set               Set a key for this session; add --permanent to save it
/api clear             Clear the session override (and the saved key, if any)

/mcp                   List configured MCP servers
/mcp add <name> --command "..." | --url "..."
/mcp remove|enable|disable <name>

/workspace [path]      Show/change the workspace directory
/cd [path]             Same thing, shorter name
>  [path-or-fragment]  Quick workspace switch with real directory suggestions

/permissions                            Show current permissions
/permissions <kind> <always|ask>        kind = edit | create | delete | terminal | git

/init                  Create REXO.md — project notes included in every system prompt
/memory                View this workspace's REXO.md
/export [file]          Save the conversation to a Markdown file
/context               Rough estimate of how much conversation you're using
/compact                Shrink older tool output to free up space

/skills                List discovered skills (project + global)
/skill <name>          Force-load a skill into the conversation now
/commands              List your custom (user-defined) slash commands
/add-dir [path]         Add a read-only extra directory; also remove/clear/list
/copy [all|<n>]         Copy an answer (or the whole conversation) to the clipboard

/tools                 List available tools (from the real tool registry)
/clear                 Clear the conversation, keep configuration
/reset                 Reload provider/model/permissions from rexo.toml/global config
/exit, /quit, /q        Exit (bare "exit"/"quit" and double Ctrl+C also work)
```

Shortcuts worth knowing beyond the slash commands: a bare **`?`** as the
very first character (before anything else is typed) opens the shortcut
list immediately, same as `{?}`. **Alt+M**/**Alt+P** jump straight to the
`/model`/`/provider` pickers from anywhere. **Ctrl+Y** toggles "selection
mode" — it releases the terminal's own mouse capture so its native
click-drag select-and-copy works normally, since having mouse capture on
(which this screen needs for scroll-wheel support) is exactly what stops
a terminal's own text selection from working. Run `/keybindings` or `{?}`
for the complete, current list.

A few things worth knowing:

- **Nothing here is a second copy of the agent.** Every command drives the
  *same* `Agent`/`Provider`/`PermissionManager`/`ToolRegistry` the AI prompt
  path uses — `/model` swaps the live provider in place, `/permissions`
  mutates the live permission manager, `/workspace` moves the real
  filesystem boundary every tool enforces. There's no restart and no stale
  second state to fall out of sync.
- **`/workspace`, `/cd`, and `>` all re-validate and re-apply the security
  boundary.** Path traversal is blocked exactly the same way after
  switching workspaces as before. Switching also clears any "always this
  session" permission grants from the *previous* workspace, so trust
  extended to one project doesn't silently carry into another opened in
  the same run.
- **REXO.md is this project's version of a persistent instructions file**
  (create one with `/init`). If it exists, its contents go into the system
  prompt on every turn, ahead of the auto-detected project type/tree/README
  excerpt — explicit guidance you wrote beats REXO's own guesses.
- **Commands marked `(planned)` in `/help` genuinely don't do anything
  yet** — they print a short, honest note instead of either failing with
  "unknown command" or silently pretending to work. See
  [Roadmap](#roadmap).
- **`/api set` never echoes the key.** Without `--permanent` it only
  overrides the resolved key for this session (not written anywhere);
  with `--permanent` it's saved to the global credential store — see
  [Configuration, providers, and credentials](#configuration-providers-and-credentials).
- **`/status` and `/config` structurally can't leak a key** — they're built
  from a `configured: bool`, never from the key value itself. There are
  tests asserting this (`cli::commands::tests::*never_contains_the_api_key*`).
- **Model discovery isn't magic** — it depends on the endpoint actually
  implementing `GET /models`. Most OpenAI-compatible providers do; some
  don't, and `/model`/`/models` fall back to manual entry rather than
  erroring when it isn't available.
- **`/reset` reloads `rexo.toml`/env for your *current* workspace** and
  clears the conversation, but deliberately leaves the workspace itself
  alone — "reset configuration" and "go back to a different directory" are
  different asks.

## Project layout

```
src/
├── main.rs                  CLI entry point
├── cli/
│   ├── mod.rs                 Session (runtime state) shared by all commands
│   ├── parser.rs               line -> slash command / '>' shortcut / plain prompt
│   ├── commands.rs             command table, handlers, provider catalog
│   ├── completion.rs            slash-command + on-disk path suggestions (pure, no terminal dep)
│   ├── output.rs                stdout-capture so command handlers can stay unchanged and still land in the TUI
│   ├── trust_dialog.rs          ratatui startup workspace-trust screen
│   └── tui/                    the persistent full-screen session (see Console UX)
│       ├── mod.rs                 TuiCore: input, transcript, header sync, event loop, suspend/resume
│       └── render.rs              pure drawing functions (header/transcript/input/help/permission modal)
├── agent/
│   ├── mod.rs                 agent loop (model → tool → permission → execute → repeat), plain + TUI variants
│   ├── tui_support.rs          AgentUi trait the TUI implements to receive a turn's output
│   ├── context/mod.rs         bounded project-context summary for the system prompt (incl. REXO.md)
│   ├── planner/mod.rs         multi-step planning — scaffolding, not wired in yet
│   └── prompts/mod.rs         system prompt text
├── config/mod.rs             rexo.toml + env loading
├── providers/
│   ├── mod.rs                 Provider trait, ChatMessage/ToolCall types, provider selection
│   ├── protocol.rs             shared OpenAI-compatible HTTP client + SSE stream parser
│   ├── nvidia/                NVIDIA NIM
│   ├── openai_compatible/     any OpenAI-compatible endpoint
│   └── local/                 local model servers (Ollama, vLLM, ...)
├── security/
│   ├── policies/mod.rs         workspace-boundary + command-risk rules (stateless)
│   └── permissions/mod.rs      approval flow (stateful: config + session + prompts), plain + interactive-modal variants
├── tools/
│   ├── mod.rs                  Tool trait, ToolRegistry
│   ├── filesystem/, search/, patch/, terminal/, git/
└── utils/mod.rs
```

Top-level, alongside `src/`:

```
install.ps1, install.bat, uninstall.ps1    Windows — build from source, put rexo on PATH
install.sh, uninstall.sh                    Linux/macOS — same, see Quickstart
build.rs                                     cfg(windows)-gated: embeds assets/rexo.ico into rexo.exe
assets/rexo.ico, assets/windows-terminal-profile.json
LICENSE                                      MIT
.github/workflows/ci.yml                     build + test on all 5 platforms, every push
.github/workflows/release.yml                on a vX.Y.Z tag: build + publish all 5 to GitHub Releases
```

## Speeding it up

Response time is almost always the model, not REXO — Kimi K3 always reasons
before answering, and no setting removes that step entirely (only how much).
In order of impact:

1. **Switch models for everyday tasks.** `rexo.toml` has two fast,
   still tool-calling-capable alternatives commented out under
   `[model].model` — `nvidia/nemotron-3-nano-30b-a3b` (small/fast) or
   `deepseek-ai/deepseek-v4-flash` (fast, good for small edits). Keep Kimi
   K3 for genuinely hard, multi-file tasks where the extra thinking pays off.
2. **Use `--fast`** for a one-off quick run — it trims `reasoning_effort`
   to `"low"` and caps `max_tokens` at 2048 for that invocation only.
3. **Lower `[model].max_tokens`** (default `4096`) permanently in
   `rexo.toml` — caps how long a single turn, including reasoning, is
   allowed to run.

A multi-step task also means multiple full model turns (read → reason →
edit → verify → ...) — each one reasons again from scratch, so total time
scales with how many tool calls the task needs, not just model speed.

## Troubleshooting

**`rexo` isn't recognized as a command, even though I built it.** `cargo
build`/`cargo run` never put anything on PATH — that's expected, not a
bug. See [Running `rexo` from anywhere](#running-rexo-from-anywhere-not-just-this-folder)
above.

**macOS says the download "cannot be opened because the developer cannot
be verified" (or Gatekeeper just silently refuses it).** Expected — see
the caveat under [Downloads](#downloads): these builds aren't
code-signed or notarized (that needs a paid Apple Developer account,
which isn't set up for this project). Run
`xattr -d com.apple.quarantine rexo` in the folder you extracted it to,
or right-click the binary → Open once, and macOS won't ask again.

**`install.sh`/`install.ps1` finished but a new terminal still can't
find `rexo`.** The scripts append a PATH line to whichever shell rc file
matches `$SHELL` (`~/.bashrc`, `~/.zshrc`, or `~/.config/fish/config.fish`
on Windows it's a registry edit, not a file) — if you use a shell they
don't detect, or a login-shell setup that doesn't source that file (e.g.
`~/.bash_profile` sourcing `~/.bashrc` isn't universal), add the printed
install directory to PATH yourself. `REXO_INSTALL_DIR` lets you pick the
directory up front instead.

**There was a `rexo>` prompt at the bottom that didn't seem to do
anything, below a box that looked like it should be the input.** Fixed —
that was two separate things stacked on top of each other: a plain-printed
banner (including a box that *looked* like an input field but was just
static text) and, underneath it, `rustyline`'s own separate prompt, which
was the only thing actually reading input. The whole session is now one
persistent screen and the bordered box you see *is* the input — see
[Console UX](#console-ux).

**A permission prompt appears and gets denied before I can answer it.**
Fixed — earlier versions auto-detected "is this a real terminal?" to decide
whether to prompt at all, and that detection could misfire on some Windows
terminal setups (ConPTY-hosted consoles, certain launch wrappers), silently
denying every prompt without ever waiting for input. REXO now only skips
prompting when you explicitly pass `--yes` or `--non-interactive` — every
other run genuinely waits for your answer, via a proper in-screen y/a/N
modal.

**It looks frozen after I send a prompt / PowerShell shows `exit code:
0xc000013a, STATUS_CONTROL_C_EXIT`.** That exit code just means Ctrl+C was
sent to the process — it's not a crash. REXO now shows a live spinner with
an elapsed timer, then the model's reasoning dimmed as it streams in, so it
shouldn't look frozen anymore. If a task is still taking too long, press
Ctrl+C — REXO catches it and exits cleanly with `Cancelled.` See
[Speeding it up](#speeding-it-up) above for making it actually faster.

**`NVIDIA_API_KEY is not set`, or any other "no API key found" message.**
Run `/connect` (or, on first launch, just answer the setup wizard) and
pick "Permanently" — that's the recommended path now. `.env`/shell
environment variables still work too (see
[Configuration](#configuration) for the `REXO_<KEY>_API_KEY` naming).

**A provider that worked from one project directory failed with `404 Not
Found` (or similar) after switching to a different workspace.** This was
the headline bug in versions before global configuration existed:
provider/model settings and credentials were tied to whichever
`rexo.toml`/`.env` happened to be in the directory you launched `rexo`
from, so switching directories could silently lose them. Global,
workspace-independent configuration (this version) fixes it structurally
— see [Configuration, providers, and credentials](#configuration-providers-and-credentials).
If you still see this on a fresh setup, it usually means the model ID
isn't valid for the endpoint it's pointed at; `/doctor` and `/status` show
exactly what's currently configured, and the error message itself now
names the endpoint/model/status rather than a bare `404 Not Found`.

## Development

```powershell
cargo check
cargo test
cargo run -- --list-tools
```

Keep `cargo check` and `cargo test` green after any change — this is worked
on incrementally, one module at a time, not all at once.

## Known limitations

Called out here rather than left implicit, per the principle this project
tries to hold itself to: don't claim something works when it's only
partially there.

- **MCP: stdio transport only, no reconnect, no resources/prompts.**
  `/mcp connect` does the real thing — spawns the server, completes the
  `initialize` handshake, discovers tools, and registers each one as a
  real, permission-gated REXO tool. What's not there yet: the SSE/HTTP
  transport (the `url` half of a server config), automatic reconnect if
  a connected server crashes (its tools just start erroring until you
  `/mcp connect` again), and the resource/prompt halves of the MCP spec
  (tool-calling only — the part REXO's own agent loop can act on).
- **Hooks run real shell commands with no sandboxing.** `/hooks`'
  `pre_tool`/`post_tool`/`session_start`/`session_end` events genuinely
  execute what you configure, with the same trust model as any other
  command REXO runs on your behalf — a hook is exactly as powerful as
  typing the command yourself, not sandboxed or resource-limited.
- **Subagents (`/agents run`) are sequential, not concurrent.** A real,
  bounded, persona-configured agent runs a task to completion using its
  own conversation — but it blocks the parent session until it finishes,
  and shares the same workspace with no isolation. Concurrent/backgrounded
  subagents need git-worktree isolation on top of this, which isn't
  built — see [Roadmap](#roadmap).
- **`/ide` has no editor extension to talk to yet.** The server side is
  real and tested (a local TCP server, a documented newline-JSON
  protocol, a discovery lockfile) — but there's no VSCode/JetBrains
  extension in this release that connects to it. Building one is a
  separate project in a different language ecosystem.
- **Plugins have no sandboxing, versioning, or remote install.** A
  plugin is a local folder you (or someone you trust) put at
  `.rexo/plugins/<name>/` — its hooks run with the same trust as any
  other hook, there's no dependency resolution between plugins, and
  there's no plugin registry to install *from*.
- **Credentials are file-based, not an OS-encrypted vault.** See
  [Configuration, providers, and credentials](#configuration-providers-and-credentials)
  — real, workspace-independent, permission-hardened persistence, but not
  Windows Credential Manager / macOS Keychain / a Secret Service item.
- **Model capabilities are mostly "unknown".** `Provider::capabilities()`
  covers 11 fields as of v0.6 (tool-calling/streaming/vision/reasoning/
  parallel_tools/structured_output/model_discovery/context_window/
  max_output/cancellation/usage_reporting) and is now shown in `/status`,
  but nothing in the agent loop gates behavior on it yet (e.g. refusing a
  vision request on a text-only model — Alt+V will still attach an image,
  it just won't be useful against a model that can't see it) — it's an
  honest "don't know" for most endpoints and most fields, not a populated
  capability matrix.
- **No adapters yet for non-OpenAI-compatible protocols** — Anthropic,
  Cohere, AWS Bedrock. Not in the `/connect` catalog at all rather than
  listed and silently broken. (Google Gemini *does* have a native adapter
  — see [Providers](#providers) — this list is what's still missing.)
- **No live pricing, context-window, or token-usage display.** `/context`
  gives a rough character-count estimate labeled as such; REXO doesn't
  maintain (and won't fabricate) a static pricing/context-window database
  that would inevitably go stale.
- **`--output jsonl` is currently identical to `json`** for a single-shot
  run — no real per-token streaming JSONL yet.
- **Exit code 3** (permission/security rejection) is reserved but not yet
  distinguished from a generic task failure (code 1).
- **`/compact` is a heuristic**, not a real summary — it replaces older
  tool output with a placeholder; it doesn't ask the model to summarize.
- **`search_files` is a plain recursive substring scan**, not fuzzy or
  indexed.
- **The workspace-trust dialog isn't remembered across runs** — it asks
  again next time, even for a workspace you already trusted.
- **`/add-dir` is real but read-only.** Extra directories you add are
  reachable by `read_file`/`list_files` via an *absolute* path only —
  `edit_file`/`create_file`/`delete_file`/`run_command`/git-write stay
  scoped to the primary workspace alone, and `search_files` doesn't reach
  them at all yet. A deliberate, safety-conscious scope-down from the
  roadmap's full "multiple simultaneous working directories," not that
  item finished — `/workspace`/`/cd` still switch rather than add for
  anything beyond reading.
- **`/copy` needs a reachable display server/clipboard**, and so does
  Alt+V's clipboard image paste — both are real OS clipboard calls
  (`arboard`), which means neither does anything useful in a
  headless/no-display environment (SSH without X forwarding, some CI) —
  `/export` still works there.
- **Skills and custom commands are keyword/template-based, not smart.**
  Skill triggers are plain case-insensitive substring matches (no
  semantic search), and custom-command placeholders are literal string
  substitution — both genuinely useful, neither claiming to be more than
  that. See [Skills](#skills) and [Custom commands](#custom-commands).
- **The Windows exe-icon embedding (`build.rs`/`winres`) builds in CI now
  but its *appearance* is still unverified.** `ci.yml`/`release.yml` both
  build it for real on Windows runners (it was never built at all when
  this project's only build environment was its own Linux sandbox), so a
  compile-time regression would now be caught — but nothing in CI opens
  File Explorer and looks at the icon, so whether it actually shows up
  correctly is still unconfirmed. Try a downloaded build and file an
  issue if it doesn't show up.
- **No git-worktree isolation or background sessions.** The one real,
  deliberately-out-of-scope-for-now architecture direction left from the
  original "harness" list — see [Roadmap](#roadmap). Everything else that
  list used to cover (hooks, a skill system, subagents) is real as of
  v0.8.0.

## Roadmap

- [x] Tool trait, registry, JSON schemas
- [x] Provider abstraction + NVIDIA/OpenAI-compatible/local providers, real SSE streaming
- [x] Agent loop with iteration/tool-call safety limits
- [x] Permission engine + command-risk/workspace-boundary policy layer
- [x] File editing (`edit_file`/`create_file`/`delete_file`)
- [x] Bounded project context (type detection, top-level tree, README excerpt, REXO.md)
- [x] Basic git tool
- [x] Interactive CLI with live streaming, spinner, and Ctrl+C cancellation
- [x] Slash-command layer with runtime provider/model/workspace switching — no restart needed
- [x] Startup workspace-trust dialog (real `ratatui` screen)
- [x] Tab-completion for slash command names and workspace paths
- [x] A full persistent TUI (fixed header + scrollable transcript + one
      real input box, the whole session as one continuously redrawn
      `ratatui` frame), plus mouse-wheel scrolling — see [Console UX](#console-ux)
- [x] Full-screen `/help` browser with a single source of truth for
      keybindings (`{?}`, `/keybindings`, and the help screen all read the
      same table), ~50 commands total
- [x] `install.ps1`/`install.bat`/`uninstall.ps1` to put `rexo` on PATH
- [x] `/doctor` health checks
- [x] **Workspace-independent global configuration** — the v0.2→v0.3
      headline fix: provider/model/credentials no longer live-and-die with
      whichever `rexo.toml`/`.env` happened to be in the current directory
- [x] Persistent, workspace-independent credential storage
      (`config::credentials`)
- [x] Named, saved provider profiles (`/connect`'s "Permanently", `/provider <name>`)
- [x] First-launch setup wizard, with v0.2-config migration offer
- [x] Searchable `/connect` over an extensible provider catalog (~18 presets)
- [x] Live model discovery (`GET /models`) with search, manual entry, and
      provider-default fallback
- [x] `Provider::capabilities()` — tool-calling/streaming/vision/reasoning,
      plus (new in v0.6) parallel_tools/structured_output/model_discovery/
      context_window/max_output/cancellation/usage_reporting, honestly
      "unknown" where REXO doesn't have real data — and now actually
      displayed in `/status`, not just defined
- [x] Actionable provider error messages (endpoint/model/status/likely
      causes, never the key) instead of a bare HTTP status
- [x] Normalized provider errors (v0.6) — every provider's failures
      classified into a closed `ProviderErrorKind` enum
      (`rate_limited`/`authentication`/`context_exceeded`/... — see
      [Providers](#providers)), surfaced in the TUI and in
      `rexo --output json`'s new `error_kind` field
- [x] `REXO_<PROVIDER>_API_KEY`-namespaced credential env vars, alongside
      legacy unnamespaced ones for v0.2 compatibility
- [x] MCP server *configuration* (`/mcp`) — protocol/execution wiring is
      the next step, see [Known limitations](#known-limitations)
- [x] `@file` references with an ignore-aware searchable picker
- [x] Headless mode (`--output json`, scriptable exit codes)
- [x] Shell mode (v0.6) — bare `!` toggles running raw PowerShell/`sh`
      commands directly instead of going through the model;
      `!<command>` runs one command regardless of mode
- [x] Startup intro animation (v0.7), with `--no-intro`/`REXO_NO_INTRO` to skip it
- [x] First-launch wizard on real arrow-key/masked-input pickers (v0.7),
      not blocking numbered-list prompts — deferred since v0.4.0
- [x] Session persistence, `/resume`, `/branch`, `/rewind` (v0.7.1) —
      conversation-level (see the caveat below); `/fork` too, scoped
      honestly as a saved divergence point, not background execution
- [x] MCP protocol client (v0.8.0) — `/mcp connect` does a real stdio
      JSON-RPC handshake, discovers tools, and registers them through the
      same permission engine as every native tool. SSE/HTTP transport,
      auto-reconnect, and resources/prompts are still missing — see
      [Known limitations](#known-limitations).
- [x] Lifecycle hooks (v0.8.0) — `pre_tool`/`post_tool`/`session_start`/
      `session_end`, real shell commands with Claude-Code-style exit-code
      semantics (0 allows, 2 blocks, anything else warns), config at
      `.rexo/hooks.toml`
- [x] Sequential subagents (v0.8.0) — `/agents create`/`run`: a real,
      persona-configured `Agent` runs a bounded task to completion in its
      own conversation and reports back. Blocking, not backgrounded — see
      [Known limitations](#known-limitations) for what that means.
- [x] Installable plugins (v0.8.0) — `/plugin enable` installs a bundle
      of skills/commands/hooks from `.rexo/plugins/<name>/`, precisely
      reversible by `/plugin disable`
- [x] A local server for editor/IDE integration (v0.8.0) — `/ide start`;
      real and tested REXO-side, but no editor extension exists yet to
      connect to it
- [x] A model-driven task list (v0.8.0) — Ctrl+T, backed by a real
      `manage_tasks` tool the model can add/complete/remove from
- [x] Single-level file-edit undo (v0.8.0) — `/undo`/Ctrl+Shift+_ reverts
      the single most recent `edit_file`/`create_file`/`delete_file` call;
      not the general workspace-checkpoint system below
- [x] Clipboard image paste (v0.8.0) — Alt+V attaches a screenshot to
      your next message for a vision-capable model (OpenAI-compatible
      multipart and native Gemini `inlineData` wire formats)
- [x] Quick side questions (v0.8.0) — `btw <question>` answers with full
      context but doesn't consume it, the same way `/rewind` reverts
      conversation without touching files

**On background/concurrent subagents and worktree isolation** — still the
one real gap left from the original "harness" wishlist. `/agents run`
(v0.8.0) is a genuine, working subagent — its own persona, its own
conversation, its own bounded tool-use loop — but it's sequential: it
blocks the session it was launched from until it finishes, and shares the
workspace with no isolation. Running several of these concurrently without
them colliding on the same files needs git-worktree isolation, which
doesn't exist yet — that's a from-scratch subsystem, not a small addition
on top of what's here now.

Also worth being precise about since it's easy to overstate: `/rewind`
is **conversation-only**. It truncates chat history back to an earlier
point; it does not revert any file edits the agent made after that
point. Reverting file state too would need workspace snapshots
(git-based or otherwise), which isn't built — `/rewind`'s own output
says this every time, not just here.
- [ ] Capability-aware agent behavior (actually gating on
      `Provider::capabilities()`, not just displaying it)
- [x] Native Google Gemini driver, with real SSE streaming as of v0.6 —
      still untested against the live API (see [Providers](#providers))
- [ ] Native (non-OpenAI-compatible) provider adapters: Anthropic, Cohere,
      AWS Bedrock
- [ ] Text/XML tool-call fallback protocol for models without native tool calling
- [ ] Capability-aware provider fallback (don't fail over to a model that
      can't do what the task needs)
- [x] Multiple simultaneous working directories, read-only (`/add-dir`) —
      write tools still scoped to the primary workspace only
- [x] Clipboard integration (`/copy`, real OS clipboard via `arboard`;
      Alt+V clipboard *image* paste as of v0.8.0)
- [x] Arrow-key, type-to-filter pickers for `/connect`/`/model`/`/models`/
      `/provider`, fully in-TUI (masked API-key entry included)
- [x] A skills system (`.rexo/skills/`, keyword-triggered + `/skill`)
- [x] Custom commands (`.rexo/commands/`, `$ARGUMENTS`/`$1..$9`)
- [ ] Real, model-generated conversation summarization (`/compact` is a
      placeholder-substitution heuristic today, not a summary)
- [ ] Live provider-reported token usage / cost display (no fabricated
      pricing — see [Known limitations](#known-limitations))
- [ ] Persisted workspace-trust decisions (currently asks every run)
- [ ] OS-native credential storage (Credential Manager/Keychain/Secret
      Service) as an alternative credential backend
- [ ] Faster/smarter search (currently a plain recursive substring scan)
- [ ] Config-file-level command allow/deny customization
- [ ] Broader automated test coverage (currently thorough unit tests per
      module, plus real PTY-driven end-to-end smoke tests against the
      compiled binary each release; no *automated*, checked-in
      integration harness yet — the PTY tests are written and run by
      hand each session, not part of `cargo test`)
- [x] Packaged releases (GitHub Releases, five platforms — see
      [Downloads](#downloads)); crates.io publishing not done yet

### Harness direction (longer-term, architected for but not built)

The provider/tool/permission abstractions here were designed with room for
these from the start. Most of the original list is real as of v0.8.0 —
hooks, a skill system, and subagents all shipped (see the checklist above
and [Known limitations](#known-limitations) for exactly what each one
does and doesn't cover). What's left:

- Session checkpoints (beyond conversation-only `/rewind` — reverting
  file state too, git-based or otherwise)
- Background sessions (run a task without occupying the foreground session)
- Git-worktree isolation for background/parallel tasks, with an explicit
  review-and-merge step — never an automatic silent merge — which is also
  the prerequisite for running `/agents run` concurrently instead of
  sequentially
- Machine-maintained project memory (`.rexo/memory/`), kept explicitly
  subordinate to human-authored `REXO.md`

## License

MIT — see `LICENSE` (add one before publishing; not included in this
scaffold).

## Contributing

This is meant to be genuinely useful and openly developed, not a closed
demo. Issues and PRs welcome once this is pushed to a public repo. Please
keep the security model in mind: nothing in `tools/`, `security/`, or
`providers/` should quietly widen what the agent is allowed to do without
it being an explicit, reviewable change.
