# Mage — Current State (April 2026)

This document describes what exists, what works, and what's missing.
It supersedes the aspirational design docs (DESIGN*.md) where they diverge.

## Architecture

```
pkg/refstr        Ref-counted strings (Str). Zero deps.
pkg/llm           LLM abstraction: Provider trait, types, events, CancelToken.
pkg/core          Agent loop, Module trait, ToolHandler, ToolRegistry, upgrade signaling.
pkg/tools         Built-in tools: Read, Edit, Write, Bash, Glob, Grep, Recompile.
pkg/app           Application layer: run(), monitor, rebuild, snapshot, TUI wiring, credentials.
pkg/tui           Terminal UI: differential renderer, markdown, editor, overlay, keymap.
pkg/build         Compilation engine: Bundle, Template, MageBuild, dependency resolution.
pkg/sdk           Re-export crate for module authors (pub use everything).
providers/anthropic   Anthropic Messages API: SSE streaming, OAuth, Claude Code identity.
xtask             Bootstrap entrypoint (cargo xtask bootstrap).
```

### Dependency graph

```
refstr
  |
llm (refstr, serde, tokio, tokio-util)
  |
core (llm, refstr, serde, tokio, async-trait)
  |
tools (core, llm, build, refstr, globset, walkdir, regex)
  |
app (core, tools, llm, tui, build, anthropic, refstr, dirs)
  |
sdk (core, tools, app, llm, tui, build, refstr, tokio, async-trait)

Sibling:
  tui (crossterm, tokio, futures, pulldown-cmark)
  build (syn, quote, toml, sha2, tar, zstd, walkdir, semver, dirs, ...)
  anthropic (llm, refstr, reqwest, futures-util, sha2, dirs)
```

## Agent Loop

The agent loop is in `pkg/core/src/agent_loop.rs`. Double-nested structure:

```
outer loop (follow-ups):
  inner loop (tool calls + steering):
    transform context (modules)
    stream LLM response (select! with cancel)
    dispatch tools (concurrent read-only, serial mutating)
    collect results (select! with cancel + command drain)
  check follow-up queue -> continue outer or finish
```

### Tool dispatch

Tools declare `is_concurrent_safe(args) -> bool`. The dispatcher:
1. Partitions tool calls into concurrent-safe and serial.
2. Spawns concurrent tools as independent `spawn_local` tasks.
3. Spawns serial tools on a single task with a local `VecDeque`.
4. Collects results via `mpsc::unbounded_channel`.
5. While collecting, also handles abort/steering via `select!`.

### Message queuing (Pi model)

During tool execution, only `Abort` takes immediate effect.
Steering messages queue for after all tools complete (next turn).
Follow-up/inject messages queue for after the entire run.
User input during streaming is queued and shown grayed out in TUI.

### Cancellation

`CancelToken` wraps `tokio_util::CancellationToken`. Hierarchical:
- `run_cancel` per agent run
- `child_token()` per tool execution
- `LoopHandle` holds the current `run_cancel` directly for instant abort

Streaming uses `select!` to race `rx.recv()` vs `cancel.cancelled()`.
Cancel takes effect immediately, even mid-stream.

## Module System

Replaces the old Extension trait. Defined in `pkg/core/src/module.rs`.

```rust
#[async_trait(?Send)]
pub trait Module: 'static {
    fn name(&self) -> &str;
    fn tools(&self) -> Vec<ToolDef> { vec![] }
    async fn gate_tool(&self, call: &ToolCall) -> GateResult { GateResult::Allow }
    async fn filter_result(&self, call: &ToolCall, result: ToolResult) -> ToolResult { result }
    async fn transform_context(&self, messages: Vec<llm::Message>) -> Vec<llm::Message> { messages }
}
```

4 methods instead of 10+. `&self` on all methods.
Lifecycle observation via AgentEvent broadcast, not Module methods.

## Tool System

Tools implement `ToolHandler`:

```rust
#[async_trait(?Send)]
pub trait ToolHandler: 'static {
    async fn execute(&self, args: Value, ctx: ToolContext) -> ToolResult;
    fn is_concurrent_safe(&self, args: &Value) -> bool { false }
}
```

`ToolContext` provides: `cancel_token()`, `send_text(view)`, `loop_handle()`.

### Built-in tools

| Tool | Concurrent | Notes |
|---|---|---|
| Read | Yes | Line-numbered output, offset/limit |
| Edit | No | Exact string replacement, uniqueness check |
| Write | No | Full file write, creates parent dirs |
| Bash | No (always serial) | Streaming stdout, cwd persistence, tab replacement |
| Glob | Yes | walkdir + globset, skips .git/node_modules/target |
| Grep | Yes | Regex, 3 output modes, glob filter |
| Recompile | No | Triggers self-recompilation, signals monitor |

### Tool rendering

