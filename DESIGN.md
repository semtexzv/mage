# Mage — Self-Replicating Coding Agent

**M**eta **Age**nt. A unified Rust workspace for building self-modifying AI coding agents.

Consolidates three projects:
- **agentcore** — Stateful agent loop, extension system, LLM abstraction, Anthropic provider
- **tau-tui-next** — Differential terminal renderer, markdown streaming, editor, keymap
- **metarust** — Dynamic Rust workspace compiler with `@dep` annotations and Jinja2 templates

## Architecture

```
                    ┌─────────────────────────────────────────┐
                    │     Synthesized Binary (mage-build)      │
                    │  Generated main.rs wires all crates      │
                    │  Bootstrap and self-replication identical │
                    └───────┬───────────┬──────────┬──────────┘
                            │           │          │
               ┌────────────┘     ┌─────┘          └──────────┐
               ▼                  ▼                           ▼
    ┌──────────────────┐  ┌─────────────┐          ┌──────────────────┐
    │    mage-core     │  │  mage-tui   │          │   mage-build     │
    │  agent loop      │  │  renderer   │          │   workspace      │
    │  extensions      │  │  markdown   │          │   compiler       │
    │  tool model      │  │  editor     │          │   (@dep, jinja)  │
    └──────┬───────────┘  └─────────────┘          └──────────────────┘
           │
    ┌──────┴───────┐
    │   mage-llm   │
    │  Provider     │
    │  types/events │
    └──────┬───────┘
           │
    ┌──────┴───────┐       ┌──────────────────┐
    │    refstr     │       │  mage-anthropic  │
    │  Str/Str<A>  │       │  (impl Provider) │
    └──────────────┘       └──────────────────┘
```

## Binary Synthesis

There is no hand-written binary crate in the workspace. All mage binaries are produced by `mage-build`'s `Bundle` + `Template` pipeline.

A `MageTemplate` (implementing the `Template` trait) renders a `main.rs` that:
- Includes extension modules via `#[path]` attributes
- Calls their `init()` hooks to register tools and providers
- Creates `AgentLoop` with providers, extensions, and configuration
- Spawns the agent loop via `session::spawn()`
- Wraps the session in an `App` (from `mage-app`)
- Runs the TUI loop (or other frontend mode)

**Bootstrap:** An `xtask bootstrap` (or equivalent build script) calls `Bundle::new("mage").with_template(MageTemplate).generate().compile()` to produce the initial binary distributed to users.

**Self-replication:** When the agent modifies extensions and recompiles, it uses the exact same `MageTemplate` and `Bundle` pipeline. Bootstrap and self-replication are identical code paths.

The generated binary IS the application. There is no separate app binary to maintain.

## Principles

Inherited from agentcore. Non-negotiable.

1. A constant > a function > an object. Minimize abstraction weight.
2. `&mut` over `Rc<RefCell<>>`. Split borrows over shared mutability.
3. Sequential await over channels. Channels only where true concurrency exists.
4. No trait when a struct suffices. No struct when a closure suffices.
5. Single-threaded (`current_thread` tokio). `Rc` over `Arc`. `Cell` over `Atomic`.

## Directory Structure

```
mage/
├── Cargo.toml                  # Workspace root
├── Cargo.lock
├── DESIGN.md                   # This file
│
├── pkg/                        # First-party crates (workspace members)
│   ├── refstr/                 # Ref-counted string (Str, Str<Atomic>)
│   │   └── Cargo.toml          #   name = "refstr"
│   │
│   ├── llm/                    # LLM abstraction: types, Provider trait, events, channel
│   │   └── Cargo.toml          #   name = "mage-llm"
│   │
│   ├── core/                   # Agent loop, extension system, tool model
│   │   └── Cargo.toml          #   name = "mage-core"
│   │
│   ├── tui/                    # Terminal UI: renderer, markdown, editor, keymap
│   │   └── Cargo.toml          #   name = "mage-tui"
│   │
│   ├── build/                  # Dynamic workspace compiler (metarust)
│   │   └── Cargo.toml          #   name = "mage-build"
│   │
│   ├── sdk/                    # Re-export crate for extensions
│   │   └── Cargo.toml          #   name = "mage"
│   │
│   └── app/                    # Application logic (session + commands)
│       └── Cargo.toml          #   name = "mage-app"
│       (No binary crate — all binaries are synthesized by mage-build)
│
├── providers/                  # LLM provider implementations (workspace members)
│   ├── anthropic/              #   name = "mage-anthropic"
│   └── openai/                 #   name = "mage-openai" (future)
│
├── extensions/                 # Built-in extensions (NOT workspace members)
│   └── tools-coding/           #   read, bash, edit, write, grep, find, ls
│
├── examples/                   # Example extensions for users
│   └── hello-ext/
│
└── xtask/                      # Dev automation
```

