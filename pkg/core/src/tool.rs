//! Tool system — Tool trait, execution tiers, results, mailbox, type erasure.
//!
//! One trait (`Tool`) defines everything about a tool: metadata, execution.
//! Rendering is deferred — will be added later as optional trait methods.
//!
//! Three execution tiers:
//! - **Sync (Tier 1)**: return `ToolResult` directly, auto-converts via `From`.
//! - **Async (Tier 2)**: `ToolExecution::running(future)` — async I/O, no custom state.
//! - **Custom (Tier 3)**: `ToolExecution::custom(|sender| future)` — streaming state via mailbox.
//!
//! The agent loop works with `ErasedTool` (object-safe wrapper) and `ErasedExecution`.

use std::cell::Cell;
use std::future::Future;
use std::pin::Pin;
use std::rc::Rc;

use llm::CancelToken;

// ---------------------------------------------------------------------------
// ToolResult — success or failure
// ---------------------------------------------------------------------------

/// The outcome of a tool execution.
#[derive(Debug, Clone)]
pub enum ToolResult {
    /// Tool succeeded.
    Success(ToolContent),
    /// Tool failed.
    Failure(ToolContent),
}

impl ToolResult {
    pub fn success(s: impl Into<String>) -> Self {
        Self::Success(ToolContent::text(s))
    }

    pub fn failure(s: impl Into<String>) -> Self {
        Self::Failure(ToolContent::text(s))
    }

    pub fn is_error(&self) -> bool {
        matches!(self, Self::Failure(_))
    }

    pub fn content(&self) -> &ToolContent {
        match self {
            Self::Success(c) | Self::Failure(c) => c,
        }
    }
}

// ---------------------------------------------------------------------------
// ToolContent — what goes back to the LLM
// ---------------------------------------------------------------------------

/// Content returned to the LLM from a tool execution.
#[derive(Debug, Clone)]
pub struct ToolContent {
    pub content: Vec<llm::UserContent>,
}

impl ToolContent {
    pub fn text(s: impl Into<String>) -> Self {
        Self {
            content: vec![llm::UserContent::Text { text: s.into() }],
        }
    }

    pub fn rich(content: Vec<llm::UserContent>) -> Self {
        Self { content }
    }
}

// ---------------------------------------------------------------------------
// Mailbox — single-slot latest-value container
// ---------------------------------------------------------------------------

/// Reader half of the mailbox. Owned by the agent loop / TUI.
pub struct Mailbox<T> {
    inner: Rc<MailboxInner<T>>,
}

/// Writer half of the mailbox. Owned by the tool's async task.
pub struct MailboxSender<T> {
    inner: Rc<MailboxInner<T>>,
}

struct MailboxInner<T> {
    value: Cell<Option<T>>,
}

impl<T> Mailbox<T> {
    pub fn new() -> (MailboxSender<T>, Mailbox<T>) {
        let inner = Rc::new(MailboxInner {
            value: Cell::new(None),
        });
        (
            MailboxSender { inner: inner.clone() },
            Mailbox { inner },
        )
    }

    /// Take the latest value, if any new value since last take.
    pub fn take(&self) -> Option<T> {
        self.inner.value.take()
    }
}

impl<T> MailboxSender<T> {
    /// Send a new value, overwriting any previous unseen value.
    pub fn send(&self, value: T) {
        self.inner.value.set(Some(value));
    }
}

// ---------------------------------------------------------------------------
// ToolExecution — the three tiers
// ---------------------------------------------------------------------------

/// What a tool's `execute()` returns. Three tiers of complexity.
pub enum ToolExecution<S: 'static = String> {
    /// Tier 1 — Sync. Result ready immediately.
    Ready(ToolResult),
    /// Tier 2 — Async. Future with default rendering.
    Running(Pin<Box<dyn Future<Output = ToolResult>>>),
    /// Tier 3 — Custom. Future + mailbox for streaming state.
    Custom {
        task: Pin<Box<dyn Future<Output = ToolResult>>>,
        mailbox: Mailbox<S>,
    },
}

impl ToolExecution<String> {
    /// Tier 2: async execution, default rendering.
    pub fn running(task: impl Future<Output = ToolResult> + 'static) -> Self {
        ToolExecution::Running(Box::pin(task))
    }
}

impl<S: 'static> ToolExecution<S> {
    /// Tier 3: custom state + mailbox.
    ///
    /// Accepts a closure that receives a `MailboxSender` and returns a future.
    /// Boxing is done internally — the caller writes a plain async block.
    pub fn custom(
        f: impl FnOnce(MailboxSender<S>) -> Pin<Box<dyn Future<Output = ToolResult>>>,
    ) -> Self {
        let (tx, mailbox) = Mailbox::new();
        ToolExecution::Custom {
            task: f(tx),
            mailbox,
        }
    }
}

