//! Shared test helpers: mock providers, test runner.

use std::rc::Rc;

use refstr::Str;

use llm::{
    AssistantMessage, AssistantMessageEvent, CancelToken, ContentBlock, Model, ModelCost,
    Provider, ProviderError, StopReason, StreamHandle, StreamOptions, StreamRequest, Usage,
};

// ---------------------------------------------------------------------------
// Test model
// ---------------------------------------------------------------------------

pub fn test_model() -> Model {
    Model {
        id: Str::from("test-model"),
        name: Str::from("Test Model"),
        api: Str::from("test"),
        provider: Str::from("test"),
        base_url: Str::from("http://localhost"),
        reasoning: false,
        input: vec![],
        cost: ModelCost::default(),
        context_window: 100_000,
        max_out: 4096,
        headers: None,
    }
}

// ---------------------------------------------------------------------------
// TextProvider — returns a fixed text response
// ---------------------------------------------------------------------------

pub struct TextProvider {
    pub text: String,
}

impl TextProvider {
    pub fn new(text: impl Into<String>) -> Self {
        Self { text: text.into() }
    }
}

impl Provider for TextProvider {
    fn stream(&self, req: StreamRequest) -> StreamHandle {
        let text = self.text.clone();
        let (tx, rx) = llm::channel::channel();
        let task = Box::pin(async move {
            let _ = tx.send(AssistantMessageEvent::Start);
            let _ = tx.send(AssistantMessageEvent::TextStart { content_index: 0 });
            let _ = tx.send(AssistantMessageEvent::TextDelta {
                content_index: 0,
                delta: Str::from(text.as_str()),
            });
            let _ = tx.send(AssistantMessageEvent::TextEnd { content_index: 0 });
            let _ = tx.send(AssistantMessageEvent::Done {
                reason: StopReason::Stop,
            });
            Ok(())
        });
        StreamHandle { events: rx, task }
    }
}

// ---------------------------------------------------------------------------
// ToolCallProvider — returns a tool call, then text on second call
// ---------------------------------------------------------------------------

pub struct ToolCallProvider {
    pub tool_name: String,
    pub tool_args: serde_json::Value,
    pub final_text: String,
    call_count: std::cell::Cell<u32>,
}

impl ToolCallProvider {
    pub fn new(
        tool_name: impl Into<String>,
        tool_args: serde_json::Value,
        final_text: impl Into<String>,
    ) -> Self {
        Self {
            tool_name: tool_name.into(),
            tool_args,
            final_text: final_text.into(),
            call_count: std::cell::Cell::new(0),
        }
    }
}

impl Provider for ToolCallProvider {
    fn stream(&self, _req: StreamRequest) -> StreamHandle {
        let count = self.call_count.get();
        self.call_count.set(count + 1);

        let (tx, rx) = llm::channel::channel();

        if count == 0 {
            // First call: return a tool call
            let name = self.tool_name.clone();
            let args = self.tool_args.clone();
            let task = Box::pin(async move {
                let _ = tx.send(AssistantMessageEvent::Start);
                let _ = tx.send(AssistantMessageEvent::ToolCallStart {
                    content_index: 0,
                    id: Str::from("call_1"),
                    name: Str::from(name.as_str()),
                });
                let _ = tx.send(AssistantMessageEvent::ToolCallEnd {
                    content_index: 0,
                    arguments: args,
                });
                let _ = tx.send(AssistantMessageEvent::Done {
                    reason: StopReason::ToolUse,
                });
                Ok(())
            });
            StreamHandle { events: rx, task }
        } else {
            // Subsequent calls: return text
            let text = self.final_text.clone();
            let task = Box::pin(async move {
                let _ = tx.send(AssistantMessageEvent::Start);
                let _ = tx.send(AssistantMessageEvent::TextStart { content_index: 0 });
                let _ = tx.send(AssistantMessageEvent::TextDelta {
                    content_index: 0,
                    delta: Str::from(text.as_str()),
                });
                let _ = tx.send(AssistantMessageEvent::TextEnd { content_index: 0 });
                let _ = tx.send(AssistantMessageEvent::Done {
                    reason: StopReason::Stop,
                });
                Ok(())
            });
            StreamHandle { events: rx, task }
        }
    }
}

// ---------------------------------------------------------------------------
// ErrorProvider — always returns an error
// ---------------------------------------------------------------------------

pub struct ErrorProvider {
    pub error: String,
}

impl Provider for ErrorProvider {
    fn stream(&self, _req: StreamRequest) -> StreamHandle {
        let error = self.error.clone();
        let (tx, rx) = llm::channel::channel();
        let task = Box::pin(async move {
            let _ = tx.send(AssistantMessageEvent::Error {
                reason: StopReason::Error,
                error: Some(Str::from(error.as_str())),
            });
            Err(ProviderError::Other(error))
        });
        StreamHandle { events: rx, task }
    }
}

// ---------------------------------------------------------------------------
// CapturingProvider — captures the requests sent to it
// ---------------------------------------------------------------------------

pub struct CapturingProvider {
    pub text: String,
    pub requests: Rc<std::cell::RefCell<Vec<llm::Context>>>,
}

impl CapturingProvider {
    pub fn new(text: impl Into<String>) -> (Self, Rc<std::cell::RefCell<Vec<llm::Context>>>) {
        let requests = Rc::new(std::cell::RefCell::new(Vec::new()));
        (
            Self {
                text: text.into(),
                requests: requests.clone(),
            },
            requests,
        )
    }
}

impl Provider for CapturingProvider {
    fn stream(&self, req: StreamRequest) -> StreamHandle {
        self.requests.borrow_mut().push(req.context);
        let text = self.text.clone();
        let (tx, rx) = llm::channel::channel();
        let task = Box::pin(async move {
            let _ = tx.send(AssistantMessageEvent::Start);
            let _ = tx.send(AssistantMessageEvent::TextStart { content_index: 0 });
            let _ = tx.send(AssistantMessageEvent::TextDelta {
                content_index: 0,
                delta: Str::from(text.as_str()),
            });
            let _ = tx.send(AssistantMessageEvent::TextEnd { content_index: 0 });
            let _ = tx.send(AssistantMessageEvent::Done {
                reason: StopReason::Stop,
            });
            Ok(())
        });
        StreamHandle { events: rx, task }
    }
}

// ---------------------------------------------------------------------------
// Run helper — run a prompt through an AgentLoop in a local task set
// ---------------------------------------------------------------------------

pub async fn run_prompt(
    agent_loop: &mut mage_core::agent_loop::AgentLoop,
    text: &str,
) -> Result<Vec<mage_core::types::AgentEvent>, mage_core::agent_loop::LoopError> {
    // We can't easily collect events from the receiver that was returned at
    // construction. Instead, rely on the loop finishing and inspect state.
    agent_loop
        .run(mage_core::types::Message::user_text(text))
        .await?;
    Ok(vec![]) // Events are consumed via the receiver returned from new()
}

pub fn providers(p: impl Provider + 'static) -> Vec<(Str, Rc<dyn Provider>)> {
    vec![("test".into(), Rc::new(p) as Rc<dyn Provider>)]
}
