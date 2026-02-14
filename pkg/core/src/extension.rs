//! Extension system.
//!
//! An **extension** is a unit of functionality that plugs into the agent loop.
//! It can observe lifecycle events, intercept decisions, and register tools
//! and providers during initialization.
//!
//! ## Layered init
//!
//! 1. **Global**: `ExtensionFactory` is registered once at process startup.
//!    It's a callback that creates a fresh `Box<dyn Extension>` for each
//!    agent loop instance.
//!
//! 2. **Per-agent**: `Extension::init(&mut self, reg)` is called when an
//!    agent loop starts. The extension registers tools and providers into
//!    the `Registry`.
//!
//! ## State ownership
//!
//! Extensions own their state via `&mut self`. If an extension needs shared
//! state (e.g. across tools it registers), that's the extension's
//! responsibility — use `Rc<RefCell<>>` internally. The framework doesn't
//! impose a sharing model.
//!

use std::future::Future;
use std::pin::Pin;

use refstr::Str;

use llm::{AssistantMessageEvent, CancelToken, Model, ToolResultMessage, UserContent};

use crate::tool::{ErasedTool, Tool, ToolResult, erase_tool};
use crate::types::{AgentMessage, DeliverAs};

// ---------------------------------------------------------------------------
// Disposition — what a decision hook returns
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub enum Disposition<T = ()> {
    /// No opinion. Continue to next hook.
    Propagate,
    /// Block the operation.
    Block { reason: Str },
    /// Return a value (amendment).
    Value(T),
}

impl<T> Disposition<T> {
    pub fn is_block(&self) -> bool {
        matches!(self, Self::Block { .. })
    }
}

impl<T> Default for Disposition<T> {
    fn default() -> Self {
        Self::Propagate
    }
}

// ---------------------------------------------------------------------------
// Amendment types — the T in Disposition<T>
// ---------------------------------------------------------------------------

pub struct BeforeStartAmend {
    pub system_prompt: Option<String>,
    pub inject_message: Option<AgentMessage>,
}

pub struct ToolResultAmend {
    pub content: Option<Vec<UserContent>>,
    pub is_error: Option<bool>,
}

pub struct InputAmend {
    pub text: String,
    /// If true, input was fully handled — don't send to agent.
    pub handled: bool,
}

pub struct ContextAmend {
    pub messages: Vec<AgentMessage>,
}

pub struct CompactAmend {
    pub summary: Str,
    pub first_kept_entry_id: Str,
}

pub struct BashAmend {
    pub output: String,
    pub exit_code: i32,
}

// ---------------------------------------------------------------------------
// Arg structs — references passed to hooks
// ---------------------------------------------------------------------------

pub struct ToolCallArgs<'a> {
    pub name: &'a str,
    pub id: &'a str,
    pub args: &'a serde_json::Value,
}

pub struct ToolResultArgs<'a> {
    pub name: &'a str,
    pub id: &'a str,
    pub result: &'a ToolResult,
    pub is_error: bool,
}

pub struct BeforeStartArgs<'a> {
    pub system_prompt: &'a str,
    pub prompt: &'a str,
}

pub struct TurnEndArgs<'a> {
    pub message: &'a AgentMessage,
    pub tool_results: &'a [ToolResultMessage],
}

pub struct MessageArgs<'a> {
    pub message: &'a AgentMessage,
}

pub struct MessageDeltaArgs<'a> {
    pub event: &'a AssistantMessageEvent,
}

pub struct ToolExecStartArgs<'a> {
    pub name: &'a str,
    pub args: &'a serde_json::Value,
}

pub struct ToolExecEndArgs<'a> {
    pub name: &'a str,
    pub result: &'a ToolResult,
    pub is_error: bool,
}

pub struct BeforeForkArgs<'a> {
    pub entry_id: &'a str,
}

pub struct UserBashArgs<'a> {
    pub command: &'a str,
}

pub struct AgentEndArgs<'a> {
    pub messages: &'a [AgentMessage],
}

pub struct ModelSelectArgs<'a> {
    pub model: &'a Model,
}

// ---------------------------------------------------------------------------
// HookCtx — split borrows, no Rc
// ---------------------------------------------------------------------------

/// Context passed to hook methods. Read access to agent state,
/// write access to the outbox for message injection.
///
/// Created by the agent loop via split borrows before each hook call.
pub struct HookCtx<'a> {
    pub model: &'a Model,
    pub system_prompt: &'a str,
    outbox: &'a mut Vec<(AgentMessage, DeliverAs)>,
    cancel: &'a CancelToken,
}

