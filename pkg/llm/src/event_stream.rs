//! High-level event stream that pairs a channel with a final-result future.
//!
//! This is the Rust equivalent of the TypeScript `EventStream<T, R>`.
//! Built on top of [`crate::channel`] so the backing channel is swappable.

use std::cell::RefCell;
use std::rc::Rc;

use crate::channel::{self, Receiver, Sender};

/// A push-based async event stream with a final result.
///
/// The producer pushes events via the [`EventStreamSender`].
/// The consumer reads events via [`EventStreamReceiver`] and awaits the result.
pub struct EventStreamReceiver<T, R> {
    rx: Receiver<T>,
    result: Rc<RefCell<Option<R>>>,
}

pub struct EventStreamSender<T, R> {
    tx: Sender<T>,
    result: Rc<RefCell<Option<R>>>,
    is_complete: fn(&T) -> bool,
    extract_result: fn(&T) -> R,
}

/// Create a new event stream pair.
pub fn event_stream<T, R>(
    is_complete: fn(&T) -> bool,
    extract_result: fn(&T) -> R,
) -> (EventStreamSender<T, R>, EventStreamReceiver<T, R>) {
    let (tx, rx) = channel::channel();
    let result = Rc::new(RefCell::new(None));
    (
        EventStreamSender {
            tx,
            result: result.clone(),
            is_complete,
            extract_result,
        },
        EventStreamReceiver { rx, result },
    )
}

impl<T, R> EventStreamSender<T, R> {
    /// Push an event. If the event is terminal, the result is extracted and stored.
    pub fn push(&self, event: T) {
        if (self.is_complete)(&event) {
            *self.result.borrow_mut() = Some((self.extract_result)(&event));
        }
        let _ = self.tx.send(event);
    }

    /// End the stream with an explicit result (no terminal event).
    pub fn end(self, result: R) {
        *self.result.borrow_mut() = Some(result);
        // Sender drops here, closing the channel.
    }
}

impl<T, R: Clone> EventStreamReceiver<T, R> {
    /// Receive the next event. Returns `None` when the stream is exhausted.
    pub async fn recv(&mut self) -> Option<T> {
        self.rx.recv().await
    }

    /// Get the final result. Returns `None` if no terminal event was received yet.
    pub fn result(&self) -> Option<R> {
        self.result.borrow().clone()
    }
}
