use anthropic::api_types::{MessageContent, MessageRole, ContentBlockParam};
use anthropic::convert::build_request_body;
use llm::*;
use refstr::Str;
use serde_json::{json, Value};

fn test_model() -> Model {
    Model {
        id: "claude-sonnet-4-20250514".into(),
        name: "Claude Sonnet".into(),
        api: "anthropic-messages".into(),
        provider: "anthropic".into(),
        base_url: "https://api.anthropic.com".into(),
        reasoning: false,
        input: vec![],
        cost: ModelCost::default(),
        context_window: 200_000,
        max_out: 8192,
        headers: None,
    }
}

#[test]
fn build_request_body_basic() {
    let model = test_model();
    let context = Context {
        system_prompt: Some("You are helpful.".into()),
        messages: vec![
            Message::User(UserMessage {
                content: UserMessageContent::Text("Hello".into()),
            }),
        ],
        tools: None,
    };
    let options = StreamOptions::default();
    let body = build_request_body(&model, &context, &options);

    assert_eq!(body.model, "claude-sonnet-4-20250514");
    assert_eq!(body.stream, Some(true));
    assert!(body.max_tokens > 0);

    // Check system prompt
    let system = body.system.as_ref().unwrap();
    assert_eq!(system.len(), 1);
    assert_eq!(system[0].text, "You are helpful.");

    // Check messages
    assert_eq!(body.messages.len(), 1);
    assert!(matches!(body.messages[0].role, MessageRole::User));
    match &body.messages[0].content {
        MessageContent::Text(t) => assert_eq!(t, "Hello"),
        _ => panic!("expected text content"),
    }

    // Serialization round-trip sanity check
    let val: Value = serde_json::to_value(&body).unwrap();
    assert_eq!(val["model"], "claude-sonnet-4-20250514");
    assert_eq!(val["stream"], true);
    assert_eq!(val["messages"][0]["role"], "user");
    assert_eq!(val["messages"][0]["content"], "Hello");
    assert!(val["system"].is_array());
}

#[test]
fn tool_result_batching() {
    let model = test_model();
    let messages = vec![
        Message::Assistant(AssistantMessage {
            content: vec![
                ContentBlock::ToolCall {
                    id: "call1".into(),
                    name: "read".into(),
                    arguments: json!({"path": "/tmp"}),
                },
                ContentBlock::ToolCall {
                    id: "call2".into(),
                    name: "write".into(),
                    arguments: json!({"path": "/tmp/out"}),
                },
            ],
            api: Str::new(),
            provider: Str::new(),
            model: Str::new(),
            usage: Usage::default(),
            stop_reason: StopReason::ToolUse,
            error_message: None,
        }),
        Message::ToolResult(ToolResultMessage {
            tool_call_id: "call1".into(),
            tool_name: "read".into(),
            content: vec![UserContent::Text { text: "file contents".into() }],
            details: None,
            is_error: false,
        }),
        Message::ToolResult(ToolResultMessage {
            tool_call_id: "call2".into(),
            tool_name: "write".into(),
            content: vec![UserContent::Text { text: "ok".into() }],
            details: None,
            is_error: false,
        }),
    ];

    let context = Context {
        system_prompt: None,
        messages,
        tools: None,
    };
    let options = StreamOptions::default();
    let body = build_request_body(&model, &context, &options);

    // Assistant message + one batched user message with 2 tool_results
    assert_eq!(body.messages.len(), 2);
    assert!(matches!(body.messages[0].role, MessageRole::Assistant));
    assert!(matches!(body.messages[1].role, MessageRole::User));
    if let MessageContent::Blocks(ref blocks) = body.messages[1].content {
        assert_eq!(blocks.len(), 2);
        assert!(matches!(blocks[0], ContentBlockParam::ToolResult { .. }));
        assert!(matches!(blocks[1], ContentBlockParam::ToolResult { .. }));
    } else {
        panic!("expected blocks content for tool results");
    }

    // Also verify serialized form
    let val: Value = serde_json::to_value(&body.messages).unwrap();
    assert_eq!(val[0]["role"], "assistant");
    assert_eq!(val[1]["role"], "user");
    let tool_results = val[1]["content"].as_array().unwrap();
    assert_eq!(tool_results.len(), 2);
    assert_eq!(tool_results[0]["type"], "tool_result");
    assert_eq!(tool_results[1]["type"], "tool_result");
}
