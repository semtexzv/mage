//! Integration tests for AgentLoop.

mod test_helpers;

use std::cell::RefCell;
use std::rc::Rc;

use async_trait::async_trait;
use refstr::Str;

use mage_core::agent_loop::AgentLoop;
use mage_core::event_stream::AgentEventReceiver;
use mage_core::extension::*;
use mage_core::types::*;

use test_helpers::*;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn make_loop(
    provider: impl llm::Provider + 'static,
    extensions: Vec<Box<dyn Extension>>,
) -> (AgentLoop, AgentEventReceiver) {
    AgentLoop::new(
        "You are a test assistant.",
        test_model(),
        llm::StreamOptions::default(),
        providers(provider),
        extensions,
    )
}

async fn collect_events(rx: &mut AgentEventReceiver) -> Vec<AgentEvent> {
    let mut events = Vec::new();
    while let Some(ev) = tokio::time::timeout(
        std::time::Duration::from_millis(100),
        rx.recv(),
    )
    .await
    .ok()
    .flatten()
    {
        events.push(ev);
    }
    events
}

macro_rules! local_test {
    ($name:ident, $body:expr) => {
        #[test]
        fn $name() {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            rt.block_on(async {
                let local = tokio::task::LocalSet::new();
                local
                    .run_until(async {
                        $body
                    })
                    .await;
            });
        }
    };
}

// ===========================================================================
// Basic loop tests
// ===========================================================================

local_test!(basic_text_response, {
    let (mut agent, mut rx) = make_loop(TextProvider::new("Hello world"), vec![]);
    agent
        .run(Message::user_text("hi"))
        .await
        .unwrap();

    // State should have: user message + assistant message
    assert_eq!(agent.state.messages.len(), 2);
    assert_eq!(agent.state.messages[0].role_name(), "user");
    assert_eq!(agent.state.messages[1].role_name(), "assistant");

    // Check assistant content
    if let MessageBody::Assistant { content, .. } = &agent.state.messages[1].body {
        assert_eq!(content.len(), 1);
        if let llm::ContentBlock::Text { text, .. } = &content[0] {
            assert_eq!(text.as_ref(), "Hello world");
        } else {
            panic!("expected text block");
        }
    } else {
        panic!("expected assistant message");
    }

    // Events should include AgentStart, MessageStart, MessageDelta(s), MessageEnd, TurnEnd, AgentEnd
    let events = collect_events(&mut rx).await;
    assert!(events.iter().any(|e| matches!(e, AgentEvent::AgentStart)));
    assert!(events.iter().any(|e| matches!(e, AgentEvent::AgentEnd { .. })));
    assert!(events.iter().any(|e| matches!(e, AgentEvent::MessageEnd { .. })));
});

local_test!(multiple_prompts_accumulate, {
    let (mut agent, _rx) = make_loop(TextProvider::new("reply"), vec![]);
    agent.run(Message::user_text("first")).await.unwrap();
    agent.run(Message::user_text("second")).await.unwrap();

    // Should have: user1, assistant1, user2, assistant2
    assert_eq!(agent.state.messages.len(), 4);
    assert_eq!(agent.state.messages[0].role_name(), "user");
    assert_eq!(agent.state.messages[1].role_name(), "assistant");
    assert_eq!(agent.state.messages[2].role_name(), "user");
    assert_eq!(agent.state.messages[3].role_name(), "assistant");
});

// ===========================================================================
// Tool execution tests
// ===========================================================================

