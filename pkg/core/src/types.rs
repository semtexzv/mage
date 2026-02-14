use refstr::Str;

use llm::{
    AssistantMessageEvent, Message, Model,
    ThinkingLevel, ToolResultMessage,
};

use crate::tool::{ErasedTool, ToolResult};

// ---------------------------------------------------------------------------
// Agent-level thinking (extends LLM's ThinkingLevel with "off")
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AgentThinkingLevel {
    #[default]
    Off,
    Minimal,
    Low,
    Medium,
    High,
    XHigh,
}

impl AgentThinkingLevel {
    pub fn to_llm(self) -> Option<ThinkingLevel> {
        match self {
            Self::Off => None,
            Self::Minimal => Some(ThinkingLevel::Minimal),
            Self::Low => Some(ThinkingLevel::Low),
            Self::Medium => Some(ThinkingLevel::Medium),
            Self::High => Some(ThinkingLevel::High),
            Self::XHigh => Some(ThinkingLevel::XHigh),
        }
    }
}

// ---------------------------------------------------------------------------
// DeliverAs — how a hook-injected message should be delivered
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeliverAs {
    /// Interrupt current tool execution with a steering message.
    Steer,
    /// Queue for after current turn completes.
    FollowUp,
    /// Queue for the next user-initiated turn.
    NextTurn,
}

// ---------------------------------------------------------------------------
// AgentMessage
// ---------------------------------------------------------------------------

/// Agent-level message. Wraps LLM `Message` and allows custom variants.
#[derive(Debug, Clone)]
pub enum AgentMessage {
    /// Standard LLM message (user, assistant, tool result).
    Llm(Message),
    /// Application-defined custom message. Opaque to the loop.
    Custom {
        role: Str,
        data: serde_json::Value,
        timestamp: u64,
    },
}

impl AgentMessage {
    pub fn role(&self) -> &str {
        match self {
            Self::Llm(m) => m.role(),
            Self::Custom { role, .. } => role,
        }
    }

    pub fn user_text(text: impl Into<String>) -> Self {
        Self::Llm(Message::User(llm::UserMessage {
            content: llm::UserMessageContent::Text(text.into()),
            timestamp: 0,
        }))
    }
}

// ---------------------------------------------------------------------------
// Agent events
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub enum AgentEvent {
    AgentStart,
    AgentEnd {
        messages: Vec<AgentMessage>,
    },
    TurnStart,
    TurnEnd {
        message: AgentMessage,
        tool_results: Vec<ToolResultMessage>,
    },
    MessageStart {
        message: AgentMessage,
    },
    MessageUpdate {
        message: AgentMessage,
        assistant_message_event: AssistantMessageEvent,
    },
    MessageEnd {
        message: AgentMessage,
    },
    ToolExecutionStart {
        tool_call_id: Str,
        tool_name: Str,
        args: serde_json::Value,
    },
    ToolExecutionEnd {
        tool_call_id: Str,
        tool_name: Str,
        result: ToolResult,
        is_error: bool,
    },
}

impl AgentEvent {
    pub fn is_terminal(&self) -> bool {
        matches!(self, Self::AgentEnd { .. })
    }
}

// ---------------------------------------------------------------------------
// Agent state — the mutable data the loop operates on
// ---------------------------------------------------------------------------

pub struct AgentState {
    pub system_prompt: String,
    pub model: Model,
    pub messages: Vec<AgentMessage>,
    pub(crate) tools: Vec<Box<dyn ErasedTool>>,
    pub options: llm::StreamOptions,
}

impl AgentState {
    /// Create a new `AgentState` with no tools.
    pub fn new(
        system_prompt: impl Into<String>,
        model: llm::Model,
        messages: Vec<AgentMessage>,
        options: llm::StreamOptions,
    ) -> Self {
        Self {
            system_prompt: system_prompt.into(),
            model,
            messages,
            tools: Vec::new(),
            options,
        }
    }

    /// Convert tools to LLM tool schemas.
    pub fn llm_tools(&self) -> Vec<llm::Tool> {
        self.tools.iter().map(|t| t.to_llm_tool()).collect()
    }
}
