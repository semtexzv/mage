//! Agent loop — the core async loop that drives the agent.
//!
//! The loop is a double-nested structure with concurrent tool dispatch:
//!
//! ```text
//! outer: loop {                        // continues on follow-up
//!     inner: loop {                    // continues while tool calls or steering
//!         stream assistant response
//!         dispatch tools (concurrent read-only, serial mutating)
//!         collect results via channel (+ handle steering/abort)
//!     }
//!     check follow-up → continue outer
//! }
//! ```

use std::collections::VecDeque;
use std::rc::Rc;
use std::time::Duration;

use refstr::Str;

use llm::{AssistantMessage, CancelToken, ContentBlock, StopReason};

use crate::event_stream::{new_agent_stream, AgentEventReceiver, AgentEventSender};
use crate::handle::{loop_handle_pair, LoopCommand, LoopHandle};
use crate::module::{GateResult, Module, ModuleSet};
use crate::tool::{ToolCall, ToolCompletion, ToolContext, ToolRegistry};
use crate::types::{AgentEvent, Message, ToolResult, ToolUpdate};

const MAX_LLM_RETRIES: u32 = 3;
const BASE_RETRY_DELAY_MS: u64 = 1000;

// ---------------------------------------------------------------------------
// Error type
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub enum LoopError {
    Provider(llm::ProviderError),
    NoProvider(String),
    Cancelled,
}

impl std::fmt::Display for LoopError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Provider(e) => write!(f, "provider error: {e}"),
            Self::NoProvider(api) => write!(f, "no provider registered for api: {api}"),
            Self::Cancelled => write!(f, "cancelled"),
        }
    }
}

impl std::error::Error for LoopError {}

// ---------------------------------------------------------------------------
// AgentLoop
// ---------------------------------------------------------------------------

/// The core agent loop. Owns conversation state, dispatches tools concurrently.
pub struct AgentLoop {
    /// Conversation history — owned exclusively by this struct, never shared.
    pub messages: Vec<Message>,
    pub system_prompt: String,
    pub model: llm::Model,
    pub options: llm::StreamOptions,

    /// Providers keyed by API name.
    providers: Vec<(Str, Rc<dyn llm::Provider>)>,

    /// All registered tools, built from module contributions.
    tool_registry: ToolRegistry,

    /// Ordered module set for pipeline interception.
    modules: Rc<ModuleSet>,

    /// Incoming commands from LoopHandle / SessionHandle.
    cmd_rx: llm::channel::Receiver<LoopCommand>,

    /// Outgoing events to UI / observers.
    event_tx: AgentEventSender,

    /// Handle for external use (cloneable).
    handle: LoopHandle,

    /// Session-level cancellation.
    pub cancel: CancelToken,
}

impl AgentLoop {
    /// Create a new agent loop.
    ///
    /// Collects tools from all modules and builds the tool registry.
    /// Returns the loop and an event receiver for the UI.
    pub fn new(
        system_prompt: impl Into<String>,
        model: llm::Model,
        options: llm::StreamOptions,
        providers: Vec<(Str, Rc<dyn llm::Provider>)>,
        modules: Vec<Rc<dyn Module>>,
    ) -> (Self, AgentEventReceiver) {
        let (handle, cmd_rx) = loop_handle_pair();
        let (event_tx, event_rx) = new_agent_stream();

        let module_set = ModuleSet::new(modules);
        let tools = module_set.collect_tools();
        let tool_schemas = tools.iter().map(|t| t.schema.clone()).collect::<Vec<_>>();
        let tool_registry = ToolRegistry::new(tools);

        let this = Self {
            messages: Vec::new(),
            system_prompt: system_prompt.into(),
            model,
            options,
            providers,
            tool_registry,
            modules: Rc::new(module_set),
            cmd_rx,
            event_tx,
            handle,
            cancel: CancelToken::new(),
        };

        // Store schemas for the LLM — accessible via tool_registry.schemas()
        let _ = tool_schemas; // schemas are pulled from registry on each call

        (this, event_rx)
    }

    /// Get a cloneable handle for external use.
    pub fn handle(&self) -> LoopHandle {
        self.handle.clone()
    }

    pub(crate) fn emit(&self, event: AgentEvent) {
        self.event_tx.push(event);
    }

    /// Blocking receive. Used by session spawn loop.
    pub(crate) async fn cmd_rx_recv(&mut self) -> Option<LoopCommand> {
        self.cmd_rx.recv().await
    }