local_test!(tool_call_and_result, {
    struct ToolExt;

    #[async_trait(?Send)]
    impl Extension for ToolExt {
        fn init(&mut self, reg: &mut ExtensionRegistry) {
            reg.tool(
                llm::Tool {
                    name: "echo".into(),
                    description: "Echo input".into(),
                    parameters: serde_json::json!({"type": "object"}),
                },
                |_id, params, _handle| async move {
                    let text = params
                        .get("text")
                        .and_then(|v| v.as_str())
                        .unwrap_or("no input");
                    ToolResult::success(format!("echo: {text}"))
                },
            );
        }
    }

    let provider = ToolCallProvider::new(
        "echo",
        serde_json::json!({"text": "hello"}),
        "Done!",
    );
    let (mut agent, mut rx) = make_loop(provider, vec![Box::new(ToolExt)]);
    agent.run(Message::user_text("test")).await.unwrap();

    // Messages: user, assistant(tool_call), tool_result, assistant(text)
    assert_eq!(agent.state.messages.len(), 4);
    assert_eq!(agent.state.messages[0].role_name(), "user");
    assert_eq!(agent.state.messages[1].role_name(), "assistant");
    assert_eq!(agent.state.messages[2].role_name(), "toolResult");
    assert_eq!(agent.state.messages[3].role_name(), "assistant");

    // Verify tool result content
    if let MessageBody::ToolResult { content, is_error, .. } = &agent.state.messages[2].body {
        assert!(!is_error);
        assert_eq!(content.len(), 1);
        if let llm::UserContent::Text { text } = &content[0] {
            assert_eq!(text, "echo: hello");
        }
    } else {
        panic!("expected tool result message");
    }

    // Check events include tool execution
    let events = collect_events(&mut rx).await;
    assert!(events.iter().any(|e| matches!(e, AgentEvent::ToolExecStart { .. })));
    assert!(events.iter().any(|e| matches!(e, AgentEvent::ToolExecEnd { .. })));
});

local_test!(unknown_tool_returns_error, {
    // Provider tries to call a tool that doesn't exist
    let provider = ToolCallProvider::new("nonexistent", serde_json::json!({}), "ok");
    let (mut agent, _rx) = make_loop(provider, vec![]);
    agent.run(Message::user_text("test")).await.unwrap();

    // Should have: user, assistant(tool_call), tool_result(error), assistant(text)
    assert_eq!(agent.state.messages.len(), 4);
    if let MessageBody::ToolResult { is_error, .. } = &agent.state.messages[2].body {
        assert!(is_error);
    } else {
        panic!("expected tool result");
    }
});

// ===========================================================================
// Extension hook tests
// ===========================================================================

local_test!(on_tool_call_can_block, {
    struct BlockerExt;

    #[async_trait(?Send)]
    impl Extension for BlockerExt {
        fn init(&mut self, reg: &mut ExtensionRegistry) {
            reg.tool(
                llm::Tool {
                    name: "dangerous".into(),
                    description: "Dangerous tool".into(),
                    parameters: serde_json::json!({"type": "object"}),
                },
                |_id, _params, _handle| async move {
                    ToolResult::success("should not run")
                },
            );
        }

        async fn on_tool_call(
            &mut self,
            event: &ToolCallEvent,
            _ctx: &mut ExtensionContext<'_>,
        ) -> Option<ToolCallResult> {
            if event.tool_name.as_ref() == "dangerous" {
                Some(ToolCallResult {
                    block: true,
                    reason: Some("too dangerous".into()),
                })
            } else {
                None
            }
        }
    }

    let provider = ToolCallProvider::new("dangerous", serde_json::json!({}), "ok");
    let (mut agent, _rx) = make_loop(provider, vec![Box::new(BlockerExt)]);
    agent.run(Message::user_text("test")).await.unwrap();

    // Tool should be blocked — result should be an error
    if let MessageBody::ToolResult { is_error, content, .. } = &agent.state.messages[2].body {
        assert!(is_error);
        if let llm::UserContent::Text { text } = &content[0] {
            assert!(text.contains("too dangerous"), "got: {text}");
        }
    } else {
        panic!("expected tool result");
    }
});

