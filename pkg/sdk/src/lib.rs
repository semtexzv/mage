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
//! struct MyExtension;
//!
//! #[async_trait]
//! impl Extension for MyExtension {
//!     fn name(&self) -> &str { "my_extension" }
//! }
//! ```

/// Extension trait, hooks, event/result types, factory registry.
pub use mage_core::extension;

/// Agent message types, events, delivery modes.
pub use mage_core::types;

/// Session handle and spawn.
pub use mage_core::session;

/// Agent loop (usually not needed by extensions directly).
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

/// Convenience prelude — everything an extension author typically needs.
pub mod prelude {
    // Extension system
    pub use crate::extension::{
        Extension, ExtensionFactory, ExtensionRegistry, ExtensionContext,
        LoopHandle, LoopCommand, ToolHandle,
        AgentEndEvent, TurnStartEvent, TurnEndEvent, ContextEvent,
        ToolCallEvent, ToolResultEvent, InputEvent,
        ContextResult, ToolCallResult, ToolResultResult, InputResult,
        BeforeAgentStartEvent, BeforeAgentStartResult,
    };
    pub use async_trait::async_trait;

    // Session
    pub use crate::session::SessionHandle;
    pub use crate::session::spawn as spawn_session;

    // Agent loop
    pub use crate::agent_loop::{AgentLoop, LoopError};

    // Agent types
    pub use crate::types::{Message, MessageBody, EntryId, AgentEvent, DeliverAs, ToolResult, ToolUpdate};

    // LLM types (the subset extensions actually touch)
    pub use crate::llm::CancelToken;
    pub use crate::llm::types::{
        UserContent, ContentBlock, Model, Usage,
    };

    // Auth
    pub use crate::llm::{AuthStatus, Authenticator, LoginStep, LoginReceiver};

    // Strings
    pub use refstr::Str;
    pub use crate::app::command::{Command, CommandRegistry, CommandError};
    pub use crate::app::app::{App, AppError};
}