`ToolUpdate { text: String }` — complete current view, not a delta.
Tool calls `ctx.send_text(view)` with the full picture each time.
TUI replaces the streaming widget on each update.

## Anthropic Provider

`providers/anthropic/src/provider.rs`. Implements `llm::Provider`.

- SSE streaming with `EventMapper` for delta events
- OAuth auto-refresh (Claude Pro/Max subscriptions)
- Claude Code identity mode (CC system prompt, tool name remapping, beta flags)
- Pi auth file sharing: `with_credential_file(~/.pi/agent/auth.json)` re-reads on every request
- API key priority: `ANTHROPIC_API_KEY` env > stored OAuth > Pi auth file
- Tool use input sanitization: orphaned tool_use blocks get synthetic tool_result
- Debug logging to `~/.mage/sse.log` and `~/.mage/request.log`

### Models

Defined in `providers/anthropic/src/models.rs`. Includes:
- Claude Opus 4.6, 4.5, 4.1, 4.0
- Claude Sonnet 4.5, 4.0
- Claude Haiku 4.5

Default model: first in list (Opus 4.6).

## TUI

`pkg/tui/` — main terminal mode (no alternate screen).

### Renderer

Differential renderer in `renderer.rs`:
- First frame: `full_render(clear: false)` — just output from cursor.
- Width change: `full_render(clear: true)` — clear viewport + scrollback.
- Normal: `diff_render()` — find first/last changed line, repaint only those.
- Line comparison: `Rc::ptr_eq` || string equality fallback.
- Nothing changed: returns `false`, no terminal writes, no cursor movement.
- Synchronized output (`\x1b[?2026h`/`l`) wraps all writes.
- Viewport tracking: `prev_vp_top`, `hw_cursor_row`, `compute_line_diff()`.

### Layout

```
[welcome line]          — shown when conversation is empty
[chat log widgets]      — User, Assistant (Markdown), Thinking, Tool, Error, Info
[spacer]                — blank lines to push chrome to bottom
[queued messages]       — grayed out with arrow, shown during agent run
[blank + spinner]       — braille animation (running) / cancelled / empty
[hr + editor + hr]
[status bar]            — model, context tokens, cache, cost
```

### Key bindings

- Escape: abort current agent run
- Ctrl-C: abort if running, quit if idle
- Ctrl-D: quit
- Enter: submit input
- /command: slash command overlay with autocomplete

## Self-Compilation

### Build pipeline

`MageBuild` in `pkg/build/src/template.rs` is the unified entry point.

```rust
MageBuild::new(&workspace_root)
    .name("mage-bootstrap")       // default: "mage-bin"
    .config(Config { approot })    // default: ~/.mage
    .extension_dir(path)           // or .standard_extension_dirs()
    .compile()?;
```

Internally:
1. Scans extension directories for `.rs` modules.
2. Creates a `Bundle` with core crates + modules + `MageTemplate`.
3. `Bundle::generate()`:
   - Copies core crate sources to `crates/<pkg-name>/` (self-contained).
   - Rewrites inter-crate path deps (`../llm` -> `../mage-llm`).
   - Generates `Cargo.toml` with workspace deps, members, packages.
   - Renders `main.rs` from template.
   - Writes preliminary snapshot (without Cargo.lock).
4. `Bundle::compile()` — invokes `cargo build --message-format=json`.
5. After pass 1 succeeds: regenerates snapshot WITH Cargo.lock.
6. Pass 2 (incremental) — recompiles with fresh snapshot embedded.

### Generated main.rs

```rust
const SNAPSHOT: &[u8] = include_bytes!("snapshot.tar.zst");

fn main() {
    mage::upgrade::set_snapshot(SNAPSHOT);

    // Subcommands before monitor
    match arg { "snapshot" => ..., "rebuild" => ..., _ => {} }

    // Monitor wrapping
    if !mage::upgrade::is_agent_mode() {
        run_monitor();
    }

    // Agent
    run_local(|| async {
        let modules = mage::tools::all();
        mage::app::run::run(modules).await;
    });
}
```

### Snapshot contents (141 entries, ~214KB compressed)

```
main.rs          — generated entry point
Cargo.toml       — generated manifest (self-contained path deps)
Cargo.lock       — pinned transitive dependencies
modules/         — user extension sources (if any)
crates/          — all 9 core crate source trees (rewritten path deps)
  mage/
  mage-core/
  mage-tools/
  mage-llm/
  refstr/
  mage-tui/
  mage-build/
  mage-app/
  mage-anthropic/
```

### Three compilation paths

| Path | Source | After compile |
|---|---|---|
| `cargo xtask bootstrap` | Workspace on disk | Print path |
| `mage rebuild` | Workspace or embedded snapshot | Signal monitor or print path |
| Recompile tool | Same as rebuild | Signal monitor (exit 42) or return result |

All three use `MageBuild` or `compile_from_snapshot_data`.

### Monitor

