//! Convert `llm::Context` to Anthropic Messages API request body.


use llm::{
    ContentBlock, Context, Message, Model, StreamOptions, Tool,
    UserContent, UserMessage, UserMessageContent,
};

use crate::api_types::{
    Base64ImageSource, CacheControl, ContentBlockParam, CreateMessageRequest,
    InputSchema, MessageContent, MessageParam, MessageRole, TextBlockParam,
    ThinkingConfig, ToolDef, ToolResultContent, ToolResultContentBlock,
};
use crate::oauth;

/// Build the Anthropic API request body from our types.
///
/// When `is_oauth` is true (Claude Pro/Max subscription), the request is
/// shaped to match Claude Code exactly:
/// - System prompt has the CC identity prepended as the first block
/// - Tool names are remapped to CC canonical casing
/// - Assistant-turn tool_use names are remapped
pub fn build_request_body(
    model: &Model,
    context: &Context,
    options: &StreamOptions,
    is_oauth: bool,
) -> CreateMessageRequest {
    let max_tokens = options.max_tokens
        .unwrap_or_else(|| model.max_tokens / 3);

    let system = build_system(context.system_prompt.as_deref(), options, is_oauth);

    let messages = convert_messages(&context.messages, is_oauth);

    let tools = context.tools.as_ref().and_then(|t| {
        if t.is_empty() { None } else { Some(convert_tools(t, is_oauth)) }
    });

    let thinking = options.reasoning.as_ref().map(|_| {
        ThinkingConfig::Enabled { budget_tokens: 1024 }
    });

    CreateMessageRequest {
        model: model.id.to_string(),
        max_tokens,
        messages,
        stream: Some(true),
        system,
        temperature: options.temperature,
        tools,
        tool_choice: None,
        thinking,
        stop_sequences: None,
        metadata: None,
        output_config: None,
    }
}

/// Build the system prompt blocks.
///
/// OAuth mode: CC identity block first, then the user's system prompt (if any).
/// API key mode: just the user's system prompt (if any).
fn build_system(
    system_prompt: Option<&str>,
    options: &StreamOptions,
    is_oauth: bool,
) -> Option<Vec<TextBlockParam>> {
    let cache_control = options.cache_retention.map(|_| CacheControl::ephemeral());

    if is_oauth {
        let mut blocks = vec![
            TextBlockParam::new(
                oauth::CLAUDE_CODE_SYSTEM_PROMPT.to_string(),
                cache_control.clone(),
            ),
        ];
        if let Some(prompt) = system_prompt {
            blocks.push(TextBlockParam::new(prompt.to_string(), cache_control));
        }
        Some(blocks)
    } else {
        system_prompt.map(|prompt| {
            vec![TextBlockParam::new(prompt.to_string(), cache_control)]
        })
    }
}

fn try_batch_tool_result(result: &mut Vec<MessageParam>, block: &ContentBlockParam) -> bool {
    let Some(last) = result.last_mut() else { return false };
    if !matches!(last.role, MessageRole::User) { return false; }
    let MessageContent::Blocks(ref mut arr) = last.content else { return false; };
    if !arr.iter().any(|b| matches!(b, ContentBlockParam::ToolResult { .. })) { return false; }
    arr.push(block.clone());
    true
}

fn convert_messages(messages: &[Message], is_oauth: bool) -> Vec<MessageParam> {
    let mut result = Vec::new();

    for msg in messages {
        match msg {
            Message::User(user_msg) => {
                if let Some(v) = convert_user_message(user_msg) {
                    result.push(v);
                }
            }
            Message::Assistant(asst) => {
                let blocks = convert_assistant_content(&asst.content, is_oauth);
                if !blocks.is_empty() {
                    result.push(MessageParam {
                        role: MessageRole::Assistant,
                        content: MessageContent::Blocks(blocks),
                    });
                }
            }
            Message::ToolResult(tr) => {
                let block = ContentBlockParam::ToolResult {
                    tool_use_id: tr.tool_call_id.to_string(),
                    content: Some(convert_user_content_blocks(&tr.content)),
                    is_error: Some(tr.is_error),
                    cache_control: None,
                };

                if !try_batch_tool_result(&mut result, &block) {
                    result.push(MessageParam {
                        role: MessageRole::User,
                        content: MessageContent::Blocks(vec![block]),
                    });
                }
            }
        }
    }

    result
}

