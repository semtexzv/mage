use std::future::Future;
use std::pin::Pin;

use crate::cancel::CancelToken;
use crate::channel::Receiver;
use crate::event::AssistantMessageEvent;
use crate::types::{Context, Model, StreamOptions};

/// Error type for provider operations.
#[derive(Debug)]
pub enum ProviderError {
    /// HTTP or network error.
    Http(String),
    /// The request was cancelled.
    Cancelled,
    /// Provider returned an API error.
    Api { status: u16, body: String },
    /// Rate limited with optional retry delay.
    RateLimited { retry_after_ms: Option<u64> },
    /// Other error.
    Other(String),
}

impl std::fmt::Display for ProviderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Http(msg) => write!(f, "HTTP error: {msg}"),
            Self::Cancelled => write!(f, "Request cancelled"),
            Self::Api { status, body } => write!(f, "API error {status}: {body}"),
            Self::RateLimited { retry_after_ms } => {
                write!(f, "Rate limited")?;
                if let Some(ms) = retry_after_ms {
                    write!(f, " (retry after {ms}ms)")?;
                }
                Ok(())
            }
            Self::Other(msg) => write!(f, "{msg}"),
        }
    }
}

impl std::error::Error for ProviderError {}

/// Everything the provider needs to make a streaming LLM request.
pub struct StreamRequest {
    pub model: Model,
    pub context: Context,
    pub options: StreamOptions,
    pub cancel: CancelToken,
}

/// The result of [`Provider::stream`]: a receiver of events plus a future
/// that drives the stream to completion.
pub struct StreamHandle {
    /// Events arrive here. Read until `is_terminal()` or channel closes.
    pub events: Receiver<AssistantMessageEvent>,
    /// The background task producing events. Await or spawn_local this.
    /// Returns `Ok(())` on success, `Err` on failure.
    pub task: Pin<Box<dyn Future<Output = Result<(), ProviderError>>>>,
}

/// LLM streaming provider trait.
///
/// Returns a [`StreamHandle`] — a receiver of events plus a future that
/// drives the HTTP stream. The caller reads events from `handle.events`
/// and awaits/spawns `handle.task`.
///
/// The `stream` method takes an owned [`StreamRequest`] and returns a
/// `'static` future handle so it can be `spawn_local`'d. The provider
/// clones whatever internal state it needs into the future.
///
/// The provider MUST push a terminal event (`Done` or `Error`) before
/// the task completes with `Ok(())`, or return `Err(...)` — in which
/// case the caller handles pushing the error event.
pub trait Provider {
    fn stream(&self, req: StreamRequest) -> StreamHandle;
}