local_test!(on_tool_result_can_modify, {
    struct AmenderExt;

    #[async_trait(?Send)]
    impl Extension for AmenderExt {
        fn init(&mut self, reg: &mut ExtensionRegistry) {
            reg.tool(
                llm::Tool {
                    name: "greet".into(),
                    description: "Greet".into(),
                    parameters: serde_json::json!({"type": "object"}),
                },
                |_id, _params, _handle| async move {
                    ToolResult::success("hello")
                },
            );
        }

        async fn on_tool_result(
            &mut self,
            _event: &ToolResultEvent,
            _ctx: &mut ExtensionContext<'_>,
        ) -> Option<ToolResultResult> {
            // Amend the result
            Some(ToolResultResult {
                content: Some(vec![llm::UserContent::Text {
                    text: "amended: hello".into(),
                }]),
                is_error: None,
            })
        }
    }

    let provider = ToolCallProvider::new("greet", serde_json::json!({}), "ok");
    let (mut agent, _rx) = make_loop(provider, vec![Box::new(AmenderExt)]);
    agent.run(Message::user_text("test")).await.unwrap();

    if let MessageBody::ToolResult { content, .. } = &agent.state.messages[2].body {
        if let llm::UserContent::Text { text } = &content[0] {
            assert_eq!(text, "amended: hello");
        }
    } else {
        panic!("expected tool result");
    }
});

local_test!(on_context_can_transform_messages, {
    struct ContextExt;

    #[async_trait(?Send)]
    impl Extension for ContextExt {
        async fn on_context(
            &mut self,
            _event: &ContextEvent,
            _ctx: &mut ExtensionContext<'_>,
        ) -> Option<ContextResult> {
            // Return modified messages — prepend a system-injected message
            Some(ContextResult {
                messages: Some(vec![
                    Message::user_text("injected by on_context"),
                ]),
            })
        }
    }

    let (provider, requests) = CapturingProvider::new("reply");
    let (mut agent, _rx) = make_loop(provider, vec![Box::new(ContextExt)]);
    agent.run(Message::user_text("original")).await.unwrap();

    // The LLM should have received the transformed messages, not the original
    let reqs = requests.borrow();
    assert_eq!(reqs.len(), 1);
    assert_eq!(reqs[0].messages.len(), 1); // Only the injected message
    if let llm::Message::User(u) = &reqs[0].messages[0] {
        if let llm::UserMessageContent::Text(t) = &u.content {
            assert_eq!(t, "injected by on_context");
        }
    } else {
        panic!("expected user message in LLM request");
    }
});

local_test!(on_input_can_transform, {
    struct InputTransformExt;

    #[async_trait(?Send)]
    impl Extension for InputTransformExt {
        async fn on_input(
            &mut self,
            event: &InputEvent,
            _ctx: &mut ExtensionContext<'_>,
        ) -> Option<InputResult> {
            Some(InputResult::Transform {
                text: format!("transformed: {}", event.text),
            })
        }
    }

    let (mut agent, _rx) = make_loop(
        TextProvider::new("reply"),
        vec![Box::new(InputTransformExt)],
    );
    agent.run(Message::user_text("original")).await.unwrap();

    // The stored user message should be the transformed text
    if let MessageBody::User { content } = &agent.state.messages[0].body {
        if let llm::UserMessageContent::Text(t) = content {
            assert_eq!(t, "transformed: original");
        }
    } else {
        panic!("expected user message");
    }
});

local_test!(on_input_handled_skips_run, {
    struct InputHandlerExt;

    #[async_trait(?Send)]
    impl Extension for InputHandlerExt {
        async fn on_input(
            &mut self,
            _event: &InputEvent,
            _ctx: &mut ExtensionContext<'_>,
        ) -> Option<InputResult> {
            Some(InputResult::Handled)
        }
    }

    let (mut agent, _rx) = make_loop(
        TextProvider::new("should not appear"),
        vec![Box::new(InputHandlerExt)],
    );
    agent.run(Message::user_text("consumed")).await.unwrap();

    // No messages should be added — input was consumed
    assert_eq!(agent.state.messages.len(), 0);
});

