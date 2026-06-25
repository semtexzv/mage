//! Anthropic Messages API provider for the `llm` crate.
//!
//! Implements `llm::Provider` for the Anthropic Messages streaming API.
//! Handles SSE parsing, message conversion, and partial JSON accumulation
//! for tool call arguments. Authentication is via an Anthropic API key.

pub mod api_types;
pub mod convert;
pub mod events;
pub mod provider;
pub mod models;
pub mod sse;

pub use provider::AnthropicProvider;
