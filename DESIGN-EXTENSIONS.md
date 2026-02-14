# Extension System Design

How extensions are authored, discovered, and loaded in mage.

---

## Extension File Format — **Decided**

Two tiers only. No per-extension Cargo.toml. All dependencies via `@dep` header annotations in the source file.

**Tier 1: Single File**
```
my_tool.rs
```
- `// @dep` for dependencies
- `fn init()` for registration
- No Cargo.toml needed — mage-build generates it
- Best for simple tools (90% of cases)

**Tier 2: Module Directory**
```
my_tool/
├── mod.rs        # @dep annotations, init() hook
├── helpers.rs
└── utils.rs
```
- Same as Tier 1 but with sub-modules
- Still no Cargo.toml — mage-build generates it
- `@dep` annotations in `mod.rs`

Full crate with own Cargo.toml (Tier 3) is explicitly rejected. If an
extension needs build scripts, proc macros, or features, it should be a
standard crate depended on via `@dep crate_name = { path = "..." }` or
published to a registry.

### Detection
```rust
fn detect_tier(path: &Path) -> ExtensionTier {
    if path.is_file() && path.extension() == Some("rs") {
        Tier1SingleFile
    } else if path.is_dir() && path.join("mod.rs").exists() {
        Tier2ModuleDir
    } else {
        Unknown
    }
}
```


### Recursive Scanning and Module Naming — **Decided**

Modroot scanning is **recursive**. Subdirectories within a modroot are
scanned for `.rs` files and `mod.rs` directories. This allows users to
organize extensions into groups:

```
~/.mage/extensions/
├── coding/
│   ├── read_file.rs
│   ├── write_file.rs
│   └── bash.rs
├── web/
│   ├── fetch_url.rs
│   └── web_search.rs
└── my_custom_tool.rs
```

**Module naming**: the relative path from the modroot root is flattened
with underscores replacing path separators. The file extension is stripped.

```
coding/read_file.rs   → module name: coding_read_file
coding/write_file.rs  → module name: coding_write_file
web/fetch_url.rs      → module name: web_fetch_url
my_custom_tool.rs     → module name: my_custom_tool
```

For module directories (Tier 2), the directory path is flattened the same way:

```
web/search_engine/mod.rs  → module name: web_search_engine
```

The generated `main.rs` uses these flattened names:

```rust
#[path = "modules/coding/read_file.rs"]
mod coding_read_file;

#[path = "modules/web/fetch_url.rs"]
mod web_fetch_url;
```

This means:
- Users organize extensions into directories for their own convenience
- The module name used in `use crate::coding_read_file` has underscores
- Internal deps (`// @dep coding_read_file`) use the flattened name
- The `#[path]` attribute points to the actual file location (preserving
  directory structure in staging)


### Duplicate Module Names — **Decided**

If two files in the same modroot (or across modroots after shadowing)
produce the same flattened module name, it's a **compile error**.

```
~/.mage/extensions/
├── coding/read_file.rs      → coding_read_file
└── coding_read_file.rs      → coding_read_file   ← CONFLICT
```

`mage-build` detects this during module resolution (before compilation)
and produces a clear error message listing both source paths. No silent
shadowing within the same precedence level.

Across modroots, normal shadowing applies: project overrides system.
But within a single modroot, duplicates are an error.

---

## Extension Init Contract — **Partially Decided**

### Problem

The current extension model has two separate mechanisms:

1. **Extension trait** (agentcore) — `impl Extension` with `init(&mut self, &mut Registry)` + hook methods
2. **Module init hook** (metarust) — a free function `pub fn init()` discovered via syn AST parsing

These need to converge for single-file extensions. A `.rs` file in `~/.mage/extensions/`
should be able to:
- Register tools
- Subscribe to lifecycle hooks
- Access agent state during hooks

But the current metarust module format only detects `fn init()` and doesn't know about
the Extension trait.

### Proposal: Convention-Based Init

A single-file extension exports a free `init` function that receives a `Registry`:

