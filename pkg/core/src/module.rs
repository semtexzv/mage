//! Module system — replaces the Extension trait with a simpler interface.
//!
//! Modules provide tools and intercept the pipeline at three points:
//! - [`Module::gate_tool`] — permission/approval before execution
//! - [`Module::filter_result`] — modify tool results after execution
//! - [`Module::transform_context`] — context window management before LLM call
//!
//! Lifecycle observation (agent start/end, turn start/end, streaming deltas)
//! is handled by subscribing to the [`AgentEvent`] broadcast channel, not by
//! methods on Module. This keeps the trait focused on pipeline interception.

use std::rc::Rc;

use async_trait::async_trait;

use crate::tool::{ToolCall, ToolDef};
use crate::types::ToolResult;

// ---------------------------------------------------------------------------
// GateResult
// ---------------------------------------------------------------------------

/// Decision from [`Module::gate_tool`].
#[derive(Debug, Clone)]
pub enum GateResult {
    /// Allow the tool to execute.
    Allow,
    /// Block execution with a reason (returned to the LLM as a tool error).
    Block(String),
}

// ---------------------------------------------------------------------------
// Module trait
// ---------------------------------------------------------------------------

/// A module hooks into the agent pipeline.
///
/// Modules are simpler than the old Extension trait — only 4 methods,
/// all with defaults. Implement only what you need.
///
/// `&self` on all methods — modules with mutable state should use
/// channels to an internal state task rather than interior mutability.
///
/// `async_trait(?Send)` — everything runs on a single-threaded runtime.
#[allow(unused_variables)]
#[async_trait(?Send)]
pub trait Module: 'static {
    /// Module identity.
    fn name(&self) -> &str;

    /// Provide tools. Called once during [`AgentLoop`] construction.
    fn tools(&self) -> Vec<ToolDef> {
        vec![]
    }

    /// Gate a tool call before execution.
    ///
    /// Modules are checked in order. The first `Block` wins.
    /// Use for permission checks, approval prompts, rate limiting.
    async fn gate_tool(&self, call: &ToolCall) -> GateResult {
        GateResult::Allow
    }

    /// Modify a tool result after execution.
    ///
    /// Modules are chained — each sees the previous module's output.
    /// Use for result budgeting, content filtering, telemetry.
    async fn filter_result(&self, call: &ToolCall, result: ToolResult) -> ToolResult {
        result
    }

    /// Transform the LLM message context before each API call.
    ///
    /// Modules are chained — each transforms the previous module's output.
    /// Use for context compaction, message pruning, prompt injection.
    async fn transform_context(&self, messages: Vec<llm::Message>) -> Vec<llm::Message> {
        messages
    }
}

// ---------------------------------------------------------------------------
// ModuleSet — ordered collection of modules
// ---------------------------------------------------------------------------

/// An ordered set of modules. Immutable after construction.
///
/// Hook dispatch iterates modules in registration order.
pub struct ModuleSet {
    modules: Vec<Rc<dyn Module>>,
}

impl ModuleSet {
    pub fn new(modules: Vec<Rc<dyn Module>>) -> Self {
        Self { modules }
    }

    /// Collect tools from all modules.
    pub fn collect_tools(&self) -> Vec<ToolDef> {
        self.modules.iter().flat_map(|m| m.tools()).collect()
    }

    /// Run gate_tool across all modules. First Block wins.
    pub async fn gate_tool(&self, call: &ToolCall) -> GateResult {
        for module in &self.modules {
            match module.gate_tool(call).await {
                GateResult::Allow => continue,
                blocked => return blocked,
            }
        }
        GateResult::Allow
    }

    /// Chain filter_result across all modules.
    pub async fn filter_result(&self, call: &ToolCall, mut result: ToolResult) -> ToolResult {
        for module in &self.modules {
            result = module.filter_result(call, result).await;
        }
        result
    }

    /// Chain transform_context across all modules.
    pub async fn transform_context(&self, mut messages: Vec<llm::Message>) -> Vec<llm::Message> {
        for module in &self.modules {
            messages = module.transform_context(messages).await;
        }
        messages
    }
}
