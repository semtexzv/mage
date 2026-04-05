# Self-Recompilation

How the binary rebuilds itself. This documents the current implementation,
not aspirational design.

## Overview

The mage binary embeds a compressed archive of its own source code (the "snapshot").
It can extract this archive, add new user modules, compile a new binary, and
hot-swap itself via the monitor process.

```
Binary (generation N)
  |
  |-- embedded snapshot.tar.zst (214KB, 141 files)
  |     contains: Cargo.toml, Cargo.lock, main.rs, crates/*, modules/*
  |
  |-- Recompile tool (or `mage rebuild`)
  |     1. Extract snapshot to temp dir
  |     2. Scan ~/.mage/modules/ and .mage/modules/ for new extensions
  |     3. Add new modules to src/modules/
  |     4. Re-render main.rs if new modules found
  |     5. Rewrite Cargo.toml path deps
  |     6. Compile (two-pass: first for Cargo.lock, second with fresh snapshot)
  |     7. Copy binary to ~/.mage/bin/
  |
  |-- Monitor catches exit code 42
  |     Spawns the new binary with same args
  |
Binary (generation N+1)
  |-- embedded snapshot.tar.zst (fresh, includes Cargo.lock)
  |-- can rebuild itself the same way
```

## Entry Points

### `cargo xtask bootstrap`

Initial build from workspace sources. Used during development.

```
workspace root (pkg/*, providers/*)
  -> MageBuild::new(&workspace_root)
       .name("mage-bootstrap")
       .config(Config { approot: target/mage-bootstrap })
       .extension_dir(modules/)
       .compile()
  -> binary at target/mage-bootstrap/bin/mage-bootstrap-<petname>
```

### `mage rebuild`

Subcommand handled before the monitor. Two modes:

**Workspace mode** (if workspace root found):
```
MageBuild::new(&root).standard_extension_dirs().compile()
```

**Snapshot mode** (no workspace — e.g., running from /tmp):
```
compile_from_snapshot_data(embedded_snapshot, &module_dirs)
```

### Recompile tool

LLM-callable tool. Same logic as `mage rebuild`, but:
- On success under monitor: calls `safe_exit(42)` -> monitor spawns new binary
- On success without monitor: returns tool result with binary path
- On failure: returns compilation errors to the LLM (last 3000 chars of stderr)

## Compilation Pipeline

### MageBuild (workspace path)

`pkg/build/src/template.rs`

```rust
MageBuild::new(workspace_root)
    .standard_extension_dirs()  // ~/.mage/modules/ + .mage/modules/
    .compile()
```

Internally:
1. Verify workspace root exists
2. Scan extension directories for .rs modules
3. Create `Bundle` with 9 core crates + MageTemplate + modules
4. `bundle.generate()`:
   a. Copy core crate sources to `~/.mage/workspaces/<name>/crates/<pkg>/`
   b. Rewrite inter-crate path deps (e.g., `path = "../llm"` -> `path = "../mage-llm"`)
   c. Generate Cargo.toml with workspace members, deps, shared workspace deps
   d. Render main.rs from MageTemplate
   e. Write preliminary snapshot (without Cargo.lock)
5. Pass 1: `bundle.compile()` — cargo build
6. Regenerate snapshot WITH Cargo.lock from the compiled workspace
7. Pass 2: incremental recompile (only binary crate changes — snapshot asset changed)

### compile_from_snapshot_data (snapshot path)

`pkg/build/src/template.rs`

1. Extract snapshot.tar.zst to temp dir
2. Restructure: main.rs -> src/main.rs, modules/ -> src/modules/
3. Scan extra module dirs, copy new .rs files to src/modules/
4. If new modules found: re-render main.rs via MageTemplate
5. Write snapshot data to src/snapshot.tar.zst (placeholder for include_bytes!)
6. Rewrite root Cargo.toml path deps -> crates/<pkg>
7. Rewrite each crate's internal path deps -> ../<pkg>
8. Add workspace members for all crates
9. Pass 1: cargo build
10. Regenerate fresh snapshot (with Cargo.lock + rewritten sources)
11. Pass 2: incremental recompile
12. Copy final binary to ~/.mage/bin/

## Snapshot Format

`snapshot.tar.zst` — zstd-compressed tar archive.

```
main.rs                          Generated entry point
Cargo.toml                       Generated manifest (self-contained path deps)
Cargo.lock                       Pinned transitive dependencies
modules/                         User extension sources (if any)
  my_tool.rs
crates/                          All 9 core crate source trees
  mage/                          SDK crate
    Cargo.toml
    src/lib.rs
  mage-core/
    Cargo.toml
    src/agent_loop.rs
    src/module.rs
    src/tool.rs
    ...
  mage-tools/
  mage-llm/
  mage-tui/
  mage-build/
  mage-app/
  mage-anthropic/
  refstr/
```

**Key property:** all path deps in Cargo.toml files are relative to the snapshot
root (crates/<pkg>). No references to external paths. The snapshot is self-contained
— it can be extracted anywhere and compiled with only a Rust toolchain.

**Size:** ~214KB compressed. 141 files. Includes Cargo.lock (67KB).

## Path Dependency Rewriting

Core crates reference each other via path deps in their Cargo.toml files.
In the original workspace, these are relative paths like `path = "../llm"`.
In the snapshot, all crates live under `crates/<package-name>/`.