local_test!(on_before_agent_start_modifies_system_prompt, {
    struct PromptModExt;

    #[async_trait(?Send)]
    impl Extension for PromptModExt {
        async fn on_before_agent_start(
            &mut self,
            _event: &BeforeAgentStartEvent,
            _ctx: &mut ExtensionContext<'_>,
        ) -> Option<BeforeAgentStartResult> {
            Some(BeforeAgentStartResult {
                system_prompt: Some("modified system prompt".into()),
            })
        }
    }

    let (provider, requests) = CapturingProvider::new("reply");
    let (mut agent, _rx) = make_loop(provider, vec![Box::new(PromptModExt)]);
    agent.run(Message::user_text("hi")).await.unwrap();

    // System prompt should be modified
    assert_eq!(agent.state.system_prompt, "modified system prompt");

    // And the LLM request should have the modified prompt
    let reqs = requests.borrow();
    assert_eq!(
        reqs[0].system_prompt.as_ref().map(|s| s.as_ref()),
        Some("modified system prompt")
    );
});

// ===========================================================================
// Extension hook ordering
// ===========================================================================

local_test!(hooks_fire_in_order, {
    struct OrderTracker {
        log: Rc<RefCell<Vec<String>>>,
        name: String,
    }

    #[async_trait(?Send)]
    impl Extension for OrderTracker {
        async fn on_agent_start(&mut self, _ctx: &mut ExtensionContext<'_>) {
            self.log.borrow_mut().push(format!("{}: agent_start", self.name));
        }
        async fn on_turn_start(&mut self, _event: &TurnStartEvent, _ctx: &mut ExtensionContext<'_>) {
            self.log.borrow_mut().push(format!("{}: turn_start", self.name));
        }
        async fn on_turn_end(&mut self, _event: &TurnEndEvent, _ctx: &mut ExtensionContext<'_>) {
            self.log.borrow_mut().push(format!("{}: turn_end", self.name));
        }
        async fn on_agent_end(&mut self, _event: &AgentEndEvent, _ctx: &mut ExtensionContext<'_>) {
            self.log.borrow_mut().push(format!("{}: agent_end", self.name));
        }
    }

    let log = Rc::new(RefCell::new(Vec::new()));
    let exts: Vec<Box<dyn Extension>> = vec![
        Box::new(OrderTracker { log: log.clone(), name: "A".into() }),
        Box::new(OrderTracker { log: log.clone(), name: "B".into() }),
    ];

    let (mut agent, _rx) = make_loop(TextProvider::new("reply"), exts);
    agent.run(Message::user_text("hi")).await.unwrap();

    let log = log.borrow();
    assert_eq!(log[0], "A: agent_start");
    assert_eq!(log[1], "B: agent_start");
    assert_eq!(log[2], "A: turn_start");
    assert_eq!(log[3], "B: turn_start");
    assert_eq!(log[4], "A: turn_end");
    assert_eq!(log[5], "B: turn_end");
    assert_eq!(log[6], "A: agent_end");
    assert_eq!(log[7], "B: agent_end");
});

// ===========================================================================
// Error handling
// ===========================================================================

local_test!(provider_error_propagates, {
    let (mut agent, _rx) = make_loop(
        ErrorProvider { error: "server down".into() },
        vec![],
    );
    let result = agent.run(Message::user_text("hi")).await;
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(format!("{err}").contains("server down"));
});

local_test!(no_provider_errors, {
    // Create a loop with provider keyed "test" but model expects "other"
    let mut model = test_model();
    model.api = Str::from("other");
    let (mut agent, _rx) = AgentLoop::new(
        "prompt",
        model,
        llm::StreamOptions::default(),
        providers(TextProvider::new("hi")),
        vec![],
    );
    let result = agent.run(Message::user_text("hi")).await;
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(format!("{err}").contains("no provider"));
});

// ===========================================================================
// Follow-up queue
// ===========================================================================