```rust
// my_tool.rs
// @dep reqwest = "0.12"
use mage::prelude::*;

struct MyTool;

impl Tool for MyTool {
    fn name(&self) -> &str { "my_tool" }
    fn description(&self) -> &str { "Does a thing" }
    fn parameters(&self) -> &serde_json::Value { &MY_TOOL_SCHEMA }

    fn execute(&self, _id: &str, params: serde_json::Value, _cancel: CancelToken) -> ToolExecution {
        let input = params["input"].as_str().unwrap_or("").to_string();
        ToolExecution::running(async move {
            ToolResult::success(format!("processed: {input}"))
        })
    }
}
pub fn init(reg: &mut Registry) {
    reg.tool(MyTool);
}
```

The generated main.rs template calls `my_tool::init(&mut registry)` during agent setup.

For extensions that need lifecycle hooks, they implement Extension and register the factory:

```rust
// my_hooks.rs
use mage::prelude::*;

struct MyHooks { call_count: u32 }

impl Extension for MyHooks {
    fn init(&mut self, reg: &mut Registry) { /* register tools */ }
    fn on_tool_call<'a>(&'a mut self, args: &'a ToolCallArgs<'a>, _ctx: &'a HookCtx)
        -> HookFuture<'a, Disposition>
    {
        self.call_count += 1;
        Box::pin(async { Disposition::Propagate })
    }
}

pub fn init(factory_reg: &mut FactoryRegistry) {
    factory_reg.register(|| Box::new(MyHooks { call_count: 0 }));
}
```

### Shared State Between Extensions and Tools

Extensions that need shared mutable state create it during `init` and
share it via `Rc<RefCell<T>>` (or `Rc<Cell<T>>` for simple values)
between the extension and the tools it registers.

```rust
// stateful_extension.rs
use mage::prelude::*;

struct SharedState {
    call_count: u32,
    cache: HashMap<String, String>,
}

pub fn init(reg: &mut Registry) {
    let state = Rc::new(RefCell::new(SharedState {
        call_count: 0,
        cache: HashMap::new(),
    }));

    // Tool 1 reads and writes the shared state
    let s = state.clone();
    reg.tool(/* ... tool that captures `s` ... */);

    // Tool 2 also accesses the same state
    let s = state.clone();
    reg.tool(/* ... another tool that captures `s` ... */);
}
```

For extensions with lifecycle hooks, the state lives on the Extension
struct itself. Tools registered during `Extension::init` can capture
`Rc` clones of fields from the struct:

```rust
struct MyExtension {
    db: Rc<RefCell<DbConnection>>,
}

impl Extension for MyExtension {
    fn init(&mut self, reg: &mut Registry) {
        let db = self.db.clone();
        reg.tool(/* ... tool that captures `db` ... */);
    }

    fn on_tool_call<'a>(&'a mut self, args: &'a ToolCallArgs<'a>, _ctx: &'a HookCtx)
        -> HookFuture<'a, Disposition>
    {
        // Hook can also access self.db
        Box::pin(async { Disposition::Propagate })
    }
}
```

State is created synchronously during init. Extensions that need async
initialization (e.g. establishing a database connection) do so lazily
on first use, not during init.


### Detection

`mage-build` already parses function signatures via syn. We can detect which variant
by inspecting `init`'s parameter types:

| Signature | Meaning |
|---|---|
| `fn init(reg: &mut Registry)` | Simple tool registration |
| `fn init(reg: &mut FactoryRegistry)` | Full extension with lifecycle hooks |
| `fn init()` | Legacy/simple — no registration, just side effects |

The template generates the appropriate wiring based on what it detects.

### Resolved Sub-questions

- **`mage::prelude`**: Yes. Re-exports everything an extension needs.
- **Async init**: No. Init is sync. Extensions that need async setup do it lazily on first use.
- **`#[mage::extension]` attribute**: Deferred. Not needed now.

### Open Sub-questions

- Registry method signature: should it take the extension directly, or a lambda/factory?
  Leaning lambda for extensions with lifecycle hooks (so each session gets a fresh instance).

---

## Module Roots and Discovery — **Open**

> **Rethinking.** The 4-tier model (built-in, system, project, session) is under
> revision. New direction: **system modroot + project modroot**. The project
> modroot builds a project-local binary stored in the project directory. When
> you run the global `mage` binary, it detects the project binary and delegates
> to it (similar to how `npx` or `cargo` workspace binaries work). The UI needs
> to be notified about binary switching. Session modroot may be folded into
> project or dropped. Details below are from the previous design and may change.

