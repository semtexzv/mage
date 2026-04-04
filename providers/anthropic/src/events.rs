//! Map Anthropic SSE events to `llm::AssistantMessageEvent`.

use refstr::Str;
use serde::Deserialize;
use llm::{AssistantMessageEvent, StopReason, Usage};

use llm::json_accum::JsonAccumulator;

// ---------------------------------------------------------------------------
// SSE payload types (deserialized from JSON `data` field)
// ---------------------------------------------------------------------------

/// Anthropic API usage fields (all optional — different events include different subsets).
#[derive(Deserialize, Default)]
struct ApiUsage {
    input_tokens: Option<u64>,
    output_tokens: Option<u64>,
    cache_read_input_tokens: Option<u64>,
    cache_creation_input_tokens: Option<u64>,
}

#[derive(Deserialize)]
struct MessageStartPayload {
    message: MessageStartBody,
}

#[derive(Deserialize)]
struct MessageStartBody {
    usage: ApiUsage,
}

#[derive(Deserialize)]
struct BlockStartPayload {
    index: usize,
    content_block: ContentBlockStart,
}

#[derive(Deserialize)]
#[serde(tag = "type")]
enum ContentBlockStart {
    #[serde(rename = "text")]
    Text {},
    #[serde(rename = "thinking")]
    Thinking {},
    #[serde(rename = "tool_use")]
    ToolUse { id: String, name: String },
}

#[derive(Deserialize)]
struct BlockDeltaPayload {
    index: usize,
    delta: ContentBlockDelta,
}

#[derive(Deserialize)]
#[serde(tag = "type")]
enum ContentBlockDelta {
    #[serde(rename = "text_delta")]
    TextDelta { text: String },
    #[serde(rename = "thinking_delta")]
    ThinkingDelta { thinking: String },
    #[serde(rename = "input_json_delta")]
    InputJsonDelta { partial_json: String },
    #[serde(rename = "signature_delta")]
    SignatureDelta { signature: String },
}

#[derive(Deserialize)]
struct BlockStopPayload {
    index: usize,
}

#[derive(Deserialize)]
struct MessageDeltaPayload {
    delta: MessageDeltaBody,
    usage: Option<ApiUsage>,
}

#[derive(Deserialize)]
struct MessageDeltaBody {
    stop_reason: Option<String>,
}

#[derive(Deserialize)]
struct ErrorPayload {
    error: Option<ErrorBody>,
}

#[derive(Deserialize)]
struct ErrorBody {
    message: Option<String>,
}

// ---------------------------------------------------------------------------
// EventMapper
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BlockType {
    Text,
    Thinking,
    ToolUse,
}

/// Tracks state needed to map Anthropic streaming events to our events.
pub struct EventMapper {
    /// Per-block JSON accumulators for tool call arguments.
    tool_json: Vec<Option<JsonAccumulator>>,
    /// Per-block accumulated signature strings (for thinking blocks).
    signatures: Vec<String>,
    /// Per-block type tracking.
    block_types: Vec<BlockType>,
    /// Usage captured from message_start and message_delta.
    pub usage: Usage,
}

impl EventMapper {
    pub fn new() -> Self {
        Self {
            tool_json: Vec::new(),
            signatures: Vec::new(),
            block_types: Vec::new(),
            usage: Usage::default(),
        }
    }

    /// Ensure all three per-block vecs have entries up to (and including) `index`.
    fn ensure_index(&mut self, index: usize) {
        if self.tool_json.len() <= index {
            self.tool_json.resize_with(index + 1, || None);
            self.signatures.resize(index + 1, String::new());
            self.block_types.resize(index + 1, BlockType::Text);
        }
    }

    /// Map an Anthropic SSE event (type + JSON data) to zero or more of our events.
    pub fn map_event(&mut self, event_type: &str, data: &str) -> Vec<AssistantMessageEvent> {
        // Debug: log raw SSE events to ~/.mage/sse.log
        if let Some(home) = dirs::home_dir() {
            use std::io::Write;
            if let Ok(mut f) = std::fs::OpenOptions::new()
                .create(true).append(true)
                .open(home.join(".mage/sse.log"))
            {
                let _ = writeln!(f, "event: {event_type}");
                let _ = writeln!(f, "data: {data}");
                let _ = writeln!(f);
            }
        }
        match event_type {
            "message_start" => self.on_message_start(data),
            "content_block_start" => self.on_content_block_start(data),
            "content_block_delta" => self.on_content_block_delta(data),
            "content_block_stop" => self.on_content_block_stop(data),
            "message_delta" => self.on_message_delta(data),
            "message_stop" => vec![],  // We emit Done from message_delta's stop_reason
            "ping" => vec![],
            "error" => self.on_error(data),
            _ => vec![],
        }
    }

    fn on_message_start(&mut self, data: &str) -> Vec<AssistantMessageEvent> {
        if let Ok(p) = serde_json::from_str::<MessageStartPayload>(data) {
            self.usage.input = p.message.usage.input_tokens.unwrap_or(0);
            self.usage.output = p.message.usage.output_tokens.unwrap_or(0);
            self.usage.cache_read = p.message.usage.cache_read_input_tokens.unwrap_or(0);
            self.usage.cache_write = p.message.usage.cache_creation_input_tokens.unwrap_or(0);
            self.usage.total_tokens = self.usage.input + self.usage.output
                + self.usage.cache_read + self.usage.cache_write;
        }
        vec![AssistantMessageEvent::Start, AssistantMessageEvent::Usage { usage: self.usage }]
    }

