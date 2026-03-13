//! Session — the primary runtime wrapper around the agent loop.
//!
//! ## Actor model
//!
//! ```text
//! TUI / App ──LoopCommand──► Session loop
//! TUI / App ◄──AgentEvent── Session loop
//! ```
//!
//! The session loop runs inside a `spawn_local` task. It receives
//! commands via [`SessionHandle`] and emits [`AgentEvent`]s through
//! the event stream.

use std::cell::Cell;
use std::rc::Rc;

use llm::CancelToken;

use crate::agent_loop::AgentLoop;
use crate::extension::{LoopCommand, LoopHandle};
use crate::types::{AgentEvent, Message};

// ---------------------------------------------------------------------------
// SessionHandle — the external interface
// ---------------------------------------------------------------------------

/// Handle to a running session.
///
/// Cheap to clone. All external interaction (TUI, commands, async code)
/// goes through this.
#[derive(Clone)]
pub struct SessionHandle {
    handle: LoopHandle,
    running: Rc<Cell<bool>>,
}

impl SessionHandle {
    /// Send user input to the session.
    pub fn send_input(&self, text: impl Into<String>) {
        self.handle.inject(Message::user_text(text));
    }

    /// Inject a message for the agent loop.
    pub fn inject(&self, msg: Message) {
        self.handle.inject(msg);
    }

    /// Cancel the current operation.
    pub fn abort(&self) {
        self.handle.abort();
    }

    /// Request a clean shutdown.
    pub fn shutdown(&self) {
        self.handle.shutdown();
    }

    /// Change the model for subsequent requests.
    pub fn set_model(&self, model: llm::Model) {
        self.handle.set_model(model);
    }

    /// Check whether the agent loop is idle (between prompts).
    pub fn is_idle(&self) -> bool {
        !self.running.get()
    }

    /// Get the underlying LoopHandle.
    pub fn loop_handle(&self) -> &LoopHandle {
        &self.handle
    }
}

impl SessionHandle {
    /// Create a disconnected handle for testing.
    pub fn test_handle() -> Self {
        let (handle, _rx) = crate::extension::loop_handle_pair();
        Self {
            handle,
            running: Rc::new(Cell::new(false)),
        }
    }
}

// ---------------------------------------------------------------------------
// spawn — start the session loop as a local task
// ---------------------------------------------------------------------------

/// Spawn the agent loop as a persistent local task.
///
/// Returns a `SessionHandle` for sending commands and checking status.
///
/// The session loop runs until it receives `Shutdown` or all handles
/// are dropped.
///
/// # Panics
/// Must be called from within a `tokio::task::LocalSet`.
pub fn spawn(
    mut agent_loop: AgentLoop,
) -> SessionHandle {
    let handle = agent_loop.handle();
    let running = Rc::new(Cell::new(false));
    let session_handle = SessionHandle {
        handle: handle.clone(),
        running: running.clone(),
    };

    tokio::task::spawn_local(async move {
        let mut idle_messages: Vec<Message> = Vec::new();

        // Main loop: wait for commands, run prompts.
        loop {
            running.set(false);

            // Wait for user input (a message injected via LoopHandle).
            let prompt = loop {
                match agent_loop.cmd_rx_recv().await {
                    Some(LoopCommand::InjectMessage(msg)) => {
                        // Check if this looks like user input (from send_input)
                        break msg;
                    }
                    Some(LoopCommand::Shutdown) => return,
                    Some(LoopCommand::Abort) => {
                        // No active run to abort.
                    }
                    Some(LoopCommand::SteerMessage(msg)) => {
                        idle_messages.push(msg);
                    }
                    Some(LoopCommand::FollowUpMessage(msg)) => {
                        idle_messages.push(msg);
                    }
                    Some(LoopCommand::SetModel(model)) => {
                        agent_loop.state.model = model;
                    }
                    None => {
                        // All handles dropped.
                        return;
                    }
                }
            };

            // Start a new prompt.
            running.set(true);
            agent_loop.state.cancel = CancelToken::new();

            // Drain any messages injected while idle.
            for msg in idle_messages.drain(..) {
                agent_loop.state.messages.push(msg);
            }

            let result = agent_loop.run(prompt).await;

            if let Err(ref e) = result {
                agent_loop.emit(AgentEvent::AgentError {
                    message: format!("{e}"),
                });
                agent_loop.emit(AgentEvent::AgentEnd {
                    messages: agent_loop.state.messages.clone(),
                });
            }

            // Drain stragglers.
            while let Some(cmd) = agent_loop.cmd_rx_try_recv() {
                match cmd {
                    LoopCommand::Shutdown => return,
                    LoopCommand::Abort => {} // already finished
                    LoopCommand::InjectMessage(_) => {} // no active run
                    LoopCommand::SteerMessage(_) => {}
                    LoopCommand::FollowUpMessage(_) => {}
                    LoopCommand::SetModel(model) => {
                        agent_loop.state.model = model;
                    }
                }
            }
        }
    });

    session_handle
}