### Problem

Where do extensions come from? The `ModuleResolver` in metarust already supports
multiple `modroots` (search directories), but we haven't specified which ones
exist, their precedence, or how they interact.

The agent needs to:
- Find built-in tools (coding tools that ship with mage)
- Find user-installed global extensions
- Find project-specific extensions
- Resolve `// @dep internal_module` references across roots
- Know which root to write to when creating new extensions

### Module Root Hierarchy

Four modroots, searched in order (later overrides earlier for same-name modules):

| # | Modroot | Path | Purpose | Writable by agent |
|---|---|---|---|---|
| 1 | **Built-in** | Compiled into binary | Coding tools: read, write, bash, grep, find, ls, edit | No (requires recompile) |
| 2 | **System** | `~/.mage/extensions/` | User-installed global extensions, shared across projects | Yes |
| 3 | **Project** | `.mage/extensions/` (relative to project root) | Project-specific extensions | Yes |
| 4 | **Session** | `{session_dir}/extensions/` | Ephemeral extensions created during this session | Yes |

#### Built-in (modroot 1)

Built-in extensions live in `extensions/tools-coding/` in the mage source tree.
They are compiled directly into `mage-host` as feature-gated dependencies.
Not on the module search path at all — they're just Rust code linked at compile time.

When the agent self-replicates, built-in tools are carried forward via the
snapshot archive (core crates include the host binary's wiring).

The agent **cannot** modify built-in extensions at runtime. To change them,
it must modify the source in the extracted snapshot and recompile.

#### System modroot (modroot 2): `~/.mage/extensions/`

Global extensions installed by the user. Shared across all projects and sessions.

```
~/.mage/extensions/
├── coding/
│   ├── read_file.rs
│   ├── write_file.rs
│   └── bash.rs
├── web/
│   ├── fetch_url.rs
│   └── web_search.rs
├── database/                  # Tier 2: module directory
│   ├── mod.rs
│   └── postgres.rs
└── custom_llm/                # Tier 2: module directory
    ├── mod.rs
    └── provider.rs
```

The user manages this directory manually or via `mage install <extension>`.
The agent can write here if configured to do so (default: no, requires
`--allow-system-write` or config flag).

#### Project modroot (modroot 3): `.mage/extensions/`

Project-local extensions. The `.mage/` directory is relative to the project root
(detected by walking up from CWD looking for `.mage/`, `.git/`, or `Cargo.toml`).

```
my-project/
├── .mage/
│   ├── config.toml            # Project-level mage config
│   └── extensions/
│       ├── deploy.rs          # Project-specific deploy tool
│       └── test_runner.rs     # Custom test runner for this project
├── src/
└── Cargo.toml
```

This is the primary place the agent writes new extensions. When the agent says
"I need a tool for X" and writes a new `.rs` file, it goes here.

Project extensions are committed to version control alongside the project.

#### Session modroot (modroot 4): ephemeral

Extensions created during a single agent session. These live in the session's
working directory and are discarded when the session ends (unless promoted).

```
~/.mage/sessions/{session-id}/
├── session.jsonl              # Conversation history
└── extensions/
    └── quick_hack.rs          # One-off tool the agent wrote mid-session
```

Use case: the agent creates a throwaway tool for a specific task, uses it,
then the session ends and the tool is garbage collected. If the tool proves
useful, the agent (or user) can promote it to project or system scope:

```
mage promote quick_hack --to project   # copies to .mage/extensions/
mage promote quick_hack --to system    # copies to ~/.mage/extensions/
```

### Resolution Order and Shadowing

`ModuleResolver` searches modroots in order: session → project → system.
(Built-ins are not searched; they're always present.)

A module in a later (higher-priority) root **shadows** the same-named module
in an earlier root:

```
~/.mage/extensions/web/fetch_url.rs            # system: web_fetch_url
my-project/.mage/extensions/web/fetch_url.rs   # project: web_fetch_url — wins
```

The project version is used. No merging, no error — just shadowing.
This lets projects override system-wide tools when needed.

Within the same modroot, duplicate flattened names are a compile error
(see "Duplicate Module Names" above).

### Internal Dependencies Across Roots

`// @dep coding_read_file` (internal dependency using the flattened name)
resolves through the same modroot search order. A project extension can
depend on a system extension:

```rust
// .mage/extensions/deploy.rs
// @dep web_fetch_url                 # resolved from ~/.mage/extensions/web/fetch_url.rs
// @dep serde_json = "1"

use crate::web_fetch_url;

pub fn init(reg: &mut Registry) { /* ... */ }
```

The resolver walks all modroots to find `web_fetch_url`, regardless of which
root `deploy.rs` lives in.

### Agent Write Target

When the agent creates a new extension, which modroot does it write to?

Default policy:
1. If a project root is detected → write to `.mage/extensions/` (modroot 3)
2. If no project root → write to session extensions (modroot 4)
3. Never write to system (modroot 2) unless explicitly configured

The agent can be configured via `.mage/config.toml`:

```toml
[extensions]
write_target = "project"   # "project" | "session" | "system"
```

### Discovery at Startup

`mage-host` startup sequence for extension discovery:

```
1. Detect project root (walk up from CWD)
2. Read .mage/config.toml if present
3. Build ModuleResolver with modroots:
   resolver.add_modroot(session_extensions_dir)   # highest priority
   resolver.add_modroot(project_extensions_dir)   # if project detected
   resolver.add_modroot(system_extensions_dir)    # ~/.mage/extensions/
4. Scan all modroots RECURSIVELY for .rs files and mod.rs directories
5. Flatten paths to module names (/ → _)
6. Check for duplicate flattened names within each modroot (error if found)
7. Apply shadowing across modroots (higher priority wins)
8. Parse each: extract @dep annotations, detect init() signature
9. Resolve internal dependencies (transitive closure)
10. Build Bundle with all discovered modules + core crates
11. Check if a cached binary exists for this bundle hash
    - Yes → run cached binary
    - No → compile via mage-build, cache result, run
```

### Modroot in the Staging Directory

When building, modules from all modroots are placed in the staging area
preserving their subdirectory structure:

```
/tmp/mage/{bundle-hash}/
├── modules/
│   ├── coding/
│   │   ├── read_file.rs       # from system modroot
│   │   └── write_file.rs      # from system modroot
│   ├── web/
│   │   └── fetch_url.rs       # from system modroot
│   ├── deploy.rs              # from project modroot
│   └── quick_hack.rs          # from session modroot
├── core/
└── ...
```

The generated `main.rs` uses `#[path]` with the subdirectory paths and
flattened module names:

```rust
#[path = "modules/coding/read_file.rs"]
mod coding_read_file;

#[path = "modules/web/fetch_url.rs"]
mod web_fetch_url;

#[path = "modules/deploy.rs"]
mod deploy;
```

The `mage-snapshot.json` metadata records provenance:

```json
{
  "modules": [
    {
      "name": "coding_read_file",
      "path": "modules/coding/read_file.rs",
      "is_dir": false,
      "origin": "system"
    },
    {
      "name": "deploy",
      "path": "modules/deploy.rs",
      "is_dir": false,
      "origin": "project"
    }
  ]
}
```

### Interaction with Self-Replication

When the agent self-replicates (modifies an extension and recompiles):

1. Extract current snapshot to working directory
2. Modify the module source in the extracted `modules/` directory
3. Recompile from the extracted directory
4. The new snapshot contains the modified source
5. **Also** write the modified source back to the appropriate modroot
   (so the change persists outside the snapshot)

Step 5 is important: without it, the modification only lives inside the
binary's embedded snapshot. If the user restarts mage without that binary,
the change is lost. Writing back to the modroot ensures persistence.

The write-back target is determined by `origin` in the snapshot metadata:
- `origin: "project"` → write to `.mage/extensions/`
- `origin: "session"` → write to session dir
- `origin: "system"` → write to `~/.mage/extensions/` (only if allowed)
- New modules (not in previous snapshot) → write to default write target

---

## Open Questions

### Module Roots

- Should `mage install <url>` fetch extensions from a registry? If so, which
  modroot do they land in? (Probably system.)
- How does the agent discover which tools it has available? Does it inspect
  the ModuleResolver at runtime, or does the compiled binary have a static
  tool manifest?