    fn on_content_block_start(&mut self, data: &str) -> Vec<AssistantMessageEvent> {
        let Ok(p) = serde_json::from_str::<BlockStartPayload>(data) else {
            return vec![];
        };

        self.ensure_index(p.index);

        let content_index = p.index;
        match p.content_block {
            ContentBlockStart::Text {} => {
                self.block_types[content_index] = BlockType::Text;
                vec![AssistantMessageEvent::TextStart { content_index }]
            }
            ContentBlockStart::Thinking {} => {
                self.block_types[content_index] = BlockType::Thinking;
                self.signatures[content_index] = String::new();
                vec![AssistantMessageEvent::ThinkingStart { content_index }]
            }
            ContentBlockStart::ToolUse { id, name } => {
                self.block_types[content_index] = BlockType::ToolUse;
                self.tool_json[content_index] = Some(JsonAccumulator::new());
                vec![AssistantMessageEvent::ToolCallStart {
                    content_index,
                    id: Str::from(id.as_str()),
                    name: Str::from(name.as_str()),
                }]
            }
        }
    }

    fn on_content_block_delta(&mut self, data: &str) -> Vec<AssistantMessageEvent> {
        let Ok(p) = serde_json::from_str::<BlockDeltaPayload>(data) else {
            return vec![];
        };

        let content_index = p.index;
        match p.delta {
            ContentBlockDelta::TextDelta { text } => {
                vec![AssistantMessageEvent::TextDelta {
                    content_index,
                    delta: Str::from(text.as_str()),
                }]
            }
            ContentBlockDelta::ThinkingDelta { thinking } => {
                vec![AssistantMessageEvent::ThinkingDelta {
                    content_index,
                    delta: Str::from(thinking.as_str()),
                }]
            }
            ContentBlockDelta::InputJsonDelta { partial_json } => {
                // Accumulate JSON for tool call arguments
                if let Some(Some(accum)) = self.tool_json.get_mut(content_index) {
                    accum.push(&partial_json);
                }
                vec![AssistantMessageEvent::ToolCallDelta {
                    content_index,
                    delta: Str::from(partial_json.as_str()),
                }]
            }
            ContentBlockDelta::SignatureDelta { signature } => {
                // Accumulate signature for thinking blocks
                if let Some(buf) = self.signatures.get_mut(content_index) {
                    buf.push_str(&signature);
                }
                vec![]
            }
        }
    }

    fn on_content_block_stop(&mut self, data: &str) -> Vec<AssistantMessageEvent> {
        let Ok(p) = serde_json::from_str::<BlockStopPayload>(data) else {
            return vec![];
        };

        let content_index = p.index;

        // If this was a tool call block, finalize the arguments
        if let Some(Some(accum)) = self.tool_json.get_mut(content_index) {
            let arguments = accum.finish();
            self.tool_json[content_index] = None;
            return vec![AssistantMessageEvent::ToolCallEnd {
                content_index,
                arguments,
            }];
        }

        // For text/thinking blocks, emit the appropriate End event.
        let block_type = self.block_types.get(content_index).copied().unwrap_or(BlockType::Text);
        if block_type == BlockType::Thinking {
            let signature = self.signatures.get(content_index)
                .filter(|s| !s.is_empty())
                .map(|s| Str::from(s.as_str()));
            return vec![AssistantMessageEvent::ThinkingEnd { content_index, signature }];
        }
        vec![AssistantMessageEvent::TextEnd { content_index }]
    }

    fn on_message_delta(&mut self, data: &str) -> Vec<AssistantMessageEvent> {
        let Ok(p) = serde_json::from_str::<MessageDeltaPayload>(data) else {
            return vec![];
        };

        // Update usage if present
        if let Some(u) = &p.usage {
            if let Some(v) = u.input_tokens { self.usage.input = v; }
            if let Some(v) = u.output_tokens { self.usage.output = v; }
            if let Some(v) = u.cache_read_input_tokens { self.usage.cache_read = v; }
            if let Some(v) = u.cache_creation_input_tokens { self.usage.cache_write = v; }
            self.usage.total_tokens = self.usage.input + self.usage.output
                + self.usage.cache_read + self.usage.cache_write;
        }

        let reason = p.delta.stop_reason.as_deref().map(map_stop_reason)
            .unwrap_or(StopReason::Stop);

        vec![AssistantMessageEvent::Usage { usage: self.usage }, AssistantMessageEvent::Done { reason }]
    }

    fn on_error(&mut self, data: &str) -> Vec<AssistantMessageEvent> {
        let message = serde_json::from_str::<ErrorPayload>(data).ok()
            .and_then(|p| p.error)
            .and_then(|e| e.message);

        vec![AssistantMessageEvent::Error {
            reason: StopReason::Error,
            error: message.map(|m| Str::from(m.as_str())),
        }]
    }
}

/// Map Anthropic stop_reason string to our StopReason.
fn map_stop_reason(reason: &str) -> StopReason {
    match reason {
        "end_turn" | "pause_turn" | "stop_sequence" => StopReason::Stop,
        "max_tokens" => StopReason::Length,
        "tool_use" => StopReason::ToolUse,
        "refusal" | "sensitive" => StopReason::Error,
        _ => StopReason::Stop,
    }
}
