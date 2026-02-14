//! `mage` — SDK crate for the Mage agent framework.
//!
//! Single dependency for extension authors and the host binary.
//! Re-exports the full module tree from internal crates.
//! No feature gates — every build has the same capabilities.
//!
//! # Quick start
//!
//! ```ignore
//! use mage::prelude::*;
//!
//! struct MyTool;
//! impl Tool for MyTool {
//!     type State = String;
//!     fn name(&self) -> &str { "my_tool" }
//!     fn description(&self) -> &str { "Does a thing" }
//!     fn parameters(&self) -> &serde_json::Value { todo!() }
//!     fn execute(&self, _id: &str, params: serde_json::Value, _cancel: CancelToken) -> ToolExecution {
//!         ToolResult::success("done").into()
//!     }
//! }
//!
//! pub fn init(reg: &mut Registry) {
//!     reg.tool(MyTool);
//! }
//! ```

/// Extension trait, hooks, dispositions, amendment types, factory registry.
pub use mage_core::extension;

/// Tool trait, execution tiers, results, mailbox.
pub use mage_core::tool;

/// Agent message types, events, delivery modes.
pub use mage_core::types;

/// Agent builder and init recipe.
pub use mage_core::agent;

/// Session — primary runtime wrapper.
pub use mage_core::session;

/// Agent loop (usually not needed by extensions directly).
pub use mage_core::agent_loop;

/// Agent event stream types.
pub use mage_core::event_stream;

/// LLM abstraction: Provider trait, Message, Model, CancelToken, events.
pub use llm;

/// Ref-counted strings: Str, Str<Atomic>, Local, Atomic, Mode.
pub use refstr;

/// Terminal UI: renderer, markdown, editor, keymap, styles.
pub use mage_tui as tui;

/// Dynamic workspace compiler.
pub use mage_build as build;

/// Convenience prelude — everything an extension author typically needs.
pub mod prelude {
    // Extension system
    pub use crate::extension::{
        Extension, Registry, FactoryRegistry, HookFuture,
        Disposition, BeforeStartAmend, ToolResultAmend, InputAmend,
        ContextAmend, CompactAmend, BashAmend,
        ToolCallArgs, ToolResultArgs, BeforeStartArgs, TurnEndArgs,
        MessageArgs, MessageDeltaArgs, ToolExecStartArgs, ToolExecEndArgs,
        BeforeForkArgs, UserBashArgs, AgentEndArgs, ModelSelectArgs,
    };

    // Session
    pub use crate::session::{AgentSession, SessionHandle};

    // Tools
    pub use crate::tool::{
        Tool, ToolExecution, ToolResult, ToolContent,
        Mailbox, MailboxSender,
    };

    // Agent types
    pub use crate::types::{Message, MessageBody, EntryId, AgentEvent, DeliverAs};

    // LLM types (the subset extensions actually touch)
    pub use crate::llm::CancelToken;
    pub use crate::llm::types::{
        UserContent, ContentBlock, Model, Usage,
    };

    // Strings
    pub use refstr::Str;
}
