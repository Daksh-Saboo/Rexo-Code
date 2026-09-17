<div align="center">

# ⚡ Rexo Code

### An open-source, provider-agnostic AI coding agent for your terminal.

**Bring your own model. Own your workflow. Run AI directly in your terminal.**

[![Version](https://img.shields.io/badge/version-0.7.3-brightgreen?style=flat-square)](../../releases)
[![License](https://img.shields.io/badge/license-MIT-blue.svg?style=flat-square)](LICENSE)
[![Built with Rust](https://img.shields.io/badge/built%20with-Rust-orange?logo=rust&logoColor=white&style=flat-square)](https://www.rust-lang.org/)
[![Platforms](https://img.shields.io/badge/platform-Windows%20%7C%20macOS%20%7C%20Linux-lightgrey?style=flat-square)](#downloads)
[![Discord](https://img.shields.io/badge/Discord-Join%20Us-5865F2?logo=discord&logoColor=white&style=flat-square)](https://discord.gg/KvBVgQYZh)
[![Ko-fi](https://img.shields.io/badge/Ko--fi-Support-FF5E5B?logo=ko-fi&logoColor=white&style=flat-square)](https://ko-fi.com/dakshislegend)
[![Patreon](https://img.shields.io/badge/Patreon-Support-F96854?logo=patreon&logoColor=white&style=flat-square)](https://www.patreon.com/cw/FronoBear)

<br>

Rexo Code reads and searches your project, proposes and applies edits,<br>
runs commands, uses tools, and iterates toward the task you give it —<br>
while keeping a permission and security layer between the model and your machine.

<br>

**Windows • macOS • Linux**

</div>

---

## 📸 Screenshots

<div align="center">

| Workspace Trust | Main CLI |
|---|---|
| <img src="https://github.com/Daksh-Saboo/Tron-Assets/blob/main/Screenshot%202026-09-14%20164004.jpg?raw=true" width="100%"> | <img src="https://github.com/Daksh-Saboo/Tron-Assets/blob/main/Screenshot%202026-09-14%20164037.jpg?raw=true" width="100%"> |

| Sending a Prompt | Agent Output |
|---|---|
| <img src="https://github.com/Daksh-Saboo/Tron-Assets/blob/main/Screenshot%202026-09-14%20164058.jpg?raw=true" width="100%"> | <img src="https://github.com/Daksh-Saboo/Tron-Assets/blob/main/Screenshot%202026-09-14%20164114.jpg?raw=true" width="100%"> |

| Command Suggestions | Help Browser |
|---|---|
| <img src="https://github.com/Daksh-Saboo/Tron-Assets/blob/main/Screenshot%202026-09-14%20164134.jpg?raw=true" width="100%"> | <img src="https://github.com/Daksh-Saboo/Tron-Assets/blob/main/Screenshot%202026-09-14%20164153.jpg?raw=true" width="100%"> |

</div>

---

## 🚀 What is Rexo Code?

**Rexo Code** is an open-source AI coding agent built in **Rust** for developers who want an AI coding workflow directly inside their terminal.

Instead of locking you to one AI provider, Rexo is designed around a **provider-agnostic architecture**.

You can connect:

- OpenAI-compatible APIs
- NVIDIA NIM
- Google Gemini
- OpenRouter
- Groq
- Together
- Fireworks
- Mistral
- DeepSeek
- Cerebras
- xAI
- Local model servers
- Custom OpenAI-compatible endpoints

Rexo can then use the selected model to:

- 🔎 Explore your project
- 📖 Read files
- 🔍 Search source code
- ✏️ Edit files
- 📄 Create files
- 🗑️ Delete files
- 💻 Run terminal commands
- 🌿 Work with Git
- 🧠 Maintain project context
- 🧩 Load skills
- ⚡ Execute custom commands
- 🔄 Iterate through multi-step tasks

All of this happens inside a persistent terminal UI.

---

## ✨ Features

### 🤖 Provider-Agnostic AI

Rexo does not lock you into one model provider.

Use cloud APIs, hosted inference, or local models through a unified provider system.

| Provider type | Support |
|---|---|
| NVIDIA NIM | ✅ |
| OpenAI-compatible APIs | ✅ |
| Google Gemini | ✅ Native driver |
| Local model servers | ✅ |
| Custom endpoints | ✅ |
| OpenRouter | ✅ |
| Groq | ✅ |
| Together | ✅ |
| Fireworks | ✅ |
| Mistral | ✅ |
| DeepSeek | ✅ |
| Cerebras | ✅ |
| xAI | ✅ |

Local servers such as **Ollama, vLLM, LM Studio, and llama.cpp servers** can be used through the local/OpenAI-compatible interfaces.

### 🖥️ Full Terminal UI

Rexo uses a persistent full-screen terminal interface instead of repeatedly printing separate prompts.

The interface includes:

- Live session header
- Provider and model information
- Workspace information
- Permission status
- Streaming responses
- Scrollable transcript
- Interactive input
- Command history
- Tab completion
- Permission dialogs
- Full-screen help browser
- Provider/model pickers
- Animated startup intro

The UI is built around `ratatui` and `crossterm`.

### 🔌 Live Provider & Model Switching

You don't need to restart Rexo to change your configuration.

```text
/model
/models
/provider
/connect
```

Provider and model pickers support:

- Arrow-key navigation
- Type-to-filter search
- Live model discovery
- Manual model entry
- Masked API-key entry
- Session-level configuration
- Persistent configuration

### 🧠 Agent Loop

Rexo follows a model → tool → result → model loop.

```text
                  ┌──────────────────────┐
                  │      User Task       │
                  └──────────┬───────────┘
                             │
                             ▼
                  ┌──────────────────────┐
                  │        Model         │
                  └──────────┬───────────┘
                             │
                       Final answer?
                        ↙         ↘
                      YES          NO
                       │            │
                  ┌─────────┐  ┌─────────────┐
                  │ Answer  │  │  Tool Call  │
                  └─────────┘  └──────┬──────┘
                                      │
                                      ▼
                             ┌──────────────────┐
                             │ Permission Check │
                             └────────┬─────────┘
                                      │
                                      ▼
                             ┌──────────────────┐
                             │   Execute Tool   │
                             └────────┬─────────┘
                                      │
                                      ▼
                                  Tool Result
                                      │
                                      └───────────────► Model
```

The loop is bounded by configurable iteration and tool-call limits so a model cannot continue indefinitely.

### 🛠️ Built-in Tools

| Tool | Purpose | Default permission |
|---|---|---|
| `read_file` | Read a UTF-8 text file | Automatic |
| `list_files` | List a directory | Automatic |
| `search_files` | Recursive literal-substring search | Automatic |
| `edit_file` | Exact find/replace | Ask |
| `create_file` | Create a new file | Ask |
| `delete_file` | Delete a file | Ask |
| `run_command` | Execute shell commands | Classified |
| `git` | Git operations | Classified |

Rexo's `edit_file` tool does not blindly overwrite files.

The model must provide:

- `old_string`
- `new_string`

Rexo verifies that the expected text exists and is unique before applying the change. If the expected content isn't found, the edit is rejected. This helps prevent accidental edits against stale or unexpected file contents.

### 🔒 Security & Permissions

Rexo is designed around the idea that AI should not automatically receive unlimited control over your machine.

#### Workspace boundaries

Filesystem operations are resolved against the active workspace.

Attempts such as:

```text
../../etc/passwd
```

or absolute paths outside the workspace are rejected.

#### Command risk classification

Terminal commands are classified before execution:

- **Automatic**
- **Ask**
- **Blocked**

Dangerous operations can be blocked outright rather than simply asking for confirmation. Command chains are also analyzed so dangerous commands cannot simply hide behind an otherwise safe command.

#### Permission prompts

When Rexo needs approval:

- `y` → Allow once
- `a` → Allow for this session
- `n` → Deny

Denied or blocked operations are returned to the model as tool results, allowing the agent to adapt instead of crashing.

> **Important:** Rexo is not a complete sandbox. It runs with the permissions of your operating-system user. Its security layer is designed as a set of controls and hard stops, not as protection against a fully compromised operating system.

### 🌊 Real Streaming

Rexo supports real streaming responses instead of waiting for the entire response before displaying anything.

**OpenAI-compatible providers** use SSE streaming where supported.

**Google Gemini** has a native driver because Gemini's API format differs from the OpenAI-compatible protocol. Gemini streaming uses:

```text
streamGenerateContent?alt=sse
```

and is parsed incrementally.

### 🧩 Skills

Rexo supports reusable project and global skills.

Create:

```text
.rexo/
└── skills/
    └── commit-style/
        └── SKILL.md
```

Example:

```markdown
---
name: commit-style
description: Project commit message conventions
triggers: commit, changelog
---

Use imperative mood.
Keep the subject under 72 characters.
Reference an issue number when one exists.
```

Skills can be:

- Automatically triggered
- Manually loaded
- Project-specific
- Global

Commands:

```text
/skills
/skill <name>
```

### ⚡ Custom Commands

Create reusable slash commands inside `.rexo/commands/`.

Example:

```markdown
---
description: Review the current code
---

Review the requested code for:
- Bugs
- Security issues
- Performance problems
- Maintainability

Focus area:
$1

Additional context:
$ARGUMENTS
```

Then run:

```text
/review security
```

Rexo supports `$ARGUMENTS`, `$1`, `$2`, `$3`, ..., `$9`.

### 🐚 Shell Mode

Type a bare `!` and press Enter to enter shell mode.

```text
> !
Shell mode on.

> git status
On branch main
nothing to commit, working tree clean

> !
Shell mode off.
```

You can also run a single command directly:

```text
!git status
```

Shell mode intentionally bypasses the AI permission engine because the command is explicitly entered by the user rather than requested by the model.

### 📎 @File References

Rexo supports direct file references inside prompts.

```text
@src/main.rs
@src/
```

You can combine multiple references:

```text
Review @src/auth.rs and @Cargo.toml
```

Rexo resolves referenced files before sending the request to the model. The picker respects `.gitignore`, skips binary files, supports directories, provides searchable suggestions, and limits how much content is injected into context.

### 🌍 Global Configuration

Rexo separates workspace configuration from global configuration.

Resolution order:

```text
CLI flags
   ↓
Session overrides
   ↓
Environment variables
   ↓
Workspace rexo.toml
   ↓
Global configuration
   ↓
Built-in defaults
```

This means your provider and model configuration does not have to disappear when you switch projects.

- **Windows:** `%LOCALAPPDATA%\RexoCode`
- **Linux:** `~/.local/share/rexo-code`
- **macOS:** `~/Library/Application Support/rexo-code`

Credentials are stored separately from workspace files.

### 🔑 Credentials

Rexo supports:

- Environment variables
- Session-only API keys
- Persistent provider credentials
- Named provider profiles

API keys are not intentionally displayed by `/status`, `/config`, or `/doctor`.

Persistent credentials are currently file-based rather than stored in Windows Credential Manager, macOS Keychain, or another OS-native encrypted vault.

---

## 📥 Downloads

Rexo provides prebuilt binaries so users don't need Rust installed just to run the application.

### Supported Platforms

| Platform | Target | Archive |
|---|---|---|
| 🪟 Windows x64 | `x86_64-pc-windows-msvc` | `rexo-code-windows-x86_64.zip` |
| 🪟 Windows ARM64 | `aarch64-pc-windows-msvc` | `rexo-code-windows-aarch64.zip` |
| 🍎 macOS Intel | `x86_64-apple-darwin` | `rexo-code-macos-x86_64.tar.gz` |
| 🍎 macOS Apple Silicon | `aarch64-apple-darwin` | `rexo-code-macos-aarch64.tar.gz` |
| 🐧 Linux x64 | `x86_64-unknown-linux-gnu` | `rexo-code-linux-x86_64.tar.gz` |

➡️ **[Download the latest release](../../releases)**

Each release archive contains the Rexo binary and the appropriate installation files. A `SHA256SUMS.txt` checksum file is also published alongside releases.

### Release verification status

Rexo builds natively for the five target platforms.

The Linux x64 target is the only one verified in the project's own development environment. The other targets are defined for native GitHub runners but should be treated as unverified until a release has actually run through those CI paths.

Windows ARM64 is cross-linked rather than executed on a hosted Windows ARM64 runner.

macOS builds are currently unsigned and unnotarized. Gatekeeper may require you to approve the binary manually.

---

## 📦 Installation

### Windows

Download and extract the Windows archive.

Run:

```powershell
.\install.ps1
```

Or:

```cmd
.\install.bat
```

For an already-built release binary:

```powershell
.\install.ps1 -SkipBuild
```

Open a new terminal window and run:

```powershell
rexo
```

### Linux / macOS

Extract the release archive and run:

```bash
./install.sh
```

For an existing downloaded binary:

```bash
./install.sh --skip-build
```

Open a new terminal and run:

```bash
rexo
```

### Build from source

If you want to develop Rexo or build it yourself, install the Rust toolchain first.

```bash
git clone <your-fork-url> rexo-code
cd rexo-code
```

Create your environment file:

**Windows:**

```powershell
copy .env.example .env
```

**Linux / macOS:**

```bash
cp .env.example .env
```

Configure your provider credentials, for example:

```env
NVIDIA_API_KEY=nvapi-...
```

Then build:

```bash
cargo build --release
```

Run the compiled binary:

**Windows:**

```powershell
target\release\rexo.exe
```

**Linux / macOS:**

```bash
./target/release/rexo
```

---

## ⚡ Quick Start

After installation:

```bash
rexo
```

You can then enter tasks such as:

```text
Find where authentication is implemented.
```

```text
Find all API endpoints and explain how authentication works.
```

```text
Add error handling to the login flow and update the tests.
```

Rexo will inspect the project, use its available tools, request permission where required, and return the result.

---

## 🖥️ Headless Mode

Rexo can also operate without the interactive TUI.

```bash
rexo "find where authentication is implemented"
```

For machine-readable output:

```bash
rexo "fix the failing test" --output json
```

Available output modes:

```text
text
json
jsonl
```

---

## 🧰 CLI

Basic syntax:

```bash
rexo [OPTIONS] [PROMPT]
```

| Option | Description |
|---|---|
| `-y`, `--yes` | Automatically approve permissions |
| `--non-interactive` | Never prompt for permissions |
| `-C`, `--workspace <DIR>` | Use a different workspace |
| `--model <MODEL>` | Override the model |
| `--provider <KIND>` | Override the provider |
| `--base-url <URL>` | Override the API base URL |
| `--output <FORMAT>` | `text`, `json`, or `jsonl` |
| `--fast` | Reduce reasoning/token budget |
| `--no-intro` | Skip the animated startup intro |
| `--list-tools` | Display available tools |

---

## ⌨️ Interactive Commands

### Core

```text
/help
/status
/config
/doctor
/exit
/quit
```

### Models & Providers

```text
/model
/models
/provider
/providers
/connect
/provider add
/base-url
/api
```

### Workspace

```text
/workspace
/cd
/init
/memory
/add-dir
```

### Permissions

```text
/permissions
/permissions edit always
/permissions terminal ask
```

### Skills & Commands

```text
/skills
/skill <name>
/commands
```

### Context & Output

```text
/context
/compact
/copy
/export
```

### Tools & Configuration

```text
/tools
/mcp
/reset
```

### Session Management

```text
/resume
/branch <name>
/fork <name>
/rewind
```

---

## ⌨️ Keyboard Shortcuts

| Shortcut | Action |
|---|---|
| `Ctrl+C` | Cancel current operation |
| `Ctrl+C` twice | Exit when input is empty |
| `Ctrl+Y` | Toggle terminal selection mode |
| `Alt+M` | Open model picker |
| `Alt+P` | Open provider picker |
| `?` | Open shortcut list |
| `↑` / `↓` | Navigate history/pickers |
| `Tab` | Complete commands/paths |
| `Esc` | Cancel current interaction |

---

## 🧠 Session Persistence

Rexo can save and restore conversations.

```text
/resume
/resume list
/resume <name>
/branch <name>
/fork <name>
/rewind
```

`/branch` saves the current conversation as a named session without switching away from it.

`/fork` saves a named divergence point and switches the current session to continue from that fork.

`/rewind` truncates conversation history back to an earlier checkpoint.

> **Note:** `/rewind` is conversation-only. It does not revert file edits. Workspace snapshots are not currently implemented.

---

## 🧪 Development

Run basic checks:

```bash
cargo check
```

Run tests:

```bash
cargo test
```

List tools:

```bash
cargo run -- --list-tools
```

Keep `cargo check` and `cargo test` green after changes.

---

## 🏗️ Architecture

Rexo is organized into independent layers:

```text
src/
├── main.rs
├── cli/
│   ├── parser.rs
│   ├── commands.rs
│   ├── completion.rs
│   ├── trust_dialog.rs
│   └── tui/
│
├── agent/
│   ├── mod.rs
│   ├── context/
│   ├── planner/
│   └── prompts/
│
├── config/
│
├── providers/
│   ├── nvidia/
│   ├── openai_compatible/
│   ├── local/
│   └── gemini/
│
├── security/
│   ├── policies/
│   └── permissions/
│
├── tools/
│   ├── filesystem/
│   ├── search/
│   ├── patch/
│   ├── terminal/
│   └── git/
│
└── utils/
```

Runtime component flow:

```text
CLI ──► Agent ──┬──► Provider
                ├──► Tool Registry
                ├──► Permission Manager
                ├──► Security Policies
                └──► Project Context
```

---

## 🚧 Current Status — v0.7.3

Rexo Code v0.7.3 is a working early-stage AI coding agent with:

- ✅ Agent loop with real tool calling
- ✅ Streaming responses
- ✅ Native Gemini SSE streaming
- ✅ OpenAI-compatible provider support
- ✅ NVIDIA NIM support
- ✅ Local model support
- ✅ Persistent full-screen terminal UI
- ✅ Animated startup intro
- ✅ Interactive provider/model pickers
- ✅ First-launch setup wizard
- ✅ Workspace trust dialog
- ✅ Permission engine
- ✅ Command-risk classification
- ✅ Workspace filesystem boundaries
- ✅ Git integration
- ✅ Skills
- ✅ Custom commands
- ✅ `@file` references
- ✅ Shell mode
- ✅ Headless JSON output
- ✅ Global configuration
- ✅ Persistent provider credentials
- ✅ Named provider profiles
- ✅ Session persistence
- ✅ `/resume`
- ✅ `/branch`
- ✅ `/fork`
- ✅ `/rewind`
- ✅ Packaged releases for five target platforms

Rexo has not yet been run against a large real-world codebase at scale. Treat it as a working foundation rather than a finished product.

---

## ⚠️ Known Limitations

Rexo aims to be explicit about what is implemented versus what is still being built.

- **MCP is configuration-only.** `/mcp` can save and manage server definitions, but the MCP protocol client and tool execution are not implemented yet.
- **Credentials are file-based.** They are not currently stored in Windows Credential Manager, macOS Keychain, or another OS-native encrypted vault.
- **Model capabilities are mostly advisory.** Capability information can be displayed, but the agent does not yet gate behavior based on every capability.
- **Native non-OpenAI-compatible adapters are incomplete.** Anthropic, Cohere, and AWS Bedrock native adapters are not implemented yet. Google Gemini has a native adapter.
- **No live pricing/token-cost database.** Rexo avoids fabricating provider pricing or context-window data.
- **`--output jsonl` is currently equivalent to `json` for a single-shot run.**
- **Exit code 3 is reserved** for permission/security rejection but is not yet distinguished from a generic task failure.
- **`/compact` is heuristic-based**, not model-generated summarization.
- **`search_files` is a recursive substring scan**, not indexed or fuzzy search.
- **Workspace trust is not persistent** across runs.
- **`/add-dir` is read-only.** Write tools remain restricted to the primary workspace.
- **`/copy` requires a usable system clipboard/display environment.**
- **Skills and custom commands are keyword/template-based**, rather than semantic.
- **Windows executable icon appearance is not independently verified by CI.**
- **Hooks, subagents, worktree isolation, and background sessions are not implemented yet.**

---

## 🗺️ Roadmap

### Agent

- [ ] Capability-aware agent behavior
- [ ] Smarter planning
- [ ] Subagents
- [ ] Background sessions
- [ ] Model-generated conversation summarization

### Providers

- [ ] Native Anthropic adapter
- [ ] Native Cohere adapter
- [ ] Native AWS Bedrock adapter
- [ ] Capability-aware provider fallback
- [ ] Tool-call fallback for models without native tool calling

### Security

- [ ] Config-level command allow/deny customization
- [ ] OS-native credential storage
- [ ] Persistent workspace trust
- [ ] Stronger workspace isolation

### Developer Experience

- [ ] Faster indexed search
- [ ] IDE/editor integration
- [ ] Installable plugin system
- [ ] Live provider token/cost reporting

### Infrastructure

- [x] CI
- [x] Packaged GitHub releases
- [x] SHA-256 release checksums
- [ ] Broader end-to-end testing
- [ ] `crates.io` publishing

### MCP

- [ ] MCP protocol client
- [ ] Server connection
- [ ] Capability discovery
- [ ] Resource/prompt discovery
- [ ] Tool execution through the existing permission engine

---

## 🤝 Contributing

Rexo Code is open source and contributions are welcome.

Whether you're submitting bug reports, feature requests, documentation improvements, or pull requests, please keep the security model in mind.

Changes to security-sensitive areas such as:

```text
tools/
security/
providers/
permissions/
```

should maintain explicit, reviewable behavior and should not silently widen what the agent is allowed to do.

---

## 💚 Support Rexo Code

Rexo Code is **free and open source**, built independently with the goal of making powerful AI developer tooling accessible to everyone.

If Rexo Code has been useful to you, your support helps keep development, maintenance, testing, and future releases moving forward.

- **[Support on Ko-fi](https://ko-fi.com/dakshislegend)**
- **[Support on Patreon](https://www.patreon.com/cw/FronoBear)**

---

## 💬 Community

Join the Rexo Code community:

**[Discord — Rexo Code Community](https://discord.gg/KvBVgQYZh)**

Use the community for:

- Releases
- Support
- Bug reports
- Feature discussions
- Development updates
- Community projects

---

## 🔐 Security

If you discover a security issue, please avoid posting sensitive exploit details publicly until the issue can be reviewed.

Never commit API keys or credentials.

Rexo configuration files such as `rexo.toml` should never contain real secrets. Use the credential system or environment variables instead.

If a credential is ever exposed, rotate it immediately.

---

## 📄 License

Rexo Code is released under the **MIT License**.

See [`LICENSE`](LICENSE) for details.

---

<div align="center">

### ⚡ Rexo Code

**Your terminal. Your models. Your code.**

Open source • Provider-agnostic • Rust • Cross-platform

<br>

⭐ GitHub • 📦 Releases • 💬 Discord • ☕ Ko-fi • ❤️ Patreon

</div>