local_test!(follow_up_causes_extra_turn, {
    struct FollowUpExt {
        injected: bool,
    }

    #[async_trait(?Send)]
    impl Extension for FollowUpExt {
        async fn on_turn_end(&mut self, _event: &TurnEndEvent, ctx: &mut ExtensionContext<'_>) {
            if !self.injected {
                self.injected = true;
                ctx.follow_up(Message::user_text("follow-up question"));
            }
        }
    }

    let (mut agent, _rx) = make_loop(
        TextProvider::new("reply"),
        vec![Box::new(FollowUpExt { injected: false })],
    );
    agent.run(Message::user_text("first")).await.unwrap();

    // Should have: user1, assistant1, follow-up user, assistant2
    assert_eq!(agent.state.messages.len(), 4);
    assert_eq!(agent.state.messages[2].role_name(), "user");
    assert_eq!(agent.state.messages[3].role_name(), "assistant");
});

// ===========================================================================
// Tool registration via extensions
// ===========================================================================

local_test!(multiple_extensions_register_tools, {
    struct ExtA;
    struct ExtB;

    #[async_trait(?Send)]
    impl Extension for ExtA {
        fn init(&mut self, reg: &mut ExtensionRegistry) {
            reg.tool(
                llm::Tool { name: "tool_a".into(), description: "A".into(), parameters: serde_json::json!({}) },
                |_id, _params, _handle| async move { ToolResult::success("a") },
            );
        }
    }

    #[async_trait(?Send)]
    impl Extension for ExtB {
        fn init(&mut self, reg: &mut ExtensionRegistry) {
            reg.tool(
                llm::Tool { name: "tool_b".into(), description: "B".into(), parameters: serde_json::json!({}) },
                |_id, _params, _handle| async move { ToolResult::success("b") },
            );
        }
    }

    let (agent, _rx) = make_loop(
        TextProvider::new("reply"),
        vec![Box::new(ExtA), Box::new(ExtB)],
    );

    // Both tools should be registered as schemas
    assert_eq!(agent.state.tool_schemas.len(), 2);
    let names: Vec<&str> = agent.state.tool_schemas.iter().map(|t| t.name.as_ref()).collect();
    assert!(names.contains(&"tool_a"));
    assert!(names.contains(&"tool_b"));
});

// ===========================================================================
// Extension context access
// ===========================================================================

local_test!(extension_context_sees_messages, {
    struct MsgCounterExt {
        count_at_turn_end: Rc<RefCell<usize>>,
    }

    #[async_trait(?Send)]
    impl Extension for MsgCounterExt {
        async fn on_turn_end(&mut self, _event: &TurnEndEvent, ctx: &mut ExtensionContext<'_>) {
            *self.count_at_turn_end.borrow_mut() = ctx.messages.len();
        }
    }

    let count = Rc::new(RefCell::new(0usize));
    let ext = MsgCounterExt { count_at_turn_end: count.clone() };
    let (mut agent, _rx) = make_loop(TextProvider::new("reply"), vec![Box::new(ext)]);
    agent.run(Message::user_text("hi")).await.unwrap();

    // At turn_end, should see user + assistant = 2 messages
    assert_eq!(*count.borrow(), 2);
});

// ===========================================================================
// on_message_delta fires during streaming
// ===========================================================================

local_test!(on_message_delta_fires, {
    struct DeltaTracker {
        deltas: Rc<RefCell<Vec<String>>>,
    }

    #[async_trait(?Send)]
    impl Extension for DeltaTracker {
        async fn on_message_delta(
            &mut self,
            event: &llm::AssistantMessageEvent,
            _ctx: &mut ExtensionContext<'_>,
        ) {
            let desc = format!("{event:?}");
            self.deltas.borrow_mut().push(desc);
        }
    }

    let deltas = Rc::new(RefCell::new(Vec::new()));
    let ext = DeltaTracker { deltas: deltas.clone() };
    let (mut agent, _rx) = make_loop(TextProvider::new("hello"), vec![Box::new(ext)]);
    agent.run(Message::user_text("hi")).await.unwrap();

    let d = deltas.borrow();
    // Should have received multiple events: Start, TextStart, TextDelta, TextEnd, Done
    assert!(d.len() >= 3, "expected at least 3 delta events, got {}", d.len());
    // First event should be Start
    assert!(d[0].contains("Start"), "first event should be Start, got: {}", d[0]);
});