    /// Non-blocking receive. Used by session spawn loop.
    pub(crate) fn cmd_rx_try_recv(&mut self) -> Option<LoopCommand> {
        self.cmd_rx.try_recv()
    }

    /// Convert messages to LLM format, filtering out ephemeral entries.
    fn to_llm_messages(&self) -> Vec<llm::Message> {
        self.messages
            .iter()
            .filter(|m| !m.ephemeral)
            .map(|m| m.to_llm())
            .collect()
    }

    // -----------------------------------------------------------------------
    // Command handling
    // -----------------------------------------------------------------------

    /// Pull all pending commands from the channel into local queues.
    ///
    /// - InjectMessage / SteerMessage → steering queue (between tool calls)
    /// - FollowUpMessage → follow-up queue (after agent would stop)
    /// - Abort/Shutdown → immediate cancel
    /// - SetModel → immediate apply
    fn drain_commands(
        &mut self,
        run_cancel: &CancelToken,
        steering: &mut VecDeque<Message>,
        followup: &mut VecDeque<Message>,
    ) {
        while let Some(cmd) = self.cmd_rx.try_recv() {
            match cmd {
                LoopCommand::InjectMessage(msg)
                | LoopCommand::SteerMessage(msg) => {
                    steering.push_back(msg);
                }
                LoopCommand::FollowUpMessage(msg) => {
                    followup.push_back(msg);
                }
                LoopCommand::Abort => run_cancel.cancel(),
                LoopCommand::Shutdown => {
                    self.cancel.cancel();
                    run_cancel.cancel();
                }
                LoopCommand::SetModel(model) => self.model = model,
            }
        }
    }

    // -----------------------------------------------------------------------
    // Main run loop
    // -----------------------------------------------------------------------

    pub async fn run(&mut self, prompt: Message) -> Result<(), LoopError> {
        let run_cancel = CancelToken::new();
        self.handle.set_run_cancel(&run_cancel);

        // Two local queues, drained at different points:
        // - steering: after tool calls complete, before next LLM call
        // - followup: after the agent would stop (triggers new outer turn)
        let mut steering: VecDeque<Message> = VecDeque::new();
        let mut followup: VecDeque<Message> = VecDeque::new();

        self.messages.push(prompt);
        // Pick up anything queued before run started.
        self.drain_commands(&run_cancel, &mut steering, &mut followup);

        let mut turn_index: usize = 0;
        self.emit(AgentEvent::AgentStart);

        // Seed pending with any steering messages that arrived before we started.
        let mut pending: Vec<Message> = steering.drain(..).collect();

        // ── outer loop: continues when follow-up messages arrive ──
        'outer: loop {
            if run_cancel.is_cancelled() {
                break;
            }

            let mut has_more_tool_calls = true;

            // ── inner loop: tool calls + steering ─────────────────
            while has_more_tool_calls || !pending.is_empty() {
                if run_cancel.is_cancelled() {
                    break 'outer;
                }

                // Inject pending messages before next LLM call.
                for msg in pending.drain(..) {
                    self.messages.push(msg);
                }

                self.emit(AgentEvent::TurnStart { turn_index });

                let base_messages = self.to_llm_messages();
                let llm_messages = self.modules.clone().transform_context(base_messages).await;

                // ── stream assistant response ────────────────────
                let assistant_msg = stream_llm(
                    &self.providers,
                    &self.model,
                    &self.system_prompt,
                    &self.options,
                    &self.tool_registry,
                    &self.event_tx,
                    llm_messages,
                    &run_cancel,
                )
                .await?;

                let final_msg = Message::from_assistant(assistant_msg.clone());
                self.messages.push(final_msg.clone());

                if matches!(assistant_msg.stop_reason, StopReason::Error) {
                    self.emit(AgentEvent::TurnEnd {
                        turn_index,
                        message: final_msg,
                        tool_results: vec![],
                    });
                    break 'outer;
                }

                // ── extract + execute tool calls ─────────────────
                let tool_calls: Vec<ToolCall> = assistant_msg
                    .content
                    .iter()
                    .filter_map(|block| match block {
                        ContentBlock::ToolCall { id, name, arguments } => Some(ToolCall {
                            id: id.clone(),
                            name: name.clone(),
                            args: arguments.clone(),
                        }),
                        _ => None,
                    })
                    .collect();

                has_more_tool_calls = !tool_calls.is_empty();

                let tool_results = if !tool_calls.is_empty() {
                    self.dispatch_and_collect(
                        tool_calls,
                        &run_cancel,
                    )
                    .await
                } else {
                    vec![]
                };

                self.emit(AgentEvent::TurnEnd {
                    turn_index,
                    message: final_msg,
                    tool_results: tool_results.iter().map(|c| c.result.clone()).collect(),
                });
                turn_index += 1;

                // After tool calls: pull steering messages for the next inner iteration.
                self.drain_commands(&run_cancel, &mut steering, &mut followup);
                pending = steering.drain(..).collect();
            }

            // Agent would stop here. Check for follow-up messages.
            self.drain_commands(&run_cancel, &mut steering, &mut followup);
            if !followup.is_empty() || !steering.is_empty() {
                // Follow-ups and any steering become pending for new outer turn.
                for msg in followup.drain(..) {
                    self.messages.push(msg);
                }
                pending = steering.drain(..).collect();
                continue 'outer;
            }
            break;
        }

