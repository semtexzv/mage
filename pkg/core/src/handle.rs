//! Loop handle — the channel-based command interface into the agent loop.
//!
//! [`LoopHandle`] is cheap to clone and safe to use from spawned tasks.
//! All methods are fire-and-forget (silently drop if the loop has shut down).

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
#[derive(Clone)]
pub struct LoopHandle {
    tx: llm::channel::Sender<LoopCommand>,
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
        let _ = self.tx.send(LoopCommand::Abort);
    }
    pub fn shutdown(&self) {
        let _ = self.tx.send(LoopCommand::Shutdown);
    }
    pub fn set_model(&self, model: Model) {
        let _ = self.tx.send(LoopCommand::SetModel(model));
    }
}

pub fn loop_handle_pair() -> (LoopHandle, llm::channel::Receiver<LoopCommand>) {
    let (tx, rx) = llm::channel::channel();
    (LoopHandle { tx }, rx)
}
