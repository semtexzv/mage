use anthropic::api_types::{ContentBlockParam, MessageContent};
use anthropic::convert::build_request_body;
use anthropic::oauth;
use llm::*;
use refstr::Str;
use serde_json::json;

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

// ---------------------------------------------------------------------------
// Token detection
// ---------------------------------------------------------------------------

#[test]
fn detect_oauth_token() {
    assert!(oauth::is_oauth_token("sk-ant-oat-abc123"));
    assert!(oauth::is_oauth_token("sk-ant-oat-"));
}

#[test]
fn detect_api_key() {
    assert!(!oauth::is_oauth_token("sk-ant-api01-abc123"));
    assert!(!oauth::is_oauth_token("sk-abc123"));
    assert!(!oauth::is_oauth_token(""));
}

// ---------------------------------------------------------------------------
// Tool name mapping
// ---------------------------------------------------------------------------

#[test]
fn to_cc_tool_name_known() {
    assert_eq!(oauth::to_cc_tool_name("read"), "Read");
    assert_eq!(oauth::to_cc_tool_name("bash"), "Bash");
    assert_eq!(oauth::to_cc_tool_name("todowrite"), "TodoWrite");
    assert_eq!(oauth::to_cc_tool_name("WebSearch"), "WebSearch");
    assert_eq!(oauth::to_cc_tool_name("GREP"), "Grep");
}

#[test]
fn to_cc_tool_name_unknown_passthrough() {
    assert_eq!(oauth::to_cc_tool_name("my_custom_tool"), "my_custom_tool");
    assert_eq!(oauth::to_cc_tool_name("FancyTool"), "FancyTool");
}

#[test]
fn from_cc_tool_name_roundtrip() {
    let originals = vec![
        "read_file".to_string(),
        "write_file".to_string(),
        "bash_exec".to_string(),
    ];
    // These don't match CC names, so they pass through
    assert_eq!(oauth::from_cc_tool_name("Read", &originals), "Read");

    // Now with names that DO match CC tool names
    let originals = vec!["read".to_string(), "write".to_string(), "bash".to_string()];
    assert_eq!(oauth::from_cc_tool_name("Read", &originals), "read");
    assert_eq!(oauth::from_cc_tool_name("Write", &originals), "write");
    assert_eq!(oauth::from_cc_tool_name("Bash", &originals), "bash");
}

#[test]
fn from_cc_tool_name_custom_passthrough() {
    let originals = vec!["my_tool".to_string()];
    // Unknown CC name, unknown original — return as-is
    assert_eq!(oauth::from_cc_tool_name("UnknownTool", &originals), "UnknownTool");
}

// ---------------------------------------------------------------------------
// Credentials
// ---------------------------------------------------------------------------

#[test]
fn credentials_expired() {
    let creds = oauth::OAuthCredentials {
        refresh_token: "rt".into(),
        access_token: "at".into(),
        expires_at_ms: 0, // long expired
    };
    assert!(creds.is_expired());
}

#[test]
fn credentials_not_expired() {
    let creds = oauth::OAuthCredentials {
        refresh_token: "rt".into(),
        access_token: "at".into(),
        expires_at_ms: u64::MAX, // far future
    };
    assert!(!creds.is_expired());
}

// ---------------------------------------------------------------------------
// OAuth-mode request building
// ---------------------------------------------------------------------------

#[test]
fn oauth_mode_prepends_cc_system_prompt() {
    let model = test_model();
    let context = Context {
        system_prompt: Some("You are helpful.".into()),
        messages: vec![
            Message::User(UserMessage {
                content: UserMessageContent::Text("Hi".into()),
            }),
        ],
        tools: None,
    };
    let options = StreamOptions::default();
    let body = build_request_body(&model, &context, &options, true);

    let system = body.system.as_ref().expect("system should be set in oauth mode");
    assert_eq!(system.len(), 2);
    assert_eq!(system[0].text, oauth::CLAUDE_CODE_SYSTEM_PROMPT);
    assert_eq!(system[1].text, "You are helpful.");
}

#[test]
fn oauth_mode_cc_system_prompt_even_without_user_prompt() {
    let model = test_model();
    let context = Context {
        system_prompt: None,
        messages: vec![
            Message::User(UserMessage {
                content: UserMessageContent::Text("Hi".into()),
            }),
        ],
        tools: None,
    };
    let options = StreamOptions::default();
    let body = build_request_body(&model, &context, &options, true);

    let system = body.system.as_ref().expect("system must be present in oauth mode");
    assert_eq!(system.len(), 1);
    assert_eq!(system[0].text, oauth::CLAUDE_CODE_SYSTEM_PROMPT);
}

