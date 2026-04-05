# Agent Core Design

## Principles

1. A constant > a function > an object. Minimize abstraction weight.
2. `&mut` over `Rc<RefCell<>>`. Split borrows over shared mutability.
3. Sequential await over channels. Channels only where true concurrency exists.
4. No trait when a struct suffices. No struct when a closure suffices.
5. Single-threaded (`current_thread` tokio). `Rc` over `Arc`. `Cell` over `Atomic`.

## Extension lifecycle

### Two-phase init

**Global** (process startup):
```rust
// Register a factory that creates fresh extension instances
let factory: ExtensionFactory = Box::new(|| Box::new(MyExtension::new()));
global_registry.register(factory);
```

**Per-agent** (each agent loop / sub-agent gets its own instance):
```rust
impl Extension for MyExtension {
    fn init(&mut self, registry: &mut Registry) {
        // Register tools, providers — whatever this extension provides
        registry.tool(ToolDef { ... });
    }
}
```

Each agent loop creates extensions from factories, calls `init`, then runs.
Sub-agents get their own extension instances — no shared mutable state
between agents unless the extension explicitly creates it internally.

### State ownership

Extensions own their state via `&mut self`. The framework doesn't dictate
sharing. If an extension needs shared state across its tools (e.g. a
database connection pool), it manages that internally:

```rust
struct MyExtension {
    db: Rc<RefCell<DbConn>>,
}

impl Extension for MyExtension {
    fn init(&mut self, registry: &mut Registry) {
        let db = self.db.clone();
        registry.tool(ToolDef {
            name: "query".into(),
            execute: Box::new(move |id, params, cancel| {
                let db = db.clone();
                Box::pin(async move { /* use db */ })
            }),
            ..
        });
    }
}
```

## Agent loop flow

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

Everything sequential. Only LLM streaming uses a channel.
## Tool execution: sequential, not parallel

Tools execute one at a time, in order. This is a deliberate design choice:

1. **Shared mutable state.** Extensions and tools share `&mut AgentState` through the
   loop. Parallel execution would require `Rc<RefCell<>>` everywhere, violating
   principle #2.

2. **Hook ordering guarantees.** `on_tool_call` → execute → `on_tool_result` fire in
   sequence. Extensions that track tool state (e.g. counting calls, enforcing rate
   limits) depend on this. Parallel execution breaks causal ordering.

3. **Steering.** After each tool completes, the loop drains the steering queue.
   A hook that observes tool A's result can inject a steering message before tool B
   runs. Parallel execution eliminates this interception point.

4. **Debuggability.** Sequential execution produces a deterministic event stream.
   Tool A's result appears before tool B's start — always. No interleaving, no races.

5. **Matches the source.** pi-mono uses `for (const call of toolCalls) { await ... }`.
   Sequential by design, not by accident.

The cost is wall-clock time when multiple independent tools could overlap. This is
acceptable: tool execution is I/O-bound, and the LLM turn (which dominates latency)
is already concurrent via channel. The simplicity payoff is worth the marginal
throughput loss.

## Dispatch semantics

| Mode             | Behavior                                 | Hooks                          |
|------------------|------------------------------------------|--------------------------------|
| **Observe**      | Call all. No return. Sync.               | agent_start, turn_end, etc.    |
| **Short-circuit**| Call until Block.                        | tool_call, before_switch/fork  |
| **Chain**        | Call until Block. Amendments accumulate. | before_start, tool_result      |
| **First-wins**   | First Block or Value wins.               | user_bash, before_compact      |

Implemented as simple loops in agent_loop.rs, not abstracted.

## Tool model

Tools are `ToolDef` — data (name/description/parameters) + a closure (execute).
Not a trait. The closure captures whatever state the extension gave it.

```rust
pub struct ToolDef {
    pub name: LocalStr,
    pub label: LocalStr,
    pub description: LocalStr,
    pub parameters: serde_json::Value,
    pub execute: BoxToolFn,
}
```

One heap alloc per invocation (the returned future). Acceptable: tools do I/O.

## HookCtx — split borrows

```rust
pub struct HookCtx<'a> {
    pub model: &'a Model,
    pub system_prompt: &'a str,
    outbox: &'a mut Vec<(AgentMessage, DeliverAs)>,
    cancel: &'a CancelToken,
}
```

The loop owns the outbox Vec. Before calling a hook, it creates HookCtx
with split borrows. After the hook returns, the loop drains the outbox
into steering/follow-up queues.

## Crate structure

```
crates/
  revstr/            — LocalStr (ref-counted string), 10 tests
  llm/              — LLM types, events, channel, cancel, provider trait
  agent-core/
    extension.rs    — Extension trait, Registry, FactoryRegistry, HookCtx, Disposition
    tool.rs         — ToolDef, BoxToolFn, ToolResult
    types.rs        — AgentMessage, AgentEvent, AgentState, DeliverAs
    agent_loop.rs   — agent loop with extension dispatch, 21 tests
    agent.rs        — Agent wrapper + AgentBuilder (with provider wiring)
    event_stream.rs — event stream type aliases
  anthropic/
    sse.rs          — SSE line parser (7 tests)
    json_accum.rs   — partial JSON accumulator (6 tests)
    events.rs       — Anthropic SSE → AssistantMessageEvent mapper (5 tests)
    convert.rs      — llm::Context → Anthropic API request body (2 tests)
    provider.rs     — AnthropicProvider (impl llm::Provider, reqwest+SSE)
```

## Done

- [x] `apply_event` fix: ToolCallStart carries id/name, ToolCallEnd carries arguments
- [x] Provider wiring in AgentBuilder (resolves by model.api, falls back to stream_fn)
- [x] FactoryRegistry for process-level extension factory storage
- [x] Anthropic provider (SSE parsing, message conversion, real HTTP streaming)
- [x] Tool execution through-loop integration tests
- [x] Typed Anthropic API request structs (api_types.rs, convert.rs rewrite)
- [x] Retry logic with exponential backoff (rate limiting, transient errors)
- [x] Thinking signature accumulation in event mapper
- [x] Usage/cost tracking (Usage event, compute_cost, EventMapper→AssistantMessage)

## Next
- Sub-agent spawning (Agent::spawn_sub with fresh extensions)