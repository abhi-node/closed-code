# closed-code

An AI-powered coding CLI written in pure Rust, powered by Gemini 3.1 Pro Preview. It features a multi-agent architecture for codebase exploration, planning, and secure code modification with diff-based user approvals.

## Features

- **Full-Screen TUI**: Built with `ratatui`, featuring a scrollable chat, inline approval overlays, command picker, and a vim-style diff viewer.
- **Multi-Agent Architecture**: A central orchestrator delegates complex tasks to specialized sub-agents (Explorer, Planner, Reviewer, Committer, Web Searcher) using Gemini function calling.
- **Platform Sandboxing**: Secure, OS-level sandboxing using macOS Seatbelt and Linux Landlock/seccomp to restrict file and network access.
- **Diff Approvals**: Colorized unified diffs with explicit user approval required for any file modification (defaults to "No" for safety).
- **Session Management**: Robust JSONL-based session persistence. Resume, fork, and compact conversation history effortlessly.
- **Context Caching**: Intelligent Gemini context caching for sub-agents to minimize token usage across invocations.
- **Git Integration**: Built-in branch awareness, automated diff reviewing, and commit generation capabilities.

## Modes

| Mode | Capabilities |
|------|-------------|
| **Explore** | Read-only codebase research via sub-agents. |
| **Plan** | Research + web search + structured plan generation. |
| **Guided** | Step-by-step execution with explicit user approval for each action. |
| **Execute** | Code modifications with colorized diffs and user approval. |
| **Auto** | Fully autonomous execution without manual approval gates (use with caution). |

## Installation

Ensure you have the stable Rust toolchain installed, then build and install from source:

```bash
cargo install --path .
```

## Quick Start

```bash
export GEMINI_API_KEY="your-api-key-here"

# Start the interactive TUI (defaults to Explore mode)
closed-code

# Start in a specific mode
closed-code --mode execute

# One-shot query (non-interactive)
closed-code ask "Explain the authentication flow in this project"

# Target a specific directory
closed-code --directory /path/to/project

# Resume a previous session
closed-code resume
```

## Architecture

The main LLM (Orchestrator) manages the state machine, conversation history, mode-specific tool registration, and sub-agent dispatch. Sub-agents are independent Gemini conversations spawned as function calls — requiring no IPC.

```text
User → TUI → Orchestrator ─┬─ Explorer (read-only research)
                           ├─ Planner (structured plans)
                           ├─ Web Search (google_search grounding)
                           ├─ Reviewer / Committer (Git operations)
                           ├─ Filesystem Tools (all modes)
                           └─ Write Tools (execute/guided/auto modes)
```

**Sub-agents**: Each is equipped with its own system prompt, tools, and isolated conversation history. They report back to the Orchestrator via a structured `create_report` tool, allowing the Orchestrator to synthesize findings. Context caching is utilized to lower token usage across invocations.

**Tool-call loop**: Send message → Gemini responds with function calls → execute Rust tools → send results back → repeat until a final text response is generated or max iterations are reached.

**Streaming**: The Orchestrator streams tokens via SSE for a highly responsive real-time TUI, while sub-agents execute synchronously for complete report ingestion.

## Security

- **Shell Allowlist**: Only a strict allowlist of safe commands is permitted (`ls`, `cat`, `head`, `tail`, `find`, `grep`, `rg`, `wc`, `file`, `tree`, `pwd`, `which`, `git`).
- **No Shell Expansion**: Commands are executed via `tokio::process::Command` with explicit arguments to prevent injection (no `sh -c`).
- **Sandboxing**: OS-level restrictions dynamically applied based on mode (`workspace-only`, `workspace-write`, `full-access`).
- **Approval-Gated**: File writes and modifications strictly require user confirmation.
- **Sub-Agent Isolation**: Separate conversation histories prevent context leakage between parent and child agents.
- **Protected Paths**: Critical directories and files like `.git/`, `.closed-code/`, `.env`, and credential files are always mounted as read-only.

## Contributing

Contributions are welcome! Please ensure your code passes all tests, adheres to standard formatting guidelines (`cargo fmt`), and doesn't introduce unapproved shell commands or bypass sandboxing measures.

## License

This project is licensed under the MIT License.
