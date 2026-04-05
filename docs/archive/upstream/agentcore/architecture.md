# Architecture Spec — agent-core & llm crates

**Status:** Implemented  
**Last verified:** 2026-02-14

---

## 1. Crate Layout

### agent-core (`crates/agent-core/src/`)

| Module         | Description                                                        |
|----------------|--------------------------------------------------------------------|
| `agent.rs`     | `AgentInit`, `AgentBuilder` — clonable recipe and builder for `AgentSession` |
| `agent_loop.rs`| `run()` — the sequential async loop that drives the agent          |
| `command.rs`   | `Command`, `CommandHandler`, `CommandError`, `SessionRegistry`     |
| `entry.rs`     | `SessionHeader`, `FileEntry`, and all entry structs for JSONL persistence |
| `event_stream.rs` | Type aliases for `AgentEventSender` / `AgentEventReceiver`     |
| `extension.rs` | `Extension` trait, `Disposition`, arg structs, `Registry`, `FactoryRegistry` |
| `message.rs`   | `Message`, `MessageBody`, `EntryId` — the agent-level message types |
| `session.rs`   | `AgentSession`, `SessionHandle`, `AgentState`, `InjectQueue`, `Notify` |
| `store.rs`     | `SessionStore`, `SessionContext`, `build_context()` — JSONL file persistence |
| `tool.rs`      | `ToolDef`, `BoxToolFn`, `ToolResult`                               |
| `types.rs`     | `DeliverAs`, `AgentEvent`                                          |

### llm (`crates/llm/src/`)

| Module          | Description                                                     |
|-----------------|-----------------------------------------------------------------|
| `cancel.rs`     | `CancelToken` — cooperative single-threaded cancellation        |
| `channel.rs`    | Single-threaded async channel (`Sender`, `Receiver`)            |
| `event.rs`      | `AssistantMessageEvent` enum and `apply_event()` accumulator    |
| `event_stream.rs` | Generic `EventStreamSender<T,R>` / `EventStreamReceiver<T,R>` |
| `json_accum.rs` | Streaming JSON accumulator for tool call arguments              |
| `provider.rs`   | `Provider` trait, `StreamRequest`, `StreamHandle`, `ProviderError` |
| `types.rs`      | `Message`, `Model`, `Context`, `Tool`, `StreamOptions`, content types |

Re-exports from `llm/src/lib.rs`:

```rust
pub use cancel::CancelToken;
pub use event::AssistantMessageEvent;
pub use provider::{Provider, ProviderError, StreamHandle, StreamRequest};
pub use types::*;
```

---

## 2. Message Types

### 2.1 `llm::Message` — LLM wire format (3 variants)

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "role", rename_all = "camelCase")]
pub enum Message {
    #[serde(rename = "user")]
    User(UserMessage),
    #[serde(rename = "assistant")]
    Assistant(AssistantMessage),
    #[serde(rename = "toolResult")]
    ToolResult(ToolResultMessage),
}
```

Supporting structs:

```rust
pub struct UserMessage {
    pub content: UserMessageContent,
    pub timestamp: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum UserMessageContent {
    Text(String),
    Rich(Vec<UserContent>),
}

pub struct AssistantMessage {
    pub content: Vec<ContentBlock>,
    pub api: LocalStr,
    pub provider: LocalStr,
    pub model: LocalStr,
    pub usage: Usage,
    pub stop_reason: StopReason,
    pub error_message: Option<LocalStr>,  // skip_serializing_if = "Option::is_none"
    pub timestamp: u64,
}

pub struct ToolResultMessage {
    pub tool_call_id: LocalStr,
    pub tool_name: LocalStr,
    pub content: Vec<UserContent>,
    pub details: Option<serde_json::Value>,  // skip_serializing_if = "Option::is_none"
    pub is_error: bool,
    pub timestamp: u64,
}
```

Content blocks:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum ContentBlock {
    #[serde(rename = "text")]
    Text { text: String, text_signature: Option<LocalStr> },
    #[serde(rename = "thinking")]
    Thinking { thinking: String, thinking_signature: Option<LocalStr> },
    #[serde(rename = "toolCall")]
    ToolCall { id: LocalStr, name: LocalStr, arguments: serde_json::Value, thought_signature: Option<LocalStr> },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum UserContent {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "image")]
    Image { data: String, mime_type: LocalStr },
}
```

### 2.2 `agent_core::Message` — agent-level message (struct + `MessageBody` enum, 6 variants)

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Message {
    #[serde(flatten)]
    pub body: MessageBody,
    pub timestamp: u64,
    #[serde(default, skip_serializing_if = "is_false")]
    pub ephemeral: bool,
}
```

**Serde caveat:** `#[serde(flatten)]` on `body` prevents `skip_serializing_if = "is_false"` from working on `ephemeral` — the field always serializes (known serde bug with flattened adjacently-tagged enums). `#[serde(default)]` handles deserialization of old data missing the field.

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "role", rename_all = "camelCase")]
pub enum MessageBody {
    #[serde(rename = "user")]
    User { content: llm::UserMessageContent },