fn convert_user_message(msg: &UserMessage) -> Option<MessageParam> {
    match &msg.content {
        UserMessageContent::Text(text) => {
            let trimmed = text.trim();
            if trimmed.is_empty() {
                return None;
            }
            Some(MessageParam {
                role: MessageRole::User,
                content: MessageContent::Text(trimmed.to_string()),
            })
        }
        UserMessageContent::Rich(blocks) => {
            let converted: Vec<ContentBlockParam> = blocks.iter()
                .filter_map(|block| match block {
                    UserContent::Text { text } => {
                        let t = text.trim();
                        if t.is_empty() { None }
                        else {
                            Some(ContentBlockParam::Text {
                                text: t.to_string(),
                                cache_control: None,
                            })
                        }
                    }
                    UserContent::Image { data, mime_type } => {
                        Some(ContentBlockParam::Image {
                            source: Base64ImageSource::new(
                                mime_type.to_string(),
                                data.to_string(),
                            ),
                            cache_control: None,
                        })
                    }
                })
                .collect();
            if converted.is_empty() {
                return None;
            }
            Some(MessageParam {
                role: MessageRole::User,
                content: MessageContent::Blocks(converted),
            })
        }
    }
}

fn convert_assistant_content(content: &[ContentBlock], is_oauth: bool) -> Vec<ContentBlockParam> {
    let mut blocks = Vec::new();
    for block in content {
        match block {
            ContentBlock::Text { text, .. } => {
                if !text.trim().is_empty() {
                    blocks.push(ContentBlockParam::Text {
                        text: text.to_string(),
                        cache_control: None,
                    });
                }
            }
            ContentBlock::Thinking { thinking, thinking_signature } => {
                if thinking.trim().is_empty() { continue; }
                let valid_sig = thinking_signature.as_ref()
                    .filter(|s| !s.is_empty());
                match valid_sig {
                    Some(sig) => blocks.push(ContentBlockParam::Thinking {
                        thinking: thinking.to_string(),
                        signature: sig.to_string(),
                    }),
                    None => blocks.push(ContentBlockParam::Text {
                        text: thinking.to_string(),
                        cache_control: None,
                    }),
                }
            }
            ContentBlock::ToolCall { id, name, arguments, .. } => {
                let outbound_name = if is_oauth {
                    oauth::to_cc_tool_name(name).to_string()
                } else {
                    name.to_string()
                };
                blocks.push(ContentBlockParam::ToolUse {
                    id: id.to_string(),
                    name: outbound_name,
                    input: arguments.clone(),
                    cache_control: None,
                });
            }
        }
    }
    blocks
}

fn convert_user_content_blocks(content: &[UserContent]) -> ToolResultContent {
    let has_images = content.iter().any(|c| matches!(c, UserContent::Image { .. }));
    if !has_images {
        // Simple text — concatenate
        let text: String = content.iter()
            .filter_map(|c| match c {
                UserContent::Text { text } => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n");
        return ToolResultContent::Text(text);
    }
    // Mixed content
    ToolResultContent::Blocks(content.iter().filter_map(|c| match c {
        UserContent::Text { text } => {
            Some(ToolResultContentBlock::Text { text: text.to_string() })
        }
        UserContent::Image { data, mime_type } => {
            Some(ToolResultContentBlock::Image {
                source: Base64ImageSource::new(
                    mime_type.to_string(),
                    data.to_string(),
                ),
            })
        }
    }).collect())
}

fn convert_tools(tools: &[Tool], is_oauth: bool) -> Vec<ToolDef> {
    tools.iter().map(|tool| {
        let params = &tool.parameters;
        let properties = params.get("properties").cloned();
        let required = params.get("required")
            .and_then(|v| v.as_array())
            .map(|arr| arr.iter().filter_map(|v| v.as_str().map(String::from)).collect());

        let name = if is_oauth {
            oauth::to_cc_tool_name(&tool.name).to_string()
        } else {
            tool.name.to_string()
        };

        ToolDef {
            name,
            description: Some(tool.description.to_string()),
            input_schema: InputSchema {
                schema_type: "object",
                properties,
                required,
            },
            cache_control: None,
        }
    }).collect()
}
