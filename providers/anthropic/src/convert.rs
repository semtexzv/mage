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
    let max_tokens = options.max_out
        .unwrap_or_else(|| model.max_out / 3);

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

    // Sanitize: ensure every tool_use has a matching tool_result.
    // If an assistant message has tool_use blocks but the next message
    // doesn't contain all matching tool_results, inject synthetic ones.
    sanitize_tool_use_pairing(&mut result);

    result
}

/// Ensure every tool_use block has a corresponding tool_result.
///
/// Scans the message list for assistant messages containing tool_use blocks.
/// If the immediately following user message doesn't contain tool_results
/// for all tool_use ids, synthetic error results are injected.
fn sanitize_tool_use_pairing(messages: &mut Vec<MessageParam>) {
    let mut i = 0;
    while i < messages.len() {
        if !matches!(messages[i].role, MessageRole::Assistant) {
            i += 1;
            continue;
        }

        // Collect tool_use ids from this assistant message.
        let tool_ids: Vec<String> = match &messages[i].content {
            MessageContent::Blocks(blocks) => blocks
                .iter()
                .filter_map(|b| match b {
                    ContentBlockParam::ToolUse { id, .. } => Some(id.clone()),
                    _ => None,
                })
                .collect(),
            _ => Vec::new(),
        };

        if tool_ids.is_empty() {
            i += 1;
            continue;
        }

        // Check the next message for matching tool_results.
        let next_result_ids: Vec<String> = if i + 1 < messages.len() {
            match &messages[i + 1].content {
                MessageContent::Blocks(blocks) => blocks
                    .iter()
                    .filter_map(|b| match b {
                        ContentBlockParam::ToolResult { tool_use_id, .. } => {
                            Some(tool_use_id.clone())
                        }
                        _ => None,
                    })
                    .collect(),
                _ => Vec::new(),
            }
        } else {
            Vec::new()
        };

        // Find missing tool_results.
        let missing: Vec<&String> = tool_ids
            .iter()
            .filter(|id| !next_result_ids.contains(id))
            .collect();

        if !missing.is_empty() {
            // Build synthetic tool_result blocks.
            let synthetic: Vec<ContentBlockParam> = missing
                .iter()
                .map(|id| ContentBlockParam::ToolResult {
                    tool_use_id: (*id).clone(),
                    content: Some(ToolResultContent::Text(
                        "Tool execution was interrupted.".to_string(),
                    )),
                    is_error: Some(true),
                    cache_control: None,
                })
                .collect();

            if i + 1 < messages.len() && matches!(messages[i + 1].role, MessageRole::User) {
                // Append to existing user message.
                if let MessageContent::Blocks(ref mut blocks) = messages[i + 1].content {
                    blocks.extend(synthetic);
                }
            } else {
                // Insert a new user message with the synthetic results.
                messages.insert(
                    i + 1,
                    MessageParam {
                        role: MessageRole::User,
                        content: MessageContent::Blocks(synthetic),
                    },
                );
            }
        }

        i += 2; // Skip past assistant + user pair.
    }
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