Two rewrites happen:

### Root Cargo.toml

```toml
# Before (original workspace):
[dependencies.mage-llm]
path = "../../../../pkg/llm"

# After (snapshot):
[dependencies.mage-llm]
path = "crates/mage-llm"
```

### Inter-crate deps

```toml
# Before (in crates/mage-core/Cargo.toml):
[dependencies]
llm = { package = "mage-llm", path = "../llm" }

# After:
[dependencies]
llm = { package = "mage-llm", path = "../mage-llm" }
```

The rewrite handles the `package = "..."` rename pattern.
It matches by package name against the available crates in the snapshot.

### Workspace metadata

The generated Cargo.toml includes:

```toml
[workspace]
members = ["crates/mage", "crates/mage-core", ...]

[workspace.package]
edition = "2024"
rust-version = "1.85"

[workspace.dependencies]
serde = { version = "1", features = ["derive", "rc"] }
serde_json = "1"
tokio = { version = "1", features = ["rt", "time", "sync", "macros"] }
tokio-util = "0.7"
async-trait = "0.1"
```

This matches the workspace root Cargo.toml so crates that use
`edition.workspace = true` or `serde.workspace = true` resolve correctly.

## Generated main.rs

MageTemplate renders:

```rust
const SNAPSHOT: &[u8] = include_bytes!("snapshot.tar.zst");

fn main() {
    mage::upgrade::set_snapshot(SNAPSHOT);

    // Subcommands (no TUI needed)
    match arg { "snapshot" => ..., "rebuild" => ... }

    // Monitor wrapping
    if !is_agent_mode() { run_monitor(); }

    // Agent
    run_local(|| async {
        let modules = mage::tools::all();
        // + any extension modules via #[path = "modules/..."]
        run(modules).await;
    });
}
```

If extension modules are discovered, the template adds:
```rust
#[path = "modules/my_tool.rs"]
mod my_tool;

// ... inside main:
modules.extend(my_tool::modules());
```

## Monitor Protocol

The monitor is the binary itself. On startup:

1. Check `MAGE_AGENT_PIPE_FD` env var
2. If not set: I am the monitor
   - Create temp file for upgrade pipe
   - Spawn self as child with `MAGE_AGENT_PIPE_FD=<path>`
   - Wait for child exit
   - If exit code 42: read path from pipe, spawn new binary, loop
   - Other exit code: pass through
3. If set: I am the agent, run normally

### Upgrade flow

```
Agent: compile new binary -> signal_upgrade(path)
  |
  |-- writes path to MAGE_AGENT_PIPE_FD temp file
  |-- calls safe_exit(42)
  |     (safe_exit calls TUI restore hook before process::exit)
  |
Monitor: sees exit 42
  |-- reads path from temp file
  |-- validates binary exists
  |-- spawns new binary with same args + inherited stdio
  |-- loops (supervises new child)
```

### Without monitor

If `MAGE_AGENT_PIPE_FD` is not set (standalone run, -p mode):
- `signal_upgrade` returns `NoMonitor`
- Binary path printed to stderr
- Agent continues running current version
- User must restart manually

## Known Limitations

### Not yet implemented

- **Session persistence**: conversation lost on recompile. New binary starts fresh.
- **Health check**: monitor blindly trusts new binary. No LLM verification or rollback.
- **generations.jsonl**: no version tracking or rollback history.
- **SDK on crates.io**: snapshots embed full source trees (~214KB). With registry deps
  they would shrink to ~5KB (only extensions + Cargo.lock + metadata).

### Edge cases in path rewriting

- `[target.'cfg(...)'.dependencies]` path deps: NOT rewritten.
  Only `[dependencies]` section is processed.
- `[patch.*]` with path deps: NOT rewritten.
  Currently no crates use patches.
- Missing crates in snapshot: silently skipped (no error).
  Could cause confusing compile errors downstream.
- Build scripts (`build.rs`) that embed paths: NOT handled.
  `env!("CARGO_MANIFEST_DIR")` will differ between original and snapshot builds.

### Two-pass compilation overhead

Every build does two cargo invocations:
1. Full compile (all deps from scratch if no cache)
2. Incremental recompile (only the binary crate, ~2-5 seconds)

Pass 2 is fast because only `src/snapshot.tar.zst` changed (an asset file).
The overhead is negligible for interactive use but doubles CI build time.

## Code Locations

| Component | File |
|---|---|
| MageBuild (unified entry) | `pkg/build/src/template.rs` |
| MageTemplate (main.rs gen) | `pkg/build/src/template.rs` |
| compile_from_snapshot_data | `pkg/build/src/template.rs` |
| Bundle::generate() | `pkg/build/src/bundle.rs` |
| Bundle::compile() | `pkg/build/src/bundle.rs` |
| write_snapshot_inner | `pkg/build/src/bundle.rs` |
| Path dep rewriting | `pkg/build/src/template.rs` |
| Module scanning | `pkg/build/src/module.rs` |
| Dependency resolution | `pkg/build/src/deps.rs` |
| Monitor loop | `pkg/app/src/monitor.rs` |
| Upgrade signaling | `pkg/core/src/upgrade.rs` |
| Rebuild subcommand | `pkg/app/src/rebuild.rs` |
| Recompile tool | `pkg/tools/src/recompile.rs` |
| Snapshot subcommand | `pkg/app/src/snapshot_cmd.rs` |
