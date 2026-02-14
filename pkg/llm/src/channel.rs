//! Channel abstraction for event streaming.
//!
//! Wraps the concrete channel implementation so consumers don't couple to it.
//! Currently uses `tokio::sync::mpsc::unbounded_channel`. Swap the body of
//! [`channel`] to change the backing implementation (e.g., crossbeam, flume,
//! custom VecDeque+Waker) without touching any call sites.

use tokio::sync::mpsc;

/// Sending half of an event channel.
pub struct Sender<T> {
    inner: mpsc::UnboundedSender<T>,
}

/// Receiving half of an event channel.
pub struct Receiver<T> {
    inner: mpsc::UnboundedReceiver<T>,
}

/// Create a new unbounded channel pair.
pub fn channel<T>() -> (Sender<T>, Receiver<T>) {
    let (tx, rx) = mpsc::unbounded_channel();
    (Sender { inner: tx }, Receiver { inner: rx })
}

impl<T> Sender<T> {
    /// Send a value. Returns `Err(value)` if the receiver has been dropped.
    pub fn send(&self, value: T) -> Result<(), T> {
        self.inner.send(value).map_err(|e| e.0)
    }

    /// Returns `true` if the receiver has been dropped.
    pub fn is_closed(&self) -> bool {
        self.inner.is_closed()
    }
}

impl<T> Receiver<T> {
    /// Receive the next value. Returns `None` when the sender is dropped
    /// and the channel is empty.
    pub async fn recv(&mut self) -> Option<T> {
        self.inner.recv().await
    }

    /// Non-blocking try_recv.
    pub fn try_recv(&mut self) -> Option<T> {
        self.inner.try_recv().ok()
    }
}

impl<T> Clone for Sender<T> {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
        }
    }
}
