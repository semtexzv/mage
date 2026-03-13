use refstr::Str;

use crate::types::{AssistantMessage, ContentBlock, StopReason, Usage};

/// Delta-only streaming events from an LLM provider.
///
/// Unlike the TypeScript version which embeds the full `partial: AssistantMessage`
/// in every event, we carry only the delta. The consumer owns the
/// `AssistantMessage` and applies deltas via [`apply_event`].
#[derive(Debug, Clone)]
pub enum AssistantMessageEvent {
    /// Stream started.
    Start,
    /// New text content block at `content_index`.
    TextStart { content_index: usize },
    /// Text delta.
    TextDelta { content_index: usize, delta: Str },
    /// Text block complete.
    TextEnd { content_index: usize },
    /// New thinking block at `content_index`.
    ThinkingStart { content_index: usize },
    /// Thinking delta.
    ThinkingDelta { content_index: usize, delta: Str },
    /// Thinking block complete.
    ThinkingEnd { content_index: usize, signature: Option<Str> },
    /// New tool call block at `content_index`. Carries id and name from the provider.
    ToolCallStart { content_index: usize, id: Str, name: Str },
    /// Raw JSON delta for tool call arguments (informational, for UI streaming).
    ToolCallDelta { content_index: usize, delta: Str },
    /// Tool call block complete. Carries the final parsed arguments.
    ToolCallEnd { content_index: usize, arguments: serde_json::Value },
    /// Stream completed normally.
    Done { reason: StopReason },
    /// Stream ended with error.
    Error { reason: StopReason, error: Option<Str> },
    /// Usage update (non-terminal).
    Usage { usage: Usage },
}

impl AssistantMessageEvent {
    pub fn is_terminal(&self) -> bool {
        matches!(self, Self::Done { .. } | Self::Error { .. })
    }
}

/// Apply a streaming event to the mutable assistant message being built.
pub fn apply_event(msg: &mut AssistantMessage, event: &AssistantMessageEvent) {
    match event {
        AssistantMessageEvent::TextStart { .. } => {
            msg.content.push(ContentBlock::Text {
                text: Str::new(),
            });
        }
        AssistantMessageEvent::TextDelta { content_index, delta } => {
            if let Some(ContentBlock::Text { text, .. }) = msg.content.get_mut(*content_index) {
                text.make_mut().push_str(delta);
            }
        }
        AssistantMessageEvent::ThinkingStart { .. } => {
            msg.content.push(ContentBlock::Thinking {
                thinking: Str::new(),
                thinking_signature: None,
            });
        }
        AssistantMessageEvent::ThinkingDelta { content_index, delta } => {
            if let Some(ContentBlock::Thinking { thinking, .. }) = msg.content.get_mut(*content_index) {
                thinking.make_mut().push_str(delta);
            }
        }
        AssistantMessageEvent::ToolCallStart { id, name, .. } => {
            msg.content.push(ContentBlock::ToolCall {
                id: id.clone(),
                name: name.clone(),
                arguments: serde_json::Value::Null,
            });
        }
        AssistantMessageEvent::ToolCallDelta { .. } => {
            // Informational for UI streaming. Provider accumulates JSON internally.
        }
        AssistantMessageEvent::ToolCallEnd { content_index, arguments } => {
            if let Some(ContentBlock::ToolCall { arguments: args, .. }) = msg.content.get_mut(*content_index) {
                *args = arguments.clone();
            }
        }
        AssistantMessageEvent::Done { reason } => {
            msg.stop_reason = *reason;
        }
        AssistantMessageEvent::Error { reason, error } => {
            msg.stop_reason = *reason;
            msg.error_message.clone_from(error);
        }
        AssistantMessageEvent::ThinkingEnd { content_index, signature } => {
            if let (Some(sig), Some(ContentBlock::Thinking { thinking_signature, .. })) =
                (signature, msg.content.get_mut(*content_index))
            {
                *thinking_signature = Some(sig.clone());
            }
        }
        AssistantMessageEvent::Usage { usage } => {
            msg.usage = *usage;
        }
        _ => {}
    }
}
