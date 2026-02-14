//! Integration tests for agent loop, hooks, and tool execution.

use std::cell::Cell;
use std::rc::Rc;

use serde_json::json;

use llm::{
    AssistantMessageEvent, CancelToken, Message,
    Model, ModelCost, StopReason, StreamOptions,
};
use mage_core::agent::AgentBuilder;
use mage_core::agent_loop::{self, LoopConfig, StreamFn};
use mage_core::event_stream;
use mage_core::extension::*;
use mage_core::tool::{Tool, ToolExecution, ToolResult};
use mage_core::types::*;

// ---------------------------------------------------------------------------
// Test helpers
// ---------------------------------------------------------------------------

fn test_model() -> Model {
    Model {
        id: "test-model".into(),
        name: "Test Model".into(),
        api: "test".into(),
        provider: "test".into(),
        base_url: "http://localhost".into(),
        reasoning: false,
        input: vec![],
        cost: ModelCost::default(),
        context_window: 200_000,
        max_tokens: 4096,
        headers: None,
    }
}

/// StreamFn that sends a fixed text response.
fn text_stream(text: &str) -> StreamFn {
    let text = text.to_owned();
    Box::new(move |_req: llm::StreamRequest| {
        let text = text.clone();
        let (tx, rx) = llm::channel::channel();
        llm::StreamHandle {
            events: rx,
            task: Box::pin(async move {
                tx.send(AssistantMessageEvent::Start).ok();
                tx.send(AssistantMessageEvent::TextStart { content_index: 0 }).ok();
                tx.send(AssistantMessageEvent::TextDelta {
                    content_index: 0,
                    delta: text.as_str().into(),
                }).ok();
                tx.send(AssistantMessageEvent::TextEnd { content_index: 0 }).ok();
                tx.send(AssistantMessageEvent::Done { reason: StopReason::Stop }).ok();
                Ok(())
            }),
        }
    })
}

fn default_convert(messages: &[AgentMessage]) -> Vec<Message> {
    messages
        .iter()
        .filter_map(|m| match m {
            AgentMessage::Llm(msg) => Some(msg.clone()),
            AgentMessage::Custom { .. } => None,
        })
        .collect()
}

// ===================================================================
// Basic loop tests
// ===================================================================

#[tokio::test]
async fn test_basic_text_response() {
    let local = tokio::task::LocalSet::new();
    local.run_until(async {
        let mut state = AgentState::new(
            "You are helpful.",
            test_model(),
            vec![AgentMessage::user_text("Hello")],
            StreamOptions::default(),
        );
        let mut hooks: Vec<Box<dyn Extension>> = vec![];
        let config = LoopConfig {
            max_turns: 10,
            stream_fn: text_stream("Hello! How can I help?"),
            options: StreamOptions::default(),
            convert_to_llm: Box::new(default_convert),
        };
        let (tx, mut rx) = event_stream::new_agent_stream();
        let cancel = CancelToken::new();

        let result = agent_loop::run(&mut state, &mut hooks, &config, &tx, &cancel).await;
        assert!(result.is_ok(), "loop should succeed: {:?}", result);

        // Drop tx so rx.recv() returns None when drained
        drop(tx);

        let mut events = vec![];
        while let Some(e) = rx.recv().await {
            events.push(e);
        }
        assert!(events.len() >= 6, "expected at least 6 events, got {}", events.len());
        assert!(matches!(events.first(), Some(AgentEvent::AgentStart)));
        assert!(matches!(events.last(), Some(AgentEvent::AgentEnd { .. })));

        // Conversation should have 2 messages: user + assistant
        assert_eq!(state.messages.len(), 2);
    }).await;
}

#[tokio::test]
async fn test_cancellation() {
    let local = tokio::task::LocalSet::new();
    local.run_until(async {
        let mut state = AgentState::new(
            "test",
            test_model(),
            vec![AgentMessage::user_text("go")],
            StreamOptions::default(),
        );
        let mut hooks: Vec<Box<dyn Extension>> = vec![];
        let config = LoopConfig {
            max_turns: 0,
            stream_fn: text_stream("won't get here"),
            options: StreamOptions::default(),
            convert_to_llm: Box::new(default_convert),
        };
        let (tx, _rx) = event_stream::new_agent_stream();
        let cancel = CancelToken::new();
        cancel.cancel();

        let result = agent_loop::run(&mut state, &mut hooks, &config, &tx, &cancel).await;
        assert!(matches!(result, Err(mage_core::agent_loop::LoopError::Cancelled)));
    }).await;
}