`pkg/app/src/monitor.rs`. The binary has two modes:

- `MAGE_AGENT_PIPE_FD` not set: I am the monitor. Spawn self as child.
- `MAGE_AGENT_PIPE_FD` set: I am the agent. Run normally.

Exit code 42: monitor reads new binary path from temp file, spawns it.
Other exit codes: pass through.

### Upgrade signaling

`mage_core::upgrade`:
- `signal_upgrade(path) -> Result<UpgradeSignal>` — writes path, returns `Ready` or `NoMonitor`.
- `safe_exit(code)` — calls registered exit hook (TUI restore), then `process::exit`.
- Caller decides whether to exit 42 or return a tool result.

## What's Working End-to-End

1. Bootstrap from workspace: `cargo xtask bootstrap` -> binary with snapshot.
2. Binary runs with TUI, tools, streaming, Pi auth.
3. `mage rebuild` from workspace or from embedded snapshot (from any directory).
4. `mage snapshot list/extract` to inspect embedded sources.
5. Recompile tool: agent triggers rebuild, monitor spawns new binary.
6. Each generation gets a fresh snapshot with Cargo.lock.
7. Abort (Escape/Ctrl-C) cancels immediately, even mid-stream.
8. Differential rendering, scrollback preserved, idle = zero terminal writes.

## What's Missing

### Critical for self-hosting

- **Session persistence**: conversations lost on restart/recompile.
  No save/load. Design says JSONL tree format.

- **Health check**: monitor blindly trusts new binary.
  Design says: LLM-based verification, HEALTHY/UNHEALTHY pipe, rollback.

- **generations.jsonl**: no version tracking, no rollback history, no `mage use <petname>`.

### Important for usability

- **System prompt**: currently one sentence. Needs tool usage instructions,
  coding conventions, behavior rules.

- **Context compaction**: no mechanism to prevent context window overflow.
  Long conversations will hit the limit and fail.

- **Non-interactive subcommands**: `-p` works but no `--model`, `--api-key`,
  `--working-dir` flags.

### Nice to have

- **OpenAI provider**: only Anthropic implemented.
- **Sub-agent spawning**: no Agent tool.
- **RPC mode**: no stdin/stdout protocol.
- **Session branching**: design says JSONL tree with id/parentId.
- **`[patch]` path rewriting**: patches with path deps not rewritten in snapshot.
- **`[target.*.dependencies]`**: target-specific deps not rewritten.
- **SDK on crates.io**: currently path deps only. Would shrink snapshots.

## File Map

```
pkg/core/src/
  agent_loop.rs    Agent loop with concurrent tool dispatch
  module.rs        Module trait, ModuleSet, GateResult
  tool.rs          ToolHandler trait, ToolDef, ToolRegistry, tool_fn helper
  handle.rs        LoopHandle, LoopCommand (channel into the loop)
  session.rs       SessionHandle, spawn()
  types.rs         Message, AgentEvent, ToolResult, ToolUpdate, ToolView
  upgrade.rs       Upgrade signaling, snapshot data, exit hook
  event_stream.rs  AgentEventSender/Receiver type aliases

pkg/tools/src/
  read.rs          Read tool
  edit.rs          Edit tool
  write.rs         Write tool
  bash.rs          Bash tool (streaming, cwd persistence)
  glob.rs          Glob tool
  grep.rs          Grep tool
  recompile.rs     Recompile tool

pkg/app/src/
  run.rs           Unified entry point: run(), run_default(), run_print()
  monitor.rs       Monitor loop, spawn_with_pipe
  rebuild.rs       `mage rebuild` subcommand
  snapshot_cmd.rs  `mage snapshot` subcommand
  tui.rs           TUI app: widgets, event handling, rendering
  app.rs           App struct, command routing
  command.rs       Slash command registry
  credentials.rs   Credential storage (OAuth tokens)

pkg/build/src/
  template.rs      MageTemplate, MageBuild, compile_from_snapshot_data, snapshot ops
  bundle.rs        Bundle: generate(), compile(), write_snapshot(), dep rewriting
  module.rs        Module parsing (@dep, init detection), scan_directory()
  deps.rs          DependencyResolver, DepSpec, semver merging
  compile.rs       CompilationResult, Diagnostic, ToolchainMetadata
  toolchain.rs     Toolchain resolution, metadata extraction
  downloader.rs    Toolchain download (optional)

providers/anthropic/src/
  provider.rs      AnthropicProvider, resolve_api_key, credential_file
  events.rs        EventMapper: SSE -> AssistantMessageEvent
  convert.rs       LLM types -> Anthropic API types, tool_use sanitization
  oauth.rs         OAuthCredentials, refresh, CC identity constants
  login.rs         Interactive PKCE login flow
  models.rs        Anthropic model catalog
  api_types.rs     Anthropic API request/response types
  sse.rs           SSE parser
```
