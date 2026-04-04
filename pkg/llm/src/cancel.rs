/// Cooperative cancellation signal with hierarchical child tokens.
///
/// Wraps [`tokio_util::sync::CancellationToken`] to provide:
/// - `cancel()` / `is_cancelled()` — synchronous signalling
/// - `cancelled()` — async wait (replaces polling loops)
/// - `child_token()` — hierarchical cancellation (child cancelled when parent is)
#[derive(Debug, Clone)]
pub struct CancelToken {
    inner: tokio_util::sync::CancellationToken,
}

impl CancelToken {
    pub fn new() -> Self {
        Self {
            inner: tokio_util::sync::CancellationToken::new(),
        }
    }

    /// Signal cancellation. All clones and child tokens observe this.
    pub fn cancel(&self) {
        self.inner.cancel();
    }

    /// Synchronous check — call at yield points in loops.
    pub fn is_cancelled(&self) -> bool {
        self.inner.is_cancelled()
    }

    /// Async wait — resolves when the token is cancelled.
    /// Use in `tokio::select!` branches.
    pub async fn cancelled(&self) {
        self.inner.cancelled().await
    }

    /// Create a child token. Cancelled when this token is cancelled,
    /// but can also be cancelled independently without affecting the parent.
    pub fn child_token(&self) -> Self {
        Self {
            inner: self.inner.child_token(),
        }
    }
}

impl Default for CancelToken {
    fn default() -> Self {
        Self::new()
    }
}
