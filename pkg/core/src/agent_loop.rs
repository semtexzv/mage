//! Agent loop — the core async loop that drives the agent.
//!
//! `AgentLoop` owns state + extensions as separate fields (borrow-friendly).
//! The loop is a double-nested structure:
//!
//! ```text
//! outer: loop {                     // continues on follow-up
//!     inner: loop {                 // continues while tool calls or steering
//!         stream assistant response
//!         execute tools (checking steering between each)
//!     }
//!     check follow-up → continue outer
//! }
//! ```

use std::rc::Rc;
use std::time::Duration;

use refstr::Str;

use llm::{
    AssistantMessage, CancelToken, ContentBlock, StopReason,
};

use crate::event_stream::{AgentEventSender, AgentEventReceiver, new_agent_stream};
use crate::extension::{
    init_extensions, Extension, LoopCommand, LoopHandle,
    LoopState, Queues, RegisteredTool, ToolHandle,
    loop_handle_pair,
    AgentEndEvent, BeforeAgentStartEvent, ContextEvent,
    InputEvent, InputResult, InputSource, TurnStartEvent, TurnEndEvent,
    ToolCallEvent, ToolResultEvent,
};
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

/// The core agent loop. Owns state + extensions as separate fields.
pub struct AgentLoop {
    /// Mutable state that extension callbacks can borrow.
    pub state: LoopState,

    /// Extensions — borrowed independently from `state`.
    extensions: Vec<Box<dyn Extension>>,

    /// Tools registered during init (closures).
    registered_tools: Vec<RegisteredTool>,

    /// Providers keyed by API name.
    providers: Vec<(Str, Rc<dyn llm::Provider>)>,

    /// Incoming commands from LoopHandle / SessionHandle.
    cmd_rx: llm::channel::Receiver<LoopCommand>,

    /// Outgoing events to UI / observers.
    event_tx: AgentEventSender,
}

impl AgentLoop {
    /// Create a new agent loop.
    ///
    /// Returns the loop and an event receiver for the UI.
    pub fn new(
        system_prompt: impl Into<String>,
        model: llm::Model,
        options: llm::StreamOptions,
        providers: Vec<(Str, Rc<dyn llm::Provider>)>,
        mut extensions: Vec<Box<dyn Extension>>,
    ) -> (Self, AgentEventReceiver) {
        let (handle, cmd_rx) = loop_handle_pair();
        let (event_tx, event_rx) = new_agent_stream();

        // Initialize extensions — collect tools and extension-registered providers.
        let (registered_tools, ext_providers) = init_extensions(&mut extensions);

        // Merge caller-provided and extension-registered providers.
        let mut all_providers = providers;
        all_providers.extend(ext_providers);

        // Extract tool schemas for LLM requests.
        let tool_schemas: Vec<llm::Tool> = registered_tools
            .iter()
            .map(|t| t.schema.clone())
            .collect();

        let this = Self {
            state: LoopState {
                messages: Vec::new(),
                system_prompt: system_prompt.into(),
                model,
                options,
                tool_schemas,
                queues: Queues::new(),
                handle,
                cancel: CancelToken::new(),
            },
            extensions,
            registered_tools,
            providers: all_providers,
            cmd_rx,
            event_tx,
        };

        (this, event_rx)
    }

    /// Get a LoopHandle for external use.
    pub fn handle(&self) -> LoopHandle {
        self.state.handle.clone()
    }

    pub(crate) fn emit(&self, event: AgentEvent) {
        self.event_tx.push(event);
    }

    /// Receive a command (blocking). Used by session spawn loop.
    pub(crate) async fn cmd_rx_recv(&mut self) -> Option<LoopCommand> {
        self.cmd_rx.recv().await
    }

    /// Try to receive a command (non-blocking). Used by session spawn loop.
    pub(crate) fn cmd_rx_try_recv(&mut self) -> Option<LoopCommand> {
        self.cmd_rx.try_recv()
    }

