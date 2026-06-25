//! Anthropic Messages API streaming provider.

use llm::{
    AssistantMessageEvent, Provider, ProviderError,
    StreamHandle, StreamRequest,
    channel::{self, Sender},
};

use crate::convert::build_request_body;
use crate::events::EventMapper;
use crate::sse::{SseEvent, SseParser};

/// `anthropic-beta` features requested on every call.
const BETA_FLAGS: &str =
    "fine-grained-tool-streaming-2025-05-14,interleaved-thinking-2025-05-14";

/// Process SSE events through the mapper and send to tx.
/// Returns true if a terminal event was sent.
fn dispatch_sse_events(
    sse_events: impl IntoIterator<Item = SseEvent>,
    mapper: &mut EventMapper,
    tx: &Sender<AssistantMessageEvent>,
) -> bool {
    for sse_event in sse_events {
        let event_type = sse_event.event.as_deref().unwrap_or("");
        for event in mapper.map_event(event_type, &sse_event.data) {
            let terminal = event.is_terminal();
            let _ = tx.send(event);
            if terminal {
                return true;
            }
        }
    }
    false
}

/// Anthropic Messages API provider.
///
/// Implements `llm::Provider`. Streams responses via SSE from the Anthropic
/// `/v1/messages` endpoint. Authenticated with an Anthropic API key
/// (`sk-ant-…`), taken from [`with_api_key`](AnthropicProvider::with_api_key),
/// the per-request `StreamOptions.api_key`, or the `ANTHROPIC_API_KEY`
/// environment variable.
///
/// The entire runtime is single-threaded (`spawn_local`); no `Send` bounds
/// are needed.
pub struct AnthropicProvider {
    /// HTTP client (reused across requests for connection pooling).
    client: reqwest::Client,
    /// API key. Falls back to `StreamOptions.api_key`, then `ANTHROPIC_API_KEY`.
    api_key: Option<String>,
}

impl AnthropicProvider {
    pub fn new() -> Self {
        Self {
            client: reqwest::Client::new(),
            api_key: None,
        }
    }

    /// Set the API key (`sk-ant-…`).
    pub fn with_api_key(mut self, key: impl Into<String>) -> Self {
        self.api_key = Some(key.into());
        self
    }

    /// Set a custom `reqwest::Client` (e.g. with proxy or timeout config).
    pub fn with_client(mut self, client: reqwest::Client) -> Self {
        self.client = client;
        self
    }
}

impl Provider for AnthropicProvider {
    fn stream(&self, req: StreamRequest) -> StreamHandle {
        let client = self.client.clone();
        let default_api_key = self.api_key.clone();
        let (tx, rx) = channel::channel();

        let model = req.model;
        let context = req.context;
        let options = req.options;
        let cancel = req.cancel;

        let task = Box::pin(async move {
            let api_key = resolve_api_key(&options, default_api_key.as_deref())?;

            let base_url = if model.base_url.is_empty() {
                "https://api.anthropic.com"
            } else {
                &model.base_url
            };
            let url = format!("{}/v1/messages", base_url.trim_end_matches('/'));

            let body = build_request_body(&model, &context, &options);

            let mut request = client.post(&url)
                .header("content-type", "application/json")
                .header("anthropic-version", "2023-06-01")
                .header("accept", "text/event-stream")
                .header("x-api-key", &api_key)
                .header("anthropic-beta", BETA_FLAGS)
                .body(serde_json::to_string(&body).map_err(|e| ProviderError::Other(e.to_string()))?);

            // Add custom headers from model and options (last, so they can override)
            if let Some(headers) = &model.headers {
                for (k, v) in headers {
                    request = request.header(k.as_ref(), v.as_ref());
                }
            }
            if let Some(headers) = &options.headers {
                for (k, v) in headers {
                    request = request.header(k.as_ref(), v.as_ref());
                }
            }

            let response = request.send().await.map_err(|e| {
                ProviderError::Http(e.to_string())
            })?;

            let status = response.status();
            if !status.is_success() {
                let body_text = response.text().await.unwrap_or_default();
                if status.as_u16() == 429 {
                    return Err(ProviderError::RateLimited { retry_after_ms: None });
                }
                return Err(ProviderError::Api {
                    status: status.as_u16(),
                    body: body_text,
                });
            }

            // Stream the SSE response
            let mut sse_parser = SseParser::new();
            let mut event_mapper = EventMapper::new();
            let mut byte_stream = response.bytes_stream();

            use futures_util::StreamExt;

            while let Some(chunk_result) = byte_stream.next().await {
                if cancel.is_cancelled() {
                    let _ = tx.send(AssistantMessageEvent::Error {
                        reason: llm::StopReason::Aborted,
                        error: Some("cancelled".into()),
                    });
                    return Err(ProviderError::Cancelled);
                }

                let chunk = chunk_result.map_err(|e| ProviderError::Http(e.to_string()))?;
                if dispatch_sse_events(sse_parser.feed(&chunk), &mut event_mapper, &tx) {
                    return Ok(());
                }
            }

            // Stream ended without a terminal event — handle gracefully
            if let Some(final_event) = sse_parser.finish() {
                if dispatch_sse_events(std::iter::once(final_event), &mut event_mapper, &tx) {
                    return Ok(());
                }
            }

            // No terminal event was sent — emit Done as fallback
            let _ = tx.send(AssistantMessageEvent::Done {
                reason: llm::StopReason::Stop,
            });

            Ok(())
        });

        StreamHandle { events: rx, task }
    }

    fn models(&self) -> Vec<llm::Model> {
        crate::models::anthropic_models()
    }
}

impl Default for AnthropicProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl llm::Authenticator for AnthropicProvider {
    fn auth_status(&self) -> llm::AuthStatus {
        if self.api_key.is_some() || std::env::var("ANTHROPIC_API_KEY").is_ok() {
            llm::AuthStatus::Authenticated
        } else {
            llm::AuthStatus::NotAuthenticated {
                message: "Set ANTHROPIC_API_KEY to authenticate with Anthropic.".into(),
            }
        }
    }

    fn login(&self) -> llm::LoginReceiver {
        // API-key authentication only — there is no interactive login flow.
        let (tx, rx) = channel::channel();
        let _ = tx.send(llm::LoginStep::Failed(
            "Interactive login is not available. Set ANTHROPIC_API_KEY.".into(),
        ));
        rx
    }
}

// ---------------------------------------------------------------------------
// API key resolution
// ---------------------------------------------------------------------------

/// Resolve the API key for a request.
///
/// Priority: per-request `StreamOptions.api_key` > the key set via
/// `with_api_key` > the `ANTHROPIC_API_KEY` environment variable.
fn resolve_api_key(
    options: &llm::StreamOptions,
    default_api_key: Option<&str>,
) -> Result<String, ProviderError> {
    if let Some(key) = options.api_key.as_ref() {
        return Ok(key.to_string());
    }
    if let Some(key) = default_api_key {
        return Ok(key.to_string());
    }
    if let Ok(key) = std::env::var("ANTHROPIC_API_KEY") {
        return Ok(key);
    }
    Err(ProviderError::Other(
        "no API key: set ANTHROPIC_API_KEY, pass StreamOptions.api_key, or use with_api_key()".into(),
    ))
}