    #[serde(rename = "assistant")]
    Assistant {
        content: Vec<llm::ContentBlock>,
        api: LocalStr,
        provider: LocalStr,
        model: LocalStr,
        usage: llm::Usage,
        stop_reason: llm::StopReason,
        error_message: Option<LocalStr>,  // skip_serializing_if = "Option::is_none"
    },

    #[serde(rename = "toolResult")]
    ToolResult {
        tool_call_id: LocalStr,
        tool_name: LocalStr,
        content: Vec<llm::UserContent>,
        details: Option<Value>,           // skip_serializing_if = "Option::is_none"
        is_error: bool,
    },

    #[serde(rename = "compactionSummary")]
    CompactionSummary { summary: String, tokens_before: u64 },

    #[serde(rename = "branchSummary")]
    BranchSummary { summary: String, from_id: EntryId },

    #[serde(rename = "custom")]
    Custom {
        custom_type: LocalStr,
        content: llm::UserMessageContent,
        display: bool,
        details: Option<Value>,           // skip_serializing_if = "Option::is_none"
    },
}
```

### 2.3 `EntryId`

```rust
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct EntryId(LocalStr);

impl EntryId {
    pub fn generate(existing: impl Fn(&EntryId) -> bool) -> Self;
    pub fn as_str(&self) -> &str;
}
// Also: Display, From<&str>, Deref<Target = str>
```

### 2.4 `Message` constructors and methods

```rust
impl Message {
    pub fn to_llm(&self) -> llm::Message;
    pub fn from_assistant(a: llm::AssistantMessage) -> Self;
    pub fn from_tool_result(tool_call_id: LocalStr, tool_name: LocalStr,
        content: Vec<llm::UserContent>, details: Option<Value>, is_error: bool) -> Self;
    pub fn user_text(text: impl Into<String>) -> Self;
    pub fn ephemeral_user_text(text: impl Into<String>) -> Self;
    pub fn compaction_summary(summary: impl Into<String>, tokens_before: u64) -> Self;
    pub fn branch_summary(summary: impl Into<String>, from_id: EntryId) -> Self;
    pub fn custom(custom_type: impl Into<LocalStr>, content: llm::UserMessageContent,
        display: bool, details: Option<Value>) -> Self;
    pub fn is_ephemeral(&self) -> bool;
    pub fn role_name(&self) -> &'static str;
}
```

`to_llm()` converts each variant to `llm::Message`:
- `User` → `llm::Message::User`
- `Assistant` → `llm::Message::Assistant`
- `ToolResult` → `llm::Message::ToolResult`
- `CompactionSummary` → `llm::Message::User` (wraps summary in `<summary>` tags)
- `BranchSummary` → `llm::Message::User` (wraps summary in `<summary>` tags)
- `Custom` → `llm::Message::User`

---

## 3. AgentState

```rust
pub struct AgentState {
    pub system_prompt: String,
    pub model: llm::Model,
    pub messages: Vec<Message>,
    pub tools: Vec<ToolDef>,
    pub options: llm::StreamOptions,
}

impl AgentState {
    pub fn llm_tools(&self) -> Vec<llm::Tool>;
}
```

---

## 4. Extension System

### 4.1 `Disposition<T>`

```rust
#[derive(Debug, Default)]
pub enum Disposition<T = ()> {
    #[default]
    Propagate,
    Block { reason: LocalStr },
    Value(T),
}

impl<T> Disposition<T> {
    pub fn is_block(&self) -> bool;
}
```

### 4.2 Amendment types

```rust
pub struct ToolResultAmend {
    pub content: Option<Vec<UserContent>>,
    pub details: Option<serde_json::Value>,
    pub is_error: Option<bool>,
}

