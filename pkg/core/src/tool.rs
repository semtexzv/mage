//! Tool types — ToolDef, BoxToolFn, ToolResult.

use std::future::Future;
use std::pin::Pin;

use refstr::LocalStr;

use llm::CancelToken;

// ---------------------------------------------------------------------------
// ToolResult
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct ToolResult {
    pub content: Vec<llm::UserContent>,
    pub details: serde_json::Value,
}

// ---------------------------------------------------------------------------
// ToolDef — a tool is data + a closure
// ---------------------------------------------------------------------------

/// The execute function for a tool.
///
/// `Fn` (not `FnOnce`): the same closure handles every invocation.
/// Each call returns a fresh `Pin<Box<Future>>` — one heap alloc per
/// invocation, acceptable because tools always do I/O.
/// For tools needing shared state, the closure captures it (e.g.
/// `Rc<RefCell<>>`) or creates per-call state inside the returned future.
pub type BoxToolFn = Box<
    dyn Fn(
        /* tool_call_id */ &str,
        /* params */       serde_json::Value,
        /* cancel */       CancelToken,
    ) -> Pin<Box<dyn Future<Output = ToolResult> + '_>>,
>;

/// A tool the agent can call.
///
/// Not a trait — `name`/`description`/`parameters` are data, `execute` is
/// a closure. Tools are stored in `Vec<ToolDef>`.
pub struct ToolDef {
    pub name: LocalStr,
    pub label: LocalStr,
    pub description: LocalStr,
    pub parameters: serde_json::Value,
    pub execute: BoxToolFn,
}

impl ToolDef {
    /// Convert to the LLM tool schema (name + description + parameters).
    pub fn to_llm_tool(&self) -> llm::Tool {
        llm::Tool {
            name: self.name.clone(),
            description: self.description.clone(),
            parameters: self.parameters.clone(),
        }
    }
}

impl std::fmt::Debug for ToolDef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ToolDef")
            .field("name", &self.name)
            .field("label", &self.label)
            .field("description", &self.description)
            .finish_non_exhaustive()
    }
}