/// Tier 1: sync result → ToolExecution via From.
impl<S: 'static> From<ToolResult> for ToolExecution<S> {
    fn from(result: ToolResult) -> Self {
        ToolExecution::Ready(result)
    }
}

// ---------------------------------------------------------------------------
// Tool trait — the canonical interface
// ---------------------------------------------------------------------------

/// A tool the agent can call.
///
/// One trait for metadata + execution. Rendering methods will be added later.
///
/// The associated type `State` is used for Tier 3 tools
/// that stream intermediate state through a `Mailbox`.
pub trait Tool: 'static {
    /// Accumulated state during execution, sent through a mailbox.
    /// Tier 1/2 tools should use `type State = String;`
    type State: 'static;

    // ── Metadata (for the LLM) ──────────────────────────────────

    fn name(&self) -> &str;
    fn description(&self) -> &str;
    fn parameters(&self) -> &serde_json::Value;

    // ── Execution ───────────────────────────────────────────────

    fn execute(
        &self,
        tool_call_id: &str,
        params: serde_json::Value,
        cancel: CancelToken,
    ) -> ToolExecution<Self::State>;

    // ── LLM output ──────────────────────────────────────────────

    /// Convert the result into the content sent back to the LLM.
    /// Default: pass through the result's ToolContent.
    fn to_result(&self, result: &ToolResult, _state: Option<&Self::State>) -> ToolContent {
        result.content().clone()
    }
}

// ---------------------------------------------------------------------------
// ErasedExecution — type-erased execution for the agent loop
// ---------------------------------------------------------------------------

/// Type-erased execution returned by `ErasedTool::execute`.
pub enum ErasedExecution {
    /// Sync result, ready immediately.
    Ready(ToolResult),
    /// Async, no custom state.
    Running(Pin<Box<dyn Future<Output = ToolResult>>>),
    /// Async with custom live output (mailbox inside, opaque to loop).
    Custom {
        task: Pin<Box<dyn Future<Output = ToolResult>>>,
        // LiveOutput will go here when rendering is added.
        // For now, we just have the task.
    },
}

// ---------------------------------------------------------------------------
// ErasedTool — object-safe wrapper for the agent loop and registry
// ---------------------------------------------------------------------------

/// Object-safe interface used by the agent loop and registry.
#[allow(dead_code)]
pub(crate) trait ErasedTool {
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    fn parameters(&self) -> &serde_json::Value;
    fn execute(
        &self,
        tool_call_id: &str,
        params: serde_json::Value,
        cancel: CancelToken,
    ) -> ErasedExecution;
    fn to_result(&self, result: &ToolResult) -> ToolContent;

    /// Convert to the LLM tool schema.
    fn to_llm_tool(&self) -> llm::Tool;
}

/// Wraps a concrete `Tool` behind `Rc<T>` for cheap cloning + type erasure.
struct ToolWrapper<T: Tool> {
    tool: Rc<T>,
}

impl<T: Tool> ErasedTool for ToolWrapper<T> {
    fn name(&self) -> &str {
        self.tool.name()
    }

    fn description(&self) -> &str {
        self.tool.description()
    }

    fn parameters(&self) -> &serde_json::Value {
        self.tool.parameters()
    }

    fn execute(
        &self,
        tool_call_id: &str,
        params: serde_json::Value,
        cancel: CancelToken,
    ) -> ErasedExecution {
        let exec = self.tool.execute(tool_call_id, params, cancel);
        match exec {
            ToolExecution::Ready(result) => ErasedExecution::Ready(result),
            ToolExecution::Running(task) => ErasedExecution::Running(task),
            ToolExecution::Custom { task, mailbox: _ } => {
                // When rendering is added, mailbox will be wrapped in LiveOutput.
                // For now, discard the mailbox — the task still runs.
                ErasedExecution::Custom { task }
            }
        }
    }

    fn to_result(&self, result: &ToolResult) -> ToolContent {
        // No state access yet (rendering deferred). Pass None.
        self.tool.to_result(result, None)
    }

    fn to_llm_tool(&self) -> llm::Tool {
        llm::Tool {
            name: self.tool.name().into(),
            description: self.tool.description().into(),
            parameters: self.tool.parameters().clone(),
        }
    }
}

/// Wrap a concrete `Tool` implementation into a boxed `ErasedTool`.
pub(crate) fn erase_tool<T: Tool>(tool: T) -> Box<dyn ErasedTool> {
    Box::new(ToolWrapper { tool: Rc::new(tool) })
}