impl<'a> HookCtx<'a> {
    pub fn new(
        model: &'a Model,
        system_prompt: &'a str,
        outbox: &'a mut Vec<(AgentMessage, DeliverAs)>,
        cancel: &'a CancelToken,
    ) -> Self {
        Self { model, system_prompt, outbox, cancel }
    }

    /// Queue a message for delivery to the agent.
    pub fn send_message(&mut self, msg: AgentMessage, deliver: DeliverAs) {
        self.outbox.push((msg, deliver));
    }

    /// Cancel the current operation.
    pub fn abort(&self) {
        self.cancel.cancel();
    }

    /// Check if cancelled.
    pub fn is_cancelled(&self) -> bool {
        self.cancel.is_cancelled()
    }
}

// ---------------------------------------------------------------------------
// Registry — passed to Extension::init for tool/provider registration
// ---------------------------------------------------------------------------

/// Mutable registry passed to `Extension::init`. Extensions use it to
/// register tools and providers into the agent.
pub struct Registry<'a> {
    pub(crate) tools: &'a mut Vec<Box<dyn ErasedTool>>,
    pub(crate) providers: &'a mut Vec<(Str, std::rc::Rc<dyn llm::Provider>)>,
}
impl<'a> Registry<'a> {
    /// Register a tool. Wraps the concrete `Tool` impl into type-erased storage.
    pub fn tool(&mut self, tool: impl Tool) {
        self.tools.push(erase_tool(tool));
    }
    /// Register a provider under an API name (e.g. `"anthropic"`).
    /// The agent resolves the provider from `model.api`.
    pub fn provider(&mut self, api: impl Into<Str>, provider: impl llm::Provider + 'static) {
        self.providers.push((api.into(), std::rc::Rc::new(provider)));
    }
}

// ---------------------------------------------------------------------------
// Extension trait
// ---------------------------------------------------------------------------

/// Async return type for decision hooks. One alloc per invocation.
pub type HookFuture<'a, T> = Pin<Box<dyn Future<Output = T> + 'a>>;

/// An extension is a unit of functionality that plugs into the agent loop.
///
/// It has two phases:
/// - `init`: called once per agent-loop instance, registers tools/providers.
/// - Hook methods: called during the agent loop at interception points.
///
/// `&mut self` provides owned state. If an extension needs shared state
/// across its tools, it manages that internally (e.g. `Rc<RefCell<>>`).
///
/// **Observe hooks** are sync. **Decision hooks** return `HookFuture`.
#[allow(unused_variables)]
pub trait Extension {
    /// Called once when the agent loop starts. Register tools, providers,
    /// and perform any setup.
    fn init(&mut self, registry: &mut Registry) {}

    // ===================================================================
    // Observe — sync, fire-and-forget
    // ===================================================================

    fn on_agent_start(&mut self, ctx: &HookCtx) {}
    fn on_agent_end(&mut self, args: &AgentEndArgs, ctx: &HookCtx) {}
    fn on_turn_start(&mut self, ctx: &HookCtx) {}
    fn on_turn_end(&mut self, args: &TurnEndArgs, ctx: &HookCtx) {}
    fn on_model_select(&mut self, args: &ModelSelectArgs, ctx: &HookCtx) {}
    fn on_session_start(&mut self, ctx: &HookCtx) {}
    fn on_session_switch(&mut self, ctx: &HookCtx) {}
    fn on_session_shutdown(&mut self, ctx: &HookCtx) {}
    fn on_message_start(&mut self, args: &MessageArgs, ctx: &HookCtx) {}
    fn on_message_delta(&mut self, args: &MessageDeltaArgs, ctx: &HookCtx) {}
    fn on_message_end(&mut self, args: &MessageArgs, ctx: &HookCtx) {}
    fn on_tool_exec_start(&mut self, args: &ToolExecStartArgs, ctx: &HookCtx) {}
    fn on_tool_exec_end(&mut self, args: &ToolExecEndArgs, ctx: &HookCtx) {}

    // ===================================================================
    // Decision — async, return Disposition<T>
    // ===================================================================