        // ── agent end ────────────────────────────────────────────
        self.handle.clear_run_cancel();
        self.emit(AgentEvent::AgentEnd {
            messages: self.messages.clone(),
        });

        Ok(())
    }

    // -----------------------------------------------------------------------
    // Concurrent tool dispatch + collection
    // -----------------------------------------------------------------------

    async fn dispatch_and_collect(
        &mut self,
        tool_calls: Vec<ToolCall>,
        cancel: &CancelToken,
    ) -> Vec<ToolCompletion> {
        let (result_tx, mut result_rx) =
            tokio::sync::mpsc::unbounded_channel::<ToolCompletion>();

        let total = tool_calls.len();

        // Classify tools into concurrent-safe and serial.
        let (concurrent, serial): (Vec<_>, Vec<_>) = tool_calls
            .into_iter()
            .partition(|c| self.tool_registry.is_concurrent_safe(&c.name, &c.args));

        // Spawn concurrent tools as independent local tasks.
        for call in concurrent {
            self.spawn_tool_task(call, result_tx.clone(), cancel);
        }

        // Spawn serial tools on a single task with a local deque.
        if !serial.is_empty() {
            self.spawn_serial_task(serial, result_tx.clone(), cancel);
        }

        drop(result_tx); // close our sender copy

        // Collect results. Messages stay in cmd_rx channel until
        // drain_commands is called after tool execution completes.
        // Only abort is handled here (via cancel token).
        let mut completions: Vec<ToolCompletion> = Vec::with_capacity(total);
        let mut collected = 0;

        while collected < total {
            tokio::select! {
                biased;

                _ = cancel.cancelled() => {
                    break;
                }

                Some(completion) = result_rx.recv() => {
                    self.event_tx.push(AgentEvent::ToolExecEnd {
                        tool_call_id: completion.call_id.clone(),
                        tool_name: completion.name.clone(),
                        result: completion.result.clone(),
                    });

                    let trm = llm::ToolResultMessage {
                        tool_call_id: completion.call_id.clone(),
                        tool_name: completion.name.clone(),
                        content: completion.result.content.clone(),
                        details: None,
                        is_error: completion.result.is_error,
                    };
                    self.messages.push(Message::from_tool_result(trm));
                    completions.push(completion);
                    collected += 1;
                }
            }
        }

        // Fill in skipped results for any tools that didn't complete.
        // (They should self-cancel, but we need tool_result messages for all calls.)
        // The spawned tasks handle this — on cancellation they send ToolResult::failure.

        completions
    }

    // -----------------------------------------------------------------------
    // Tool task spawning
    // -----------------------------------------------------------------------

    fn spawn_tool_task(
        &self,
        call: ToolCall,
        result_tx: tokio::sync::mpsc::UnboundedSender<ToolCompletion>,
        cancel: &CancelToken,
    ) {
        let handler = match self.tool_registry.get(&call.name) {
            Some(t) => t.handler.clone(),
            None => {
                let _ = result_tx.send(ToolCompletion {
                    call_id: call.id.clone(),
                    name: call.name.clone(),
                    result: ToolResult::failure(format!("Tool '{}' not found", call.name)),
                });
                return;
            }
        };

        let modules = self.modules.clone();
        let child_cancel = cancel.child_token();
        let event_tx = self.event_tx.clone();
        let loop_handle = self.handle.clone();

        tokio::task::spawn_local(async move {
            let result = run_tool_pipeline(
                &call,
                &*handler,
                &modules,
                &child_cancel,
                &event_tx,
                &loop_handle,
            )
            .await;
            let _ = result_tx.send(ToolCompletion {
                call_id: call.id,
                name: call.name,
                result,
            });
        });
    }

    fn spawn_serial_task(
        &self,
        calls: Vec<ToolCall>,
        result_tx: tokio::sync::mpsc::UnboundedSender<ToolCompletion>,
        cancel: &CancelToken,
    ) {
        // Pre-resolve handlers so we don't need the registry in the task.
        let mut tasks: Vec<(ToolCall, Rc<dyn crate::tool::ToolHandler>)> = Vec::new();
        for call in calls {
            match self.tool_registry.get(&call.name) {
                Some(t) => tasks.push((call, t.handler.clone())),
                None => {
                    let _ = result_tx.send(ToolCompletion {
                        call_id: call.id.clone(),
                        name: call.name.clone(),
                        result: ToolResult::failure(format!(
                            "Tool '{}' not found",
                            call.name
                        )),
                    });
                }
            }
        }

        if tasks.is_empty() {
            return;
        }

        let modules = self.modules.clone();
        let child_cancel = cancel.child_token();
        let event_tx = self.event_tx.clone();
        let loop_handle = self.handle.clone();

        tokio::task::spawn_local(async move {
            let mut queue = VecDeque::from(tasks);
            while let Some((call, handler)) = queue.pop_front() {
                if child_cancel.is_cancelled() {
                    let _ = result_tx.send(ToolCompletion {
                        call_id: call.id,
                        name: call.name,
                        result: ToolResult::failure("Cancelled"),
                    });
                    continue;
                }
                let result = run_tool_pipeline(
                    &call,
                    &*handler,
                    &modules,
                    &child_cancel,
                    &event_tx,
                    &loop_handle,
                )
                .await;
                let _ = result_tx.send(ToolCompletion {
                    call_id: call.id,
                    name: call.name,
                    result,
                });
            }
        });
    }
}

