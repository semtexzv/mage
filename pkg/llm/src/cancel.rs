use std::cell::Cell;
use std::rc::Rc;

/// Cooperative cancellation signal. 8 bytes, no atomics.
///
/// Equivalent to JS `AbortController` / `AbortSignal`.
/// Check at poll points in streams and loops.
#[derive(Debug, Clone)]
pub struct CancelToken {
    cancelled: Rc<Cell<bool>>,
}

impl CancelToken {
    pub fn new() -> Self {
        Self {
            cancelled: Rc::new(Cell::new(false)),
        }
    }

    pub fn cancel(&self) {
        self.cancelled.set(true);
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancelled.get()
    }
}

impl Default for CancelToken {
    fn default() -> Self {
        Self::new()
    }
}