pub struct ContextAmend {
    pub messages: Vec<Message>,
}
```

### 4.3 Arg structs

```rust
pub struct ToolCallArgs<'a>     { pub name: &'a str, pub id: &'a str, pub args: &'a serde_json::Value }
pub struct ToolResultArgs<'a>   { pub name: &'a str, pub id: &'a str, pub result: &'a ToolResult, pub is_error: bool }
pub struct TurnEndArgs<'a>      { pub message: &'a Message, pub tool_results: &'a [llm::ToolResultMessage] }
pub struct MessageArgs<'a>      { pub message: &'a Message }
pub struct MessageDeltaArgs<'a> { pub event: &'a AssistantMessageEvent }
pub struct ToolExecStartArgs<'a>{ pub name: &'a str, pub args: &'a serde_json::Value }
pub struct ToolExecEndArgs<'a>  { pub name: &'a str, pub result: &'a ToolResult, pub is_error: bool }
pub struct AgentEndArgs<'a>     { pub messages: &'a [Message] }
pub struct ModelSelectArgs<'a>  { pub model: &'a Model }
pub struct CompactionArgs<'a>   { pub summary: &'a str, pub first_kept_entry_id: &'a EntryId, pub tokens_before: u64, pub details: Option<&'a serde_json::Value> }
pub struct BranchChangeArgs<'a> { pub from_id: &'a EntryId, pub summary: Option<&'a str>, pub details: Option<&'a serde_json::Value> }
```

### 4.4 `Registry`

Passed to `Extension::init()`. Extensions register tools and providers here.

```rust
pub struct Registry<'a> {
    pub(crate) tools: &'a mut Vec<ToolDef>,
    pub(crate) providers: &'a mut Vec<(LocalStr, Rc<dyn llm::Provider>)>,
}

impl<'a> Registry<'a> {
    pub fn tool(&mut self, tool: ToolDef);
    pub fn provider(&mut self, api: impl Into<LocalStr>, provider: impl llm::Provider + 'static);
}
```

### 4.5 `Extension` trait

```rust
pub type HookFuture<'a, T> = Pin<Box<dyn Future<Output = T> + 'a>>;

pub trait Extension {
    // --- Init (takes &mut Registry, NOT &mut AgentSession) ---
    fn init(&mut self, registry: &mut Registry) {}

    // --- Observe hooks (sync, fire-and-forget) ---
    fn on_agent_start(&mut self, session: &mut AgentSession) {}
    fn on_agent_end(&mut self, args: &AgentEndArgs, session: &mut AgentSession) {}
    fn on_turn_start(&mut self, session: &mut AgentSession) {}
    fn on_turn_end(&mut self, args: &TurnEndArgs, session: &mut AgentSession) {}
    fn on_model_select(&mut self, args: &ModelSelectArgs, session: &mut AgentSession) {}
    fn on_message_start(&mut self, args: &MessageArgs, session: &mut AgentSession) {}
    fn on_message_delta(&mut self, args: &MessageDeltaArgs, session: &mut AgentSession) {}
    fn on_message_end(&mut self, args: &MessageArgs, session: &mut AgentSession) {}
    fn on_tool_exec_start(&mut self, args: &ToolExecStartArgs, session: &mut AgentSession) {}
    fn on_tool_exec_end(&mut self, args: &ToolExecEndArgs, session: &mut AgentSession) {}
    fn on_compaction(&mut self, args: &CompactionArgs, session: &mut AgentSession) {}
    fn on_branch_change(&mut self, args: &BranchChangeArgs, session: &mut AgentSession) {}

    // --- Decision hooks (async, return Disposition<T>) ---
    fn on_tool_call<'a>(&'a mut self, args: &'a ToolCallArgs<'a>, session: &'a mut AgentSession)
        -> HookFuture<'a, Disposition> { Box::pin(async { Disposition::Propagate }) }

    fn on_tool_result<'a>(&'a mut self, args: &'a ToolResultArgs<'a>, session: &'a mut AgentSession)
        -> HookFuture<'a, Disposition<ToolResultAmend>> { Box::pin(async { Disposition::Propagate }) }

