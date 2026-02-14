//! Agent loop — the sequential async flow that drives the agent.
//!
//! Everything is sequential. Only LLM streaming uses a channel.
//! Hook dispatch uses `mem::take` to temporarily remove extensions
//! from the session, then passes `&mut AgentSession` to each hook.

use std::time::Duration;

use llm::{
    AssistantMessage, ContentBlock, StopReason,
};

use crate::event_stream::AgentEventSender;
use crate::extension::{
    Disposition, MessageArgs, MessageDeltaArgs,
    ToolCallArgs, ToolExecEndArgs, ToolExecStartArgs,
    ToolResultArgs, TurnEndArgs, AgentEndArgs, ContextAmend, Registry,
};
use crate::session::AgentSession;
use crate::types::{AgentEvent, Message};
use crate::tool::{ToolResult, ToolContent};

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
    MaxTurnsExceeded(u32),
}

impl std::fmt::Display for LoopError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Provider(e) => write!(f, "provider error: {e}"),
            Self::NoProvider(api) => write!(f, "no provider registered for api: {api}"),
            Self::Cancelled => write!(f, "cancelled"),
            Self::MaxTurnsExceeded(n) => write!(f, "exceeded max turns ({n})"),
        }
    }
}

impl std::error::Error for LoopError {}

// ---------------------------------------------------------------------------
// StreamFn + Config
// ---------------------------------------------------------------------------

/// The streaming function: given a StreamRequest, returns a StreamHandle.
pub type StreamFn = Box<
    dyn Fn(llm::StreamRequest) -> llm::StreamHandle,
>;

/// Configuration for a single agent loop run.
pub struct LoopConfig {
    pub max_turns: u32,
    pub stream_fn: StreamFn,
    pub options: llm::StreamOptions,
    pub convert_to_llm: Box<dyn Fn(&[Message]) -> Vec<llm::Message>>,
}

// ---------------------------------------------------------------------------
// Agent loop — takes &mut AgentSession
// ---------------------------------------------------------------------------

pub async fn run(
    session: &mut AgentSession,
    events: &AgentEventSender,
) -> Result<(), LoopError> {
    session.set_running(true);

    // --- extension init: let each extension register tools/providers ---
    {
        let mut exts = std::mem::take(&mut session.exts);
        let mut ext_providers = Vec::new();
        let mut registry = Registry {
            tools: &mut session.state.tools,
            providers: &mut ext_providers,
        };
        for ext in exts.iter_mut() {
            ext.init(&mut registry);
        }
        session.exts = exts;
        // Extension-registered providers are currently unused by the loop
        // (stream_fn is pre-built). Reserved for future use.
        let _ = ext_providers;
    }

    // Queues for steering and follow-up messages.
    let mut steering_queue: Vec<Message> = Vec::new();
    let mut follow_up_queue: Vec<Message> = Vec::new();

    // --- agent_start ---
    events.push(AgentEvent::AgentStart);
    fire_observe!(session, on_agent_start);
    session.drain_inject(&mut steering_queue, &mut follow_up_queue);

    let mut turn_count: u32 = 0;

    loop {
        if session.cancel.is_cancelled() {
            session.set_running(false);
            return Err(LoopError::Cancelled);
        }
        if session.config.max_turns > 0 && turn_count >= session.config.max_turns {
            session.set_running(false);
            return Err(LoopError::MaxTurnsExceeded(session.config.max_turns));
        }
        turn_count += 1;

        // --- turn_start ---
        events.push(AgentEvent::TurnStart);
        fire_observe!(session, on_turn_start);
        session.drain_inject(&mut steering_queue, &mut follow_up_queue);

        // --- on_context (chain): hooks may replace message list ---
        let llm_messages = {
            let mut agent_messages = session.state.messages.clone();
            let mut exts = std::mem::take(&mut session.exts);
            for h in exts.iter_mut() {
                let d = h.on_context(&agent_messages, session).await;
                match d {
                    Disposition::Propagate => {}
                    Disposition::Block { .. } => {
                        session.exts = exts;
                        session.set_running(false);
                        return Err(LoopError::Cancelled);
                    }
                    Disposition::Value(ContextAmend { messages }) => {
                        agent_messages = messages;
                    }
                }
            }
            session.exts = exts;
            session.drain_inject(&mut steering_queue, &mut follow_up_queue);
            (session.config.convert_to_llm)(&agent_messages)
        };

        // --- stream LLM ---
        let assistant_msg = stream_llm(
            session, events, llm_messages,
        )
        .await?;

        // Add assistant message to conversation
        let final_agent_msg = Message::from_assistant(assistant_msg.clone());
        session.state.messages.push(final_agent_msg.clone());

        // --- tool execution ---
        let tool_calls: Vec<_> = assistant_msg
            .content
            .iter()
            .filter_map(|block| match block {
                ContentBlock::ToolCall { id, name, arguments, .. } => {
                    Some((id.clone(), name.clone(), arguments.clone()))
                }
                _ => None,
            })
            .collect();

        let tool_result_messages = if !tool_calls.is_empty() {
            execute_tools(
                session, events,
                &mut steering_queue, &mut follow_up_queue,
                &tool_calls,
            )
            .await
        } else {
            vec![]
        };

        // --- turn_end ---
        events.push(AgentEvent::TurnEnd {
            message: final_agent_msg.clone(),
            tool_results: tool_result_messages.clone(),
        });
        {
            let args = TurnEndArgs {
                message: &final_agent_msg,
                tool_results: &tool_result_messages,
            };
            let mut exts = std::mem::take(&mut session.exts);
            for h in exts.iter_mut() {
                h.on_turn_end(&args, session);
            }
            session.exts = exts;
        }
        session.drain_inject(&mut steering_queue, &mut follow_up_queue);

        // Add tool results to conversation
        for trm in &tool_result_messages {
            session.state.messages.push(Message::from_tool_result(trm.clone()));
        }

        // If stop reason wasn't ToolUse, check follow-ups or finish
        if assistant_msg.stop_reason != StopReason::ToolUse {
            if follow_up_queue.is_empty() {
                break;
            }
            session.state.messages.extend(follow_up_queue.drain(..));
            continue;
        }

        // Continuing to next turn — LLM will see tool results
        if !follow_up_queue.is_empty() {
            session.state.messages.extend(follow_up_queue.drain(..));
        }
    }

    // --- agent_end ---
    let final_messages = session.state.messages.clone();
    events.push(AgentEvent::AgentEnd { messages: final_messages.clone() });
    {
        let args = AgentEndArgs { messages: &final_messages };
        let mut exts = std::mem::take(&mut session.exts);
        for h in exts.iter_mut() {
            h.on_agent_end(&args, session);
        }
        session.exts = exts;
    }

    session.set_running(false);
    Ok(())
}

