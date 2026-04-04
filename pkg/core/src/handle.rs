//! Loop handle — the channel-based command interface into the agent loop.
//!
//! [`LoopHandle`] is cheap to clone and safe to use from spawned tasks.
//! All methods are fire-and-forget (silently drop if the loop has shut down).

use std::cell::Cell;
use std::rc::Rc;

use llm::CancelToken;
use llm::Model;

use crate::types::Message;

/// Commands sent to the agent loop via [`LoopHandle`].
#[derive(Debug)]
pub enum LoopCommand {
    InjectMessage(Message),
    SteerMessage(Message),
    FollowUpMessage(Message),
    Abort,
    Shutdown,
    SetModel(Model),
}

/// Lightweight, cloneable handle to the agent loop.
///
/// Use to send commands from module callbacks or spawned tasks.
/// `abort()` cancels the current run immediately (including mid-stream).
#[derive(Clone)]
pub struct LoopHandle {
    tx: llm::channel::Sender<LoopCommand>,
    /// Current run's cancel token. Set by the agent loop at the start of each run.
    /// Abort cancels this directly for instant response (no channel latency).
    run_cancel: Rc<Cell<Option<CancelToken>>>,
}

impl LoopHandle {
    pub fn inject(&self, msg: Message) {
        let _ = self.tx.send(LoopCommand::InjectMessage(msg));
    }
    pub fn steer(&self, msg: Message) {
        let _ = self.tx.send(LoopCommand::SteerMessage(msg));
    }
    pub fn follow_up(&self, msg: Message) {
        let _ = self.tx.send(LoopCommand::FollowUpMessage(msg));
    }
    pub fn abort(&self) {
        // Cancel directly for instant effect (even mid-stream).
        if let Some(cancel) = self.run_cancel.take() {
            cancel.cancel();
            self.run_cancel.set(Some(cancel));
        }
        // Also send the command so the loop drains it.
        let _ = self.tx.send(LoopCommand::Abort);
    }
    pub fn shutdown(&self) {
        if let Some(cancel) = self.run_cancel.take() {
            cancel.cancel();
            self.run_cancel.set(Some(cancel));
        }
        let _ = self.tx.send(LoopCommand::Shutdown);
    }
    pub fn set_model(&self, model: Model) {
        let _ = self.tx.send(LoopCommand::SetModel(model));
    }

    /// Set the cancel token for the current run.
    /// Called by the agent loop at the start of each `run()`.
    pub(crate) fn set_run_cancel(&self, cancel: &CancelToken) {
        self.run_cancel.set(Some(cancel.clone()));
    }

    /// Clear the cancel token (run ended).
    pub(crate) fn clear_run_cancel(&self) {
        self.run_cancel.set(None);
    }
}

pub fn loop_handle_pair() -> (LoopHandle, llm::channel::Receiver<LoopCommand>) {
    let (tx, rx) = llm::channel::channel();
    (
        LoopHandle {
            tx,
            run_cancel: Rc::new(Cell::new(None)),
        },
        rx,
    )
}
