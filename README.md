# closed-code

An AI-powered coding CLI written in pure Rust, powered by Gemini 3.1 Pro Preview. Multi-agent architecture for codebase exploration, planning, and code modification with diff-based approval.

## Modes

| Mode | Capabilities |
|------|-------------|
| **Explore** | Read-only codebase research via sub-agents |
| **Plan** | Research + web search + structured plan generation |
| **Execute** | Code modifications with colorized diffs and user approval |

## Quick Start

```bash
export GEMINI_API_KEY="your-key-here"

# Interactive REPL
closed-code

# Start in a specific mode
closed-code --mode execute

# One-shot query
closed-code ask "Explain the authentication flow in this project"

# Target a directory
closed-code --directory /path/to/project
```

## Architecture

The main LLM (orchestrator) manages conversation history, mode-specific tool registration, and sub-agent dispatch. Sub-agents are independent Gemini conversations spawned as function calls — no IPC needed.

```
User → REPL → Orchestrator ─┬─ Explorer (read-only research)
                             ├─ Planner (structured plans)
                             ├─ Web Search (google_search grounding)
                             ├─ Filesystem Tools (all modes)
                             └─ Write Tools (execute mode, approval-gated)
```

**Sub-agents**: Each gets its own system prompt, tools, and conversation history. They report back via a structured `create_report` tool. The orchestrator synthesizes their findings.

**Tool-call loop**: Send message → Gemini responds with function calls → execute tools → send results back → repeat until text response or max iterations.

**Streaming**: Main agent streams tokens via SSE for real-time display. Sub-agents use non-streaming calls since the orchestrator needs their complete response.

## Key Features (at full build)

- **10-phase incremental build** — usable after every phase (see [Phase Spec](phase_spec.md))
- **Gemini function calling** with custom Part deserializer for camelCase JSON
- **Approval-gated writes** — colorized unified diffs, default No for safety
- **Platform sandboxing** — macOS Seatbelt, Linux Landlock + seccomp
- **Session management** — resume, fork, compact conversations (JSONL persistence)
- **MCP client** — external tool servers via STDIO transport
- **Full-screen TUI** — ratatui with scrollable chat, inline approval overlays, vim-style diff viewer
- **Multi-agent parallel execution** — isolated git worktrees per agent
- **Git integration** — branch awareness, `/diff`, `/review`, `/commit`
- **Fuzzy file search** — `@` trigger with nucleo matching
- **25+ slash commands**, shell prefix (`!`), multiline input (Ctrl+G)

## Project Structure

```
closed-code/
  Cargo.toml
  src/
    main.rs, cli.rs, repl.rs, config.rs, error.rs
    gemini/   — API types, streaming client, SSE parser
    agent/    — orchestrator, explorer, planner, web searcher
    tool/     — registry, filesystem, shell, spawn, write/edit, report
    ui/       — diff, approval, markdown, spinner, theme
    mode/     — Mode enum (Explore, Plan, Execute)
    git/      — context, diff, auto-commit
    sandbox/  — seatbelt (macOS), landlock (linux), fallback
    session/  — JSONL persistence, resume/fork/compact
    mcp/      — MCP client, tool discovery
    tui/      — ratatui app, widgets, overlays
```

## Security

- **Shell allowlist**: only `ls`, `cat`, `head`, `tail`, `find`, `grep`, `rg`, `wc`, `file`, `tree`, `pwd`, `which`, `git`
- **No shell expansion**: commands run via `tokio::process::Command` with explicit args (no `sh -c`)
- **Read-only by default**: write tools only registered in Execute mode
- **Approval-gated**: every file modification requires confirmation (default No)
- **Sub-agent isolation**: separate conversation histories, no parent context leakage
- **Protected paths**: `.git/`, `.closed-code/`, `.env`, credential files always read-only

## Implementation Roadmap

See **[Phase Spec](phase_spec.md)** for the full 10-phase implementation plan (~23,200 lines of Rust total):

| Phase | Name | After This Phase |
|-------|------|-----------------|
| 1 | Foundation + Gemini + REPL | Chat with Gemini in terminal |
| 2 | Tool System + Filesystem | Explore code via LLM |
| 3 | Sub-Agent Architecture | Delegate research to sub-agents |
| 4 | Execute Mode + Diffs | Write code with diff review |
| 5 | Config + Enhanced REPL | Production-ready CLI |
| 6 | Git Integration | Git-aware coding assistant |
| 7 | Sandboxing | Safe autonomous execution |
| 8 | Sessions + MCP | Resume conversations, extend tools |
| 9 | Full-Screen TUI | Visual parity with Codex CLI |
| 10 | Polish + Multi-Agent | Full feature parity |

## Requirements

- Rust (stable toolchain)
- [Gemini API key](https://aistudio.google.com/app/apikey)

## Design Decisions

| Decision | Rationale |
|----------|-----------|
| Single binary | Components are tightly coupled; workspace adds build complexity |
| Sub-agents as function calls | No IPC — async functions within the tool-call loop |
| REPL first, TUI last | Validate business logic before investing in complex UI |
| Approval gates before sandbox | User confirmation provides safety before platform hardening |
| JSONL for sessions | Append-only, incrementally parseable, human-readable |
| ratatui for TUI | Battle-tested, active ecosystem, familiar widget model |
