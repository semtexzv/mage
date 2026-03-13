//! Agent-level event stream type aliases.

use llm::event_stream;

use crate::types::AgentEvent;

pub type AgentEventSender = event_stream::EventStreamSender<AgentEvent>;
pub type AgentEventReceiver = event_stream::EventStreamReceiver<AgentEvent>;

/// Create a new agent event stream pair.
pub fn new_agent_stream() -> (AgentEventSender, AgentEventReceiver) {
    event_stream::event_stream()
}