// ---------------------------------------------------------------------------
// Stream LLM response
// ---------------------------------------------------------------------------

async fn stream_llm(
    session: &mut AgentSession,
    events: &AgentEventSender,
    llm_messages: Vec<llm::Message>,
) -> Result<AssistantMessage, LoopError> {
    let llm_context = llm::Context {
        system_prompt: Some(session.state.system_prompt.as_str().into()),
        messages: llm_messages,
        tools: if session.state.tools.is_empty() { None } else { Some(session.state.llm_tools()) },
    };

    let mut last_error: Option<llm::ProviderError> = None;

    for attempt in 0..=MAX_LLM_RETRIES {
        if attempt > 0 {
            // We are retrying — compute delay and sleep
            let prev_err = last_error.as_ref().unwrap();
            let delay = retry_delay_ms(prev_err, attempt - 1, &session.config.options);
            match delay {
                None => {
                    // Delay exceeds max_retry_delay_ms — stop retrying
                    return Err(LoopError::Provider(last_error.take().unwrap()));
                }
                Some(ms) => {
                    tokio::time::sleep(Duration::from_millis(ms)).await;
                }
            }
            if session.cancel.is_cancelled() {
                return Err(LoopError::Cancelled);
            }
        }
        let handle = (session.config.stream_fn)(llm::StreamRequest {
            model: session.state.model.clone(),
            context: llm_context.clone(),
            options: session.config.options.clone(),
            cancel: session.cancel.clone(),
        });
        let mut rx = handle.events;
        let stream_handle = tokio::task::spawn_local(handle.task);
        let mut assistant_msg = AssistantMessage::empty(
            session.state.model.api.clone(),
            session.state.model.provider.clone(),
            session.state.model.id.clone(),
            StopReason::Stop,
        );
    // message_start
        {
            let msg = Message::from_assistant(assistant_msg.clone());
            events.push(AgentEvent::MessageStart { message: msg.clone() });
            let args = MessageArgs { message: &msg };
            let mut exts = std::mem::take(&mut session.exts);
            for h in exts.iter_mut() {
                h.on_message_start(&args, session);
            }
            session.exts = exts;
        }
    // Read events
        while let Some(event) = rx.recv().await {
            llm::event::apply_event(&mut assistant_msg, &event);
            let msg = Message::from_assistant(assistant_msg.clone());
            events.push(AgentEvent::MessageUpdate {
                message: msg,
                assistant_message_event: event.clone(),
            });

            {
                let delta_args = MessageDeltaArgs { event: &event };
                let mut exts = std::mem::take(&mut session.exts);
                for h in exts.iter_mut() {
                    h.on_message_delta(&delta_args, session);
                }
                session.exts = exts;
            }
            if event.is_terminal() {
                break;
            }
        }
        // Wait for stream task
        let stream_result = stream_handle.await.unwrap_or_else(|e| {
            Err(llm::ProviderError::Other(format!("stream task panicked: {e}")))
        });

        match stream_result {
            Ok(()) => {
                // message_end
                {
                    let msg = Message::from_assistant(assistant_msg.clone());
                    events.push(AgentEvent::MessageEnd { message: msg.clone() });
                    let args = MessageArgs { message: &msg };
                    let mut exts = std::mem::take(&mut session.exts);
                    for h in exts.iter_mut() {
                        h.on_message_end(&args, session);
                    }
                    session.exts = exts;
                }
                return Ok(assistant_msg);
            }
            Err(e) => {
                if matches!(e, llm::ProviderError::Cancelled) && session.cancel.is_cancelled() {
                    return Err(LoopError::Cancelled);
                }
                if !is_retryable(&e) || attempt == MAX_LLM_RETRIES {
                    // message_end (even on error)
                    {
                        let msg = Message::from_assistant(assistant_msg.clone());
                        events.push(AgentEvent::MessageEnd { message: msg.clone() });
                        let args = MessageArgs { message: &msg };
                        let mut exts = std::mem::take(&mut session.exts);
                        for h in exts.iter_mut() {
                            h.on_message_end(&args, session);
                        }
                        session.exts = exts;
                    }
                    return Err(LoopError::Provider(e));
                }
                last_error = Some(e);
                // Don't fire message_end — the attempt is being discarded
            }
        }
    }

    // Unreachable: loop always returns
    Err(LoopError::Provider(last_error.unwrap()))
}