// ===================================================================
// Observe hook tests
// ===================================================================

struct SharedCounter {
    agent_starts: Cell<u32>,
    agent_ends: Cell<u32>,
    turn_starts: Cell<u32>,
    turn_ends: Cell<u32>,
    message_starts: Cell<u32>,
    message_ends: Cell<u32>,
    message_deltas: Cell<u32>,
}

impl SharedCounter {
    fn new() -> Self {
        Self {
            agent_starts: Cell::new(0),
            agent_ends: Cell::new(0),
            turn_starts: Cell::new(0),
            turn_ends: Cell::new(0),
            message_starts: Cell::new(0),
            message_ends: Cell::new(0),
            message_deltas: Cell::new(0),
        }
    }
}

struct CountingHook(Rc<SharedCounter>);

impl Extension for CountingHook {
    fn on_agent_start(&mut self, _ctx: &HookCtx) {
        self.0.agent_starts.set(self.0.agent_starts.get() + 1);
    }
    fn on_agent_end(&mut self, _args: &AgentEndArgs, _ctx: &HookCtx) {
        self.0.agent_ends.set(self.0.agent_ends.get() + 1);
    }
    fn on_turn_start(&mut self, _ctx: &HookCtx) {
        self.0.turn_starts.set(self.0.turn_starts.get() + 1);
    }
    fn on_turn_end(&mut self, _args: &TurnEndArgs, _ctx: &HookCtx) {
        self.0.turn_ends.set(self.0.turn_ends.get() + 1);
    }
    fn on_message_start(&mut self, _args: &MessageArgs, _ctx: &HookCtx) {
        self.0.message_starts.set(self.0.message_starts.get() + 1);
    }
    fn on_message_end(&mut self, _args: &MessageArgs, _ctx: &HookCtx) {
        self.0.message_ends.set(self.0.message_ends.get() + 1);
    }
    fn on_message_delta(&mut self, _args: &MessageDeltaArgs, _ctx: &HookCtx) {
        self.0.message_deltas.set(self.0.message_deltas.get() + 1);
    }
}

#[tokio::test]
async fn test_observe_hooks_fire() {
    let local = tokio::task::LocalSet::new();
    local.run_until(async {
        let counter = Rc::new(SharedCounter::new());

        let mut state = AgentState::new(
            "test",
            test_model(),
            vec![AgentMessage::user_text("hi")],
            StreamOptions::default(),
        );
        let mut hooks: Vec<Box<dyn Extension>> = vec![
            Box::new(CountingHook(counter.clone())),
        ];
        let config = LoopConfig {
            max_turns: 10,
            stream_fn: text_stream("hello"),
            options: StreamOptions::default(),
            convert_to_llm: Box::new(default_convert),
        };
        let (tx, _rx) = event_stream::new_agent_stream();
        let cancel = CancelToken::new();

        agent_loop::run(&mut state, &mut hooks, &config, &tx, &cancel).await.unwrap();

        assert_eq!(counter.agent_starts.get(), 1);
        assert_eq!(counter.agent_ends.get(), 1);
        assert_eq!(counter.turn_starts.get(), 1);
        assert_eq!(counter.turn_ends.get(), 1);
        assert_eq!(counter.message_starts.get(), 1);
        assert_eq!(counter.message_ends.get(), 1);
        // Deltas: Start, TextStart, TextDelta, TextEnd, Done = 5 events
        assert!(counter.message_deltas.get() >= 4,
            "expected >= 4 deltas, got {}", counter.message_deltas.get());
    }).await;
}

// ===================================================================
// Hook ordering
// ===================================================================

struct OrderTracker {
    id: u32,
    log: Rc<std::cell::RefCell<Vec<u32>>>,
}

impl Extension for OrderTracker {
    fn on_agent_start(&mut self, _ctx: &HookCtx) {
        self.log.borrow_mut().push(self.id);
    }
}