    fn on_context<'a>(&'a mut self, messages: &'a [Message], session: &'a mut AgentSession)
        -> HookFuture<'a, Disposition<ContextAmend>> { Box::pin(async { Disposition::Propagate }) }

    // --- Session-level init (registers commands) ---
    fn session_init(&mut self, registry: &mut SessionRegistry) {}
}
```

**Key:** `init()` takes `&mut Registry`, not `&mut AgentSession`. Only observe/decision hooks receive `&mut AgentSession`.

### 4.6 `mem::take` borrow pattern

The agent loop cannot hold `&mut session.exts` while also passing `&mut session` to hooks.
Solution: temporarily remove extensions with `std::mem::take`, iterate, then put them back:

```rust
let mut exts = std::mem::take(&mut session.exts);
for ext in exts.iter_mut() {
    ext.on_turn_start(session);
}
session.exts = exts;
```

### 4.7 `ExtensionFactory` and `FactoryRegistry`

```rust
pub type ExtensionFactory = Rc<dyn Fn() -> Box<dyn Extension>>;

pub struct FactoryRegistry {
    factories: Vec<ExtensionFactory>,
}

impl FactoryRegistry {
    pub const fn new() -> Self;
    pub fn register(&mut self, factory: impl Fn() -> Box<dyn Extension> + 'static);
    pub fn create_all(&self) -> Vec<Box<dyn Extension>>;
    pub fn clone_factories(&self) -> Vec<ExtensionFactory>;
    pub fn len(&self) -> usize;
    pub fn is_empty(&self) -> bool;
}
```

---

## 5. AgentSession and SessionHandle

### 5.1 `InjectQueue`

```rust
pub type InjectQueue = Rc<RefCell<VecDeque<(Message, DeliverAs)>>>;
```

### 5.2 `Notify`

Minimal single-threaded notification primitive. `notified()` returns a future that completes when `notify()` is called.

```rust
pub struct Notify {
    waker: RefCell<Option<std::task::Waker>>,
}

impl Notify {
    pub fn new() -> Self;
    pub fn notify(&self);
    pub fn notified(&self) -> NotifyFuture<'_>;
}
```

### 5.3 `SessionHandle`

Cheap clone handle for async code running outside the loop. Can inject messages and abort, but cannot access state, store, tools, or extensions.

```rust
#[derive(Clone)]
pub struct SessionHandle {
    inject: InjectQueue,
    cancel: CancelToken,
    idle_notify: Rc<Notify>,
    running: Rc<Cell<bool>>,
}

impl SessionHandle {
    pub fn inject(&self, msg: Message, deliver: DeliverAs);
    pub fn abort(&self);
    pub fn is_idle(&self) -> bool;
    pub async fn wait_for_idle(&self);
}
```

### 5.4 `AgentSession`

```rust
pub struct AgentSession {
    pub state: AgentState,
    pub exts: Vec<Box<dyn crate::extension::Extension>>,

    // TODO: pub store: Option<SessionStore>,  -- added in Phase 5

    commands: HashMap<LocalStr, Command>,

    pub inject: InjectQueue,
    pub cancel: CancelToken,
    pub(crate) idle_notify: Rc<Notify>,
    pub(crate) running: Rc<Cell<bool>>,
}
```

**Visibility:** `inject` and `cancel` are `pub`. `idle_notify` and `running` are `pub(crate)`.

**Methods:**

```rust
impl AgentSession {
    pub fn from_parts(state: AgentState, exts: Vec<Box<dyn Extension>>) -> Self;
    pub fn handle(&self) -> SessionHandle;
    pub fn register_command(&mut self, cmd: Command);
    pub async fn execute_command(&mut self, name: &str, args: &str) -> Result<(), CommandError>;
    pub fn init_commands(&mut self);
    pub fn set_running(&self, val: bool);

    // --- Convenience API (moved from former Agent struct) ---
    pub async fn prompt(&mut self, text: &str) -> Result<AgentEventReceiver, LoopError>;
    pub fn abort(&self);
    pub fn steer(&mut self, text: &str);
    pub fn messages(&self) -> &[Message];
    pub fn model(&self) -> &llm::Model;
}
```

**Not on AgentSession:** `new()`, `load()`, `resume()`, `rebuild_context()` — not yet implemented.

---

## 6. Commands

```rust
pub type CommandHandler = Rc<dyn Fn(&str, SessionHandle) -> Pin<Box<dyn Future<Output = Result<(), CommandError>>>>>;

pub struct Command {
    pub name: LocalStr,
    pub description: Option<LocalStr>,
    pub handler: CommandHandler,
}

#[derive(Debug)]
pub enum CommandError {
    NotFound(LocalStr),
    NotIdle,
    Other(String),
}

pub struct SessionRegistry<'a> {
    pub(crate) commands: &'a mut HashMap<LocalStr, Command>,
}

