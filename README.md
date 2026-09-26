<div align="center">

# ⚡ Rexo Code

### An open-source, provider-agnostic AI coding agent for your terminal.

**Bring your own model. Own your workflow. Build with AI on your terms.**

<br>

[![Release](https://img.shields.io/github/v/release/Daksh-Saboo/Rexo-Code?style=flat\&label=release\&color=2ea44f)](https://github.com/Daksh-Saboo/Rexo-Code/releases)
[![Stars](https://img.shields.io/github/stars/Daksh-Saboo/Rexo-Code?style=flat\&color=e3b341)](https://github.com/Daksh-Saboo/Rexo-Code/stargazers)
[![Downloads](https://img.shields.io/github/downloads/Daksh-Saboo/Rexo-Code/total?style=flat\&label=downloads\&color=0969da)](https://github.com/Daksh-Saboo/Rexo-Code/releases)
[![License](https://img.shields.io/github/license/Daksh-Saboo/Rexo-Code?style=flat\&color=8957e5)](https://github.com/Daksh-Saboo/Rexo-Code/blob/main/LICENSE)
[![Rust](https://img.shields.io/badge/Rust-2024-dea584?style=flat\&logo=rust\&logoColor=white)](https://www.rust-lang.org/)

[![Discord](https://img.shields.io/discord/1548239215999852696?style=flat\&logo=discord\&logoColor=white\&label=Discord\&color=5865F2)](https://discord.gg/KvBVgQYZh)
[![Ko-fi](https://ko-fi.com/img/githubbutton_sm.svg)](https://ko-fi.com/E1E71SEJEY)
[![Patreon](https://img.shields.io/badge/Patreon-support-F96854?style=flat\&logo=patreon\&logoColor=white)](https://www.patreon.com/cw/FronoBear)

<br>

Rexo Code reads your project, understands context, searches files,<br>
edits code, runs commands, uses external tools, and works through tasks<br>
directly from your terminal.

<br>

**Windows • Linux • macOS**

</div>

---

## 🎬 Rexo in Action

<p align="center">
  <img src="https://github.com/Daksh-Saboo/Tron-Assets/blob/main/Screenshot%202026-09-26%20084029.jpg?raw=true" width="48%">
  <img src="https://github.com/Daksh-Saboo/Tron-Assets/blob/main/Screenshot%202026-09-26%20084605.jpg?raw=true" width="48%">
</p>

<p align="center">
  <img src="https://github.com/Daksh-Saboo/Tron-Assets/blob/main/Screenshot%202026-09-26%20084644.jpg?raw=true" width="48%">
  <img src="https://github.com/Daksh-Saboo/Tron-Assets/blob/main/Screenshot%202026-09-26%20084703.jpg?raw=true" width="48%">
</p>

<p align="center">
  <img src="https://github.com/Daksh-Saboo/Tron-Assets/blob/main/Screenshot%202026-09-26%20084946.jpg?raw=true" width="70%">
</p>

# 🚀 Why Rexo?

Rexo is built around a simple idea:

> **Your coding agent should work with your models, your tools, and your workflow.**

Rexo doesn't require you to use a single AI provider or a proprietary ecosystem.

Use hosted models, OpenAI-compatible APIs, Gemini, NVIDIA NIM, local model servers, or your own endpoint.

Rexo provides the **agent, tools, terminal interface, permissions, context, sessions, and extensibility layer**.

You choose the model.

---

# ⚡ Install

Download the latest release from the **[Releases](https://github.com/Daksh-Saboo/Rexo-Code/releases)** page.

## Windows

Extract the release archive and run:

```powershell
.\install.ps1
```

Or:

```powershell
.\install.bat
```

The installer installs Rexo into your user environment and adds it to your PATH.

## Linux

```bash
./install.sh
```

## macOS

For Apple Silicon:

```bash
./install.sh
```

> macOS releases are currently unsigned/notarized. macOS may require additional approval before execution.

## Build From Source

```bash
git clone https://github.com/Daksh-Saboo/Rexo-Code.git
cd Rexo-Code
cargo build --release
```

For complete installation instructions, see the **[Wiki](https://github.com/Daksh-Saboo/Rexo-Code/wiki)**.

---

# 🧠 Quick Start

Start Rexo inside a project:

```bash
rexo
```

Give it a task:

```text
Find where authentication is implemented.
```

Or:

```text
Add error handling to the login flow and update the tests.
```

Rexo can:

```text
Understand → Inspect → Plan → Execute → Verify → Iterate
```

It can inspect files, search your project, edit code, run commands, use external tools, and request permission when an operation requires approval.

## Headless Mode

Run Rexo directly from the command line:

```bash
rexo "find where authentication is implemented"
```

Machine-readable output:

```bash
rexo "fix the failing test" --output json
```

This makes Rexo useful for scripts, automation, CI workflows, and other tooling.

---

# ✨ Features

### 🤖 Agentic Coding

* Project-aware agent loop
* Multi-step task execution
* Tool calling
* File search and inspection
* Code editing
* Shell execution
* Verification and iteration
* Subagents
* Task management

### 🧠 Context & Sessions

* Project context
* Persistent sessions
* Resume conversations
* Branch and fork sessions
* Rewind workflows
* Task-aware context management

### 🔌 MCP

Model Context Protocol support allows Rexo to connect external tools and services.

* MCP servers
* MCP tool discovery
* MCP tool execution
* JSON-RPC stdio
* External tool integration

### 🧩 Skills & Plugins

Extend Rexo without changing its core.

* Skills
* Plugins
* Custom commands
* Hooks
* Reusable workflows
* Project-specific automation

### 🪝 Hooks

Use lifecycle hooks to extend Rexo's behavior around agent workflows.

### 🖥️ Terminal Experience

* Full-screen TUI
* Interactive coding workflow
* Shell mode with `!`
* Live shell output
* Shell undo
* Multiline input
* `$EDITOR` integration
* Stash workflow
* Task panel
* Image input
* Streaming responses

### 🔐 Permissions

Rexo is designed around explicit control.

Operations can require approval before the agent executes them, allowing you to control what Rexo can access and modify.

### 🧰 Automation

Rexo can operate interactively or as part of automated workflows.

* Headless mode
* JSON output
* CLI commands
* Scripts
* CI workflows
* External automation

---

# 🔌 Providers

Rexo is **provider-agnostic**.

You are not locked into a single AI provider.

Supported provider types include:

* NVIDIA NIM
* OpenAI-compatible APIs
* Google Gemini
* Local model servers
* Custom OpenAI-compatible endpoints

Any compatible service can potentially be integrated through the provider system.

See the **[Providers Wiki](https://github.com/Daksh-Saboo/Rexo-Code/wiki/Providers)** for configuration and supported providers.

---

# 🧩 Extend Rexo

Rexo is designed to be extended.

## Skills

Create reusable instructions:

```text
.rexo/skills/<name>/SKILL.md
```

Skills can define specialized workflows that Rexo can use when working on a project.

## Custom Commands

Create commands inside:

```text
.rexo/commands/<name>.md
```

Then invoke them with:

```text
/my-command
```

## MCP

Connect external tools and services through MCP:

```text
/mcp
/mcp add
/mcp connect
```

## Plugins

Plugins provide a way to package reusable capabilities, commands, skills, and hooks around Rexo.

---

# 🛠️ How Rexo Works

At a high level, Rexo follows an agent loop:

```text
User Request
     ↓
Project Context
     ↓
Model Reasoning
     ↓
Tool Selection
     ↓
Permission Check
     ↓
Tool Execution
     ↓
Result
     ↓
Model
     ↓
Verification / Next Action
```

The loop continues until the task is completed, requires user input, or reaches an execution boundary.

Rexo's architecture is designed to keep the model, tools, providers, permissions, and terminal experience modular.

Learn more in **[How Rexo Works](https://github.com/Daksh-Saboo/Rexo-Code/wiki/How-Rexo-Works)**.

---

# 📚 Documentation

The complete documentation is available in the **[Rexo Code Wiki](https://github.com/Daksh-Saboo/Rexo-Code/wiki)**.

### 🚀 Getting Started

* [Installation](https://github.com/Daksh-Saboo/Rexo-Code/wiki/Installation)
* [Windows](https://github.com/Daksh-Saboo/Rexo-Code/wiki/Windows)
* [Linux](https://github.com/Daksh-Saboo/Rexo-Code/wiki/Linux)
* [macOS](https://github.com/Daksh-Saboo/Rexo-Code/wiki/macOS)
* [First Run](https://github.com/Daksh-Saboo/Rexo-Code/wiki/First-Run)
* [Quick Start](https://github.com/Daksh-Saboo/Rexo-Code/wiki/Quick-Start)

### ⚙️ Configuration

* [Configuration](https://github.com/Daksh-Saboo/Rexo-Code/wiki/Configuration)
* [Global Configuration](https://github.com/Daksh-Saboo/Rexo-Code/wiki/Global-Configuration)
* [Workspace Configuration](https://github.com/Daksh-Saboo/Rexo-Code/wiki/Workspace-Configuration)
* [Providers](https://github.com/Daksh-Saboo/Rexo-Code/wiki/Providers)
* [Models](https://github.com/Daksh-Saboo/Rexo-Code/wiki/Models)
* [Credentials](https://github.com/Daksh-Saboo/Rexo-Code/wiki/Credentials)
* [Environment Variables](https://github.com/Daksh-Saboo/Rexo-Code/wiki/Environment-Variables)

### 🛠️ Features

* [CLI Reference](https://github.com/Daksh-Saboo/Rexo-Code/wiki/CLI-Reference)
* [Tools](https://github.com/Daksh-Saboo/Rexo-Code/wiki/Tools)
* [Permissions](https://github.com/Daksh-Saboo/Rexo-Code/wiki/Permissions)
* [Sessions](https://github.com/Daksh-Saboo/Rexo-Code/wiki/Sessions)
* [Skills](https://github.com/Daksh-Saboo/Rexo-Code/wiki/Skills)
* [Custom Commands](https://github.com/Daksh-Saboo/Rexo-Code/wiki/Custom-Commands)
* [MCP](https://github.com/Daksh-Saboo/Rexo-Code/wiki/MCP)
* [Hooks](https://github.com/Daksh-Saboo/Rexo-Code/wiki/Hooks)
* [Plugins](https://github.com/Daksh-Saboo/Rexo-Code/wiki/Plugins)
* [Subagents](https://github.com/Daksh-Saboo/Rexo-Code/wiki/Subagents)
* [Tasks](https://github.com/Daksh-Saboo/Rexo-Code/wiki/Tasks)
* [Streaming](https://github.com/Daksh-Saboo/Rexo-Code/wiki/Streaming)
* [Headless Mode](https://github.com/Daksh-Saboo/Rexo-Code/wiki/Headless-Mode)

### 🧠 Architecture

* [How Rexo Works](https://github.com/Daksh-Saboo/Rexo-Code/wiki/How-Rexo-Works)
* [Agent Loop](https://github.com/Daksh-Saboo/Rexo-Code/wiki/Agent-Loop)
* [Project Context](https://github.com/Daksh-Saboo/Rexo-Code/wiki/Project-Context)
* [Tool Calling](https://github.com/Daksh-Saboo/Rexo-Code/wiki/Tool-Calling)
* [Security Model](https://github.com/Daksh-Saboo/Rexo-Code/wiki/Security-Model)

### 👨‍💻 Development

* [Architecture](https://github.com/Daksh-Saboo/Rexo-Code/wiki/Architecture)
* [Building From Source](https://github.com/Daksh-Saboo/Rexo-Code/wiki/Building-From-Source)
* [Testing](https://github.com/Daksh-Saboo/Rexo-Code/wiki/Testing)
* [Contributing](https://github.com/Daksh-Saboo/Rexo-Code/wiki/Contributing)

### 🧰 Help

* [FAQ](https://github.com/Daksh-Saboo/Rexo-Code/wiki/FAQ)
* [Troubleshooting](https://github.com/Daksh-Saboo/Rexo-Code/wiki/Troubleshooting)
* [Security](https://github.com/Daksh-Saboo/Rexo-Code/wiki/Security)

---

# 📦 Releases

Prebuilt releases currently target:

```text
Windows x86_64
Windows ARM64
Linux x86_64
macOS ARM64
```

Every release includes the appropriate installation scripts and release artifacts.

**[→ View Rexo Code Releases](https://github.com/Daksh-Saboo/Rexo-Code/releases)**

---

# 🧪 Project Status

Rexo Code is actively developed.

The project is evolving quickly, and some features may continue to change between releases.

If you find a bug:

1. Check the documentation and FAQ.
2. Search existing issues.
3. Reproduce the problem if possible.
4. Open an issue with your platform, Rexo version, provider/model, logs, and reproduction steps.

---

# 🤝 Contributing

Contributions are welcome.

You can help by:

* Reporting bugs
* Suggesting features
* Improving documentation
* Building integrations
* Improving providers
* Creating skills
* Creating plugins
* Contributing code
* Helping other users

See the **[Contributing Guide](https://github.com/Daksh-Saboo/Rexo-Code/wiki/Contributing)** for more information.

---

# 💚 Support Rexo

Rexo Code is free and open source.

If Rexo is useful to you, you can support development by:

* ⭐ Starring the repository
* 🐛 Reporting bugs
* 💡 Suggesting improvements
* 💬 Joining the community
* ☕ Supporting development

[![Ko-fi](https://ko-fi.com/img/githubbutton_sm.svg)](https://ko-fi.com/E1E71SEJEY)

[![Patreon](https://img.shields.io/badge/Patreon-support-F96854?style=flat\&logo=patreon\&logoColor=white)](https://www.patreon.com/cw/FronoBear)

---

# 💬 Community

Join the Rexo Code Discord for:

* Support
* Bug reports
* Feature discussions
* Development
* Releases
* Community projects
* Showcases

**[→ Join the Rexo Code Discord](https://discord.gg/KvBVgQYZh)**

---

<div align="center">

# ⚡ Rexo Code

### Your terminal. Your models. Your code.

**Open Source • Provider-Agnostic • Rust • Cross-Platform**

<br>

**Build. Extend. Experiment.**

</div>