## Crate Map

| Directory | Crate Name | Origin | Purpose |
|---|---|---|---|
| `pkg/refstr` | `refstr` | agentcore/crates/pistr | Ref-counted strings, zero external deps |
| `pkg/llm` | `mage-llm` | agentcore/crates/llm | Message/ContentBlock/Usage types, Provider trait, AssistantMessageEvent, CancelToken, channel |
| `pkg/core` | `mage-core` | agentcore/crates/agent-core | Extension trait + ExtensionRegistry, AgentLoop, AgentEvent, ToolResult |
| `pkg/tui` | `mage-tui` | tau-tui-next | Differential renderer, StyleStack, Markdown (incremental, streaming), Editor (pills), Keymap (packed u64), Overlay |
| `pkg/build` | `mage-build` | metarust | Bundle, Module (@dep parsing), Template trait, Toolchain, Compiler, DependencyResolver, toolchain downloader |
| `pkg/sdk` | `mage` | new | Thin re-export crate for extension authors. Re-exports mage-core Extension trait, ExtensionRegistry, AgentLoop, types |
| `pkg/app` | `mage-app` | new | Application logic: App struct, command registry. Sits between mage-core and UI layer |
| `providers/anthropic` | `mage-anthropic` | agentcore/crates/anthropic | AnthropicProvider (HTTP/SSE streaming, partial JSON, API types) |

There is no host binary crate. All mage binaries (including the initial bootstrap) are synthesized by `mage-build` using the `Template` trait. The `MageTemplate` generates `main.rs` that wires extensions, creates `AgentLoop`, spawns via `session::spawn()`, and runs the app. Bootstrap and self-replication use the same code path.

## Dependency Graph

```
refstr  (zero deps)
  ↓
mage-llm  (refstr, serde, serde_json, tokio)
  ↓
mage-core  (mage-llm, refstr, serde, serde_json, tokio)
  ↓
mage-app  (mage-core, mage-llm, refstr)

Synthesized binary  (mage-app, mage-core, mage-llm, mage-tui, mage-build, mage-anthropic)
  │
  └── mage (sdk)  (re-exports mage-core, mage-app types)

Sibling crates (no inter-dependencies):
  mage-anthropic  (mage-llm, refstr, reqwest)
  mage-tui  (crossterm, tokio, futures, pulldown-cmark)
  mage-build  (syn, quote, minijinja, sha2, toml, ...)
```

## Extension System

### Static Linking

Extensions are compiled into the synthesized binary. No cdylib, no dlopen, no FFI.

Two forms:

**1. Built-in extensions** — Rust crates in `extensions/`, included by the `MageTemplate` during binary synthesis:
```
MageTemplate.render_main() includes built-in extension modules
and calls their init() hooks alongside user extensions.
```

**2. MetaRust extensions** — Single `.rs` files (or `mod.rs` directories) with `@dep` annotations, compiled on the fly by `mage-build` into a fresh binary that includes the extension.

### Extension as a Single File

```rust
// ~/.mage/extensions/fetch_url.rs
// @dep reqwest = { version = "0.12", features = ["json"] }
use mage::prelude::*;

struct FetchUrlExtension;

#[async_trait(?Send)]
impl Extension for FetchUrlExtension {
    fn init(&mut self, reg: &mut ExtensionRegistry) {
        reg.tool(
            llm::Tool {
                name: "fetch_url".into(),
                description: "Fetch a URL and return its content".into(),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": { "url": { "type": "string" } },
                    "required": ["url"]
                }),
            },
            |_id, params, _handle| async move {
                let url = params["url"].as_str().unwrap_or("");
                match reqwest::get(url).await.and_then(|r| r.text().await) {
                    Ok(body) => ToolResult::success(body),
                    Err(e) => ToolResult::failure(format!("{e}")),
                }
            },
        );
    }
}
```

### Extension Discovery

```
~/.mage/extensions/          System-wide extensions
.mage/extensions/            Project-local extensions
```

