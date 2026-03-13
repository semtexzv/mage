//! Extension system.
//!
//! Extensions hook into agent lifecycle events, register tools and providers,
//! and interact with the loop via [`ExtensionContext`] and [`LoopHandle`].
//!
//! ## Dual access pattern
//!
//! [`ExtensionContext`] provides direct mutable references for synchronous work
//! during a callback. For async work that outlives the callback (e.g. a network
//! call that later injects a message), extract a [`LoopHandle`] — it is `Clone`
//! and communicates back over an internal channel.
//!
//! ## Hook dispatch
//!
//! All hooks are `async` and receive `&mut ExtensionContext<'_>`. Hooks that
//! participate in decisions return `Option<ResultStruct>` — `None` means "no
//! opinion", `Some(result)` means "I want to modify this". Extensions are
//! iterated in order; each sees the output of the previous.

use std::collections::VecDeque;
use std::future::Future;
use std::pin::Pin;
use std::rc::Rc;

use async_trait::async_trait;
use refstr::Str;

use llm::{CancelToken, Model};

use crate::types::{Message, ToolResult, ToolUpdate};

// ---------------------------------------------------------------------------
// LoopCommand / LoopHandle — command channel into the loop
// ---------------------------------------------------------------------------

/// Commands sent to the agent loop via [`LoopHandle`].
#[derive(Debug)]
pub enum LoopCommand {
    InjectMessage(Message),
    SteerMessage(Message),
    FollowUpMessage(Message),
    Abort,
    Shutdown,
    SetModel(Model),
}

/// Lightweight, cloneable handle to the agent loop.
///
/// Use to send commands from extension callbacks or spawned tasks.
/// Methods are fire-and-forget (silently drop if the loop has shut down).
#[derive(Clone)]
pub struct LoopHandle {
    tx: llm::channel::Sender<LoopCommand>,
}

impl LoopHandle {
    pub fn inject(&self, msg: Message) {
        let _ = self.tx.send(LoopCommand::InjectMessage(msg));
    }
    pub fn steer(&self, msg: Message) {
        let _ = self.tx.send(LoopCommand::SteerMessage(msg));
    }
    pub fn follow_up(&self, msg: Message) {
        let _ = self.tx.send(LoopCommand::FollowUpMessage(msg));
    }
    pub fn abort(&self) {
        let _ = self.tx.send(LoopCommand::Abort);
    }
    pub fn shutdown(&self) {
        let _ = self.tx.send(LoopCommand::Shutdown);
    }
    pub fn set_model(&self, model: Model) {
        let _ = self.tx.send(LoopCommand::SetModel(model));
    }
}

pub fn loop_handle_pair() -> (LoopHandle, llm::channel::Receiver<LoopCommand>) {
    let (tx, rx) = llm::channel::channel();
    (LoopHandle { tx }, rx)
}

// ---------------------------------------------------------------------------
// ToolHandle — per-tool-call handle for cancellation and progress
// ---------------------------------------------------------------------------

/// Handle given to a tool during execution.
///
/// Provides cancellation awareness, progress streaming, and loop access.
/// Created per tool call; updates are drained by the loop's `execute_tool`.
pub struct ToolHandle {
    pub tool_call_id: String,
    cancel: CancelToken,
    update_tx: tokio::sync::mpsc::UnboundedSender<ToolUpdate>,
    loop_handle: LoopHandle,
}

impl ToolHandle {
    pub(crate) fn new(
        tool_call_id: String,
        cancel: CancelToken,
        update_tx: tokio::sync::mpsc::UnboundedSender<ToolUpdate>,
        loop_handle: LoopHandle,
    ) -> Self {
        Self {
            tool_call_id,
            cancel,
            update_tx,
            loop_handle,
        }
    }

    /// Sync cancellation check — call at yield points in long-running tools.
    pub fn is_cancelled(&self) -> bool {
        self.cancel.is_cancelled()
    }

    /// The per-tool cancellation token, if the tool needs to pass it deeper.
    pub fn cancel_token(&self) -> &CancelToken {
        &self.cancel
    }

    /// Send a progress update to the UI. Fire-and-forget (unbounded).
    pub fn send_update(&self, update: ToolUpdate) {
        let _ = self.update_tx.send(update);
    }

    /// Access the loop handle for injecting messages, steering, etc.
    pub fn loop_handle(&self) -> &LoopHandle {
        &self.loop_handle
    }
}

// ---------------------------------------------------------------------------
// RegisteredTool — closure-based tool registration
// ---------------------------------------------------------------------------

/// A tool registered by an extension during init.
///
/// Instead of a trait, tools are closures:
/// `Fn(call_id, args, handle) -> Future<Output = ToolResult>`
pub struct RegisteredTool {
    pub schema: llm::Tool,
    pub execute: Box<
        dyn Fn(String, serde_json::Value, ToolHandle) -> Pin<Box<dyn Future<Output = ToolResult>>>,
    >,
}

