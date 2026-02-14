//! Agent-level event stream type aliases.

use llm::event_stream;

use crate::types::{AgentEvent, AgentMessage};

pub type AgentEventSender = event_stream::EventStreamSender<AgentEvent, Vec<AgentMessage>>;
pub type AgentEventReceiver = event_stream::EventStreamReceiver<AgentEvent, Vec<AgentMessage>>;

/// Create a new agent event stream pair.
pub fn new_agent_stream() -> (AgentEventSender, AgentEventReceiver) {
    event_stream::event_stream(
        |e: &AgentEvent| e.is_terminal(),
        |e: &AgentEvent| match e {
            AgentEvent::AgentEnd { messages } => messages.clone(),
            _ => unreachable!(),
        },
    )
}