    fn on_tool_call<'a>(
        &'a mut self,
        args: &'a ToolCallArgs<'a>,
        ctx: &'a HookCtx,
    ) -> HookFuture<'a, Disposition> {
        Box::pin(async { Disposition::Propagate })
    }

    fn on_before_start<'a>(
        &'a mut self,
        args: &'a BeforeStartArgs<'a>,
        ctx: &'a HookCtx,
    ) -> HookFuture<'a, Disposition<BeforeStartAmend>> {
        Box::pin(async { Disposition::Propagate })
    }

    fn on_tool_result<'a>(
        &'a mut self,
        args: &'a ToolResultArgs<'a>,
        ctx: &'a HookCtx,
    ) -> HookFuture<'a, Disposition<ToolResultAmend>> {
        Box::pin(async { Disposition::Propagate })
    }

    fn on_input<'a>(
        &'a mut self,
        text: &'a str,
        ctx: &'a HookCtx,
    ) -> HookFuture<'a, Disposition<InputAmend>> {
        Box::pin(async { Disposition::Propagate })
    }

    fn on_context<'a>(
        &'a mut self,
        messages: &'a [AgentMessage],
        ctx: &'a HookCtx,
    ) -> HookFuture<'a, Disposition<ContextAmend>> {
        Box::pin(async { Disposition::Propagate })
    }

    fn on_before_switch<'a>(
        &'a mut self,
        ctx: &'a HookCtx,
    ) -> HookFuture<'a, Disposition> {
        Box::pin(async { Disposition::Propagate })
    }

    fn on_before_fork<'a>(
        &'a mut self,
        args: &'a BeforeForkArgs<'a>,
        ctx: &'a HookCtx,
    ) -> HookFuture<'a, Disposition> {
        Box::pin(async { Disposition::Propagate })
    }

    fn on_before_compact<'a>(
        &'a mut self,
        ctx: &'a HookCtx,
    ) -> HookFuture<'a, Disposition<CompactAmend>> {
        Box::pin(async { Disposition::Propagate })
    }

    fn on_user_bash<'a>(
        &'a mut self,
        args: &'a UserBashArgs<'a>,
        ctx: &'a HookCtx,
    ) -> HookFuture<'a, Disposition<BashAmend>> {
        Box::pin(async { Disposition::Propagate })
    }
}

// ---------------------------------------------------------------------------
// ExtensionFactory — global registration
// ---------------------------------------------------------------------------

/// A factory that creates a fresh `Extension` instance for each agent loop.
///
/// Registered once at process startup. Called each time a new agent loop
/// (or sub-agent) starts, so each gets its own extension state.
///
/// `Rc<dyn Fn>` so it's cheaply clonable — needed for `AgentInit` which is `Clone`.
pub type ExtensionFactory = std::rc::Rc<dyn Fn() -> Box<dyn Extension>>;

// ---------------------------------------------------------------------------
// FactoryRegistry — process-level extension factory storage
// ---------------------------------------------------------------------------

/// Process-level registry of extension factories.
///
/// Register factories at startup. When an agent loop starts, call
/// [`FactoryRegistry::create_all`] to get fresh extension instances.
///
/// Not `Send`/`Sync` — single-threaded runtime.
pub struct FactoryRegistry {
    factories: Vec<ExtensionFactory>,
}

impl FactoryRegistry {
    pub const fn new() -> Self {
        Self { factories: Vec::new() }
    }

    /// Register a factory. Called once at process startup per extension type.
    pub fn register(&mut self, factory: impl Fn() -> Box<dyn Extension> + 'static) {
        self.factories.push(std::rc::Rc::new(factory));
    }

    /// Create fresh extension instances from all registered factories.
    /// Called once per agent loop / sub-agent.
    pub fn create_all(&self) -> Vec<Box<dyn Extension>> {
        self.factories.iter().map(|f| f()).collect()
    }

    /// Clone the factory list. Since factories are `Rc<dyn Fn>`, this is cheap.
    /// Used by `AgentBuilder::ext_from_registry` to embed factories into `AgentInit`.
    pub fn clone_factories(&self) -> Vec<ExtensionFactory> {
        self.factories.clone()
    }

    /// Number of registered factories.
    pub fn len(&self) -> usize {
        self.factories.len()
    }

    /// Returns `true` if no factories are registered.
    pub fn is_empty(&self) -> bool {
        self.factories.is_empty()
    }
}

impl Default for FactoryRegistry {
    fn default() -> Self {
        Self::new()
    }
}