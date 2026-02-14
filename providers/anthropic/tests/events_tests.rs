use anthropic::events::EventMapper;
use llm::{AssistantMessageEvent, StopReason};

#[test]
fn message_start() {
    let mut mapper = EventMapper::new();
    let events = mapper.map_event("message_start", r#"{
        "type": "message_start",
        "message": {
            "id": "msg_123",
            "type": "message",
            "role": "assistant",
            "content": [],
            "model": "claude-sonnet-4-20250514",
            "stop_reason": null,
            "usage": {"input_tokens": 100, "output_tokens": 0}
        }
    }"#);
    assert_eq!(events.len(), 2);
    assert!(matches!(events[0], AssistantMessageEvent::Start));
    assert_eq!(mapper.usage.input, 100);
}

#[test]
fn text_block_flow() {
    let mut mapper = EventMapper::new();

    let e1 = mapper.map_event("content_block_start", r#"{
        "type": "content_block_start",
        "index": 0,
        "content_block": {"type": "text", "text": ""}
    }"#);
    assert!(matches!(e1[0], AssistantMessageEvent::TextStart { content_index: 0 }));

    let e2 = mapper.map_event("content_block_delta", r#"{
        "type": "content_block_delta",
        "index": 0,
        "delta": {"type": "text_delta", "text": "Hello"}
    }"#);
    assert!(matches!(&e2[0], AssistantMessageEvent::TextDelta { content_index: 0, delta } if &**delta == "Hello"));

    let e3 = mapper.map_event("content_block_stop", r#"{"type": "content_block_stop", "index": 0}"#);
    assert!(matches!(e3[0], AssistantMessageEvent::TextEnd { content_index: 0 }));
}

#[test]
fn tool_use_flow() {
    let mut mapper = EventMapper::new();

    let e1 = mapper.map_event("content_block_start", r#"{
        "type": "content_block_start",
        "index": 0,
        "content_block": {"type": "tool_use", "id": "toolu_123", "name": "read_file"}
    }"#);
    assert!(matches!(&e1[0], AssistantMessageEvent::ToolCallStart {
        content_index: 0, id, name
    } if &**id == "toolu_123" && &**name == "read_file"));

    mapper.map_event("content_block_delta", r#"{
        "type": "content_block_delta",
        "index": 0,
        "delta": {"type": "input_json_delta", "partial_json": "{\"path\":"}
    }"#);
    mapper.map_event("content_block_delta", r#"{
        "type": "content_block_delta",
        "index": 0,
        "delta": {"type": "input_json_delta", "partial_json": "\"/tmp/test\"}"}
    }"#);

    let e3 = mapper.map_event("content_block_stop", r#"{"type": "content_block_stop", "index": 0}"#);
    match &e3[0] {
        AssistantMessageEvent::ToolCallEnd { content_index: 0, arguments } => {
            assert_eq!(arguments["path"], "/tmp/test");
        }
        other => panic!("expected ToolCallEnd, got {:?}", other),
    }
}

#[test]
fn message_delta_stop() {
    let mut mapper = EventMapper::new();
    let events = mapper.map_event("message_delta", r#"{
        "type": "message_delta",
        "delta": {"stop_reason": "end_turn"},
        "usage": {"output_tokens": 42}
    }"#);
    assert!(matches!(events[1], AssistantMessageEvent::Done { reason: StopReason::Stop }));
    assert_eq!(mapper.usage.output, 42);
}

#[test]
fn tool_use_stop_reason() {
    let mut mapper = EventMapper::new();
    let events = mapper.map_event("message_delta", r#"{
        "type": "message_delta",
        "delta": {"stop_reason": "tool_use"},
        "usage": {"output_tokens": 10}
    }"#);
    assert!(matches!(events[1], AssistantMessageEvent::Done { reason: StopReason::ToolUse }));
}

#[test]
fn error_event() {
    let mut mapper = EventMapper::new();
    let events = mapper.map_event("error", r#"{
        "type": "error",
        "error": {"type": "overloaded_error", "message": "Overloaded"}
    }"#);
    match &events[0] {
        AssistantMessageEvent::Error { reason: StopReason::Error, error } => {
            assert_eq!(error.as_deref(), Some("Overloaded"));
        }
        other => panic!("expected Error event, got {:?}", other),
    }
}

#[test]
fn thinking_with_signature() {
    let mut mapper = EventMapper::new();

    // Start thinking block
    let e1 = mapper.map_event("content_block_start", r#"{
        "type": "content_block_start",
        "index": 0,
        "content_block": {"type": "thinking", "thinking": ""}
    }"#);
    assert!(matches!(e1[0], AssistantMessageEvent::ThinkingStart { content_index: 0 }));

    // Thinking delta
    mapper.map_event("content_block_delta", r#"{
        "type": "content_block_delta",
        "index": 0,
        "delta": {"type": "thinking_delta", "thinking": "Let me think..."}
    }"#);

    // Signature deltas
    mapper.map_event("content_block_delta", r#"{
        "type": "content_block_delta",
        "index": 0,
        "delta": {"type": "signature_delta", "signature": "sig_part1"}
    }"#);
    mapper.map_event("content_block_delta", r#"{
        "type": "content_block_delta",
        "index": 0,
        "delta": {"type": "signature_delta", "signature": "_part2"}
    }"#);

    // Stop thinking block
    let e3 = mapper.map_event("content_block_stop", r#"{"type": "content_block_stop", "index": 0}"#);
    match &e3[0] {
        AssistantMessageEvent::ThinkingEnd { content_index: 0, signature } => {
            assert_eq!(signature.as_deref(), Some("sig_part1_part2"));
        }
        other => panic!("expected ThinkingEnd with signature, got {:?}", other),
    }
}

#[test]
fn thinking_without_signature() {
    let mut mapper = EventMapper::new();

    mapper.map_event("content_block_start", r#"{
        "type": "content_block_start",
        "index": 0,
        "content_block": {"type": "thinking", "thinking": ""}
    }"#);

    mapper.map_event("content_block_delta", r#"{
        "type": "content_block_delta",
        "index": 0,
        "delta": {"type": "thinking_delta", "thinking": "Thinking..."}
    }"#);

    let e3 = mapper.map_event("content_block_stop", r#"{"type": "content_block_stop", "index": 0}"#);
    match &e3[0] {
        AssistantMessageEvent::ThinkingEnd { content_index: 0, signature } => {
            assert!(signature.is_none());
        }
        other => panic!("expected ThinkingEnd without signature, got {:?}", other),
    }
}

#[test]
fn usage_events() {
    let mut mapper = EventMapper::new();

    // message_start should emit Start + Usage
    let events = mapper.map_event("message_start", r#"{
        "type": "message_start",
        "message": {
            "id": "msg_1",
            "type": "message",
            "role": "assistant",
            "content": [],
            "model": "claude-sonnet-4-20250514",
            "stop_reason": null,
            "usage": {"input_tokens": 50, "output_tokens": 0, "cache_read_input_tokens": 10}
        }
    }"#);
    assert_eq!(events.len(), 2);
    assert!(matches!(events[0], AssistantMessageEvent::Start));
    match &events[1] {
        AssistantMessageEvent::Usage { usage } => {
            assert_eq!(usage.input, 50);
            assert_eq!(usage.output, 0);
            assert_eq!(usage.cache_read, 10);
        }
        other => panic!("expected Usage event, got {:?}", other),
    }

    // message_delta should emit Usage + Done
    let events = mapper.map_event("message_delta", r#"{
        "type": "message_delta",
        "delta": {"stop_reason": "end_turn"},
        "usage": {"output_tokens": 75}
    }"#);
    assert_eq!(events.len(), 2);
    match &events[0] {
        AssistantMessageEvent::Usage { usage } => {
            assert_eq!(usage.output, 75);
            assert_eq!(usage.input, 50); // retained from message_start
        }
        other => panic!("expected Usage event, got {:?}", other),
    }
    assert!(matches!(events[1], AssistantMessageEvent::Done { reason: StopReason::Stop }));
}
