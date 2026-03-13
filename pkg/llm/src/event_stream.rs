//! High-level event stream that pairs a channel with a final-result future.
//!
//! This is the Rust equivalent of the TypeScript `EventStream<T, R>`.
//! Built on top of [`crate::channel`] so the backing channel is swappable.

use crate::channel::{self, Receiver, Sender};

/// Sending half of an event stream.
pub struct EventStreamSender<T> {
    tx: Sender<T>,
}

/// Receiving half of an event stream.
pub struct EventStreamReceiver<T> {
    rx: Receiver<T>,
}

/// Create a new event stream pair.
pub fn event_stream<T>() -> (EventStreamSender<T>, EventStreamReceiver<T>) {
    let (tx, rx) = channel::channel();
    (EventStreamSender { tx }, EventStreamReceiver { rx })
}

impl<T> EventStreamSender<T> {
    /// Push an event.
    pub fn push(&self, event: T) {
        let _ = self.tx.send(event);
    }
}

impl<T> EventStreamReceiver<T> {
    /// Receive the next event. Returns `None` when the stream is exhausted.
    pub async fn recv(&mut self) -> Option<T> {
        self.rx.recv().await
    }
}