`mage-build` handles compilation:
- Scans extension directories for `.rs` files and `mod.rs` directories
- Parses `@dep` annotations to discover external dependencies
- Detects `init()` function signature via syn AST introspection
- Generates a workspace with the extension + mage SDK as core crates
- Compiles via `cargo build` with JSON diagnostic parsing
- Caches results based on source hash (SHA-256)

### Compilation Flow

```
Discovery:
  ~/.mage/extensions/my_tool.rs
  .mage/extensions/my_tool.rs

          │
          ▼

mage-build Module::parse_file()
  → extracts @dep annotations
  → finds init() hook via syn
  → resolves internal deps via ModuleResolver

          │
          ▼

mage-build Bundle::new("my-app-with-my-tool")
  .sdk_version("0.3.2")           ← SDK from crates.io, not path deps
  .add_module(my_tool_module)      ← the extension
  .with_template(MageTemplate)     ← generates main.rs that wires everything
  .with_toolchain(system)

          │
          ▼

Bundle::generate()
  → writes Cargo.toml (with SDK registry dep, resolved extension deps, patches)
  → renders main.rs from template (includes #[path] to extension, calls init())
  → writes to /tmp/mage/{bundle-hash}/ (transient staging, see DESIGN-REPRODUCIBLE-BUILDS.md)

          │
          ▼

Bundle::compile()
  → cargo build --message-format=json
  → parses diagnostics (errors, warnings)
  → copies output binary to ~/.mage/bin/mage-{petname}
  → returns CompilationResult with executable path
```

### Template System

Templates are Jinja2 (via minijinja). They receive the module list as context and generate:

1. **main.rs** — Entry point that `#[path = "..."]`-includes each module and calls their `init()` hooks
2. **Dependencies** — Additional Cargo dependencies the template itself requires

The template for mage extensions would generate something like:

```rust
// Generated main.rs
#[path = "modules/fetch_url.rs"]
mod fetch_url;

fn main() {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let local = tokio::task::LocalSet::new();
    local.block_on(&rt, async {
        let extensions: Vec<Box<dyn Extension>> = vec![Box::new(fetch_url::FetchUrlExtension)];
        let providers = vec![/* ... */];
        let (agent_loop, event_rx) = AgentLoop::new(
            "system prompt", model, options, providers, extensions,
        );
        let handle = session::spawn(agent_loop);
        // ... run TUI with handle ...
    });
}
```

## Self-Modification Flow
The entire point. An agent can modify its own capabilities:

```
1. Agent runs with current tools
2. Agent decides it needs a new tool (or wants to fix a broken one)
3. Agent writes/modifies .mage/extensions/new_tool.rs
4. Agent invokes mage-build to compile a new binary with the extension
5. Agent writes new binary path to monitor pipe, exits 42
6. Monitor spawns new binary, runs health check (LLM-verified)
7. On pass: new agent resumes session with the new/fixed tool
8. On fail: monitor rolls back to previous version automatically
```

See `DESIGN-MONITOR.md` for the full monitor + health check + rollback protocol.
See `DESIGN-DISTRIBUTION.md` for binary naming, generations.jsonl, version
selection (`mage --use <petname>`), and cleanup.

`mage-build`'s snapshot captures the entire source state as a zstd-compressed
tar archive embedded in the binary (see DESIGN-REPRODUCIBLE-BUILDS.md):
- All module source code (extensions only — not SDK sources)
- Generated template output
- SDK version and hash (not source — resolved from crates.io)
- Cargo.lock (pins SDK + all transitive deps for exact reproducibility)
- Dependency specs and resolved versions
- Parent hash for lineage tracking
This enables:
- **Versioned self-modification** — each generation's source is captured
- **Rollback** — monitor automatically reverts on health check failure
- **Rebuild from archive** — `mage build --from-snapshot` on any machine
- **History** — agent can include its modification history as context
- **Concurrent versions** — `mage --use happy-wolf` runs an older version
The `metarust/tests/agent_test.rs` already demonstrates this pattern:
an agent that generates its next generation, compiles it, and executes it.

## Agent Loop

Inherited from agentcore, which ports pi-mono's architecture.