#[tokio::test]
async fn test_hook_ordering() {
    let local = tokio::task::LocalSet::new();
    local.run_until(async {
        let log = Rc::new(std::cell::RefCell::new(Vec::new()));

        let mut state = AgentState::new(
            "test",
            test_model(),
            vec![AgentMessage::user_text("hi")],
            StreamOptions::default(),
        );
        let mut hooks: Vec<Box<dyn Extension>> = vec![
            Box::new(OrderTracker { id: 1, log: log.clone() }),
            Box::new(OrderTracker { id: 2, log: log.clone() }),
            Box::new(OrderTracker { id: 3, log: log.clone() }),
        ];
        let config = LoopConfig {
            max_turns: 10,
            stream_fn: text_stream("ok"),
            options: StreamOptions::default(),
            convert_to_llm: Box::new(default_convert),
        };
        let (tx, _rx) = event_stream::new_agent_stream();
        let cancel = CancelToken::new();

        agent_loop::run(&mut state, &mut hooks, &config, &tx, &cancel).await.unwrap();

        let order = log.borrow();
        assert_eq!(*order, vec![1, 2, 3]);
    }).await;
}

// ===================================================================
// Decision hook: tool call blocking (direct, not through loop)
// ===================================================================

struct ToolBlocker {
    blocked_name: String,
    block_count: u32,
}

impl Extension for ToolBlocker {
    fn on_tool_call<'a>(
        &'a mut self,
        args: &'a ToolCallArgs<'a>,
        _ctx: &'a HookCtx,
    ) -> HookFuture<'a, Disposition> {
        Box::pin(async move {
            if *args.name == *self.blocked_name {
                self.block_count += 1;
                Disposition::Block { reason: "blocked by test".into() }
            } else {
                Disposition::Propagate
            }
        })
    }
}

#[tokio::test]
async fn test_tool_call_blocking() {
    let local = tokio::task::LocalSet::new();
    local.run_until(async {
        let mut blocker = ToolBlocker {
            blocked_name: "dangerous_tool".into(),
            block_count: 0,
        };

        let cancel = CancelToken::new();
        let model = test_model();
        let mut outbox = vec![];
        let ctx = HookCtx::new(&model, "test", &mut outbox, &cancel);

        let args = ToolCallArgs {
            name: "dangerous_tool",
            id: "call_1",
            args: &json!({}),
        };

        let result = blocker.on_tool_call(&args, &ctx).await;
        assert!(result.is_block());
        assert_eq!(blocker.block_count, 1);

        let mut outbox2 = vec![];
        let ctx2 = HookCtx::new(&model, "test", &mut outbox2, &cancel);
        let safe_args = ToolCallArgs {
            name: "safe_tool",
            id: "call_2",
            args: &json!({}),
        };
        let result2 = blocker.on_tool_call(&safe_args, &ctx2).await;
        assert!(matches!(result2, Disposition::Propagate));
        assert_eq!(blocker.block_count, 1);
    }).await;
}

// ===================================================================
// Tool execution (unit, no loop)
// ===================================================================

struct EchoTool;

impl Tool for EchoTool {
    type State = String;

    fn name(&self) -> &str { "echo" }
    fn description(&self) -> &str { "Echoes input" }
    fn parameters(&self) -> &serde_json::Value {
        // Return a static ref via lazy; or just leak. For tests, a const works.
        // Use a thread_local or Box::leak for the static ref.
        static PARAMS: std::sync::LazyLock<serde_json::Value> = std::sync::LazyLock::new(|| {
            json!({"type": "object", "properties": {"text": {"type": "string"}}})
        });
        &PARAMS
    }

    fn execute(
        &self,
        _tool_call_id: &str,
        params: serde_json::Value,
        _cancel: CancelToken,
    ) -> ToolExecution<Self::State> {
        let text = params.get("text")
            .and_then(|v| v.as_str())
            .unwrap_or("no input")
            .to_owned();
        ToolExecution::running(async move {
            ToolResult::success(text)
        })
    }
}

#[tokio::test]
async fn test_tool_execution() {
    let tool = EchoTool;

    let execution = tool.execute("call_1", json!({"text": "hello"}), CancelToken::new());
    let result = match execution {
        ToolExecution::Running(fut) => fut.await,
        ToolExecution::Ready(r) => r,
        ToolExecution::Custom { task, .. } => task.await,
    };
    assert!(!result.is_error());
    let content = result.content();
    assert_eq!(content.content.len(), 1);
    match &content.content[0] {
        llm::UserContent::Text { text } => assert_eq!(text, "hello"),
        _ => panic!("expected text"),
    }
}

// ===================================================================
// Context amendment hook (through loop)
// ===================================================================

struct ContextInjector;