// ---------------------------------------------------------------------------
// Tool execution
// ---------------------------------------------------------------------------

#[allow(clippy::too_many_arguments)]
async fn execute_tools(
    session: &mut AgentSession,
    events: &AgentEventSender,
    steering_queue: &mut Vec<Message>,
    follow_up_queue: &mut Vec<Message>,
    tool_calls: &[(refstr::Str, refstr::Str, serde_json::Value)],
) -> Vec<llm::ToolResultMessage> {
    let mut tool_result_messages = Vec::new();

    for (call_id, call_name, call_args) in tool_calls {
        if session.cancel.is_cancelled() {
            break;
        }

        // --- on_tool_call (short-circuit) ---
        let mut blocked = false;
        {
            let mut exts = std::mem::take(&mut session.exts);
            for h in exts.iter_mut() {
                let tc_args = ToolCallArgs {
                    name: call_name,
                    id: call_id,
                    args: call_args,
                };
                match h.on_tool_call(&tc_args, session).await {
                    Disposition::Propagate | Disposition::Value(()) => {}
                    Disposition::Block { reason } => {
                        tool_result_messages.push(make_error_result(
                            call_id.clone(),
                            call_name.clone(),
                            format!("Tool call blocked: {reason}"),
                        ));
                        blocked = true;
                        break;
                    }
                }
            }
            session.exts = exts;
        }
        session.drain_inject(steering_queue, follow_up_queue);
        if blocked {
            continue;
        }

        // Find the tool
        let tool_idx = match session.state.tools.iter().position(|t| t.name() == &**call_name) {
            Some(i) => i,
            None => {
                tool_result_messages.push(make_error_result(
                    call_id.clone(),
                    call_name.clone(),
                    format!("Tool '{}' not found", call_name),
                ));
                continue;
            }
        };

        // tool_exec_start
        events.push(AgentEvent::ToolExecutionStart {
            tool_call_id: call_id.clone(),
            tool_name: call_name.clone(),
            args: call_args.clone(),
        });
        {
            let args = ToolExecStartArgs { name: call_name, args: call_args };
            let mut exts = std::mem::take(&mut session.exts);
            for h in exts.iter_mut() {
                h.on_tool_exec_start(&args, session);
            }
            session.exts = exts;
        }

        // Execute the tool
        let erased_exec = session.state.tools[tool_idx].execute(
            call_id, call_args.clone(), session.cancel.clone(),
        );
        let mut result = match erased_exec {
            crate::tool::ErasedExecution::Ready(r) => r,
            crate::tool::ErasedExecution::Running(task) => task.await,
            crate::tool::ErasedExecution::Custom { task } => task.await,
        };
        let mut is_error = result.is_error();

        // --- on_tool_result (chain) ---
        {
            let mut exts = std::mem::take(&mut session.exts);
            for h in exts.iter_mut() {
                let tr_args = ToolResultArgs {
                    name: call_name,
                    id: call_id,
                    result: &result,
                    is_error,
                };
                match h.on_tool_result(&tr_args, session).await {
                    Disposition::Propagate => {}
                    Disposition::Block { .. } => break,
                    Disposition::Value(amend) => {
                        if let Some(content) = amend.content {
                            result = if is_error {
                                ToolResult::Failure(ToolContent::rich(content))
                            } else {
                                ToolResult::Success(ToolContent::rich(content))
                            };
                        }
                        if let Some(err) = amend.is_error {
                            is_error = err;
                        }
                    }
                }
            }
            session.exts = exts;
        }
        session.drain_inject(steering_queue, follow_up_queue);

        // tool_exec_end
        events.push(AgentEvent::ToolExecutionEnd {
            tool_call_id: call_id.clone(),
            tool_name: call_name.clone(),
            result: result.clone(),
            is_error,
        });
        {
            let args = ToolExecEndArgs { name: call_name, result: &result, is_error };
            let mut exts = std::mem::take(&mut session.exts);
            for h in exts.iter_mut() {
                h.on_tool_exec_end(&args, session);
            }
            session.exts = exts;
        }

        let tool_content = session.state.tools[tool_idx].to_result(&result);
        tool_result_messages.push(llm::ToolResultMessage {
            tool_call_id: call_id.clone(),
            tool_name: call_name.clone(),
            content: tool_content.content,
            details: None,
            is_error,
            timestamp: 0,
        });

        // Check steering queue
        if !steering_queue.is_empty() {
            session.state.messages.extend(steering_queue.drain(..));
        }
    }

    tool_result_messages
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn make_error_result(
    tool_call_id: refstr::Str,
    tool_name: refstr::Str,
    message: String,
) -> llm::ToolResultMessage {
    llm::ToolResultMessage {
        tool_call_id,
        tool_name,
        content: vec![llm::UserContent::Text { text: message }],
        details: None,
        is_error: true,
        timestamp: 0,
    }
}

fn is_retryable(err: &llm::ProviderError) -> bool {
    match err {
        llm::ProviderError::RateLimited { .. } => true,
        llm::ProviderError::Http(_) => true,
        llm::ProviderError::Api { status, .. } => matches!(status, 429 | 500 | 502 | 503 | 504),
        llm::ProviderError::Cancelled => false,
        llm::ProviderError::Other(_) => false,
    }
}

/// Returns `Some(delay_ms)` for the retry, or `None` if the delay would exceed
/// `max_retry_delay_ms` (meaning we should stop retrying).
fn retry_delay_ms(
    err: &llm::ProviderError,
    attempt: u32,
    options: &llm::StreamOptions,
) -> Option<u64> {
    let delay = match err {
        llm::ProviderError::RateLimited { retry_after_ms: Some(ms) } => *ms,
        _ => BASE_RETRY_DELAY_MS.saturating_mul(1u64 << attempt),
    };
    // Clamp to max_retry_delay_ms if set; if delay exceeds it, signal stop
    if let Some(max_delay) = options.max_retry_delay_ms {
        if max_delay > 0 && delay > max_delay {
            return None;
        }
    }
    Some(delay)
}


// ---------------------------------------------------------------------------
// Macro for observe hooks (sync, no return value)
// ---------------------------------------------------------------------------

/// Fires a sync observe hook on all extensions.
/// Uses mem::take pattern to borrow session mutably.
macro_rules! fire_observe {
    ($session:expr, $method:ident) => {
        {
            let mut exts = std::mem::take(&mut $session.exts);
            for h in exts.iter_mut() {
                h.$method($session);
            }
            $session.exts = exts;
        }
    };
    ($session:expr, $method:ident, $args:expr) => {
        {
            let mut exts = std::mem::take(&mut $session.exts);
            for h in exts.iter_mut() {
                h.$method($args, $session);
            }
            $session.exts = exts;
        }
    };
}
use fire_observe;