```
extension.init(registry)        ← register tools (closures) / providers
  │
on_before_agent_start            ← modify system prompt
on_agent_start
  │
outer loop (follow-ups):
  inner loop (tool-use + steering):
  ├─ on_turn_start
  ├─ on_context(messages)       ← chain: Option<ContextResult>
  ├─ stream LLM response
  │    (emit: MessageStart/Delta/End)
  │
  for each tool_call in response:
  │  ├─ on_tool_call             ← Option<ToolCallResult>, block if .block=true
  │  ├─ execute tool closure     ← RegisteredTool.execute(id, args, ToolHandle)
  │  ├─ on_tool_result           ← Option<ToolResultResult>, may modify
  │  └─ drain steering queue
  │
  ├─ on_turn_end
  └─ drain follow_up queue → continue outer or finish
  │
on_agent_end
```

Sequential. Deterministic event stream. Tools one at a time.
Only LLM streaming uses a channel.

### Hook Categories

| Category | Return type | Hooks |
|---|---|---|
| **Lifecycle** | `()` (no return) | on_agent_start, on_agent_end, on_turn_start, on_turn_end, on_message_delta |
| **Decision (block)** | `Option<XResult>` where XResult has `block: bool` | on_tool_call, on_input |
| **Decision (modify)** | `Option<XResult>` with optional fields | on_context, on_tool_result, on_before_agent_start |

### Tool Model

Tools are closures, not traits. Registered during `Extension::init()` via `ExtensionRegistry::tool(schema, closure)`.

```rust
reg.tool(
    llm::Tool { name, description, parameters },
    |call_id: String, args: serde_json::Value, handle: ToolHandle| async move {
        // handle.is_cancelled() for cooperative cancellation
        // handle.send_update(ToolUpdate { .. }) for progress
        // handle.loop_handle() to inject/steer messages
        ToolResult::success("result")
    },
);
```

`ToolResult` is a struct: `{ content: Vec<UserContent>, is_error: bool }`.
`ToolHandle` provides: cancellation checking, progress updates via `send_update()`, and a `LoopHandle` for message injection.

## TUI

From tau-tui-next. Main terminal mode (no alternate screen), scrollback preserved.

Key design:
- **Differential renderer** — O(1) line diffing via `Rc<str>` identity comparison
- **Incremental markdown** — Block-level caching, only last block re-rendered on append. Designed for streaming LLM output
- **StyleStack** — Nested style state with O(1) push/pop and pre-built SGR sequences
- **Editor** — Multi-line with pill support (Unicode PUA sentinels for complex paste content)
- **Keymap** — Packed u64 key bindings with const constructors: `ctrl(ch('c'))`, `alt(LEFT)`

## LLM Abstraction

From agentcore's llm crate.

```rust
pub trait Provider {
    fn stream(&self, req: StreamRequest) -> StreamHandle;
}
```

`StreamRequest` bundles model, context, options, and cancel token. `StreamHandle` wraps a receiver of `AssistantMessageEvent` plus a join handle for the background task. Every event carries the full partial `AssistantMessage`. Consumers never maintain their own state.

Event protocol:
```
Start → (TextStart → TextDelta* → TextEnd | ThinkingStart → ThinkingDelta* → ThinkingEnd | ToolcallStart → ToolcallDelta* → ToolcallEnd)* → Done|Error
```

## Build System (mage-build)

From metarust. The compilation engine that makes self-modification possible.

### Key Abstractions

**Module** — A single `.rs` file or `mod.rs` directory. Dependencies inline via `// @dep`:
```rust
// @dep serde = "1.0"                              → External { name, spec: Version }
// @dep serde = { version = "1", features = ["derive"] }  → External { name, spec: Full }
// @dep my_other_module                             → Internal(name)
```

Module parsing via `syn` extracts:
- All `@dep` annotations (regex)
- Function signatures (especially `init()` hooks)
- Whether the module is single-file or directory

**Bundle** — Aggregates modules + template + toolchain + SDK version + patches:
```rust
let bundle = Bundle::new("my-agent")
    .with_config(config)
    .sdk_version("0.3.2")
    .add_module(extension_module)
    .add_shared(shared_lib_path)
    .add_patch("crates-io", "tokio", PatchSource::Path(tokio_path))
    .with_template(my_template)
    .with_toolchain(Toolchain::resolve_system()?);

bundle.generate()?;                    // writes staging dir to /tmp/mage/{hash}/
let result = bundle.compile()?;        // cargo build, returns CompilationResult
// snapshot is generated during bundle.generate() as src/snapshot.tar.zst
```

**DependencyResolver** — Semver-aware dedup across modules, templates, and core crates:
- Same version → dedup
- Compatible versions → tighter constraint wins
- Feature arrays → merged (union)
- Path/git deps → must be identical
- Incompatible → error (or pick latest, configurable)

