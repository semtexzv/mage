use std::fmt;
use std::ops::Deref;

use refstr::Str;
use uuid::Uuid;

use llm::{
    AssistantMessageEvent, Model,
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
// EntryId
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct EntryId(Str);

impl EntryId {
    pub fn generate(existing: impl Fn(&EntryId) -> bool) -> Self {
        loop {
            let id = EntryId(Str::from(Uuid::new_v4().to_string().as_str()));
            if !existing(&id) {
                return id;
            }
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for EntryId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<&str> for EntryId {
    fn from(s: &str) -> Self {
        EntryId(Str::from(s))
    }
}

impl Deref for EntryId {
    type Target = str;

    fn deref(&self) -> &str {
        &self.0
    }
}

// ---------------------------------------------------------------------------
// MessageBody / Message
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub enum MessageBody {
    User {
        content: llm::UserMessageContent,
    },
    Assistant {
        content: Vec<llm::ContentBlock>,
        api: Str,
        provider: Str,
        model: Str,
        usage: llm::Usage,
        stop_reason: llm::StopReason,
        error_message: Option<Str>,
    },
    ToolResult {
        tool_call_id: Str,
        tool_name: Str,
        content: Vec<llm::UserContent>,
        details: Option<serde_json::Value>,
        is_error: bool,
    },
    CompactionSummary {
        summary: String,
        tokens_before: u64,
    },
    BranchSummary {
        summary: String,
        from_id: EntryId,
    },
    Custom {
        custom_type: Str,
        content: llm::UserMessageContent,
        display: bool,
        details: Option<serde_json::Value>,
    },
}

#[derive(Debug, Clone)]
pub struct Message {
    pub body: MessageBody,
    pub timestamp: u64,
    pub ephemeral: bool,
}

impl Message {
    pub fn to_llm(&self) -> llm::Message {
        match &self.body {
            MessageBody::User { content } => {
                llm::Message::User(llm::UserMessage {
                    content: content.clone(),
                    timestamp: self.timestamp,
                })
            }
            MessageBody::Assistant {
                content,
                api,
                provider,
                model,
                usage,
                stop_reason,
                error_message,
            } => {
                llm::Message::Assistant(llm::AssistantMessage {
                    content: content.clone(),
                    api: api.clone(),
                    provider: provider.clone(),
                    model: model.clone(),
                    usage: usage.clone(),
                    stop_reason: stop_reason.clone(),
                    error_message: error_message.clone(),
                    timestamp: self.timestamp,
                })
            }
            MessageBody::ToolResult {
                tool_call_id,
                tool_name,
                content,
                details,
                is_error,
            } => {
                llm::Message::ToolResult(ToolResultMessage {
                    tool_call_id: tool_call_id.clone(),
                    tool_name: tool_name.clone(),
                    content: content.clone(),
                    details: details.clone(),
                    is_error: *is_error,
                    timestamp: self.timestamp,
                })
            }
            MessageBody::CompactionSummary { summary, .. } => {
                llm::Message::User(llm::UserMessage {
                    content: llm::UserMessageContent::Text(
                        format!("<summary>{summary}</summary>"),
                    ),
                    timestamp: self.timestamp,
                })
            }
            MessageBody::BranchSummary { summary, .. } => {
                llm::Message::User(llm::UserMessage {
                    content: llm::UserMessageContent::Text(
                        format!("<summary>{summary}</summary>"),
                    ),
                    timestamp: self.timestamp,
                })
            }
            MessageBody::Custom { content, .. } => {
                llm::Message::User(llm::UserMessage {
                    content: content.clone(),
                    timestamp: self.timestamp,
                })
            }
        }
    }

    pub fn user_text(text: impl Into<String>) -> Self {
        Self {
            body: MessageBody::User {
                content: llm::UserMessageContent::Text(text.into()),
            },
            timestamp: 0,
            ephemeral: false,
        }
    }

    pub fn ephemeral_user_text(text: impl Into<String>) -> Self {
        Self {
            body: MessageBody::User {
                content: llm::UserMessageContent::Text(text.into()),
            },
            timestamp: 0,
            ephemeral: true,
        }
    }

    pub fn from_assistant(a: llm::AssistantMessage) -> Self {
        let timestamp = a.timestamp;
        Self {
            body: MessageBody::Assistant {
                content: a.content,
                api: a.api,
                provider: a.provider,
                model: a.model,
                usage: a.usage,
                stop_reason: a.stop_reason,
                error_message: a.error_message,
            },
            timestamp,
            ephemeral: false,
        }
    }

    pub fn from_tool_result(tr: ToolResultMessage) -> Self {
        let timestamp = tr.timestamp;
        Self {
            body: MessageBody::ToolResult {
                tool_call_id: tr.tool_call_id,
                tool_name: tr.tool_name,
                content: tr.content,
                details: tr.details,
                is_error: tr.is_error,
            },
            timestamp,
            ephemeral: false,
        }
    }

    pub fn compaction_summary(summary: impl Into<String>, tokens_before: u64) -> Self {
        Self {
            body: MessageBody::CompactionSummary {
                summary: summary.into(),
                tokens_before,
            },
            timestamp: 0,
            ephemeral: false,
        }
    }

    pub fn branch_summary(summary: impl Into<String>, from_id: EntryId) -> Self {
        Self {
            body: MessageBody::BranchSummary {
                summary: summary.into(),
                from_id,
            },
            timestamp: 0,
            ephemeral: false,
        }
    }

    pub fn custom(
        custom_type: impl Into<Str>,
        content: llm::UserMessageContent,
        display: bool,
        details: Option<serde_json::Value>,
    ) -> Self {
        Self {
            body: MessageBody::Custom {
                custom_type: custom_type.into(),
                content,
                display,
                details,
            },
            timestamp: 0,
            ephemeral: false,
        }
    }

    pub fn is_ephemeral(&self) -> bool {
        self.ephemeral
    }

    pub fn role_name(&self) -> &'static str {
        match &self.body {
            MessageBody::User { .. } => "user",
            MessageBody::Assistant { .. } => "assistant",
            MessageBody::ToolResult { .. } => "toolResult",
            MessageBody::CompactionSummary { .. } => "compactionSummary",
            MessageBody::BranchSummary { .. } => "branchSummary",
            MessageBody::Custom { .. } => "custom",
        }
    }
}

// ---------------------------------------------------------------------------
// Agent events
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub enum AgentEvent {
    AgentStart,
    AgentEnd {
        messages: Vec<Message>,
    },
    TurnStart,
    TurnEnd {
        message: Message,
        tool_results: Vec<ToolResultMessage>,
    },
    MessageStart {
        message: Message,
    },
    MessageUpdate {
        message: Message,
        assistant_message_event: AssistantMessageEvent,
    },
    MessageEnd {
        message: Message,
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
    pub messages: Vec<Message>,
    pub(crate) tools: Vec<Box<dyn ErasedTool>>,
    pub options: llm::StreamOptions,
}

impl AgentState {
    /// Create a new `AgentState` with no tools.
    pub fn new(
        system_prompt: impl Into<String>,
        model: llm::Model,
        messages: Vec<Message>,
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
