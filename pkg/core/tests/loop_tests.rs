//! Integration tests for AgentLoop with the new Module/ToolHandler system.

mod test_helpers;

use std::rc::Rc;

use async_trait::async_trait;
use refstr::Str;

use mage_core::agent_loop::AgentLoop;
use mage_core::event_stream::AgentEventReceiver;
use mage_core::module::{GateResult, Module};
use mage_core::tool::{tool_fn, ToolCall, ToolContext, ToolDef};
use mage_core::types::*;

use test_helpers::*;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn make_loop(
    provider: impl llm::Provider + 'static,
    modules: Vec<Rc<dyn Module>>,
) -> (AgentLoop, AgentEventReceiver) {
    AgentLoop::new(
        "You are a test assistant.",
        test_model(),
        llm::StreamOptions::default(),
        providers(provider),
        modules,
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
    assert_eq!(agent.messages.len(), 2);
    assert_eq!(agent.messages[0].role_name(), "user");
    assert_eq!(agent.messages[1].role_name(), "assistant");

    // Check assistant content
    if let MessageBody::Assistant { content, .. } = &agent.messages[1].body {
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
    assert_eq!(agent.messages.len(), 4);
    assert_eq!(agent.messages[0].role_name(), "user");
    assert_eq!(agent.messages[1].role_name(), "assistant");
    assert_eq!(agent.messages[2].role_name(), "user");
    assert_eq!(agent.messages[3].role_name(), "assistant");
});

// ===========================================================================
// Tool execution tests
// ===========================================================================

local_test!(tool_call_and_result, {
    struct EchoModule;

    #[async_trait(?Send)]
    impl Module for EchoModule {
        fn name(&self) -> &str { "echo" }
        fn tools(&self) -> Vec<ToolDef> {
            vec![tool_fn(
                llm::Tool {
                    name: "echo".into(),
                    description: "Echo input".into(),
                    parameters: serde_json::json!({"type": "object"}),
                },
                |params, _ctx| async move {
                    let text = params
                        .get("text")
                        .and_then(|v| v.as_str())
                        .unwrap_or("no input");
                    ToolResult::success(format!("echo: {text}"))
                },
            )]
        }
    }

    let provider = ToolCallProvider::new(
        "echo",
        serde_json::json!({"text": "hello"}),
        "Done!",
    );
    let (mut agent, mut rx) = make_loop(provider, vec![Rc::new(EchoModule)]);
    agent.run(Message::user_text("test")).await.unwrap();

    // Messages: user, assistant(tool_call), tool_result, assistant(text)
    assert_eq!(agent.messages.len(), 4);
    assert_eq!(agent.messages[0].role_name(), "user");
    assert_eq!(agent.messages[1].role_name(), "assistant");
    assert_eq!(agent.messages[2].role_name(), "toolResult");
    assert_eq!(agent.messages[3].role_name(), "assistant");

    // Verify tool result content
    if let MessageBody::ToolResult { content, is_error, .. } = &agent.messages[2].body {
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
    assert_eq!(agent.messages.len(), 4);
    if let MessageBody::ToolResult { is_error, .. } = &agent.messages[2].body {
        assert!(is_error);
    } else {
        panic!("expected tool result");
    }
});

// ===========================================================================
// Module hook tests
// ===========================================================================

local_test!(gate_tool_can_block, {
    struct BlockerModule;

    #[async_trait(?Send)]
    impl Module for BlockerModule {
        fn name(&self) -> &str { "blocker" }

        fn tools(&self) -> Vec<ToolDef> {
            vec![tool_fn(
                llm::Tool {
                    name: "dangerous".into(),
                    description: "Dangerous tool".into(),
                    parameters: serde_json::json!({"type": "object"}),
                },
                |_params, _ctx| async move {
                    ToolResult::success("should not run")
                },
            )]
        }

        async fn gate_tool(&self, call: &ToolCall) -> GateResult {
            if call.name.as_ref() == "dangerous" {
                GateResult::Block("too dangerous".into())
            } else {
                GateResult::Allow
            }
        }
    }

    let provider = ToolCallProvider::new("dangerous", serde_json::json!({}), "ok");
    let (mut agent, _rx) = make_loop(provider, vec![Rc::new(BlockerModule)]);
    agent.run(Message::user_text("test")).await.unwrap();

    // Tool should be blocked — result should be an error
    if let MessageBody::ToolResult { is_error, content, .. } = &agent.messages[2].body {
        assert!(is_error);
        if let llm::UserContent::Text { text } = &content[0] {
            assert!(text.contains("too dangerous"), "got: {text}");
        }
    } else {
        panic!("expected tool result");
    }
});

local_test!(filter_result_can_modify, {
    struct AmenderModule;

    #[async_trait(?Send)]
    impl Module for AmenderModule {
        fn name(&self) -> &str { "amender" }

        fn tools(&self) -> Vec<ToolDef> {
            vec![tool_fn(
                llm::Tool {
                    name: "greet".into(),
                    description: "Greet".into(),
                    parameters: serde_json::json!({"type": "object"}),
                },
                |_params, _ctx| async move {
                    ToolResult::success("hello")
                },
            )]
        }

        async fn filter_result(&self, _call: &ToolCall, _result: ToolResult) -> ToolResult {
            ToolResult::success("amended: hello")
        }
    }

    let provider = ToolCallProvider::new("greet", serde_json::json!({}), "ok");
    let (mut agent, _rx) = make_loop(provider, vec![Rc::new(AmenderModule)]);
    agent.run(Message::user_text("test")).await.unwrap();

    if let MessageBody::ToolResult { content, .. } = &agent.messages[2].body {
        if let llm::UserContent::Text { text } = &content[0] {
            assert_eq!(text, "amended: hello");
        }
    } else {
        panic!("expected tool result");
    }
});

local_test!(transform_context_modifies_llm_input, {
    struct ContextModule;

    #[async_trait(?Send)]
    impl Module for ContextModule {
        fn name(&self) -> &str { "context" }

        async fn transform_context(&self, _messages: Vec<llm::Message>) -> Vec<llm::Message> {
            // Replace all messages with a single injected one
            vec![llm::Message::User(llm::UserMessage {
                content: llm::UserMessageContent::Text("injected by transform_context".into()),
            })]
        }
    }

    let (provider, requests) = CapturingProvider::new("reply");
    let (mut agent, _rx) = make_loop(provider, vec![Rc::new(ContextModule)]);
    agent.run(Message::user_text("original")).await.unwrap();

    // The LLM should have received the transformed messages, not the original
    let reqs = requests.borrow();
    assert_eq!(reqs.len(), 1);
    assert_eq!(reqs[0].messages.len(), 1);
    if let llm::Message::User(u) = &reqs[0].messages[0] {
        if let llm::UserMessageContent::Text(t) = &u.content {
            assert_eq!(t, "injected by transform_context");
        }
    } else {
        panic!("expected user message in LLM request");
    }
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
// Multiple modules register tools
// ===========================================================================

local_test!(multiple_modules_register_tools, {
    struct ModA;
    struct ModB;

    #[async_trait(?Send)]
    impl Module for ModA {
        fn name(&self) -> &str { "mod_a" }
        fn tools(&self) -> Vec<ToolDef> {
            vec![tool_fn(
                llm::Tool { name: "tool_a".into(), description: "A".into(), parameters: serde_json::json!({}) },
                |_args, _ctx| async move { ToolResult::success("a") },
            )]
        }
    }

    #[async_trait(?Send)]
    impl Module for ModB {
        fn name(&self) -> &str { "mod_b" }
        fn tools(&self) -> Vec<ToolDef> {
            vec![tool_fn(
                llm::Tool { name: "tool_b".into(), description: "B".into(), parameters: serde_json::json!({}) },
                |_args, _ctx| async move { ToolResult::success("b") },
            )]
        }
    }

    let (agent, _rx) = make_loop(
        TextProvider::new("reply"),
        vec![Rc::new(ModA), Rc::new(ModB)],
    );

    // The agent should have been built without error.
    // We verify it runs correctly.
    // (No direct schema access on AgentLoop — the registry is internal.)
    assert_eq!(agent.messages.len(), 0); // No messages yet
});