// ---------------------------------------------------------------------------
// Tool pipeline — gate → execute (with progress draining) → filter
// ---------------------------------------------------------------------------

async fn run_tool_pipeline(
    call: &ToolCall,
    handler: &dyn crate::tool::ToolHandler,
    modules: &ModuleSet,
    cancel: &CancelToken,
    event_tx: &AgentEventSender,
    loop_handle: &LoopHandle,
) -> ToolResult {
    // Emit start event.
    event_tx.push(AgentEvent::ToolExecStart {
        tool_call_id: call.id.clone(),
        tool_name: call.name.clone(),
        args: call.args.clone(),
    });

    // 1. Gate: modules can block.
    match modules.gate_tool(call).await {
        GateResult::Allow => {}
        GateResult::Block(reason) => return ToolResult::failure(reason),
    }

    // 2. Execute with progress update draining.
    let (update_tx, mut update_rx) = tokio::sync::mpsc::unbounded_channel::<ToolUpdate>();
    let ctx = ToolContext::new(cancel.child_token(), update_tx, loop_handle.clone());

    let mut tool_fut = std::pin::pin!(handler.execute(call.args.clone(), ctx));

    let result = loop {
        tokio::select! {
            biased;

            () = std::future::ready(()), if cancel.is_cancelled() => {
                break ToolResult::failure("Cancelled");
            }

            result = &mut tool_fut => {
                // Drain any buffered updates.
                while let Ok(update) = update_rx.try_recv() {
                    event_tx.push(AgentEvent::ToolExecUpdate {
                        tool_call_id: call.id.clone(),
                        tool_name: call.name.clone(),
                        update,
                    });
                }
                break result;
            }

            Some(update) = update_rx.recv() => {
                event_tx.push(AgentEvent::ToolExecUpdate {
                    tool_call_id: call.id.clone(),
                    tool_name: call.name.clone(),
                    update,
                });
            }
        }
    };

    // 3. Filter: modules modify result.
    modules.filter_result(call, result).await
}

// ---------------------------------------------------------------------------
// LLM streaming
// ---------------------------------------------------------------------------

