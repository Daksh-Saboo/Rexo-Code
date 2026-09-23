<div align="center">

# ⚡ Rexo Code

### An open-source, provider-agnostic AI coding agent for your terminal.

**Bring your own model. Own your workflow. Run AI directly in your terminal.**

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

Rexo Code reads your project, searches files, edits code, runs tools,<br>
and works through tasks directly from your terminal.

<br>

**Windows • Linux • macOS**

</div>

---

## 🎬 See Rexo in Action

> Videos, GIFs, and screenshots coming soon.

<!-- Add your main demo GIF/video here -->

<p align="center">
  <img src="docs/images/rexo-demo.gif" alt="Rexo Code demo" width="900">
</p>

---

## 🚀 Install

Download the latest release from the **[Releases](https://github.com/Daksh-Saboo/Rexo-Code/releases)** page.

### Windows

Download the Windows archive and run:

```powershell
.\install.ps1
```

Or use:

```powershell
.\install.bat
```

The installer places Rexo in your user installation directory and adds it to your PATH.

### Linux

Extract the release archive and run:

```bash
./install.sh
```

### macOS

For **Apple Silicon (ARM64)**:

```bash
./install.sh
```

> macOS releases are currently unsigned/notarized. macOS may require you to approve the application before running it.

### From Source

If you want to build Rexo yourself:

```bash
git clone https://github.com/Daksh-Saboo/Rexo-Code.git
cd Rexo-Code
cargo build --release
```

See the Wiki for the complete installation and build instructions.

---

## ⚡ Quick Start

Start Rexo:

```bash
rexo
```

Then give it a task:

```text
Find where authentication is implemented.
```

Or:

```text
Add error handling to the login flow and update the tests.
```

Rexo can inspect your project, use its tools, make changes, run commands, and ask for permission when an operation requires approval.

### Headless Mode

Run a task without the interactive TUI:

```bash
rexo "find where authentication is implemented"
```

Machine-readable output:

```bash
rexo "fix the failing test" --output json
```

---

## ✨ Features

Rexo Code includes:

* 🤖 **Provider-agnostic AI** — use different hosted or local models
* 🧠 **Agentic coding** — inspect, edit, run, verify, and iterate
* 🔐 **Permission & security system** — control what the agent can do
* 🖥️ **Full-screen terminal UI** — built for interactive coding
* 🔌 **MCP** — connect external tools through MCP servers
* 🪝 **Hooks** — extend agent lifecycle events
* 🧩 **Plugins** — bundle skills, commands, and hooks
* 📚 **Skills** — reusable project and global workflows
* ⚡ **Custom commands** — create your own `/commands`
* 🌿 **Session persistence** — resume, branch, fork, and rewind conversations
* 🤖 **Subagents** — run bounded agents with their own context
* 📋 **Tasks** — model-driven task management
* 🖼️ **Image input** — attach clipboard images with `Alt+V`
* 💻 **Shell mode** — run commands directly with `!`
* 📡 **Streaming** — live model responses
* 🧰 **Headless mode** — JSON output for automation

---

## 🔌 Providers

Rexo is designed to work with different AI providers instead of locking you into one service.

Supported provider types include:

* NVIDIA NIM
* OpenAI-compatible APIs
* Google Gemini
* Local model servers
* Custom OpenAI-compatible endpoints

Popular compatible providers and services can be configured through Rexo's provider system.

For the complete provider list and configuration:

**→ [Wiki: Providers](https://github.com/Daksh-Saboo/Rexo-Code/wiki/Providers)**

---

## 🧩 Extend Rexo

### Skills

Create reusable instructions inside:

```text
.rexo/skills/<name>/SKILL.md
```

### Custom Commands

Create your own commands inside:

```text
.rexo/commands/<name>.md
```

Then use them directly:

```text
/my-command
```

### MCP

Connect external tools and services through the Model Context Protocol:

```text
/mcp
/mcp add
/mcp connect
```

---

## 📚 Documentation

The **Rexo Code Wiki** contains the complete documentation.

### 🚀 Getting Started

* [Installation](https://github.com/Daksh-Saboo/Rexo-Code/wiki/Installation)
* [Windows](https://github.com/Daksh-Saboo/Rexo-Code/wiki/Windows)
* [Linux](https://github.com/Daksh-Saboo/Rexo-Code/wiki/Linux)
* [macOS](https://github.com/Daksh-Saboo/Rexo-Code/wiki/macOS)
* [First Run](https://github.com/Daksh-Saboo/Rexo-Code/wiki/First-Run)
* [Quick Start](https://github.com/Daksh-Saboo/Rexo-Code/wiki/Quick-Start)

### ⚙️ Configuration

* [Configuration](https://github.com/Daksh-Saboo/Rexo-Code/wiki/Configuration)
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

### 👨‍💻 Development

* [Architecture](https://github.com/Daksh-Saboo/Rexo-Code/wiki/Architecture)
* [How Rexo Works](https://github.com/Daksh-Saboo/Rexo-Code/wiki/How-Rexo-Works)
* [Building From Source](https://github.com/Daksh-Saboo/Rexo-Code/wiki/Building-From-Source)
* [Testing](https://github.com/Daksh-Saboo/Rexo-Code/wiki/Testing)
* [Contributing](https://github.com/Daksh-Saboo/Rexo-Code/wiki/Contributing)

### 🧰 Help

* [FAQ](https://github.com/Daksh-Saboo/Rexo-Code/wiki/FAQ)
* [Troubleshooting](https://github.com/Daksh-Saboo/Rexo-Code/wiki/Troubleshooting)
* [Security](https://github.com/Daksh-Saboo/Rexo-Code/wiki/Security)

---

## 📦 Releases

Download the latest version:

**[→ View Rexo Code Releases](https://github.com/Daksh-Saboo/Rexo-Code/releases)**

Prebuilt releases currently target:

```text
Windows x64
Windows ARM64
Linux x64
macOS ARM64
```

Every release includes the appropriate installer/uninstaller scripts.

---

## 💚 Support

Rexo Code is free and open source.

If you find it useful, you can support development through:

[![Ko-fi](https://ko-fi.com/img/githubbutton_sm.svg)](https://ko-fi.com/E1E71SEJEY)

[![Patreon](https://img.shields.io/badge/Patreon-support-F96854?style=flat\&logo=patreon\&logoColor=white)](https://www.patreon.com/cw/FronoBear)

---

## 💬 Community

Join the Rexo Code Discord for:

* Support
* Bug reports
* Feature discussions
* Development
* Releases
* Community projects

**[→ Join the Rexo Code Discord](https://discord.gg/KvBVgQYZh)**

---

<div align="center">

### ⚡ Rexo Code

**Your terminal. Your models. Your code.**

Open Source • Provider-Agnostic • Rust • Cross-Platform

</div>
