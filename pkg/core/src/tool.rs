//! Tool system — trait-based tool handlers with concurrent execution support.
//!
//! Tools implement [`ToolHandler`] and are wrapped in [`ToolDef`] with their
//! JSON schema. The [`ToolRegistry`] holds all registered tools and provides
//! lookup + concurrency classification.

use std::rc::Rc;

use async_trait::async_trait;
use refstr::Str;

use llm::CancelToken;

use crate::handle::LoopHandle;
use crate::types::{ToolResult, ToolUpdate};

// ---------------------------------------------------------------------------
// ToolCall — extracted from an LLM response
// ---------------------------------------------------------------------------

/// A tool call extracted from an LLM response content block.
#[derive(Debug, Clone)]
pub struct ToolCall {
    pub id: Str,
    pub name: Str,
    pub args: serde_json::Value,
}

// ---------------------------------------------------------------------------
// ToolContext — given to a tool during execution
// ---------------------------------------------------------------------------

/// Context provided to a tool during execution.
///
/// Provides cancellation awareness, progress streaming, and loop access.
/// Created per tool call; the spawning task drains progress updates.
pub struct ToolContext {
    cancel: CancelToken,
    update_tx: tokio::sync::mpsc::UnboundedSender<ToolUpdate>,
    loop_handle: LoopHandle,
}

impl ToolContext {
    pub(crate) fn new(
        cancel: CancelToken,
        update_tx: tokio::sync::mpsc::UnboundedSender<ToolUpdate>,
        loop_handle: LoopHandle,
    ) -> Self {
        Self {
            cancel,
            update_tx,
            loop_handle,
        }
    }

    /// Synchronous cancellation check — call at yield points.
    pub fn is_cancelled(&self) -> bool {
        self.cancel.is_cancelled()
    }

    /// The per-tool cancellation token, for passing deeper.
    pub fn cancel_token(&self) -> &CancelToken {
        &self.cancel
    }

    /// Send a progress update to the UI. Fire-and-forget.
    pub fn send_update(&self, update: ToolUpdate) {
        let _ = self.update_tx.send(update);
    }

    /// Access the loop handle for injecting/steering messages.
    pub fn loop_handle(&self) -> &LoopHandle {
        &self.loop_handle
    }
}

// ---------------------------------------------------------------------------
// ToolCompletion — result of a tool execution
// ---------------------------------------------------------------------------

/// Completed tool execution result, sent back to the agent loop.
#[derive(Debug, Clone)]
pub struct ToolCompletion {
    pub call_id: Str,
    pub name: Str,
    pub result: ToolResult,
}

// ---------------------------------------------------------------------------
// ToolHandler trait
// ---------------------------------------------------------------------------

/// The tool handler trait. Implement this for each tool.
///
/// `async_trait(?Send)` — runs on a single-threaded runtime (spawn_local).
#[allow(unused_variables)]
#[async_trait(?Send)]
pub trait ToolHandler: 'static {
    /// Execute the tool with the given arguments.
    async fn execute(&self, args: serde_json::Value, ctx: ToolContext) -> ToolResult;

    /// Whether this specific invocation is safe to run concurrently
    /// with other concurrent-safe tools.
    ///
    /// Default: `false` (conservative — serial execution).
    ///
    /// Override per-tool to enable concurrency. For input-dependent
    /// classification (e.g. `Bash("ls")` is safe, `Bash("rm")` is not),
    /// inspect the `args` parameter.
    fn is_concurrent_safe(&self, args: &serde_json::Value) -> bool {
        false
    }
}

// ---------------------------------------------------------------------------
// ToolDef — schema + handler
// ---------------------------------------------------------------------------

/// A registered tool: JSON schema for the LLM + handler for execution.
pub struct ToolDef {
    pub schema: llm::Tool,
    pub handler: Rc<dyn ToolHandler>,
}

// ---------------------------------------------------------------------------
// ToolRegistry — lookup and classification
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Closure-based convenience
// ---------------------------------------------------------------------------

/// Create a [`ToolDef`] from a closure. Convenience for simple tools.
///
/// ```ignore
/// tool_fn(
///     llm::Tool { name: "echo".into(), description: "Echo".into(), parameters: json!({}) },
///     |args, _ctx| async move { ToolResult::success("echoed") },
/// )
/// ```
pub fn tool_fn<F, Fut>(schema: llm::Tool, f: F) -> ToolDef
where
    F: Fn(serde_json::Value, ToolContext) -> Fut + 'static,
    Fut: std::future::Future<Output = ToolResult> + 'static,
{
    struct ClosureHandler<F>(F);

    #[async_trait(?Send)]
    impl<F, Fut> ToolHandler for ClosureHandler<F>
    where
        F: Fn(serde_json::Value, ToolContext) -> Fut + 'static,
        Fut: std::future::Future<Output = ToolResult> + 'static,
    {
        async fn execute(&self, args: serde_json::Value, ctx: ToolContext) -> ToolResult {
            (self.0)(args, ctx).await
        }
    }

    ToolDef {
        schema,
        handler: Rc::new(ClosureHandler(f)),
    }
}

/// Like [`tool_fn`], but also specifies a concurrent-safety classifier.
pub fn tool_fn_concurrent<F, Fut>(
    schema: llm::Tool,
    f: F,
    is_safe: fn(&serde_json::Value) -> bool,
) -> ToolDef
where
    F: Fn(serde_json::Value, ToolContext) -> Fut + 'static,
    Fut: std::future::Future<Output = ToolResult> + 'static,
{
    struct ConcurrentClosureHandler<F> {
        f: F,
        is_safe: fn(&serde_json::Value) -> bool,
    }

    #[async_trait(?Send)]
    impl<F, Fut> ToolHandler for ConcurrentClosureHandler<F>
    where
        F: Fn(serde_json::Value, ToolContext) -> Fut + 'static,
        Fut: std::future::Future<Output = ToolResult> + 'static,
    {
        async fn execute(&self, args: serde_json::Value, ctx: ToolContext) -> ToolResult {
            (self.f)(args, ctx).await
        }
        fn is_concurrent_safe(&self, args: &serde_json::Value) -> bool {
            (self.is_safe)(args)
        }
    }

    ToolDef {
        schema,
        handler: Rc::new(ConcurrentClosureHandler { f, is_safe }),
    }
}

// ---------------------------------------------------------------------------
// ToolRegistry
// ---------------------------------------------------------------------------

/// Registry of all available tools. Built from module contributions at startup.
pub struct ToolRegistry {
    tools: Vec<ToolDef>,
}

impl ToolRegistry {
    pub fn new(tools: Vec<ToolDef>) -> Self {
        Self { tools }
    }

    /// Look up a tool by name.
    pub fn get(&self, name: &str) -> Option<&ToolDef> {
        self.tools.iter().find(|t| *t.schema.name == *name)
    }

    /// Extract schemas for sending to the LLM.
    pub fn schemas(&self) -> Vec<llm::Tool> {
        self.tools.iter().map(|t| t.schema.clone()).collect()
    }

    /// Check if a specific tool invocation is safe to run concurrently.
    pub fn is_concurrent_safe(&self, name: &str, args: &serde_json::Value) -> bool {
        self.get(name)
            .map(|t| t.handler.is_concurrent_safe(args))
            .unwrap_or(false)
    }
}