// ---------------------------------------------------------------------------
// Queues — message injection timing
// ---------------------------------------------------------------------------

pub struct Queues {
    /// Messages injected before the next LLM call.
    pub inject: VecDeque<Message>,
    /// Messages that interrupt current tool execution.
    pub steering: VecDeque<Message>,
    /// Messages queued for after the current turn.
    pub followup: VecDeque<Message>,
}

impl Queues {
    pub fn new() -> Self {
        Self {
            inject: VecDeque::new(),
            steering: VecDeque::new(),
            followup: VecDeque::new(),
        }
    }
}

impl Default for Queues {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// LoopState — all state that extension callbacks can borrow
// ---------------------------------------------------------------------------

/// Stored as a separate struct so the borrow checker allows simultaneous
/// `&mut LoopState` + `&mut Vec<Box<dyn Extension>>`.
pub struct LoopState {
    pub messages: Vec<Message>,
    pub system_prompt: String,
    pub model: Model,
    pub options: llm::StreamOptions,
    /// Tool schemas for LLM requests (derived from registered tools).
    pub tool_schemas: Vec<llm::Tool>,
    pub queues: Queues,
    pub handle: LoopHandle,
    pub cancel: CancelToken,
}

impl LoopState {
    /// Build an [`ExtensionContext`] borrowing from this state.
    pub fn ext_ctx(&mut self) -> ExtensionContext<'_> {
        ExtensionContext {
            messages: &self.messages,
            system_prompt: &self.system_prompt,
            model: &self.model,
            queues: &mut self.queues,
            handle: self.handle.clone(),
        }
    }

    /// Convert messages to LLM messages, filtering out ephemeral entries.
    pub fn to_llm_messages(&self) -> Vec<llm::Message> {
        self.messages
            .iter()
            .filter(|m| !m.ephemeral)
            .map(|m| m.to_llm())
            .collect()
    }
}

// ---------------------------------------------------------------------------
// ExtensionContext — narrow borrow passed to every hook
// ---------------------------------------------------------------------------

/// Context passed to every extension callback.
///
/// Provides read access to conversation state and write access to queues.
/// Extract a [`LoopHandle`] for async work that outlives the callback.
pub struct ExtensionContext<'a> {
    pub messages: &'a [Message],
    pub system_prompt: &'a str,
    pub model: &'a Model,
    queues: &'a mut Queues,
    handle: LoopHandle,
}

impl ExtensionContext<'_> {
    /// Extract a [`LoopHandle`] for use in async tasks.
    pub fn handle(&self) -> LoopHandle {
        self.handle.clone()
    }

    /// Inject a message before the next LLM call.
    pub fn inject(&mut self, msg: Message) {
        self.queues.inject.push_back(msg);
    }

    /// Steer: interrupt current tool execution.
    pub fn steer(&mut self, msg: Message) {
        self.queues.steering.push_back(msg);
    }

    /// Queue a follow-up message for after the current turn.
    pub fn follow_up(&mut self, msg: Message) {
        self.queues.followup.push_back(msg);
    }

    pub fn has_pending_messages(&self) -> bool {
        !self.queues.inject.is_empty()
            || !self.queues.steering.is_empty()
            || !self.queues.followup.is_empty()
    }
}

// ---------------------------------------------------------------------------
// Event structs — passed to hooks
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct AgentEndEvent {
    pub messages: Vec<Message>,
}

#[derive(Debug, Clone)]
pub struct TurnStartEvent {
    pub turn_index: usize,
}

#[derive(Debug, Clone)]
pub struct TurnEndEvent {
    pub turn_index: usize,
    pub message: Message,
    pub tool_results: Vec<ToolResult>,
}

#[derive(Debug, Clone)]
pub struct ContextEvent {
    pub messages: Vec<Message>,
}

#[derive(Debug, Clone)]
pub struct BeforeAgentStartEvent {
    pub prompt: String,
    pub system_prompt: String,
}

#[derive(Debug, Clone)]
pub struct ToolCallEvent {
    pub tool_call_id: Str,
    pub tool_name: Str,
    pub arguments: serde_json::Value,
}

#[derive(Debug, Clone)]
pub struct ToolResultEvent {
    pub tool_call_id: Str,
    pub tool_name: Str,
    pub result: ToolResult,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputSource {
    Interactive,
    Rpc,
    Extension,
}

#[derive(Debug, Clone)]
pub struct InputEvent {
    pub text: String,
    pub source: InputSource,
}

// ---------------------------------------------------------------------------
// Result structs — returned by decision hooks
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default)]
pub struct ContextResult {
    pub messages: Option<Vec<Message>>,
}

