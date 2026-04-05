# Self-Recompilation

How the binary rebuilds itself.

## Overview

The mage binary embeds a compressed archive of its own source code (the "snapshot").
It can extract this archive, add new user modules, compile a new binary, and
hot-swap itself via the monitor process.

```
Binary (generation N)
  |
  |-- embedded snapshot.tar.zst (~214KB, ~141 files)
  |     contains: Cargo.toml, Cargo.lock, main.rs, crates/*, modules/*
  |
  |-- Recompile tool (or `mage rebuild`)
  |     1. Extract snapshot to /tmp/mage-rebuild-<pid>/
  |     2. Scan ~/.mage/modules/ for user modules
  |     3. Add new modules to src/modules/
  |     4. Re-render main.rs if new modules found
  |     5. Generate Cargo.lock via `cargo generate-lockfile`
  |     6. Compile with fresh snapshot embedded
  |     7. Copy binary to ~/.mage/bin/
  |
  |-- Monitor catches exit code 42
  |     1. Spawns new binary
  |     2. New binary reports HEALTHY or UNHEALTHY      [NOT YET IMPLEMENTED]
  |     3. Monitor commits or rolls back                [NOT YET IMPLEMENTED]
  |     4. New binary resumes session                   [NOT YET IMPLEMENTED]
  |
Binary (generation N+1)
  |-- embedded snapshot.tar.zst (fresh)
  |-- can rebuild itself the same way
```

## Module discovery

Only `~/.mage/modules/` is scanned. No project-local modules.
This avoids path issues when the binary is run from different directories
and ensures that with global modules, the latest committed version always wins.

Module conflicts (two modules providing the same tool name) should be
detected and reported as an error. [NOT YET IMPLEMENTED]

## Compilation Pipeline

### Workspace dependencies (planned refactor)

Currently, inter-crate deps use relative paths in each crate's Cargo.toml:
```toml
# pkg/core/Cargo.toml
llm = { package = "mage-llm", path = "../llm" }
```

This requires fragile path rewriting when crates are copied to a snapshot.

**Planned:** move ALL inter-crate deps to `[workspace.dependencies]` in the
root Cargo.toml. Member crates use `dep.workspace = true`:

```toml
# Root Cargo.toml
[workspace.dependencies]
mage-core = { path = "pkg/core" }
llm = { package = "mage-llm", path = "pkg/llm" }
refstr = { path = "pkg/refstr" }
# ...

# pkg/core/Cargo.toml
[dependencies]
llm.workspace = true
refstr.workspace = true
```

This means only the ROOT Cargo.toml needs path rewriting during snapshot
extraction. Individual crate Cargo.toml files are never modified.
Eliminates `rewrite_crate_internal_deps` entirely.

### MageBuild (workspace path)

`pkg/build/src/template.rs`

```rust
MageBuild::new(workspace_root)
    .standard_extension_dirs()  // ~/.mage/modules/
    .compile()
```

Steps:
1. Verify workspace root exists
2. Scan `~/.mage/modules/` for .rs modules
3. Create `Bundle` with core crates + MageTemplate + modules
4. `bundle.generate()`:
   a. Copy core crate sources to `~/.mage/workspaces/<name>/crates/<pkg>/`
   b. Generate root Cargo.toml (rewrite workspace.dependencies paths to crates/)
   c. Render main.rs from MageTemplate
   d. Generate Cargo.lock via `cargo generate-lockfile`
   e. Write snapshot (includes Cargo.lock)
5. Compile — single pass (snapshot is already complete)

### compile_from_snapshot_data (snapshot path)

`pkg/build/src/template.rs`

1. Extract snapshot.tar.zst to `/tmp/mage-rebuild-<pid>/`
2. Restructure: main.rs -> src/main.rs, modules/ -> src/modules/
3. Scan `~/.mage/modules/`, copy new .rs files to src/modules/
4. If new modules found: re-render main.rs via MageTemplate
5. Rewrite root Cargo.toml workspace.dependencies paths to crates/<pkg>
6. Generate fresh Cargo.lock
7. Write fresh snapshot (includes new modules + Cargo.lock)
8. Compile
9. Copy binary to ~/.mage/bin/

## Snapshot Format

`snapshot.tar.zst` — zstd-compressed tar archive.

```
main.rs                          Generated entry point
Cargo.toml                       Generated manifest (workspace.dependencies with crates/ paths)
Cargo.lock                       Pinned transitive dependencies
modules/                         User module sources (if any)
  my_tool.rs
crates/                          All core crate source trees
  mage-sdk/                     SDK re-export crate (renamed from mage)
    Cargo.toml                   Uses dep.workspace = true (no relative paths)
    src/lib.rs
  mage-core/
    Cargo.toml                   Uses dep.workspace = true
    src/...
  mage-tools/
  mage-llm/
  mage-tui/
  mage-build/
  mage-app/
  mage-anthropic/
  refstr/
```

**Key property:** only the root Cargo.toml has `path = "crates/..."` entries.
All crate Cargo.toml files use `dep.workspace = true` — no relative paths,
never modified during extraction.