impl Extension for ContextInjector {
    fn on_context<'a>(
        &'a mut self,
        messages: &'a [AgentMessage],
        _ctx: &'a HookCtx,
    ) -> HookFuture<'a, Disposition<ContextAmend>> {
        Box::pin(async move {
            let mut new_messages = messages.to_vec();
            new_messages.push(AgentMessage::user_text("[injected by hook]"));
            Disposition::Value(ContextAmend { messages: new_messages })
        })
    }
}

#[tokio::test]
async fn test_context_amendment() {
    let local = tokio::task::LocalSet::new();
    local.run_until(async {
        let received = Rc::new(std::cell::RefCell::new(Vec::<Vec<Message>>::new()));
        let rm = received.clone();

        let stream_fn: StreamFn = Box::new(move |req: llm::StreamRequest| {
            rm.borrow_mut().push(req.context.messages.clone());
            let (tx, rx) = llm::channel::channel();
            llm::StreamHandle {
                events: rx,
                task: Box::pin(async move {
                    tx.send(AssistantMessageEvent::Start).ok();
                    tx.send(AssistantMessageEvent::TextStart { content_index: 0 }).ok();
                    tx.send(AssistantMessageEvent::TextDelta {
                        content_index: 0,
                        delta: "ok".into(),
                    }).ok();
                    tx.send(AssistantMessageEvent::TextEnd { content_index: 0 }).ok();
                    tx.send(AssistantMessageEvent::Done { reason: StopReason::Stop }).ok();
                    Ok(())
                }),
            }
        });

        let mut state = AgentState::new(
            "test",
            test_model(),
            vec![AgentMessage::user_text("original")],
            StreamOptions::default(),
        );
        let mut hooks: Vec<Box<dyn Extension>> = vec![
            Box::new(ContextInjector),
        ];
        let config = LoopConfig {
            max_turns: 10,
            stream_fn,
            options: StreamOptions::default(),
            convert_to_llm: Box::new(default_convert),
        };
        let (tx, _rx) = event_stream::new_agent_stream();
        let cancel = CancelToken::new();

        agent_loop::run(&mut state, &mut hooks, &config, &tx, &cancel).await.unwrap();

        let msgs = received.borrow();
        assert_eq!(msgs.len(), 1);
        // Should have 2 messages: original + injected
        assert_eq!(msgs[0].len(), 2, "LLM should see original + injected message");
    }).await;
}

// ===================================================================
// Tool result amendment (direct)
// ===================================================================

struct ResultAmender;

impl Extension for ResultAmender {
    fn on_tool_result<'a>(
        &'a mut self,
        _args: &'a ToolResultArgs<'a>,
        _ctx: &'a HookCtx,
    ) -> HookFuture<'a, Disposition<ToolResultAmend>> {
        Box::pin(async move {
            Disposition::Value(ToolResultAmend {
                content: Some(vec![llm::UserContent::Text {
                    text: "[amended result]".into(),
                }]),
                is_error: Some(false),
            })
        })
    }
}

#[tokio::test]
async fn test_tool_result_amendment() {
    let mut amender = ResultAmender;
    let cancel = CancelToken::new();
    let model = test_model();
    let mut outbox = vec![];
    let ctx = HookCtx::new(&model, "test", &mut outbox, &cancel);

    let original = ToolResult::success("original");

    let args = ToolResultArgs {
        name: "some_tool",
        id: "call_1",
        result: &original,
        is_error: false,
    };

    match amender.on_tool_result(&args, &ctx).await {
        Disposition::Value(amend) => {
            let content = amend.content.unwrap();
            assert_eq!(content.len(), 1);
            match &content[0] {
                llm::UserContent::Text { text } => assert_eq!(text, "[amended result]"),
                _ => panic!("expected text"),
            }
            assert_eq!(amend.is_error, Some(false));
        }
        _ => panic!("expected Value disposition"),
    }
}

// ===================================================================
// AgentBuilder
// ===================================================================

#[tokio::test]
async fn test_agent_builder() {
    let local = tokio::task::LocalSet::new();
    local.run_until(async {
        let mut agent = AgentBuilder::new(test_model())
            .system_prompt("You are helpful.")
            .stream_fn(text_stream("I'm here to help!"))
            .max_turns(5)
            .build();

        let rx = agent.prompt("Hello").await;
        assert!(rx.is_ok());

        assert_eq!(agent.messages().len(), 2); // user + assistant
    }).await;
}

// ===================================================================
// AgentBuilder with hook
// ===================================================================

