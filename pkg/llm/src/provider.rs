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
    /// List available models for this provider.
    ///
    /// Returns an empty vec by default. Providers that know their model
    /// catalog override this.
    fn models(&self) -> Vec<crate::types::Model> {
        Vec::new()
    }
}

// ---------------------------------------------------------------------------
// Authentication
// ---------------------------------------------------------------------------

/// Whether the provider is ready to make API calls.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthStatus {
    /// Authenticated and ready.
    Authenticated,
    /// Has credentials that may be expired but can auto-refresh.
    RefreshRequired,
    /// Not authenticated — interactive login required.
    NotAuthenticated { message: String },
}

/// A step in an interactive login flow.
///
/// The app layer decides how to present these to the user (open browser,
/// print URL, show message). The authenticator drives the flow; the app
/// renders it.
pub enum LoginStep {
    /// Display a message to the user (e.g. "Opening browser...").
    Message(String),
    /// Open this URL in the user's browser.
    OpenUrl(String),
    /// Prompt the user for text input (e.g. paste authorization code).
    /// The login flow blocks until the reply is sent.
    Prompt {
        message: String,
        reply: tokio::sync::oneshot::Sender<String>,
    },
    /// Login completed successfully. The provider is now authenticated.
    Done,
    /// Login failed.
    Failed(String),
}

/// Receiver for login flow steps.  The app reads steps and acts on them.
pub type LoginReceiver = crate::channel::Receiver<LoginStep>;

/// Optional companion to [`Provider`]. Providers that require interactive
/// authentication (OAuth, device code, etc.) implement this.
///
/// The login flow is asynchronous and non-blocking:
///   1. App calls `login()` which returns a `LoginReceiver`.
///   2. A background task drives the flow (PKCE, polling, etc.) and
///      sends `LoginStep` values through the channel.
///   3. The app reads steps and acts: opens browser, shows messages.
///   4. `LoginStep::Done` signals success — the provider is now
///      authenticated and subsequent `stream()` calls will work.
///
/// This design keeps the provider in control of the auth protocol
/// while the app controls presentation. No `Send` bounds — everything
/// runs on the local task set.
pub trait Authenticator {
    /// Check whether the provider has valid credentials.
    fn auth_status(&self) -> AuthStatus;

    /// Start an interactive login flow.
    ///
    /// Returns a channel that emits [`LoginStep`]s. The background task
    /// is spawned via `tokio::task::spawn_local` inside this method.
    fn login(&self) -> LoginReceiver;
}