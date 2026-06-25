# mage

**A coding agent that rewrites its own binary.**

Mage is an experimental coding agent that extends itself by recompilation. The running
binary carries a compressed snapshot of its own source tree; when you give it a new
capability, it splices that capability in, compiles a fresh binary, and hot-swaps into the
new generation — which can then do the same again. Self-modification at the level of the
compiled artifact, not just prompts or configuration.

## The goal

Most agents are static — a fixed binary calling a model. Mage explores the inverse: an
agent whose capabilities are **compiled into the binary** and grow over time. Each new tool
or module becomes part of a new generation of the agent itself. The target is a
**self-extending agent** that explores, writes a new module, recompiles itself to include
it, confirms the new generation is healthy, and continues — keeping a lineage of
generations it can move forward and back through.

## How it works

The binary embeds `snapshot.tar.zst` — a zstd-compressed archive of its whole source tree
(nine workspace crates, a generated `main.rs`, and a pinned `Cargo.lock`). The
self-recompilation loop:

```
generation N  — binary with an embedded snapshot of its own source
   │   Recompile tool  (or `mage rebuild`)
   │   1. extract the embedded snapshot
   │   2. pick up new modules from ~/.mage/modules/
   │   3. splice them in, re-render main.rs
   │   4. pin dependencies and compile a fresh binary
   │   5. embed a fresh snapshot inside the new binary
   ▼   hot-swap via the monitor process
generation N+1 — can rebuild itself the same way
```

A supervising **monitor** process runs the agent as a child. When the agent compiles a
successor it signals the monitor, which brings up the new generation.

## Architecture

A Rust workspace (`edition 2024`) split into focused crates:

```
pkg/refstr            ref-counted strings (zero dependencies)
pkg/llm               provider abstraction — Provider trait, events, cancellation
pkg/core              agent loop, Module trait, tool registry, upgrade signaling
pkg/tools             built-in tools: Read, Edit, Write, Bash, Glob, Grep, Recompile
pkg/build             the compilation engine — snapshot bundling, templating, dep resolution
pkg/tui               differential terminal renderer, markdown, editor
pkg/app               run loop, monitor, rebuild/snapshot subcommands, credentials
pkg/sdk               re-export crate for module authors
providers/anthropic   Anthropic Messages API — SSE streaming, OAuth
```

Some highlights:

- **Concurrent tool dispatch** — read-only tools run in parallel, mutating tools serially;
  mid-stream abort is instant via hierarchical cancel tokens.
- **Differential TUI** — repaints only the lines that changed, preserves scrollback, and
  does zero terminal writes when idle.
- **Module trait** — a four-method extension surface (`tools`, `gate_tool`,
  `filter_result`, `transform_context`) that user modules implement and the agent compiles
  into itself.
- **Self-contained snapshots** — each generation embeds a complete, lock-pinned source
  tree, so any binary can reproduce or extend itself from nothing but itself.

## Build

```sh
cargo xtask bootstrap     # builds the binary with an embedded snapshot
```

## Goals & targets

Where mage is headed:

- **Health-checked generations** — a new binary verifies itself (tools load, model
  responds) before the monitor commits to it.
- **Rollback by lineage** — an append-only log of every generation, so any healthy
  ancestor can be restored.
- **Session continuity across rebuilds** — the conversation survives a recompile; the next
  generation resumes where the last left off.
- **Drop-in modules** — author a tool as a standalone `.rs` file in `~/.mage/modules/`, and
  the next generation ships with it.
- **Beyond a single provider** — more model backends and sub-agent spawning.

## Status

A personal research project exploring self-modifying agents — experimental, and pointed at
the ideas above.