**Toolchain** — Auto-detect from PATH, load from sysroot, download specific versions:
```rust
Toolchain::resolve_system()?           // finds cargo/rustc in PATH
Toolchain::from_sysroot("/path")?      // from custom sysroot
Toolchain::from_cache("1.85.0")?       // from ~/.mage/toolchains/
```

**Template** trait:
```rust
pub trait Template {
    fn render_main(&self, ctx: &RenderContext) -> Result<String>;
    fn render_dependencies(&self, ctx: &RenderContext) -> Result<Vec<Dependency>> {
        Ok(Vec::new())
    }
}
```

### Workspace Generation

`Bundle::generate()` writes:
```
/tmp/mage/{bundle-hash}/
├── Cargo.toml         # Generated: [package], [dependencies] with mage = "X.Y.Z", extension deps, [patch]
├── Cargo.lock         # Pinned: SDK + all transitive deps
├── src/
│   ├── main.rs        # Generated from template
│   └── snapshot.tar.zst  # Embedded via include_bytes!
└── modules/           # Symlinks to extension sources
```

Module source files are referenced at their original paths via `#[path = "..."]` attributes.
The SDK (`mage`) is a registry dependency in the generated Cargo.toml, not a local path. No `core/` directory exists in staging.

### Compilation

`Bundle::compile()`:
1. Spawns `cargo build --message-format=json`
2. Parses JSON diagnostics (errors, warnings, artifact paths)
3. Copies output binary to `~/.mage/bin/mage-{petname}`, writes `.meta` sidecar
4. Appends to `generations.jsonl` (see `DESIGN-DISTRIBUTION.md`)
5. Extracts dep-info for incremental rebuild tracking
6. Cleans up transient staging directory
7. Returns `CompilationResult` with success/failure, executable path, diagnostics

## Operational Modes (from pi-mono)

| Mode | Description |
|---|---|
| **Interactive** | Full TUI with editor, markdown streaming, keybindings, overlays |
| **Print/JSON** | Single prompt → response, no session state. `-p` flag |
| **RPC** | Stdin/stdout protocol for process integration |
| **SDK** | Embedded in other Rust applications via mage-core |

## Session Persistence (from pi-mono)

JSONL with tree structure (entries with id/parentId).
Enables in-place branching without new files.
Compaction: summarize old messages while keeping recent history.

## Extension Points (via mage-core Extension trait)

| Event | Category | Purpose |
|---|---|---|
| `on_agent_start` | lifecycle | Lifecycle notification |
| `on_agent_end` | lifecycle | Lifecycle notification |
| `on_before_agent_start` | decision (modify) | Modify system prompt before agent starts |
| `on_turn_start` | lifecycle | Turn boundary |
| `on_turn_end` | lifecycle | Turn boundary |
| `on_message_delta` | lifecycle | Streaming token |
| `on_tool_call` | decision (block) | Intercept/block tool call |
| `on_tool_result` | decision (modify) | Amend tool result |
| `on_input` | decision (block) | Intercept user input |
| `on_context` | decision (modify) | Replace/prune message list before LLM call |

## What's Done (from agentcore)

- [x] Provider trait + channel-based streaming
- [x] Agent loop with extension dispatch (21+ tests)
- [x] Extension two-phase init (factory → per-agent)
- [x] Tool execution with steering/follow-up queues
- [x] Anthropic provider (HTTP/SSE, partial JSON, API types)
- [x] Retry logic with exponential backoff
- [x] Thinking signature accumulation
- [x] Usage/cost tracking
- [x] TUI: differential renderer, markdown streaming, editor, keymap, overlays
- [x] MetaRust: module parsing, bundle generation, compilation, dep resolution, toolchain management

## What's Next

- [ ] Consolidate into this workspace (copy source, rename crates, fix imports)
- [ ] pkg/sdk: re-export crate for extension authors
- [ ] MageTemplate: Template implementation that generates the bootstrap and all subsequent binaries
- [ ] Extension discovery and compilation pipeline (mage-build integration)
- [ ] Built-in tools: read, bash, edit, write, grep, find, ls
- [ ] Self-modification loop: agent writes extension → mage-build compiles → exec replaces process
- [ ] Sub-agent spawning
- [ ] OpenAI provider
- [ ] Session persistence (JSONL tree)
- [ ] RPC mode