async fn stream_llm(
    providers: &[(Str, Rc<dyn llm::Provider>)],
    model: &llm::Model,
    system_prompt: &str,
    options: &llm::StreamOptions,
    tool_registry: &ToolRegistry,
    event_tx: &AgentEventSender,
    llm_messages: Vec<llm::Message>,
    cancel: &CancelToken,
) -> Result<AssistantMessage, LoopError> {
    let tool_schemas = tool_registry.schemas();

    let llm_context = llm::Context {
        system_prompt: Some(Str::from(system_prompt)),
        messages: llm_messages,
        tools: if tool_schemas.is_empty() {
            None
        } else {
            Some(tool_schemas)
        },
    };

    // Resolve provider by model's API.
    let provider: Rc<dyn llm::Provider> = providers
        .iter()
        .find(|(api, _)| **api == *model.api)
        .map(|(_, p)| p.clone())
        .ok_or_else(|| LoopError::NoProvider(model.api.to_string()))?;

    let mut last_error: Option<llm::ProviderError> = None;

    for attempt in 0..=MAX_LLM_RETRIES {
        if attempt > 0 {
            let prev_err = last_error.as_ref().unwrap();
            let delay = retry_delay_ms(prev_err, attempt - 1, options);
            match delay {
                None => {
                    return Err(LoopError::Provider(last_error.take().unwrap()));
                }
                Some(ms) => {
                    tokio::time::sleep(Duration::from_millis(ms)).await;
                }
            }
            if cancel.is_cancelled() {
                return Err(LoopError::Cancelled);
            }
        }

        let stream_handle = provider.stream(llm::StreamRequest {
            model: model.clone(),
            context: llm_context.clone(),
            options: options.clone(),
            cancel: cancel.clone(),
        });
        let mut rx = stream_handle.events;
        let stream_task = tokio::task::spawn_local(stream_handle.task);

        let mut assistant_msg = AssistantMessage::empty(
            model.api.clone(),
            model.provider.clone(),
            model.id.clone(),
            StopReason::Stop,
        );

        // MessageStart
        {
            let msg = Message::from_assistant(assistant_msg.clone());
            event_tx.push(AgentEvent::MessageStart { message: msg });
        }

        // Read streaming events, racing against cancellation.
        let mut cancelled_during_stream = false;
        loop {
            tokio::select! {
                biased;
                _ = cancel.cancelled() => {
                    cancelled_during_stream = true;
                    break;
                }
                event = rx.recv() => {
                    match event {
                        Some(event) => {
                            let terminal = event.is_terminal();
                            llm::event::apply_event(&mut assistant_msg, &event);
                            event_tx.push(AgentEvent::MessageDelta {
                                event,
                            });
                            if terminal {
                                break;
                            }
                        }
                        None => break,
                    }
                }
            }
        }

        if cancelled_during_stream {
            // Emit MessageEnd for the partial message so the TUI completes.
            assistant_msg.stop_reason = StopReason::Aborted;
            let msg = Message::from_assistant(assistant_msg);
            event_tx.push(AgentEvent::MessageEnd { message: msg });
            drop(stream_task);
            return Err(LoopError::Cancelled);
        }

        // Wait for stream task.
        let stream_result = stream_task.await.unwrap_or_else(|e| {
            Err(llm::ProviderError::Other(format!(
                "stream task panicked: {e}"
            )))
        });

        match stream_result {
            Ok(()) => {
                let msg = Message::from_assistant(assistant_msg.clone());
                event_tx.push(AgentEvent::MessageEnd { message: msg });
                return Ok(assistant_msg);
            }
            Err(e) => {
                if matches!(e, llm::ProviderError::Cancelled) && cancel.is_cancelled() {
                    return Err(LoopError::Cancelled);
                }
                if !is_retryable(&e) || attempt == MAX_LLM_RETRIES {
                    let msg = Message::from_assistant(assistant_msg.clone());
                    event_tx.push(AgentEvent::MessageEnd { message: msg });
                    return Err(LoopError::Provider(e));
                }
                last_error = Some(e);
            }
        }
    }

    Err(LoopError::Provider(last_error.unwrap()))
}

// ---------------------------------------------------------------------------
// Retry helpers
// ---------------------------------------------------------------------------

fn is_retryable(err: &llm::ProviderError) -> bool {
    match err {
        llm::ProviderError::RateLimited { .. } => true,
        llm::ProviderError::Http(_) => true,
        llm::ProviderError::Api { status, .. } => matches!(status, 429 | 500 | 502 | 503 | 504),
        llm::ProviderError::Cancelled => false,
        llm::ProviderError::Other(_) => false,
    }
}

fn retry_delay_ms(
    err: &llm::ProviderError,
    attempt: u32,
    options: &llm::StreamOptions,
) -> Option<u64> {
    let delay = match err {
        llm::ProviderError::RateLimited {
            retry_after_ms: Some(ms),
        } => *ms,
        _ => BASE_RETRY_DELAY_MS.saturating_mul(1u64 << attempt),
    };
    if let Some(max_delay) = options.max_retry_delay_ms {
        if max_delay > 0 && delay > max_delay {
            return None;
        }
    }
    Some(delay)
}