    fn drain_commands(&mut self, run_cancel: &CancelToken) {
        while let Some(cmd) = self.cmd_rx.try_recv() {
            self.apply_command(cmd, run_cancel);
        }
    }

    fn apply_command(&mut self, cmd: LoopCommand, run_cancel: &CancelToken) {
        match cmd {
            LoopCommand::InjectMessage(msg) => self.state.queues.inject.push_back(msg),
            LoopCommand::SteerMessage(msg) => self.state.queues.steering.push_back(msg),
            LoopCommand::FollowUpMessage(msg) => self.state.queues.followup.push_back(msg),
            LoopCommand::Abort => run_cancel.cancel(),
            LoopCommand::Shutdown => {
                self.state.cancel.cancel();
                run_cancel.cancel();
            }
            LoopCommand::SetModel(model) => self.state.model = model,
        }
    }

    // -----------------------------------------------------------------------
    // Main run loop
    // -----------------------------------------------------------------------

    pub async fn run(&mut self, prompt: Message) -> Result<(), LoopError> {
        let run_cancel = CancelToken::new();

        // ── on_input ─────────────────────────────────────────────────
        let text = extract_text(&prompt);
        let input_event = InputEvent {
            text: text.clone(),
            source: InputSource::Interactive,
        };
        let input_result =
            emit_input(&mut self.extensions, &mut self.state, &input_event).await;
        let prompt = match input_result {
            InputResult::Handled => return Ok(()),
            InputResult::Transform { text } => Message::user_text(text),
            InputResult::Continue => prompt,
        };

        // ── on_before_agent_start ────────────────────────────────────
        let bas = BeforeAgentStartEvent {
            prompt: extract_text(&prompt),
            system_prompt: self.state.system_prompt.clone(),
        };
        let bas_result =
            emit_before_agent_start(&mut self.extensions, &mut self.state, bas).await;
        self.state.system_prompt = bas_result.system_prompt;

        // Push prompt + any injected messages
        self.state.messages.push(prompt.clone());
        self.drain_commands(&run_cancel);
        while let Some(msg) = self.state.queues.inject.pop_front() {
            self.state.messages.push(msg);
        }

        let mut new_messages: Vec<Message> = vec![prompt];
        let mut turn_index: usize = 0;

        // ── on_agent_start ───────────────────────────────────────────
        for ext in &mut self.extensions {
            ext.on_agent_start(&mut self.state.ext_ctx()).await;
        }
        self.emit(AgentEvent::AgentStart);

        // ── outer loop ───────────────────────────────────────────────
        'outer: loop {
            if run_cancel.is_cancelled() {
                break 'outer;
            }

            let mut has_more_tool_calls = true;
            self.drain_commands(&run_cancel);
            let mut pending: Vec<Message> = self.state.queues.steering.drain(..).collect();

            // ── inner loop ───────────────────────────────────────────
            while has_more_tool_calls || !pending.is_empty() {
                if run_cancel.is_cancelled() {
                    break 'outer;
                }

                // on_turn_start
                let ts_event = TurnStartEvent { turn_index };
                for ext in &mut self.extensions {
                    ext.on_turn_start(&ts_event, &mut self.state.ext_ctx()).await;
                }
                self.emit(AgentEvent::TurnStart { turn_index });

                // Inject pending steering messages
                for msg in pending.drain(..) {
                    self.state.messages.push(msg.clone());
                    new_messages.push(msg);
                }

                // on_context — chain message modifications
                let mut llm_messages = self.state.to_llm_messages();
                let ctx_event = ContextEvent {
                    messages: self.state.messages.clone(),
                };
                for ext in &mut self.extensions {
                    if let Some(r) =
                        ext.on_context(&ctx_event, &mut self.state.ext_ctx()).await
                    {
                        if let Some(m) = r.messages {
                            // Replace with transformed messages, converting to LLM format
                            llm_messages = m
                                .iter()
                                .filter(|msg| !msg.ephemeral)
                                .map(|msg| msg.to_llm())
                                .collect();
                        }
                    }
                }

                // ── stream assistant response ────────────────────────
                let assistant_msg = stream_llm(
                    &self.providers,
                    &mut self.state,
                    &mut self.extensions,
                    &self.event_tx,
                    llm_messages,
                    &run_cancel,
                )
                .await?;

                let final_msg = Message::from_assistant(assistant_msg.clone());
                self.state.messages.push(final_msg.clone());
                new_messages.push(final_msg.clone());

                if matches!(assistant_msg.stop_reason, StopReason::Error) {
                    let te = TurnEndEvent {
                        turn_index,
                        message: final_msg.clone(),
                        tool_results: vec![],
                    };
                    for ext in &mut self.extensions {
                        ext.on_turn_end(&te, &mut self.state.ext_ctx()).await;
                    }
                    self.emit(AgentEvent::TurnEnd {
                        turn_index,
                        message: final_msg,
                        tool_results: vec![],
                    });
                    break 'outer;
                }

                // ── execute tool calls ───────────────────────────────
                let tool_calls: Vec<(Str, Str, serde_json::Value)> = assistant_msg
                    .content
                    .iter()
                    .filter_map(|block| match block {
                        ContentBlock::ToolCall { id, name, arguments, .. } => {
                            Some((id.clone(), name.clone(), arguments.clone()))
                        }
                        _ => None,
                    })
                    .collect();

                has_more_tool_calls = !tool_calls.is_empty();
                let mut tool_results: Vec<ToolResult> = Vec::new();

                for (i, (call_id, call_name, call_args)) in tool_calls.iter().enumerate() {
                    // Check cancellation or steering interruption
                    self.drain_commands(&run_cancel);
                    if run_cancel.is_cancelled() || !self.state.queues.steering.is_empty() {
                        for (_skip_id, _skip_name, _) in &tool_calls[i..] {
                            let r = ToolResult::skipped();
                            let trm = make_tool_result_message(
                                _skip_id.clone(),
                                _skip_name.clone(),
                                &r,
                            );
                            self.state.messages.push(Message::from_tool_result(trm));
                            tool_results.push(r);
                        }
                        break;
                    }

                    // on_tool_call — may block
                    let tc_event = ToolCallEvent {
                        tool_call_id: call_id.clone(),
                        tool_name: call_name.clone(),
                        arguments: call_args.clone(),
                    };
                    let mut blocked = false;
                    for ext in &mut self.extensions {
                        if let Some(r) =
                            ext.on_tool_call(&tc_event, &mut self.state.ext_ctx()).await
                        {
                            if r.block {
                                let reason =
                                    r.reason.unwrap_or_else(|| "Blocked by extension".into());
                                let result = ToolResult::failure(reason);
                                let trm = make_tool_result_message(
                                    call_id.clone(),
                                    call_name.clone(),
                                    &result,
                                );
                                self.state.messages.push(Message::from_tool_result(trm));
                                tool_results.push(result);
                                blocked = true;
                                break;
                            }
                        }
                    }
                    if blocked {
                        continue;
                    }

                    self.emit(AgentEvent::ToolExecStart {
                        tool_call_id: call_id.clone(),
                        tool_name: call_name.clone(),
                        args: call_args.clone(),
                    });

                    // Execute
                    let mut result = execute_tool(
                        &self.registered_tools,
                        &self.state.handle,
                        call_id,
                        call_name,
                        call_args,
                        &run_cancel,
                        &self.event_tx,
                    )
                    .await;

                    // on_tool_result — may modify
                    let tr_event = ToolResultEvent {
                        tool_call_id: call_id.clone(),
                        tool_name: call_name.clone(),
                        result: result.clone(),
                    };
                    for ext in &mut self.extensions {
                        if let Some(r) =
                            ext.on_tool_result(&tr_event, &mut self.state.ext_ctx()).await
                        {
                            if let Some(c) = r.content {
                                result.content = c;
                            }
                            if let Some(e) = r.is_error {
                                result.is_error = e;
                            }
                        }
                    }

                    self.emit(AgentEvent::ToolExecEnd {
                        tool_call_id: call_id.clone(),
                        tool_name: call_name.clone(),
                        result: result.clone(),
                    });

                    let trm =
                        make_tool_result_message(call_id.clone(), call_name.clone(), &result);
                    self.state.messages.push(Message::from_tool_result(trm));
                    new_messages.push(
                        self.state.messages.last().cloned().unwrap(),
                    );
                    tool_results.push(result);
                }

                // on_turn_end
                let te = TurnEndEvent {
                    turn_index,
                    message: final_msg.clone(),
                    tool_results: tool_results.clone(),
                };
                for ext in &mut self.extensions {
                    ext.on_turn_end(&te, &mut self.state.ext_ctx()).await;
                }
                self.emit(AgentEvent::TurnEnd {
                    turn_index,
                    message: final_msg,
                    tool_results,
                });
                turn_index += 1;

                self.drain_commands(&run_cancel);
                pending = self.state.queues.steering.drain(..).collect();
            }

            // ── follow-up ────────────────────────────────────────────
            self.drain_commands(&run_cancel);
            let follow_ups: Vec<Message> = self.state.queues.followup.drain(..).collect();
            if !follow_ups.is_empty() {
                for msg in &follow_ups {
                    self.state.messages.push(msg.clone());
                    new_messages.push(msg.clone());
                }
                continue 'outer;
            }
            break 'outer;
        }