#[tokio::test]
async fn test_agent_builder_with_hook() {
    let local = tokio::task::LocalSet::new();
    local.run_until(async {
        let counter = Rc::new(SharedCounter::new());

        let mut agent = AgentBuilder::new(test_model())
            .system_prompt("test")
            .stream_fn(text_stream("response"))
            .ext(CountingHook(counter.clone()))
            .max_turns(5)
            .build();

        agent.prompt("go").await.unwrap();

        assert_eq!(counter.agent_starts.get(), 1);
        assert_eq!(counter.agent_ends.get(), 1);
        assert_eq!(counter.message_starts.get(), 1);
        assert_eq!(counter.message_ends.get(), 1);
    }).await;
}

// ===================================================================
// HookCtx reads
// ===================================================================

#[tokio::test]
async fn test_hook_ctx_reads() {
    let cancel = CancelToken::new();
    let model = test_model();
    let mut outbox = vec![];
    let ctx = HookCtx::new(&model, "my system prompt", &mut outbox, &cancel);

    assert_eq!(ctx.system_prompt, "my system prompt");
    assert_eq!(&*ctx.model.id, "test-model");
    assert!(!ctx.is_cancelled());
}

#[tokio::test]
async fn test_hook_ctx_cancel() {
    let cancel = CancelToken::new();
    let model = test_model();
    let mut outbox = vec![];
    let ctx = HookCtx::new(&model, "test", &mut outbox, &cancel);

    assert!(!ctx.is_cancelled());
    ctx.abort();
    assert!(ctx.is_cancelled());
}

#[tokio::test]
async fn test_hook_ctx_send_message() {
    let cancel = CancelToken::new();
    let model = test_model();
    let mut outbox = vec![];

    {
        let mut ctx = HookCtx::new(&model, "test", &mut outbox, &cancel);
        ctx.send_message(AgentMessage::user_text("injected"), DeliverAs::FollowUp);
        ctx.send_message(AgentMessage::user_text("steering"), DeliverAs::Steer);
    }

    assert_eq!(outbox.len(), 2);
    assert!(matches!(outbox[0].1, DeliverAs::FollowUp));
    assert!(matches!(outbox[1].1, DeliverAs::Steer));
}

// ===================================================================
// Disposition
// ===================================================================

#[test]
fn test_disposition_default() {
    let d: Disposition<()> = Disposition::default();
    assert!(matches!(d, Disposition::Propagate));
}

#[test]
fn test_disposition_block() {
    let d: Disposition = Disposition::Block { reason: "no".into() };
    assert!(d.is_block());
}

#[test]
fn test_disposition_value() {
    let d = Disposition::Value(42);
    assert!(!d.is_block());
    match d {
        Disposition::Value(v) => assert_eq!(v, 42),
        _ => panic!("expected Value"),
    }
}

// ===================================================================
// Extension init — tool registration via Registry
// ===================================================================

struct GreetTool;

impl Tool for GreetTool {
    type State = String;

    fn name(&self) -> &str { "greet" }
    fn description(&self) -> &str { "Greets someone" }
    fn parameters(&self) -> &serde_json::Value {
        static PARAMS: std::sync::LazyLock<serde_json::Value> = std::sync::LazyLock::new(|| {
            json!({"type": "object", "properties": {"name": {"type": "string"}}})
        });
        &PARAMS
    }

    fn execute(
        &self,
        _tool_call_id: &str,
        params: serde_json::Value,
        _cancel: CancelToken,
    ) -> ToolExecution<Self::State> {
        let name = params.get("name")
            .and_then(|v| v.as_str())
            .unwrap_or("world")
            .to_owned();
        ToolExecution::running(async move {
            ToolResult::success(format!("Hello, {name}!"))
        })
    }
}

struct ToolRegisteringExt;

impl Extension for ToolRegisteringExt {
    fn init(&mut self, registry: &mut mage_core::extension::Registry) {
        registry.tool(GreetTool);
    }
}

#[tokio::test]
async fn test_extension_registers_tool_via_init() {
    let local = tokio::task::LocalSet::new();
    local.run_until(async {
        let mut state = AgentState::new(
            "test",
            test_model(),
            vec![AgentMessage::user_text("hi")],
            StreamOptions::default(),
        );
        let mut exts: Vec<Box<dyn Extension>> = vec![
            Box::new(ToolRegisteringExt),
        ];
        let config = LoopConfig {
            max_turns: 10,
            stream_fn: text_stream("ok"),
            options: StreamOptions::default(),
            convert_to_llm: Box::new(default_convert),
        };
        let (tx, _rx) = event_stream::new_agent_stream();
        let cancel = CancelToken::new();

        agent_loop::run(&mut state, &mut exts, &config, &tx, &cancel).await.unwrap();

        // After init, the extension should have registered the "greet" tool
        assert_eq!(state.llm_tools().len(), 1);
        assert_eq!(&*state.llm_tools()[0].name, "greet");
    }).await;
}

