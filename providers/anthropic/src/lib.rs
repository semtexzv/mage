//! Anthropic Messages API provider for the `llm` crate.
//!
//! Implements `llm::Provider` for the Anthropic Messages streaming API.
//! Handles SSE parsing, message conversion, partial JSON accumulation
//! for tool call arguments, and OAuth token support for Claude Pro/Max
//! subscriptions.

pub mod api_types;
pub mod convert;
pub mod events;
pub mod oauth;
pub mod login;
pub mod provider;
pub mod models;
pub mod sse;

pub use provider::AnthropicProvider;