#[test]
fn api_key_mode_no_cc_system_prompt() {
    let model = test_model();
    let context = Context {
        system_prompt: Some("You are helpful.".into()),
        messages: vec![
            Message::User(UserMessage {
                content: UserMessageContent::Text("Hi".into()),
            }),
        ],
        tools: None,
    };
    let options = StreamOptions::default();
    let body = build_request_body(&model, &context, &options, false);

    let system = body.system.as_ref().unwrap();
    assert_eq!(system.len(), 1);
    assert_eq!(system[0].text, "You are helpful.");
}

#[test]
fn api_key_mode_no_system_prompt_at_all() {
    let model = test_model();
    let context = Context {
        system_prompt: None,
        messages: vec![
            Message::User(UserMessage {
                content: UserMessageContent::Text("Hi".into()),
            }),
        ],
        tools: None,
    };
    let options = StreamOptions::default();
    let body = build_request_body(&model, &context, &options, false);
    assert!(body.system.is_none());
}

#[test]
fn oauth_mode_remaps_tool_names_in_definitions() {
    let model = test_model();
    let context = Context {
        system_prompt: None,
        messages: vec![
            Message::User(UserMessage {
                content: UserMessageContent::Text("Hi".into()),
            }),
        ],
        tools: Some(vec![
            Tool {
                name: "read".into(),
                description: "Read a file".into(),
                parameters: json!({"type": "object", "properties": {"path": {"type": "string"}}}),
            },
            Tool {
                name: "custom_tool".into(),
                description: "Something custom".into(),
                parameters: json!({"type": "object"}),
            },
        ]),
    };
    let options = StreamOptions::default();
    let body = build_request_body(&model, &context, &options, true);

    let tools = body.tools.as_ref().unwrap();
    assert_eq!(tools[0].name, "Read");          // remapped
    assert_eq!(tools[1].name, "custom_tool");    // passthrough
}

#[test]
fn api_key_mode_preserves_tool_names() {
    let model = test_model();
    let context = Context {
        system_prompt: None,
        messages: vec![
            Message::User(UserMessage {
                content: UserMessageContent::Text("Hi".into()),
            }),
        ],
        tools: Some(vec![
            Tool {
                name: "read".into(),
                description: "Read a file".into(),
                parameters: json!({"type": "object"}),
            },
        ]),
    };
    let options = StreamOptions::default();
    let body = build_request_body(&model, &context, &options, false);

    let tools = body.tools.as_ref().unwrap();
    assert_eq!(tools[0].name, "read"); // not remapped
}

#[test]
fn oauth_mode_remaps_tool_names_in_assistant_messages() {
    let model = test_model();
    let context = Context {
        system_prompt: None,
        messages: vec![
            Message::Assistant(AssistantMessage {
                content: vec![
                    ContentBlock::ToolCall {
                        id: "call1".into(),
                        name: "read".into(),
                        arguments: json!({"path": "/tmp"}),
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
                content: vec![UserContent::Text { text: "data".into() }],
                details: None,
                is_error: false,
            }),
        ],
        tools: None,
    };
    let options = StreamOptions::default();
    let body = build_request_body(&model, &context, &options, true);

    // The assistant turn should have the tool_use name remapped
    if let MessageContent::Blocks(ref blocks) = body.messages[0].content {
        match &blocks[0] {
            ContentBlockParam::ToolUse { name, .. } => {
                assert_eq!(name, "Read"); // remapped
            }
            other => panic!("expected ToolUse, got {:?}", other),
        }
    } else {
        panic!("expected blocks content");
    }
}

#[test]
fn oauth_serialization_matches_expected_shape() {
    let model = test_model();
    let context = Context {
        system_prompt: Some("Be helpful.".into()),
        messages: vec![
            Message::User(UserMessage {
                content: UserMessageContent::Text("Hello".into()),
            }),
        ],
        tools: Some(vec![
            Tool {
                name: "bash".into(),
                description: "Run commands".into(),
                parameters: json!({"type": "object", "properties": {"cmd": {"type": "string"}}}),
            },
        ]),
    };
    let options = StreamOptions::default();
    let body = build_request_body(&model, &context, &options, true);

    let val: serde_json::Value = serde_json::to_value(&body).unwrap();

    // System prompt: CC identity first, then user's
    let system = val["system"].as_array().unwrap();
    assert_eq!(system.len(), 2);
    assert!(system[0]["text"].as_str().unwrap().contains("Claude Code"));
    assert_eq!(system[1]["text"], "Be helpful.");

    // Tool name remapped
    assert_eq!(val["tools"][0]["name"], "Bash");

    // Messages preserved
    assert_eq!(val["messages"][0]["role"], "user");
    assert_eq!(val["messages"][0]["content"], "Hello");
}