// ===================================================================
// Tool execution through the loop (full cycle)
// ===================================================================

/// Extension that registers an echo tool via init.
struct EchoToolExt;

impl Extension for EchoToolExt {
    fn init(&mut self, registry: &mut mage_core::extension::Registry) {
        registry.tool(EchoToolForLoop);
    }
}

struct EchoToolForLoop;

impl Tool for EchoToolForLoop {
    type State = String;

    fn name(&self) -> &str { "echo" }
    fn description(&self) -> &str { "Echoes input" }
    fn parameters(&self) -> &serde_json::Value {
        static PARAMS: std::sync::LazyLock<serde_json::Value> = std::sync::LazyLock::new(|| {
            json!({"type": "object", "properties": {"text": {"type": "string"}}})
        });
        &PARAMS
    }

    fn execute(
        &self,
        _tool_call_id: &str,
        params: serde_json::Value,
        _cancel: CancelToken,
    ) -> ToolExecution<Self::State> {
        let text = params.get("text")
            .and_then(|v| v.as_str())
            .unwrap_or("no input")
            .to_owned();
        ToolExecution::running(async move {
            ToolResult::success(format!("pong: {text}"))
        })
    }
}

/// StreamFn that first returns a tool call, then on the second call returns text.
fn tool_use_then_text_stream() -> StreamFn {
    let call_count = std::rc::Rc::new(Cell::new(0u32));
    Box::new(move |_req: llm::StreamRequest| {
        let n = call_count.get();
        call_count.set(n + 1);
        let (tx, rx) = llm::channel::channel();
        if n == 0 {
            // First call: return a tool_use response
            llm::StreamHandle {
                events: rx,
                task: Box::pin(async move {
                    tx.send(AssistantMessageEvent::Start).ok();
                    tx.send(AssistantMessageEvent::ToolCallStart {
                        content_index: 0,
                        id: "call_abc".into(),
                        name: "echo".into(),
                    }).ok();
                    tx.send(AssistantMessageEvent::ToolCallDelta {
                        content_index: 0,
                        delta: r#"{"text":"ping"}"#.into(),
                    }).ok();
                    tx.send(AssistantMessageEvent::ToolCallEnd {
                        content_index: 0,
                        arguments: serde_json::json!({"text": "ping"}),
                    }).ok();
                    tx.send(AssistantMessageEvent::Done { reason: StopReason::ToolUse }).ok();
                    Ok(())
                }),
            }
        } else {
            // Subsequent calls: return a text response
            llm::StreamHandle {
                events: rx,
                task: Box::pin(async move {
                    tx.send(AssistantMessageEvent::Start).ok();
                    tx.send(AssistantMessageEvent::TextStart { content_index: 0 }).ok();
                    tx.send(AssistantMessageEvent::TextDelta {
                        content_index: 0,
                        delta: "Got it: pong".into(),
                    }).ok();
                    tx.send(AssistantMessageEvent::TextEnd { content_index: 0 }).ok();
                    tx.send(AssistantMessageEvent::Done { reason: StopReason::Stop }).ok();
                    Ok(())
                }),
            }
        }
    })
}