## SDK Crate Naming

The SDK crate should be renamed from `mage` to `mage-sdk` to avoid the
package name collision with the generated binary.

In user-authored modules, the SDK is accessed as:
```rust
use mage_sdk::prelude::*;
```

Or with a Cargo alias:
```toml
# In generated Cargo.toml
sdk = { package = "mage-sdk", path = "crates/mage-sdk" }
```

## Generated main.rs

MageTemplate renders:

```rust
const SNAPSHOT: &[u8] = include_bytes!("snapshot.tar.zst");

fn main() {
    mage_sdk::upgrade::set_snapshot(SNAPSHOT);

    // Subcommands (no TUI needed)
    match std::env::args().nth(1).as_deref() {
        Some("snapshot") => { /* list/extract */ },
        Some("rebuild") => { /* recompile */ },
        _ => {}
    }

    // Monitor wrapping
    if !mage_sdk::upgrade::is_agent_mode() { run_monitor(); }

    // Agent
    run_local(|| async {
        let mut modules = mage_sdk::tools::all();
        // + extension modules
        run(modules).await;
    });
}
```

## Monitor Protocol

The binary has two modes based on `MAGE_AGENT_PIPE_FD`:

- **Not set:** monitor mode — spawn self as child, supervise
- **Set:** agent mode — run normally

### Upgrade flow (current)

```
Agent: compile new binary -> signal_upgrade(path) -> safe_exit(42)
Monitor: exit 42 -> read path from pipe -> spawn new binary
```

### Upgrade flow (planned)

```
Agent: compile new binary -> save session -> signal_upgrade(path) -> safe_exit(42)
Monitor: exit 42 -> read path from pipe -> spawn new binary with health check
New binary: load session -> verify tools -> ask LLM "are you working?" -> HEALTHY
Monitor: commit to new generation (append to generations.jsonl)
```

On UNHEALTHY or timeout:
```
Monitor: kill new binary -> append "failed" to generations.jsonl -> spawn previous binary
Previous binary: resumes session
```

## Versioning (planned)

`~/.mage/bin/generations.jsonl` — append-only log:

```json
{"name":"mage-brave-eagle","generation":1,"status":"healthy","timestamp":"..."}
{"name":"mage-happy-wolf","generation":2,"status":"healthy","parent":"mage-brave-eagle"}
{"name":"mage-quick-fox","generation":3,"status":"pending","parent":"mage-happy-wolf"}
{"name":"mage-quick-fox","generation":3,"status":"failed"}
{"name":"mage-happy-wolf","generation":2,"status":"healthy"}
```

Last line = current version. Rollback = append previous version as healthy.

## Current Limitations

### Path rewriting (to be eliminated)

Currently walks every crate's Cargo.toml to rewrite path deps.
Will be replaced by workspace.dependencies approach (see above).

Remaining edge cases after the refactor:
- `[workspace.dependencies]` paths need rewriting in root Cargo.toml only
- `[patch]` sections with path deps: not handled (none currently used)
- Build scripts with embedded paths: not handled

### Two-pass compilation

Currently does two cargo invocations (compile + recompile with snapshot).
Planned fix: use `cargo generate-lockfile` to get Cargo.lock without compiling,
then write the complete snapshot before the single compile pass.

### Session continuity

Not implemented. Conversation lost on recompile. Planned:
- JSONL session format (id/parentId tree)
- Save before exit 42
- Load on startup (check session file path in env/args)

## Code Locations

| Component | File |
|---|---|
| MageBuild | `pkg/build/src/template.rs` |
| MageTemplate | `pkg/build/src/template.rs` |
| compile_from_snapshot_data | `pkg/build/src/template.rs` |
| Bundle::generate() | `pkg/build/src/bundle.rs` |
| Bundle::compile() | `pkg/build/src/bundle.rs` |
| write_snapshot | `pkg/build/src/bundle.rs` |
| Module scanning | `pkg/build/src/module.rs` |
| Dependency resolution | `pkg/build/src/deps.rs` |
| Monitor loop | `pkg/app/src/monitor.rs` |
| Upgrade signaling | `pkg/core/src/upgrade.rs` |
| Rebuild subcommand | `pkg/app/src/rebuild.rs` |
| Recompile tool | `pkg/tools/src/recompile.rs` |
| Snapshot subcommand | `pkg/app/src/snapshot_cmd.rs` |

## TODO

- [ ] Refactor to workspace.dependencies (eliminate per-crate path rewriting)
- [ ] Rename SDK crate: mage -> mage-sdk
- [ ] Use `cargo generate-lockfile` instead of two-pass compile
- [ ] Only scan ~/.mage/modules/ (remove .mage/modules/ and project-local)
- [ ] Module conflict detection (duplicate tool names)
- [ ] Health check in monitor (HEALTHY/UNHEALTHY pipe)
- [ ] Session persistence (save before exit 42, load on startup)
- [ ] generations.jsonl (version tracking, rollback)
- [ ] Wrapper script for /usr/local/bin/mage