#[derive(Debug, Clone, Default)]
pub struct BeforeAgentStartResult {
    pub system_prompt: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct ToolCallResult {
    pub block: bool,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct ToolResultResult {
    pub content: Option<Vec<llm::UserContent>>,
    pub is_error: Option<bool>,
}

#[derive(Debug, Clone)]
pub enum InputResult {
    Continue,
    Transform { text: String },
    Handled,
}

// ---------------------------------------------------------------------------
// Extension trait
// ---------------------------------------------------------------------------

/// An extension hooks into the agent loop lifecycle.
///
/// All hooks are async and receive `&mut ExtensionContext<'_>`.
/// Decision hooks return `Option<ResultStruct>` — `None` = no opinion.
///
/// `async_trait(?Send)` — everything runs on a single-threaded runtime.
#[allow(unused_variables)]
#[async_trait(?Send)]
pub trait Extension {
    fn name(&self) -> &str {
        "unnamed"
    }

    /// Called once during setup. Register tools, providers, and perform setup.
    fn init(&mut self, registry: &mut ExtensionRegistry) {}

    // === Agent lifecycle ===

    async fn on_agent_start(&mut self, ctx: &mut ExtensionContext<'_>) {}

    async fn on_agent_end(&mut self, event: &AgentEndEvent, ctx: &mut ExtensionContext<'_>) {}

    /// Modify the system prompt before the loop runs.
    async fn on_before_agent_start(
        &mut self,
        event: &BeforeAgentStartEvent,
        ctx: &mut ExtensionContext<'_>,
    ) -> Option<BeforeAgentStartResult> {
        None
    }

    // === Turn lifecycle ===

    async fn on_turn_start(&mut self, event: &TurnStartEvent, ctx: &mut ExtensionContext<'_>) {}

    async fn on_turn_end(&mut self, event: &TurnEndEvent, ctx: &mut ExtensionContext<'_>) {}

    /// Transform messages before sending to the LLM.
    async fn on_context(
        &mut self,
        event: &ContextEvent,
        ctx: &mut ExtensionContext<'_>,
    ) -> Option<ContextResult> {
        None
    }

    // === Message streaming ===

    async fn on_message_delta(
        &mut self,
        event: &llm::AssistantMessageEvent,
        ctx: &mut ExtensionContext<'_>,
    ) {
    }

    // === Tools ===

    /// Return `Some(ToolCallResult { block: true, .. })` to prevent execution.
    async fn on_tool_call(
        &mut self,
        event: &ToolCallEvent,
        ctx: &mut ExtensionContext<'_>,
    ) -> Option<ToolCallResult> {
        None
    }

    /// Return `Some` to modify the tool result.
    async fn on_tool_result(
        &mut self,
        event: &ToolResultEvent,
        ctx: &mut ExtensionContext<'_>,
    ) -> Option<ToolResultResult> {
        None
    }

    // === Input ===

    /// Return `Handled` to consume input, `Transform` to rewrite it.
    async fn on_input(
        &mut self,
        event: &InputEvent,
        ctx: &mut ExtensionContext<'_>,
    ) -> Option<InputResult> {
        None
    }
}

// ---------------------------------------------------------------------------
// ExtensionFactory
// ---------------------------------------------------------------------------

/// A factory that creates a fresh Extension instance for each agent loop.
pub type ExtensionFactory = Box<dyn Fn() -> Box<dyn Extension>>;

// ---------------------------------------------------------------------------
// ExtensionRegistry — passed to Extension::init
// ---------------------------------------------------------------------------

/// Registry for tool and provider registration during init.
pub struct ExtensionRegistry {
    pub(crate) tools: Vec<RegisteredTool>,
    pub(crate) providers: Vec<(Str, Rc<dyn llm::Provider>)>,
}

impl ExtensionRegistry {
    pub fn new() -> Self {
        Self {
            tools: Vec::new(),
            providers: Vec::new(),
        }
    }

    /// Register a tool with a closure.
    pub fn tool<F, Fut>(&mut self, schema: llm::Tool, execute: F)
    where
        F: Fn(String, serde_json::Value, ToolHandle) -> Fut + 'static,
        Fut: Future<Output = ToolResult> + 'static,
    {
        self.tools.push(RegisteredTool {
            schema,
            execute: Box::new(move |id, args, handle| Box::pin(execute(id, args, handle))),
        });
    }

    /// Register a provider under an API name.
    pub fn provider(&mut self, api: impl Into<Str>, provider: impl llm::Provider + 'static) {
        self.providers.push((api.into(), Rc::new(provider)));
    }
}

impl Default for ExtensionRegistry {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// init_extensions
// ---------------------------------------------------------------------------

/// Initialize extensions: call init on each, collect tools and providers.
pub fn init_extensions(
    extensions: &mut [Box<dyn Extension>],
) -> (Vec<RegisteredTool>, Vec<(Str, Rc<dyn llm::Provider>)>) {
    let mut registry = ExtensionRegistry::new();
    for ext in extensions.iter_mut() {
        ext.init(&mut registry);
    }
    (registry.tools, registry.providers)
}