#[tokio::test]
async fn test_tool_execution_through_loop() {
    let local = tokio::task::LocalSet::new();
    local.run_until(async {
        let mut state = AgentState::new(
            "test",
            test_model(),
            vec![AgentMessage::user_text("ping")],
            StreamOptions::default(),
        );
        let mut hooks: Vec<Box<dyn Extension>> = vec![
            Box::new(EchoToolExt),
        ];
        let config = LoopConfig {
            max_turns: 10,
            stream_fn: tool_use_then_text_stream(),
            options: StreamOptions::default(),
            convert_to_llm: Box::new(default_convert),
        };
        let (tx, mut rx) = event_stream::new_agent_stream();
        let cancel = CancelToken::new();

        let result = agent_loop::run(&mut state, &mut hooks, &config, &tx, &cancel).await;
        assert!(result.is_ok(), "loop should succeed: {:?}", result);

        drop(tx);

        // Collect events
        let mut events = vec![];
        while let Some(e) = rx.recv().await {
            events.push(e);
        }

        // Should have tool execution events
        let has_tool_exec_start = events.iter().any(|e| matches!(e, AgentEvent::ToolExecutionStart { .. }));
        let has_tool_exec_end = events.iter().any(|e| matches!(e, AgentEvent::ToolExecutionEnd { .. }));
        assert!(has_tool_exec_start, "should have ToolExecutionStart event");
        assert!(has_tool_exec_end, "should have ToolExecutionEnd event");

        // Conversation should have: user + assistant(tool_call) + tool_result + assistant(text)
        assert_eq!(state.messages.len(), 4,
            "expected 4 messages (user, assistant+tool_call, tool_result, assistant+text), got {}\n{:?}",
            state.messages.len(), state.messages.iter().map(|m| m.role()).collect::<Vec<_>>());

        // Verify the tool result message content
        match &state.messages[2] {
            AgentMessage::Llm(Message::ToolResult(tr)) => {
                assert_eq!(&*tr.tool_name, "echo");
                assert!(!tr.is_error);
                match &tr.content[0] {
                    llm::UserContent::Text { text } => assert_eq!(text, "pong: ping"),
                    _ => panic!("expected text content in tool result"),
                }
            }
            other => panic!("expected tool result at index 2, got {:?}", other.role()),
        }
    }).await;
}

// ===================================================================
// Tool execution with blocking hook (through loop)
// ===================================================================

struct DangerousTool;

impl Tool for DangerousTool {
    type State = String;

    fn name(&self) -> &str { "dangerous" }
    fn description(&self) -> &str { "A dangerous tool" }
    fn parameters(&self) -> &serde_json::Value {
        static PARAMS: std::sync::LazyLock<serde_json::Value> = std::sync::LazyLock::new(|| {
            json!({"type": "object"})
        });
        &PARAMS
    }

    fn execute(
        &self,
        _tool_call_id: &str,
        _params: serde_json::Value,
        _cancel: CancelToken,
    ) -> ToolExecution<Self::State> {
        panic!("should never be called");
    }
}

struct DangerousToolExt;

impl Extension for DangerousToolExt {
    fn init(&mut self, registry: &mut mage_core::extension::Registry) {
        registry.tool(DangerousTool);
    }
}

#[tokio::test]
async fn test_tool_blocked_through_loop() {
    let local = tokio::task::LocalSet::new();
    local.run_until(async {
        // StreamFn: first call returns tool_use for "dangerous", second returns text stop
        let call_count = std::rc::Rc::new(Cell::new(0u32));
        let stream_fn: StreamFn = {
            let call_count = call_count.clone();
            Box::new(move |_req: llm::StreamRequest| {
                let n = call_count.get();
                call_count.set(n + 1);
                let (tx, rx) = llm::channel::channel();
                if n == 0 {
                    llm::StreamHandle {
                        events: rx,
                        task: Box::pin(async move {
                            tx.send(AssistantMessageEvent::Start).ok();
                            tx.send(AssistantMessageEvent::ToolCallStart {
                                content_index: 0,
                                id: "call_danger".into(),
                                name: "dangerous".into(),
                            }).ok();
                            tx.send(AssistantMessageEvent::ToolCallEnd {
                                content_index: 0,
                                arguments: json!({}),
                            }).ok();
                            tx.send(AssistantMessageEvent::Done { reason: StopReason::ToolUse }).ok();
                            Ok(())
                        }),
                    }
                } else {
                    llm::StreamHandle {
                        events: rx,
                        task: Box::pin(async move {
                            tx.send(AssistantMessageEvent::Start).ok();
                            tx.send(AssistantMessageEvent::TextStart { content_index: 0 }).ok();
                            tx.send(AssistantMessageEvent::TextDelta {
                                content_index: 0,
                                delta: "ok, blocked".into(),
                            }).ok();
                            tx.send(AssistantMessageEvent::TextEnd { content_index: 0 }).ok();
                            tx.send(AssistantMessageEvent::Done { reason: StopReason::Stop }).ok();
                            Ok(())
                        }),
                    }
                }
            })
        };

        let mut state = AgentState::new(
            "test",
            test_model(),
            vec![AgentMessage::user_text("do dangerous thing")],
            StreamOptions::default(),
        );
        let mut hooks: Vec<Box<dyn Extension>> = vec![
            Box::new(DangerousToolExt),
            Box::new(ToolBlocker {
                blocked_name: "dangerous".into(),
                block_count: 0,
            }),
        ];
        let config = LoopConfig {
            max_turns: 10,
            stream_fn,
            options: StreamOptions::default(),
            convert_to_llm: Box::new(default_convert),
        };
        let (tx, _rx) = event_stream::new_agent_stream();
        let cancel = CancelToken::new();

        let result = agent_loop::run(&mut state, &mut hooks, &config, &tx, &cancel).await;
        assert!(result.is_ok(), "loop should succeed even with blocked tool: {:?}", result);

        // Tool result should be an error from the block
        let tool_results: Vec<_> = state.messages.iter().filter_map(|m| match m {
            AgentMessage::Llm(Message::ToolResult(tr)) => Some(tr),
            _ => None,
        }).collect();
        assert_eq!(tool_results.len(), 1, "should have one tool result");
        assert!(tool_results[0].is_error, "blocked tool result should be marked as error");
    }).await;
}

