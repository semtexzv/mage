//! Session — the primary runtime wrapper around the agent loop.
//!
//! `AgentSession` holds the agent state, extensions, inject queue, and
//! cancellation token. It is the mutable context passed to
//! `agent_loop::run()` and to extension hooks.
//!
//! `SessionHandle` is a cheap clone for async code running outside the
//! loop — can inject messages and abort, but cannot access state.

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::future::Future;
use std::pin::Pin;
use std::rc::Rc;
use std::task::{Poll, Waker};


use llm::{CancelToken, Model};

use crate::agent_loop::{self, LoopConfig, LoopError};
use crate::event_stream::{self, AgentEventReceiver};
use crate::extension::Extension;
use crate::types::{Message, AgentState, DeliverAs};

// ---------------------------------------------------------------------------
// InjectQueue — async message injection
// ---------------------------------------------------------------------------

/// Queue for injecting messages into the agent loop from outside.
///
/// Shared via `Rc<RefCell<>>` between the session and its handles.
pub type InjectQueue = Rc<RefCell<VecDeque<(Message, DeliverAs)>>>;

// ---------------------------------------------------------------------------
// Notify — minimal single-threaded notification primitive
// ---------------------------------------------------------------------------

/// Single-threaded notification: `notified()` returns a future that
/// resolves when `notify()` is called.
pub struct Notify {
    waker: Cell<Option<Waker>>,
}

impl Notify {
    pub fn new() -> Self {
        Self { waker: Cell::new(None) }
    }

    /// Wake the pending `notified()` future, if any.
    pub fn notify(&self) {
        if let Some(w) = self.waker.take() {
            w.wake();
        }
    }

    /// Returns a future that completes when [`notify()`](Self::notify) is called.
    pub fn notified(&self) -> NotifyFuture<'_> {
        NotifyFuture { notify: self }
    }
}

impl Default for Notify {
    fn default() -> Self {
        Self::new()
    }
}

/// Future returned by [`Notify::notified`].
pub struct NotifyFuture<'a> {
    notify: &'a Notify,
}

impl Future for NotifyFuture<'_> {
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> Poll<()> {
        // If a waker was already stored and then consumed by notify(),
        // we would have been woken — but poll sees no waker → ready.
        // On first poll, store the waker.
        let existing = self.notify.waker.take();
        if existing.is_some() {
            // Already notified between creation and first poll.
            Poll::Ready(())
        } else {
            self.notify.waker.set(Some(cx.waker().clone()));
            Poll::Pending
        }
    }
}

// ---------------------------------------------------------------------------
// SessionHandle — cheap clone for external async code
// ---------------------------------------------------------------------------

/// Handle to an `AgentSession` for code running outside the loop.
///
/// Can inject messages and abort, but cannot access state, tools,
/// or extensions.
#[derive(Clone)]
pub struct SessionHandle {
    inject: InjectQueue,
    cancel: CancelToken,
    idle_notify: Rc<Notify>,
    running: Rc<Cell<bool>>,
}

impl SessionHandle {
    /// Queue a message for delivery to the agent loop.
    pub fn inject(&self, msg: Message, deliver: DeliverAs) {
        self.inject.borrow_mut().push_back((msg, deliver));
        // If the loop is waiting for injection, wake it.
        self.idle_notify.notify();
    }

    /// Cancel the current operation.
    pub fn abort(&self) {
        self.cancel.cancel();
    }

    /// Check whether the agent loop is idle (between turns, not streaming).
    pub fn is_idle(&self) -> bool {
        !self.running.get()
    }

    /// Wait until the agent loop reaches idle state.
    pub async fn wait_for_idle(&self) {
        while self.running.get() {
            self.idle_notify.notified().await;
        }
    }
}

// ---------------------------------------------------------------------------
// AgentSession — the primary runtime object
// ---------------------------------------------------------------------------

/// The primary runtime object. Holds state, extensions, inject queue,
/// and cancellation. Passed to `agent_loop::run()` and extension hooks.
pub struct AgentSession {
    /// Mutable agent state: system prompt, model, messages, tools, options.
    pub state: AgentState,
    /// Extensions — temporarily removed during hook dispatch via `mem::take`.
    pub exts: Vec<Box<dyn Extension>>,
    /// Config for the agent loop (stream_fn, max_turns, etc.).
    pub(crate) config: LoopConfig,

    // TODO: pub store: Option<SessionStore>,  — Phase 5

    /// Queue for injecting messages from outside the loop.
    pub inject: InjectQueue,
    /// Cancellation token for the current operation.
    pub cancel: CancelToken,
    /// Notified when the loop becomes idle.
    pub(crate) idle_notify: Rc<Notify>,
    /// Whether the loop is currently running.
    pub(crate) running: Rc<Cell<bool>>,
}

impl AgentSession {
    /// Create a session from pre-built parts.
    pub fn from_parts(
        state: AgentState,
        exts: Vec<Box<dyn Extension>>,
        config: LoopConfig,
    ) -> Self {
        Self {
            state,
            exts,
            config,
            inject: Rc::new(RefCell::new(VecDeque::new())),
            cancel: CancelToken::new(),
            idle_notify: Rc::new(Notify::new()),
            running: Rc::new(Cell::new(false)),
        }
    }

    /// Create a cheap clone handle for external async code.
    pub fn handle(&self) -> SessionHandle {
        SessionHandle {
            inject: self.inject.clone(),
            cancel: self.cancel.clone(),
            idle_notify: self.idle_notify.clone(),
            running: self.running.clone(),
        }
    }

    /// Set the running flag. Called by the agent loop.
    pub(crate) fn set_running(&self, val: bool) {
        self.running.set(val);
        if !val {
            self.idle_notify.notify();
        }
    }

    // -----------------------------------------------------------------
    // Convenience API (moved from former Agent struct)
    // -----------------------------------------------------------------

    /// Run the agent with the given user prompt.
    /// Returns the event receiver — the caller reads events from it.
    pub async fn prompt(&mut self, text: &str) -> Result<AgentEventReceiver, LoopError> {
        self.state.messages.push(Message::user_text(text));
        self.cancel = CancelToken::new();

        let (tx, rx) = event_stream::new_agent_stream();
        agent_loop::run(self, &tx).await?;
        Ok(rx)
    }

    /// Cancel the current operation.
    pub fn abort(&self) {
        self.cancel.cancel();
    }

    /// Inject a steering message (interrupts current tool execution).
    pub fn steer(&mut self, text: &str) {
        self.state.messages.push(Message::user_text(text));
    }

    /// Access the conversation history.
    pub fn messages(&self) -> &[Message] {
        &self.state.messages
    }

    /// Access the model.
    pub fn model(&self) -> &Model {
        &self.state.model
    }

    // -----------------------------------------------------------------
    // Outbox helpers — used by agent_loop for hook context
    // -----------------------------------------------------------------

    /// Drain the inject queue into the outbox vectors.
    pub(crate) fn drain_inject(
        &self,
        steering: &mut Vec<Message>,
        follow_up: &mut Vec<Message>,
    ) {
        let mut queue = self.inject.borrow_mut();
        for (msg, deliver) in queue.drain(..) {
            match deliver {
                DeliverAs::Steer => steering.push(msg),
                DeliverAs::FollowUp | DeliverAs::NextTurn => follow_up.push(msg),
            }
        }
    }
}