impl<'a> SessionRegistry<'a> {
    pub fn command(&mut self, cmd: Command);
}
```

`execute_command()` returns `Result<(), CommandError>` — not `Result<bool, CommandError>`.

---

## 7. Agent Loop

### 7.1 Entry point

```rust
pub async fn run(
    session: &mut AgentSession,
    events: &AgentEventSender,
) -> Result<(), LoopError>
```

### 7.2 `LoopError`

```rust
#[derive(Debug)]
pub enum LoopError {
    Provider(llm::ProviderError),
    NoProvider(String),
    Cancelled,
}
```

### 7.3 Flow

1. **Extension init:** Temporarily removes extensions via `mem::take`, creates a `Registry`, calls `ext.init(&mut registry)` for each — registers tools and providers.
2. **Provider resolution:** Finds the provider matching `session.state.model.api` from the registered providers. Returns `LoopError::NoProvider` if none match.
3. **Agent start:** Fires `AgentEvent::AgentStart`, calls `on_agent_start` hooks.
4. **Turn loop:**
   - `on_turn_start` hooks
   - `on_context` decision hooks (may replace message list via `ContextAmend`)
   - **LLM streaming:** `StreamRequest` → `Provider::stream()` → read `AssistantMessageEvent`s → fire `on_message_start`, `on_message_delta`, `on_message_end` hooks. Retries up to 3 times with exponential backoff for retryable errors.
   - Add assistant message to `session.state.messages`
   - **Tool execution:** For each `ToolCall` in the response:
     - `on_tool_call` decision hook (can `Block`)
     - Find tool in `session.state.tools` by name
     - `on_tool_exec_start` observe hook
     - Execute tool closure
     - `on_tool_result` decision hook (can amend content/details/is_error)
     - `on_tool_exec_end` observe hook
   - `on_turn_end` hooks
   - Add tool result messages to conversation
   - If stop reason is not `ToolUse`: drain follow-up queue or break
5. **Agent end:** Fires `AgentEvent::AgentEnd`, calls `on_agent_end` hooks.

The inject queue is drained after every hook dispatch point, routing messages to either `steering_queue` (interrupts tool execution) or `follow_up_queue` (appended after turn).

---

## 8. AgentBuilder and AgentInit

`AgentSession` is the primary runtime object. `AgentBuilder` and `AgentInit` are construction helpers.

### 8.1 `AgentInit`

Clonable recipe for spawning sessions. Each `spawn()` produces a fresh `AgentSession` with fresh extension instances, empty history, and own cancel token.

```rust
#[derive(Clone)]
pub struct AgentInit {
    pub model: Model,
    pub system_prompt: String,
    pub options: StreamOptions,
    ext_factories: Rc<Vec<ExtensionFactory>>,
}
impl AgentInit {
    pub fn spawn(&self) -> AgentSession;
    pub fn spawn_with(&self, f: impl FnOnce(&mut AgentInit)) -> AgentSession;
}
```

### 8.2 `AgentBuilder`

```rust
pub struct AgentBuilder {
    system_prompt: String,
    model: Model,
    exts: Vec<Box<dyn Extension>>,
    ext_factories: Vec<ExtensionFactory>,
    options: StreamOptions,
}
impl AgentBuilder {
    pub fn new(model: Model) -> Self;
    pub fn system_prompt(mut self, prompt: impl Into<String>) -> Self;
    pub fn ext(mut self, ext: impl Extension + 'static) -> Self;
    pub fn ext_factory(mut self, f: impl Fn() -> Box<dyn Extension> + 'static) -> Self;
    pub fn ext_from_registry(mut self, registry: &FactoryRegistry) -> Self;
    pub fn options(mut self, options: StreamOptions) -> Self;
    pub fn into_init(self) -> AgentInit;
    pub fn build(self) -> AgentSession;
}
```

`.build()` is shorthand for `.into_init().spawn()`.

Direct `.ext()` instances are consumed by the first spawn. For sub-agent support, use `.ext_factory()` — it creates fresh instances per spawn. `.build()` is convenience for `into_init().spawn()`.

---

## 9. Tool System

```rust
pub type BoxToolFn = Box<
    dyn Fn(
        /* tool_call_id */ &str,
        /* params */       serde_json::Value,
        /* cancel */       CancelToken,
    ) -> Pin<Box<dyn Future<Output = ToolResult> + '_>>,
>;

pub struct ToolDef {
    pub name: LocalStr,
    pub label: LocalStr,
    pub description: LocalStr,
    pub parameters: serde_json::Value,
    pub execute: BoxToolFn,
}

impl ToolDef {
    pub fn to_llm_tool(&self) -> llm::Tool;
}

#[derive(Debug, Clone)]
pub struct ToolResult {
    pub content: Vec<llm::UserContent>,
    pub details: serde_json::Value,
}
```

Tools are `Fn` (not `FnOnce`) — the same closure handles every invocation. Each call returns a fresh `Pin<Box<Future>>`. For shared state, the closure captures `Rc<RefCell<>>`.

---

## 10. Events

### 10.1 `AgentEvent`

```rust
#[derive(Debug, Clone)]
pub enum AgentEvent {
    AgentStart,
    AgentEnd { messages: Vec<Message> },
    TurnStart,
    TurnEnd { message: Message, tool_results: Vec<ToolResultMessage> },
    MessageStart { message: Message },
    MessageUpdate { message: Message, assistant_message_event: llm::AssistantMessageEvent },
    MessageEnd { message: Message },
    ToolExecutionStart { tool_call_id: LocalStr, tool_name: LocalStr, args: serde_json::Value },
    ToolExecutionUpdate { tool_call_id: LocalStr, tool_name: LocalStr, args: serde_json::Value, partial_result: ToolResult },
    ToolExecutionEnd { tool_call_id: LocalStr, tool_name: LocalStr, result: ToolResult, is_error: bool },
}

impl AgentEvent {
    pub fn is_terminal(&self) -> bool; // true for AgentEnd
}
```

### 10.2 `DeliverAs`

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeliverAs {
    Steer,     // Interrupt current tool execution with a steering message
    FollowUp,  // Queue for after current turn completes
    NextTurn,  // Queue for the next user-initiated turn
}
```

### 10.3 Event stream type aliases

```rust
pub type AgentEventSender = event_stream::EventStreamSender<AgentEvent, Vec<Message>>;
pub type AgentEventReceiver = event_stream::EventStreamReceiver<AgentEvent, Vec<Message>>;

pub fn new_agent_stream() -> (AgentEventSender, AgentEventReceiver);
```

---

## 11. LLM Crate Types

### 11.1 `Provider` trait

```rust
pub trait Provider {
    fn stream(&self, req: StreamRequest) -> StreamHandle;
}
```

### 11.2 `StreamRequest` / `StreamHandle`

```rust
pub struct StreamRequest {
    pub model: Model,
    pub context: Context,
    pub options: StreamOptions,
    pub cancel: CancelToken,
}

pub struct StreamHandle {
    pub events: Receiver<AssistantMessageEvent>,
    pub task: Pin<Box<dyn Future<Output = Result<(), ProviderError>>>>,
}
```

### 11.3 `ProviderError`

```rust
pub enum ProviderError {
    Http(String),
    Cancelled,
    Api { status: u16, body: String },
    RateLimited { retry_after_ms: Option<u64> },
    Other(String),
}
```

### 11.4 `Model`

```rust
pub struct Model {
    pub id: LocalStr,
    pub name: LocalStr,
    pub api: LocalStr,
    pub provider: LocalStr,
    pub base_url: LocalStr,
    pub reasoning: bool,
    pub input: Vec<InputModality>,
    pub cost: ModelCost,
    pub context_window: u64,
    pub max_tokens: u64,
    pub headers: Option<Vec<(LocalStr, LocalStr)>>,  // skip_serializing_if = "Option::is_none"
}
```

### 11.5 `Context` / `Tool`

```rust
pub struct Context {
    pub system_prompt: Option<LocalStr>,
    pub messages: Vec<Message>,
    pub tools: Option<Vec<Tool>>,
}

pub struct Tool {
    pub name: LocalStr,
    pub description: LocalStr,
    pub parameters: serde_json::Value,
}
```

### 11.6 `StreamOptions`

```rust
#[derive(Debug, Clone, Default)]
pub struct StreamOptions {
    pub temperature: Option<f64>,
    pub max_tokens: Option<u64>,
    pub api_key: Option<LocalStr>,
    pub cache_retention: Option<CacheRetention>,
    pub session_id: Option<LocalStr>,
    pub headers: Option<Vec<(LocalStr, LocalStr)>>,
    pub max_retry_delay_ms: Option<u64>,
    pub reasoning: Option<ThinkingLevel>,
}
```

### 11.7 `ThinkingLevel`

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ThinkingLevel {
    #[default]
    Off,
    Minimal,
    Low,
    Medium,
    High,
    #[serde(rename = "xhigh")]
    XHigh,
}
```

### 11.8 `AssistantMessageEvent`

```rust
#[derive(Debug, Clone)]
pub enum AssistantMessageEvent {
    Start,
    TextStart { content_index: usize },
    TextDelta { content_index: usize, delta: LocalStr },
    TextEnd { content_index: usize },
    ThinkingStart { content_index: usize },
    ThinkingDelta { content_index: usize, delta: LocalStr },
    ThinkingEnd { content_index: usize, signature: Option<LocalStr> },
    ToolCallStart { content_index: usize, id: LocalStr, name: LocalStr },
    ToolCallDelta { content_index: usize, delta: LocalStr },
    ToolCallEnd { content_index: usize, arguments: serde_json::Value },
    Done { reason: StopReason },
    Error { reason: StopReason, error: Option<LocalStr> },
    Usage { usage: Usage },
}

impl AssistantMessageEvent {
    pub fn is_terminal(&self) -> bool; // Done | Error
}

pub fn apply_event(msg: &mut AssistantMessage, event: &AssistantMessageEvent);
```

### 11.9 `CancelToken`

```rust
#[derive(Debug, Clone)]
pub struct CancelToken {
    cancelled: Rc<Cell<bool>>,
}

impl CancelToken {
    pub fn new() -> Self;
    pub fn cancel(&self);
    pub fn is_cancelled(&self) -> bool;
}
```

### 11.10 `EventStreamSender<T, R>` / `EventStreamReceiver<T, R>`

Generic event stream with a final result extracted from a terminal event.

```rust
pub struct EventStreamSender<T, R> { /* tx, result, is_complete, extract_result */ }
pub struct EventStreamReceiver<T, R> { /* rx, result */ }

impl<T, R> EventStreamSender<T, R> {
    pub fn push(&self, event: T);
    pub fn end(self, result: R);
}

impl<T, R: Clone> EventStreamReceiver<T, R> {
    pub async fn recv(&mut self) -> Option<T>;
    pub fn result(&self) -> Option<R>;
}

pub fn event_stream<T, R>(
    is_complete: fn(&T) -> bool,
    extract_result: fn(&T) -> R,
) -> (EventStreamSender<T, R>, EventStreamReceiver<T, R>);
```

### 11.11 Other types

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum StopReason { Stop, Length, ToolUse, Error, Aborted }

pub struct Usage {
    pub input: u64, pub output: u64,
    pub cache_read: u64, pub cache_write: u64,
    pub total_tokens: u64, pub cost: Cost,
}

pub struct Cost {
    pub input: f64, pub output: f64,
    pub cache_read: f64, pub cache_write: f64,
    pub total: f64,
}

pub struct ModelCost { pub input: f64, pub output: f64, pub cache_read: f64, pub cache_write: f64 }

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[serde(rename_all = "kebab-case")]
pub enum InputModality { Text, Image }

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum CacheRetention { None, Short, Long }
```

---

## 12. Future Work

The following items are planned but **not yet implemented**:

- **Two-trait async pattern** (`Extension`/`DynExtension`, `Provider`/`DynProvider`) — for more ergonomic async trait dispatch
- **`store: Option<SessionStore>`** field on `AgentSession` — currently a TODO comment, to be wired in Phase 5
- **`AgentSession::resume()`**, `rebuild_context()`, `new()`, `load()` — not yet implemented
- **Anthropic as pure extension** — moving the Anthropic provider to be registered entirely via the extension system