// ===================================================================
// AgentBuilder with provider (wiring test)
// ===================================================================

struct TestProvider {
    response_text: String,
}

impl llm::Provider for TestProvider {
    fn stream(&self, _req: llm::StreamRequest) -> llm::StreamHandle {
        let text = self.response_text.clone();
        let (tx, rx) = llm::channel::channel();
        llm::StreamHandle {
            events: rx,
            task: Box::pin(async move {
                tx.send(AssistantMessageEvent::Start).ok();
                tx.send(AssistantMessageEvent::TextStart { content_index: 0 }).ok();
                tx.send(AssistantMessageEvent::TextDelta {
                    content_index: 0,
                    delta: text.as_str().into(),
                }).ok();
                tx.send(AssistantMessageEvent::TextEnd { content_index: 0 }).ok();
                tx.send(AssistantMessageEvent::Done { reason: StopReason::Stop }).ok();
                Ok(())
            }),
        }
    }
}

#[tokio::test]
async fn test_agent_builder_with_provider() {
    let local = tokio::task::LocalSet::new();
    local.run_until(async {
        let mut agent = AgentBuilder::new(test_model())
            .system_prompt("You are helpful.")
            .provider("test", TestProvider { response_text: "Hello from provider!".into() })
            .max_turns(5)
            .build();

        let rx = agent.prompt("Hello").await;
        assert!(rx.is_ok(), "should succeed with provider: {:?}", rx.as_ref().err());
        assert_eq!(agent.messages().len(), 2); // user + assistant
    }).await;
}

// ===================================================================
// FactoryRegistry
// ===================================================================

#[tokio::test]
async fn test_factory_registry() {
    use mage_core::extension::FactoryRegistry;

    let local = tokio::task::LocalSet::new();
    local.run_until(async {
        let counter = Rc::new(SharedCounter::new());
        let counter_for_factory = counter.clone();

        let mut factory_reg = FactoryRegistry::new();
        factory_reg.register(move || {
            Box::new(CountingHook(counter_for_factory.clone())) as Box<dyn Extension>
        });

        assert_eq!(factory_reg.len(), 1);
        assert!(!factory_reg.is_empty());

        let mut agent = AgentBuilder::new(test_model())
            .system_prompt("test")
            .stream_fn(text_stream("hello"))
            .ext_from_registry(&factory_reg)
            .max_turns(5)
            .build();

        agent.prompt("go").await.unwrap();

        assert_eq!(counter.agent_starts.get(), 1);
        assert_eq!(counter.agent_ends.get(), 1);

        // Create a second agent from the same registry — gets fresh instances
        let counter2 = Rc::new(SharedCounter::new());
        let counter2_for_factory = counter2.clone();

        let mut factory_reg2 = FactoryRegistry::new();
        factory_reg2.register(move || {
            Box::new(CountingHook(counter2_for_factory.clone())) as Box<dyn Extension>
        });

        let mut agent2 = AgentBuilder::new(test_model())
            .system_prompt("test")
            .stream_fn(text_stream("world"))
            .ext_from_registry(&factory_reg2)
            .max_turns(5)
            .build();

        agent2.prompt("go").await.unwrap();

        // Each agent got its own extension instance
        assert_eq!(counter.agent_starts.get(), 1);  // unchanged from first agent
        assert_eq!(counter2.agent_starts.get(), 1); // second agent's counter
    }).await;
}
