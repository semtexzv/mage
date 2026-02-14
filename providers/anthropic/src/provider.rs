//! Anthropic Messages API streaming provider.

use llm::{
    AssistantMessageEvent, Provider, ProviderError,
    StreamHandle, StreamRequest,
    channel::{self, Sender},
};

use crate::convert::build_request_body;
use crate::events::EventMapper;
use crate::oauth;
use crate::sse::{SseEvent, SseParser};

/// Process SSE events through the mapper and send to tx.
/// Returns true if a terminal event was sent.
fn dispatch_sse_events(
    sse_events: impl IntoIterator<Item = SseEvent>,
    mapper: &mut EventMapper,
    tx: &Sender<AssistantMessageEvent>,
    tool_names: Option<&[String]>,
) -> bool {
    for sse_event in sse_events {
        let event_type = sse_event.event.as_deref().unwrap_or("");
        for mut event in mapper.map_event(event_type, &sse_event.data) {
            // Reverse-map CC tool names back to the caller's original names
            if let Some(names) = tool_names {
                if let AssistantMessageEvent::ToolCallStart { ref mut name, .. } = event {
                    *name = oauth::from_cc_tool_name(name, names).into();
                }
            }
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
/// Implements `llm::Provider`. Streams responses via SSE from the
/// Anthropic `/v1/messages` endpoint.
///
/// When the API key is an OAuth token (`sk-ant-oat-…`), the provider
/// automatically switches to Claude Code identity mode:
/// - Bearer auth instead of `x-api-key`
/// - Claude Code `user-agent`, `x-app`, and beta headers
/// - CC system prompt prepended
/// - Tool names remapped to CC canonical casing (and back on response)
///
/// This is transparent to the caller — the only visible difference is
/// that `StreamOptions.api_key` must contain a valid OAuth access token.
/// Use [`oauth::refresh_token`] to keep it fresh.
pub struct AnthropicProvider {
    /// HTTP client (reused across requests for connection pooling).
    client: reqwest::Client,
    /// Default API key (or OAuth token). Can be overridden per-request via `StreamOptions.api_key`.
    api_key: Option<String>,
}

impl AnthropicProvider {
    pub fn new() -> Self {
        Self {
            client: reqwest::Client::new(),
            api_key: None,
        }
    }

    pub fn with_api_key(mut self, key: impl Into<String>) -> Self {
        self.api_key = Some(key.into());
        self
    }

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
            let api_key = options.api_key.as_ref()
                .map(|k| k.to_string())
                .or(default_api_key)
                .ok_or_else(|| ProviderError::Other(
                    "no API key: set via StreamOptions.api_key or AnthropicProvider::with_api_key".into()
                ))?;

            let is_oauth = oauth::is_oauth_token(&api_key);

            let base_url = if model.base_url.is_empty() {
                "https://api.anthropic.com"
            } else {
                &model.base_url
            };
            let url = format!("{}/v1/messages", base_url.trim_end_matches('/'));

            let body = build_request_body(&model, &context, &options, is_oauth);

            let mut request = client.post(&url)
                .header("content-type", "application/json")
                .header("anthropic-version", "2023-06-01");

            // Auth and identity headers depend on token type
            if is_oauth {
                request = request
                    .header("accept", "application/json")
                    .header("authorization", format!("Bearer {}", api_key))
                    .header("anthropic-beta", oauth::OAUTH_BETA_FLAGS)
                    .header("user-agent", format!("claude-cli/{} (external, cli)", oauth::CLAUDE_CODE_VERSION))
                    .header("x-app", "cli")
                    .header("anthropic-dangerous-direct-browser-access", "true");
            } else {
                request = request
                    .header("accept", "text/event-stream")
                    .header("x-api-key", &api_key)
                    .header("anthropic-beta", oauth::API_KEY_BETA_FLAGS);
            }

            request = request
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

            // Collect original tool names for reverse-mapping in OAuth mode
            let tool_names: Option<Vec<String>> = if is_oauth {
                context.tools.as_ref().map(|tools| {
                    tools.iter().map(|t| t.name.to_string()).collect()
                })
            } else {
                None
            };

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
                if dispatch_sse_events(sse_parser.feed(&chunk), &mut event_mapper, &tx, tool_names.as_deref()) {
                    return Ok(());
                }
            }

            // Stream ended without a terminal event — handle gracefully
            if let Some(final_event) = sse_parser.finish() {
                if dispatch_sse_events(std::iter::once(final_event), &mut event_mapper, &tx, tool_names.as_deref()) {
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
}

impl Default for AnthropicProvider {
    fn default() -> Self {
        Self::new()
    }
}
