# opencode-rs

**An AI coding agent for your terminal, written from scratch in Rust.**

opencode-rs is a Rust rewrite of [OpenCode](https://github.com/sst/opencode). You talk to it in plain language, and it reads your code, searches the project, edits files and runs commands to get the task done — while asking your permission for anything you have not allowed in advance. It ships as a single self-contained program with no runtime to install.

> **Status: early release (v0.1).** All features below are implemented and covered by automated tests, but the project is young and has seen little real-world use yet. Please [open an issue](https://github.com/exgg1453/opencode-rs/issues) if something breaks.

## Contents

- [Features](#features)
- [Installation](#installation)
- [Quick start](#quick-start)
- [Using the terminal UI](#using-the-terminal-ui)
- [Headless mode: `opencode run`](#headless-mode-opencode-run)
- [Providers and models](#providers-and-models)
- [Configuration](#configuration)
- [Permissions](#permissions)
- [Model thinking](#model-thinking)
- [Themes](#themes)
- [MCP servers](#mcp-servers)
- [Plugins](#plugins)
- [Language servers (LSP)](#language-servers-lsp)
- [HTTP server and session sharing](#http-server-and-session-sharing)
- [Where data is stored](#where-data-is-stored)
- [Project layout](#project-layout)
- [Building from source](#building-from-source)
- [Known limitations](#known-limitations)
- [License](#license)

## Features

- **Agent loop with built-in tools:** `read`, `write`, `edit`, `bash`, `grep`, `glob` and `list`. The model calls them, sees the results and keeps going until the task is done.
- **15 LLM providers:** Anthropic, OpenAI, Google Gemini, Azure OpenAI, OpenRouter, Groq, DeepSeek, Mistral, xAI, Together, Fireworks, Cerebras and Perplexity, plus local models through Ollama and LM Studio. Any other OpenAI-compatible server works too.
- **Interactive terminal UI** with markdown rendering, collapsible tool output, a session list and 34 color themes.
- **Headless mode** (`opencode run`) for scripts and CI, with plain-text or JSON-lines output.
- **Permission system:** allow, ask or deny per tool, with wildcard rules for shell commands.
- **Model thinking:** see the model's reasoning live, at a level you choose.
- **Resilient streaming:** rate limits, server errors and dropped connections are retried automatically.
- **MCP client** (stdio and HTTP) to plug in external tool servers.
- **Plugins** in any language, talking JSON-RPC over stdio.
- **LSP diagnostics:** after editing a file the agent sees compiler errors from your language server and can fix them.
- **HTTP API** with a live event stream, and **session sharing** as a read-only web page on your own server.
- **Saved sessions** you can resume, export to JSON or Markdown, and import again.

## Installation

### One-line install

Each script detects your CPU, downloads the right binary, verifies its checksum and installs it.

**Linux & macOS:**

```sh
curl -fsSL https://raw.githubusercontent.com/exgg1453/opencode-rs/main/install.sh | sh
```

**Windows (PowerShell):**

```powershell
irm https://raw.githubusercontent.com/exgg1453/opencode-rs/main/install.ps1 | iex
```

The Unix script installs to `/usr/local/bin` when it can, otherwise `~/.local/bin`; the Windows script installs to `%LOCALAPPDATA%\opencode\bin` and adds it to your PATH. Set `OCRS_INSTALL_DIR` to choose the folder, or `OCRS_VERSION=v0.1.2` to pin a version. Prefer not to pipe a script into a shell unseen? Read it first ([install.sh](https://github.com/exgg1453/opencode-rs/blob/main/install.sh), [install.ps1](https://github.com/exgg1453/opencode-rs/blob/main/install.ps1)), or use the manual download below.

### Download a ready-made program

Go to the [latest release](https://github.com/exgg1453/opencode-rs/releases/latest) and download the file for your computer. Nothing else needs to be installed.

| Your computer | File to download |
|---|---|
| Windows, 64-bit (almost every Windows 10/11 PC) | `opencode-windows-64bit.zip` |
| Windows, 32-bit (older PCs) | `opencode-windows-32bit.zip` |
| Mac with Apple Silicon (M1, M2, M3, M4…) | `opencode-macos-apple-silicon.tar.gz` |
| Mac with an Intel processor | `opencode-macos-intel.tar.gz` |
| Linux, 64-bit Intel/AMD (most desktops, laptops and servers) | `opencode-linux-64bit.tar.gz` |
| Linux on ARM (Raspberry Pi 4/5, ARM servers, Android via Termux) | `opencode-linux-arm64.tar.gz` |

**Not sure which one?**
- **Windows:** *Settings → System → About → System type* shows "64-bit" or "32-bit".
- **Mac:**  menu → *About This Mac*. "Chip: Apple M…" means Apple Silicon; "Processor: Intel…" means Intel.
- **Linux:** run `uname -m`. `x86_64` → `linux-64bit`, `aarch64` → `linux-arm64`.

**Linux and macOS, from the terminal:**

```sh
# Pick the line for your computer:
FILE=opencode-linux-64bit          # Linux, Intel/AMD
FILE=opencode-linux-arm64          # Linux, ARM
FILE=opencode-macos-apple-silicon  # Mac, M1/M2/M3/M4
FILE=opencode-macos-intel          # Mac, Intel

curl -fsSL "https://github.com/exgg1453/opencode-rs/releases/latest/download/$FILE.tar.gz" | tar -xz
sudo install "$FILE/opencode" /usr/local/bin/   # or: mv "$FILE/opencode" ~/.local/bin/
opencode --version
```

On macOS, if Gatekeeper blocks the program the first time, run `xattr -d com.apple.quarantine /usr/local/bin/opencode`.

**Windows:** unzip the file, then run `opencode.exe` from *Command Prompt* or *PowerShell* in the unzipped folder. To use it from anywhere, move `opencode.exe` into a folder on your `PATH`. The `bash` tool needs a bash on `PATH`, for example from [Git for Windows](https://gitforwindows.org/) or WSL.

Every archive comes with a `.sha256` file so you can verify the download.

### With Cargo

If you already have Rust installed:

```sh
cargo install --git https://github.com/exgg1453/opencode-rs oc-cli
```

## Quick start

```sh
# 1. Save an API key once (input is hidden). Or set ANTHROPIC_API_KEY instead.
opencode auth login anthropic

# 2. Start the terminal UI inside your project
cd my-project
opencode

# ...or ask a one-off question without the UI
opencode run "explain what this project does"
```

The default model is `anthropic/claude-sonnet-5-5`. Use `-m provider/model` to pick another, for example `opencode -m openai/gpt-5` or `opencode -m ollama/llama3.3`.

## Using the terminal UI

Run `opencode` (or `opencode tui`). The screen shows the conversation on top, the prompt box below it and a status bar with the model, folder, session title and thinking level.

| Key | Action |
|---|---|
| `Enter` | Send the message |
| `Shift+Enter`, `Alt+Enter`, `Ctrl+J` | New line in the prompt |
| `Esc` | Interrupt the running answer |
| `PgUp` / `PgDn`, mouse wheel | Scroll the conversation |
| `Ctrl+O` | Expand or collapse tool output |
| `Ctrl+T` | Expand or collapse the model's thinking |
| `Ctrl+N` | New session |
| `Ctrl+L` | Open the session list |
| `Ctrl+U`, `Ctrl+W`, `Ctrl+A`, `Ctrl+E` | Delete line / delete word / line start / line end |
| `Ctrl+C` | Quit |

| Command | Action |
|---|---|
| `/new` | Start a new session |
| `/sessions` | Switch to a saved session |
| `/theme [name]` | Pick a color theme (live preview), or set one directly |
| `/thinking [off\|low\|medium\|high]` | Show or change the thinking level |
| `/share`, `/unshare` | Publish this session as a web page, or remove it |
| `/help` | Show keys and commands |
| `/exit` | Quit |

When the agent wants to do something that needs approval, a dialog shows the command or a diff preview of the file change. Press `y` (or `Enter`) to allow once, `a` to always allow it in this session, or `n` (or `Esc`) to deny.

## Headless mode: `opencode run`

```sh
opencode run "add a unit test for parse_args"
git diff | opencode run "review this diff"        # message from stdin
opencode run --continue "now update the README"   # continue the last session
opencode run --session <id> "..."                 # continue a specific session
opencode run --format json "..." > events.jsonl   # one JSON event per line
```

The answer is printed to standard output; tool activity and thinking go to standard error. The exit code is non-zero when the run fails.

| Option | Meaning |
|---|---|
| `-c`, `--continue` | Continue the most recent session |
| `-s`, `--session <id>` | Continue the given session |
| `--format default\|json` | Plain text, or JSON lines for scripts |
| `-y`, `--yes` | Approve every permission prompt automatically (deny rules still apply) |
| `--thinking <level>` | Thinking level for this run |
| `--hide-thinking` | Do not print the model's reasoning |

Without a terminal (for example in CI), anything that would ask for permission is denied unless you pass `--yes`.

### Other commands

| Command | What it does |
|---|---|
| `opencode models` | List all providers and whether each one has a key |
| `opencode auth login [provider]` | Save an API key (hidden input) |
| `opencode auth logout <provider>` | Remove a saved key |
| `opencode auth list` | Show which providers have saved keys or environment variables (keys are never printed) |
| `opencode sessions` | List saved sessions |
| `opencode export <id> [--format json\|md]` | Print a session as JSON or Markdown |
| `opencode import <file>` | Import a session exported as JSON |
| `opencode share <id>` / `opencode unshare <id>` | Publish or remove a session web page |
| `opencode serve` | Start the HTTP API (see below) |

Global options: `-m/--model`, `--cwd <dir>` (work in another folder) and `--thinking <level>`.

## Providers and models

A model is written as `provider/model`. Keys are read from `opencode auth login` first, then from the environment variable.

| Provider | Id | Key |
|---|---|---|
| Anthropic | `anthropic` | `ANTHROPIC_API_KEY` |
| OpenAI | `openai` | `OPENAI_API_KEY` |
| Google Gemini | `google` | `GEMINI_API_KEY` or `GOOGLE_GENERATIVE_AI_API_KEY` |
| Azure OpenAI | `azure` | `AZURE_API_KEY` + `AZURE_RESOURCE_NAME` (or a `baseURL` option) |
| OpenRouter | `openrouter` | `OPENROUTER_API_KEY` |
| Groq | `groq` | `GROQ_API_KEY` |
| DeepSeek | `deepseek` | `DEEPSEEK_API_KEY` |
| Mistral | `mistral` | `MISTRAL_API_KEY` |
| xAI | `xai` | `XAI_API_KEY` |
| Together AI | `together` | `TOGETHER_API_KEY` |
| Fireworks AI | `fireworks` | `FIREWORKS_API_KEY` |
| Cerebras | `cerebras` | `CEREBRAS_API_KEY` |
| Perplexity | `perplexity` | `PERPLEXITY_API_KEY` |
| Ollama (local) | `ollama` | none — uses `http://localhost:11434/v1` |
| LM Studio (local) | `lmstudio` | none — uses `http://localhost:1234/v1` |

**Any other OpenAI-compatible server** works by giving it a name and a `baseURL`; its key comes from `<NAME>_API_KEY`:

```json
{
  "model": "myserver/qwen3-coder",
  "provider": {
    "myserver": { "options": { "baseURL": "https://llm.example.com/v1" } }
  }
}
```

`ANTHROPIC_BASE_URL` and `OPENAI_BASE_URL` override the endpoints of those two providers. Requests that hit rate limits (429) or server errors (5xx) are retried up to three times, and so is a response stream that drops halfway; the partial answer is discarded so nothing is duplicated.

## Configuration

Settings are read from `~/.config/opencode/opencode.json` (global) and then `opencode.json` in the project folder, which overrides the global file. Both are optional and must be plain JSON (no comments).

```json
{
  "model": "anthropic/claude-sonnet-5-5",
  "thinking": "medium",
  "theme": "tokyonight",
  "permission": {
    "edit": "ask",
    "bash": { "*": "ask", "git status": "allow", "git diff *": "allow", "rm *": "deny" }
  },
  "provider": {
    "azure": { "options": { "resourceName": "my-resource", "apiVersion": "2024-10-21" } }
  },
  "mcp": {
    "filesystem": { "type": "local", "command": ["npx", "-y", "@modelcontextprotocol/server-filesystem", "."] }
  },
  "plugin": {
    "guard": { "command": ["./tools/guard-plugin"] }
  },
  "lsp": {
    "rust-analyzer": { "disabled": false }
  },
  "share_url": "https://share.example.com"
}
```

| Key | Meaning |
|---|---|
| `model` | Default model as `provider/model` |
| `thinking` | `off`, `low`, `medium` or `high` |
| `theme` | Theme name for the terminal UI |
| `permission` | Permission rules (see [Permissions](#permissions)) |
| `provider` | Per-provider options, e.g. `baseURL` or Azure settings |
| `mcp` | MCP servers to connect to |
| `plugin` | Plugins to start |
| `lsp` | Language server overrides; `"lsp": false` turns LSP off |
| `share_url` | Server used by `share` (default `http://127.0.0.1:4096`) |

## Trusted folders

A project's own `opencode.json` and `.opencode/plugins/` can define programs (MCP servers, language servers, plugins) that would otherwise start the moment you open the folder. Because you might open a repository you just cloned, opencode-rs **ignores those project-defined commands until you trust the folder** — global settings under `~/.config/opencode` always run, since they are your own. When a folder defines such commands and isn't trusted, you'll see a warning; to allow them:

```sh
opencode trust            # trust the current folder
opencode trust --list     # show trusted folders
opencode trust --remove   # revoke trust for the current folder
```

Only trust folders whose code you trust. Everything else — the model, the built-in tools, your own global plugins — works untrusted.

## Permissions

Before every tool call the agent checks your rules. Each rule is `allow`, `ask` or `deny`; when several rules match, **the last one wins**.

```json
"permission": {
  "edit": "ask",
  "read": "allow",
  "bash": {
    "*": "ask",
    "git *": "allow",
    "npm test": "allow",
    "rm -rf *": "deny"
  }
}
```

- A plain string such as `"edit": "ask"` applies to everything that tool does.
- `edit` covers the `write` and `edit` tools; the pattern is matched against the file path.
- `bash` patterns are matched against the command, with `*` as a wildcard (`"git *"` also matches plain `git`). Chained commands such as `a && b | c` are split and every part is checked; one `deny` blocks the whole command.
- Choosing **always** in a prompt remembers a pattern like `git commit *` for the rest of the session. Deny rules always win.

**Defaults:** everything is allowed except these, which ask first:
- Touching files outside the project folder
- Reading `.env` files (`.env.example` is allowed)
- Calling the same tool with the same input three times in a row, which usually means the agent is stuck in a loop

## Model thinking

Many models can reason before they answer. Set the level with `"thinking"` in the config, `--thinking` on the command line, or `/thinking` in the UI:

| Level | Effect |
|---|---|
| `off` | No extra reasoning requested |
| `low`, `medium`, `high` | More reasoning, slower and more expensive answers |

The reasoning is shown live as a dimmed **Thinking** block (`Ctrl+T` expands it) or on standard error in `opencode run`. It works with Anthropic (adaptive thinking on current Claude models), OpenAI and Azure (`reasoning_effort`), Google Gemini, and with any OpenAI-compatible provider that streams reasoning (DeepSeek, OpenRouter, Groq and others).

## Themes

34 themes are built in: `opencode` (default), `system` (uses your terminal's colors), `aura`, `ayu`, `carbonfox`, `catppuccin`, `catppuccin-frappe`, `catppuccin-macchiato`, `cobalt2`, `cursor`, `dracula`, `everforest`, `flexoki`, `github`, `gruvbox`, `kanagawa`, `lucent-orng`, `material`, `matrix`, `mercury`, `monokai`, `nightowl`, `nord`, `one-dark`, `orng`, `osaka-jade`, `palenight`, `rosepine`, `solarized`, `synthwave84`, `tokyonight`, `vercel`, `vesper` and `zenburn`.

Use `/theme` in the UI to browse them with live preview; your choice is remembered. You can add your own themes as JSON files in `~/.config/opencode/themes/` or `.opencode/themes/` in a project, using the same format as OpenCode themes.

## MCP servers

[Model Context Protocol](https://modelcontextprotocol.io) servers add extra tools. Each tool appears to the model as `<server>_<tool>`.

```json
"mcp": {
  "files":  { "type": "local",  "command": ["npx", "-y", "@modelcontextprotocol/server-filesystem", "."], "environment": { "DEBUG": "0" } },
  "remote": { "type": "remote", "url": "https://mcp.example.com/mcp", "headers": { "Authorization": "Bearer ..." } },
  "off":    { "type": "local",  "command": ["some-server"], "enabled": false }
}
```

`local` servers are started as child processes (stdio); `remote` servers use Streamable HTTP. A server that fails to start prints a warning and the rest still load.

## Plugins

Plugins are separate programs, written in any language, that opencode-rs starts and talks to with JSON-RPC over stdin/stdout. A plugin can:

- inspect or change a tool's input before it runs, or block the call
- change a tool's output after it runs
- watch every event of the session
- add new tools for the model to use

Plugins are loaded from the `plugin` section of the config and from executable files in `~/.config/opencode/plugins/` and `.opencode/plugins/`. A plugin that crashes or stops answering is disabled without affecting the agent. The protocol, with a complete Python example, is described in [crates/oc-ext/PLUGINS.md](crates/oc-ext/PLUGINS.md), and [example_plugin.rs](crates/oc-ext/src/bin/example_plugin.rs) is a working Rust plugin that blocks dangerous shell commands and adds a `current_time` tool.

> Plugins run with your user's permissions. Only install plugins you trust.

## Language servers (LSP)

After the agent writes or edits a file, opencode-rs asks the matching language server for errors and adds them to the tool result, so the model can fix its own mistakes. A server is only started when a file of its language is touched and the program is installed on your `PATH`.

| Language | Server |
|---|---|
| Rust | `rust-analyzer` |
| TypeScript / JavaScript | `typescript-language-server` |
| Python | `pyright-langserver`, or `pylsp` as a fallback |
| Go | `gopls` |
| C / C++ | `clangd` |

Override or add servers in the config:

```json
"lsp": {
  "pyright": { "disabled": true },
  "zls": { "command": ["zls"], "extensions": ["zig"], "root_markers": ["build.zig"] }
}
```

`"lsp": false` turns the feature off. `OC_LSP_TIMEOUT_MS` changes how long to wait for diagnostics (default 3000).

## HTTP server and session sharing

`opencode serve` starts an HTTP API (default `http://127.0.0.1:4096`) so other programs can drive the agent:

| Endpoint | Purpose |
|---|---|
| `GET /health` | Health check |
| `GET /session`, `POST /session` | List or create sessions |
| `GET /session/{id}`, `DELETE /session/{id}` | Read or delete a session |
| `GET /session/{id}/message`, `POST /session/{id}/message` | Read messages, or send `{"text": "..."}` and get the answer |
| `POST /session/{id}/abort` | Stop the running answer |
| `POST /session/{id}/permission/{permission_id}` | Answer a permission request with `{"response": "once" \| "always" \| "reject"}` |
| `GET /event` | Server-Sent Events stream of everything happening (text, tools, thinking, permission requests); token via `?token=` |
| `POST /share`, `GET /share/{id}`, `GET /share/{id}/data` | Session sharing |

**Authentication.** Because the API can run commands on the host, every request (except public share pages) needs a bearer token. `opencode serve` prints a fresh token on start, or set your own with `OPENCODE_SERVER_TOKEN` (≥16 chars). Send it as `Authorization: Bearer <token>`; `GET /event` also accepts `?token=<token>` because browsers cannot set headers on `EventSource`. The server also rejects requests whose `Origin` is not allow-listed (blocking cross-site browser pages) and, on loopback, whose `Host` is not a loopback name (blocking DNS-rebinding). Allow a web app with `OPENCODE_CORS_ORIGINS=https://app.example.com` and extra host names with `OPENCODE_ALLOWED_HOSTS`. Session working directories are confined to the directory `serve` was started in.

**Sharing:** `opencode share <id>` (or `/share` in the UI) uploads the session to the share server and prints a link to a read-only web page of the conversation. Run `opencode serve` on a machine others can reach, point `share_url` (or `OPENCODE_SHARE_URL`) at it, and give the client the server's token with `OPENCODE_SHARE_TOKEN`. Set `OPENCODE_PUBLIC_URL` on the server to control the links it hands out (its host is added to the allow-list automatically). Each share also has its own secret, so only you can update or remove it with `opencode unshare <id>`.

> Even with a token, treat the server as powerful: anyone who has the token can run commands on the host. Keep it on `127.0.0.1` unless you deliberately expose it, and use a strong `OPENCODE_SERVER_TOKEN` behind a TLS proxy if you do.

## Where data is stored

| What | Location |
|---|---|
| Global config | `~/.config/opencode/opencode.json` (`$XDG_CONFIG_HOME`) |
| Project config | `./opencode.json` |
| Custom themes | `~/.config/opencode/themes/`, `.opencode/themes/` |
| Plugins | `~/.config/opencode/plugins/`, `.opencode/plugins/` |
| Sessions | `~/.local/share/opencode-rs/sessions/` (`$XDG_DATA_HOME`) |
| API keys | `~/.local/share/opencode-rs/auth.json` (readable only by you) |
| Shared sessions | `~/.local/share/opencode-rs/shares.json` |
| UI preferences | `~/.local/share/opencode-rs/tui.json` |

## Project layout

The project is a Cargo workspace of seven crates:

| Crate | Responsibility |
|---|---|
| [`oc-ai`](crates/oc-ai) | Provider clients (Anthropic, OpenAI-compatible, Gemini), SSE streaming, retries, thinking |
| [`oc-core`](crates/oc-core) | Agent loop, built-in tools, sessions, config, permissions, credentials, share client |
| [`oc-cli`](crates/oc-cli) | The `opencode` program and its commands |
| [`oc-tui`](crates/oc-tui) | Terminal UI (ratatui), themes, permission dialog |
| [`oc-server`](crates/oc-server) | HTTP API (axum), event stream, permission broker, share pages |
| [`oc-ext`](crates/oc-ext) | MCP client and process plugins |
| [`oc-lsp`](crates/oc-lsp) | Language server client for diagnostics |

## Building from source

Requires [Rust](https://rustup.rs) (stable).

```sh
git clone https://github.com/exgg1453/opencode-rs
cd opencode-rs
cargo build --release          # program at target/release/opencode
cargo test --workspace         # run the test suite
cargo install --path crates/oc-cli
```

**Releasing:** pushing a tag that starts with `v` makes GitHub Actions build every platform and publish a release:

```sh
git tag v0.1.2 && git push origin v0.1.2
```

## Known limitations

- Not yet supported: Amazon Bedrock, OAuth logins for MCP servers, comments in `opencode.json`.
- Shell commands are not checked for paths outside the project (for example `cat /etc/passwd` is not flagged).
- Plugin tool names are not namespaced and can clash with built-in tools; plugins cannot call back into opencode-rs.
- There is no desktop or web app; only the terminal UI and the HTTP API.
- If a model refuses a request or reaches its output limit, the UI does not show a dedicated message.

## License

[MIT](LICENSE). The bundled theme files come from [OpenCode](https://github.com/sst/opencode) and are used under its MIT license ([crates/oc-tui/themes/LICENSE-opencode](crates/oc-tui/themes/LICENSE-opencode)). opencode-rs is an independent project and is not affiliated with the OpenCode team.
