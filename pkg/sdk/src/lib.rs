//! `mage-sdk` — SDK crate for the Mage agent framework.
//!
//! Single dependency for module authors and the host binary.
//! Re-exports the full module tree from internal crates.
//!
//! # Quick start
//!
//! ```ignore
//! use mage_sdk::prelude::*;
//!
//! struct MyModule;
//!
//! #[async_trait]
//! impl Module for MyModule {
//!     fn name(&self) -> &str { "my_module" }
//! }
//! ```

/// Handle and command types for loop communication.
pub use mage_core::handle;

/// Upgrade signaling (monitor pipe protocol).
pub use mage_core::upgrade;

/// Module trait, ModuleSet, GateResult.
pub use mage_core::module;

/// Tool system: ToolHandler, ToolDef, ToolRegistry.
pub use mage_core::tool;

/// Agent message types, events, delivery modes.
pub use mage_core::types;

/// Session handle and spawn.
pub use mage_core::session;

/// Built-in tools: Read, Edit, Write, Bash, Glob, Grep.
pub use mage_tools as tools;

/// Agent loop (usually not needed by modules directly).
pub use mage_core::agent_loop;

/// Application layer: commands, input routing, session lifecycle.
pub use mage_app as app;

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

/// Run an async closure on a single-threaded tokio runtime with a LocalSet.
///
/// This is the standard entry point for mage binaries. Handles runtime
/// construction so generated code doesn't need `tokio` as a direct dependency.
pub fn run_local<F, Fut>(f: F)
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = ()>,
{
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("failed to build tokio runtime");
    let local = tokio::task::LocalSet::new();
    local.block_on(&rt, f());
}

/// Convenience prelude — everything a module author typically needs.
pub mod prelude {
    // Module system
    pub use crate::module::{GateResult, Module, ModuleSet};

    // Tool system
    pub use crate::tool::{ToolCall, ToolContext, ToolDef, ToolHandler, ToolCompletion};

    // Handle
    pub use crate::handle::{LoopCommand, LoopHandle};

    pub use async_trait::async_trait;

    // Session
    pub use crate::session::SessionHandle;
    pub use crate::session::spawn as spawn_session;

    // Agent loop
    pub use crate::agent_loop::{AgentLoop, LoopError};

    // Agent types
    pub use crate::types::{AgentEvent, Message, MessageBody, EntryId, ToolResult, ToolUpdate};

    // LLM types (the subset modules actually touch)
    pub use crate::llm::CancelToken;
    pub use crate::llm::types::{UserContent, ContentBlock, Model, Usage};

    // Auth
    pub use crate::llm::{AuthStatus, Authenticator, LoginStep, LoginReceiver};

    // Strings
    pub use refstr::Str;
    pub use crate::app::command::{Command, CommandRegistry, CommandError};
    pub use crate::app::app::{App, AppError};
}