        // ── on_agent_end ─────────────────────────────────────────────
        let end_event = AgentEndEvent {
            messages: new_messages.clone(),
        };
        for ext in &mut self.extensions {
            ext.on_agent_end(&end_event, &mut self.state.ext_ctx()).await;
        }
        self.emit(AgentEvent::AgentEnd {
            messages: new_messages,
        });

        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Free functions — borrow only what they need
// ---------------------------------------------------------------------------

fn extract_text(msg: &Message) -> String {
    match &msg.body {
        crate::types::MessageBody::User { content } => match content {
            llm::UserMessageContent::Text(t) => t.clone(),
            llm::UserMessageContent::Rich(parts) => parts
                .iter()
                .filter_map(|p| match p {
                    llm::UserContent::Text { text } => Some(text.as_str()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join(""),
        },
        _ => String::new(),
    }
}

/// Stream an assistant response. Borrows only what's needed.
async fn stream_llm(
    providers: &[(Str, Rc<dyn llm::Provider>)],
    state: &mut LoopState,
    extensions: &mut [Box<dyn Extension>],
    event_tx: &AgentEventSender,
    llm_messages: Vec<llm::Message>,
    cancel: &CancelToken,
) -> Result<AssistantMessage, LoopError> {
    let llm_context = llm::Context {
        system_prompt: Some(Str::from(state.system_prompt.as_str())),
        messages: llm_messages,
        tools: if state.tool_schemas.is_empty() {
            None
        } else {
            Some(state.tool_schemas.clone())
        },
    };

    // Resolve provider by model's API
    let provider: Rc<dyn llm::Provider> = providers
        .iter()
        .find(|(api, _)| **api == *state.model.api)
        .map(|(_, p)| p.clone())
        .ok_or_else(|| LoopError::NoProvider(state.model.api.to_string()))?;

    let mut last_error: Option<llm::ProviderError> = None;

    for attempt in 0..=MAX_LLM_RETRIES {
        if attempt > 0 {
            let prev_err = last_error.as_ref().unwrap();
            let delay = retry_delay_ms(prev_err, attempt - 1, &state.options);
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
            model: state.model.clone(),
            context: llm_context.clone(),
            options: state.options.clone(),
            cancel: cancel.clone(),
        });
        let mut rx = stream_handle.events;
        let stream_task = tokio::task::spawn_local(stream_handle.task);

        let mut assistant_msg = AssistantMessage::empty(
            state.model.api.clone(),
            state.model.provider.clone(),
            state.model.id.clone(),
            StopReason::Stop,
        );

        // message_start
        {
            let msg = Message::from_assistant(assistant_msg.clone());
            event_tx.push(AgentEvent::MessageStart { message: msg });
        }

        // Read events
        while let Some(event) = rx.recv().await {
            llm::event::apply_event(&mut assistant_msg, &event);

            event_tx.push(AgentEvent::MessageDelta { event: event.clone() });

            // Fire on_message_delta hooks.
            for ext in extensions.iter_mut() {
                ext.on_message_delta(&event, &mut state.ext_ctx()).await;
            }

            if event.is_terminal() {
                break;
            }
        }

        // Wait for stream task
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

/// Execute a tool call with progress update draining.
async fn execute_tool(
    registered_tools: &[RegisteredTool],
    loop_handle: &LoopHandle,
    call_id: &Str,
    call_name: &Str,
    call_args: &serde_json::Value,
    run_cancel: &CancelToken,
    event_tx: &AgentEventSender,
) -> ToolResult {
    let Some(tool) = registered_tools.iter().find(|t| *t.schema.name == **call_name) else {
        return ToolResult::failure(format!("Tool '{}' not found", call_name));
    };

    let tool_cancel = CancelToken::new();
    let (update_tx, mut update_rx) = tokio::sync::mpsc::unbounded_channel::<ToolUpdate>();

    let handle = ToolHandle::new(
        call_id.to_string(),
        tool_cancel.clone(),
        update_tx,
        loop_handle.clone(),
    );

    let mut tool_fut = std::pin::pin!((tool.execute)(
        call_id.to_string(),
        call_args.clone(),
        handle
    ));

    loop {
        tokio::select! {
            biased;

            () = std::future::ready(()), if run_cancel.is_cancelled() => {
                tool_cancel.cancel();
                return ToolResult::failure("Cancelled");
            }

            result = &mut tool_fut => {
                // Drain any buffered updates
                while let Ok(update) = update_rx.try_recv() {
                    event_tx.push(AgentEvent::ToolExecUpdate {
                        tool_call_id: call_id.clone(),
                        tool_name: call_name.clone(),
                        update,
                    });
                }
                return result;
            }

            Some(update) = update_rx.recv() => {
                event_tx.push(AgentEvent::ToolExecUpdate {
                    tool_call_id: call_id.clone(),
                    tool_name: call_name.clone(),
                    update,
                });
            }
        }
    }
}

/// Emit `on_input` across extensions, chaining transforms.
async fn emit_input(
    extensions: &mut [Box<dyn Extension>],
    state: &mut LoopState,
    event: &InputEvent,
) -> InputResult {
    let mut current_text = event.text.clone();
    let original_text = current_text.clone();

    for ext in extensions.iter_mut() {
        let input = InputEvent {
            text: current_text.clone(),
            source: event.source,
        };
        if let Some(r) = ext.on_input(&input, &mut state.ext_ctx()).await {
            match r {
                InputResult::Handled => return InputResult::Handled,
                InputResult::Transform { text } => current_text = text,
                InputResult::Continue => {}
            }
        }
    }

    if current_text != original_text {
        InputResult::Transform {
            text: current_text,
        }
    } else {
        InputResult::Continue
    }
}

/// Emit `on_before_agent_start`, chaining system prompt modifications.
async fn emit_before_agent_start(
    extensions: &mut [Box<dyn Extension>],
    state: &mut LoopState,
    mut event: BeforeAgentStartEvent,
) -> BeforeAgentStartEvent {
    for ext in extensions.iter_mut() {
        if let Some(r) = ext
            .on_before_agent_start(&event, &mut state.ext_ctx())
            .await
        {
            if let Some(sp) = r.system_prompt {
                event.system_prompt = sp;
            }
        }
    }
    event
}

fn make_tool_result_message(
    tool_call_id: Str,
    tool_name: Str,
    result: &ToolResult,
) -> llm::ToolResultMessage {
    llm::ToolResultMessage {
        tool_call_id,
        tool_name,
        content: result.content.clone(),
        details: None,
        is_error: result.is_error,
    }
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
