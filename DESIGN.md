# Mage — Self-Replicating Coding Agent

**M**eta **Age**nt. A unified Rust workspace for building self-modifying AI coding agents.

Consolidates three projects:
- **agentcore** — Stateful agent loop, extension system, LLM abstraction, Anthropic provider
- **tau-tui-next** — Differential terminal renderer, markdown streaming, editor, keymap
- **metarust** — Dynamic Rust workspace compiler with `@dep` annotations and Jinja2 templates

## Architecture

```
                    ┌─────────────────────────────────────────┐
                    │             mage-host (binary)          │
                    │  CLI, session management, modes         │
                    │  (interactive / print / RPC / SDK)      │
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
│   └── bin/                    # Host binary and CLI
│       └── Cargo.toml          #   name = "mage-host", [[bin]] name = "mage"
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
| `pkg/core` | `mage-core` | agentcore/crates/agent-core | Extension trait + Registry, agent loop, Tool trait, AgentEvent, HookCtx, Disposition |
| `pkg/tui` | `mage-tui` | tau-tui-next | Differential renderer, StyleStack, Markdown (incremental, streaming), Editor (pills), Keymap (packed u64), Overlay |
| `pkg/build` | `mage-build` | metarust | Bundle, Module (@dep parsing), Template trait, Toolchain, Compiler, DependencyResolver, toolchain downloader |
| `pkg/sdk` | `mage` | new | Thin re-export crate for extension authors. Re-exports mage-core Extension trait, Tool trait, types |
| `pkg/bin` | `mage-host` | new | CLI binary. Session management, extension discovery/compilation, interactive/print/RPC modes |
| `providers/anthropic` | `mage-anthropic` | agentcore/crates/anthropic | AnthropicProvider (HTTP/SSE streaming, partial JSON, API types) |

## Dependency Graph

```
refstr  (zero deps)
  ↓
mage-llm  (refstr, serde, serde_json, tokio)
  ↓
mage-core  (mage-llm, refstr, serde, serde_json, tokio)
  │
  ├── mage-anthropic  (mage-llm, refstr, reqwest, bytes, futures-util)
  │
  ├── mage-tui  (crossterm, tokio, futures, pulldown-cmark, unicode-*)
  │
  └── mage-build  (syn, quote, minijinja, sha2, toml, regex, tar, zstd, tempfile, ...)
       │
       └── mage-host  (mage-core, mage-llm, mage-tui, mage-build, mage-anthropic, clap)
            │
            └── mage (sdk)  (re-exports mage-core types)
```

## Extension System

### Static Linking

Extensions are compiled into the host binary. No cdylib, no dlopen, no FFI.

Two forms:

**1. Built-in extensions** — Rust crates in `extensions/`, compiled as features of mage-host:
```toml
# pkg/bin/Cargo.toml
[features]
default = ["ext-tools-coding"]
ext-tools-coding = ["dep:tools-coding"]
```

**2. MetaRust extensions** — Single `.rs` files (or `mod.rs` directories) with `@dep` annotations, compiled on the fly by `mage-build` into a fresh binary that includes the extension.

### Extension as a Single File

```rust
// ~/.mage/extensions/fetch_url.rs
// @dep reqwest = { version = "0.12", features = ["json"] }
use mage::prelude::*;

struct FetchUrlTool;
impl Tool for FetchUrlTool {
    fn name(&self) -> &str { "fetch_url" }
    fn description(&self) -> &str { "Fetch a URL and return its content" }
    fn parameters(&self) -> &serde_json::Value { &FETCH_URL_SCHEMA }

    fn execute(&self, _id: &str, params: serde_json::Value, _cancel: CancelToken) -> ToolExecution {
        let url = params["url"].as_str().unwrap_or("").to_string();
        ToolExecution::running(async move {
            match reqwest::get(&url).await.and_then(|r| r.text().await) {
                Ok(body) => ToolResult::success(body),
                Err(e) => ToolResult::failure(format!("{e}")),
            }
        })
    }
}
pub fn init(registry: &mut Registry) {
    registry.tool(FetchUrlTool);
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
#[path = "modules/my_tool.rs"]
mod my_tool;

fn main() {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    rt.block_on(async {
        // ... wire up agent, register extension, run loop ...
        let mut registry = mage_core::extension::Registry::default();
        my_tool::init(&mut registry);
        // ... start agent with registered tools ...
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
extension.init(registry)        ← register tools/providers
  │
agent_start
  │
for each turn:
  ├─ on_context(messages)       ← chain: hooks may replace message list
  ├─ stream LLM response       ← only real channel (provider HTTP+SSE)
  │    (observe: message_start/delta/end)
  │
  for each tool_call in response:
  │  ├─ on_tool_call(name,args) ← short-circuit: may block
  │  ├─ tool.execute(args)      ← the actual work
  │  ├─ on_tool_result(result)  ← chain: may amend
  │  └─ drain steering queue
  │
  ├─ (observe: turn_end)
  └─ drain follow_up queue → next turn or finish
  │
agent_end
```

Sequential. Deterministic event stream. Tools one at a time.
Only LLM streaming uses a channel.

### Dispatch Semantics

| Mode | Behavior | Hooks |
|---|---|---|
| **Observe** | Call all. No return. | agent_start, turn_end, etc. |
| **Short-circuit** | Call until Block. | tool_call, before_switch/fork |
| **Chain** | Call until Block. Amendments accumulate. | before_start, tool_result |
| **First-wins** | First Block or Value wins. | user_bash, before_compact |

### Tool Model

```rust
pub trait Tool: 'static {
    type State: 'static = String;
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    fn parameters(&self) -> &serde_json::Value;
    fn execute(&self, tool_call_id: &str, params: serde_json::Value, cancel: CancelToken) -> ToolExecution<Self::State>;
}
```

Trait, not data struct. Three execution tiers: sync (return ToolResult directly), async (future, default rendering), custom (future + mailbox + custom rendering). See DESIGN-TOOL-RENDERING.md.

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
    fn stream(
        &self,
        model: Model,
        context: Context,
        options: StreamOptions,
        cancel: CancelToken,
        tx: Sender<AssistantMessageEvent>,
    ) -> Pin<Box<dyn Future<Output = Result<(), ProviderError>>>>;
}
```

Every event carries the full partial `AssistantMessage`. Consumers never maintain their own state.

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

## Extension Points (from pi-mono, via agentcore Extension trait)

| Event | Dispatch | Purpose |
|---|---|---|
| `on_agent_start` | observe | Lifecycle notification |
| `on_agent_end` | observe | Lifecycle notification |
| `on_turn_start` | observe | Turn boundary |
| `on_turn_end` | observe | Turn boundary |
| `on_context` | chain | Replace/prune message list before LLM call |
| `on_message` | observe | LLM message received |
| `on_message_delta` | observe | Streaming token |
| `on_tool_call` | short-circuit | Intercept/block tool call |
| `on_tool_result` | chain | Amend tool result |
| `on_input` | short-circuit | Intercept user input |
| `on_bash` | first-wins | Override bash execution |

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
- [ ] pkg/bin: CLI binary with session management
- [ ] Extension discovery and compilation pipeline (mage-build integration)
- [ ] Built-in tools: read, bash, edit, write, grep, find, ls
- [ ] MageTemplate: Jinja2 template that generates agent binaries with extensions
- [ ] Self-modification loop: agent writes extension → mage-build compiles → exec replaces process
- [ ] Sub-agent spawning
- [ ] OpenAI provider
- [ ] Session persistence (JSONL tree)
- [ ] RPC mode
