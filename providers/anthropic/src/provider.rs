//! Anthropic Messages API streaming provider.

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

use llm::{
    AssistantMessageEvent, Provider, ProviderError,
    StreamHandle, StreamRequest,
    channel::{self, Sender},
};

use crate::convert::build_request_body;
use crate::events::EventMapper;
use crate::oauth::{self, OAuthCredentials};
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
/// ## OAuth auto-refresh
///
/// When constructed with [`with_oauth`], the provider holds shared
/// [`OAuthCredentials`] in an `Rc<RefCell<>>`.  Before each request, if
/// the access token is expired the provider calls
/// [`oauth::refresh_token`] and updates the shared credentials
/// transparently.  The caller can read the updated credentials via
/// [`oauth_credentials`] for persistence.
///
/// This works because the entire runtime is single-threaded (`spawn_local`);
/// no `Send` bounds are needed.
pub struct AnthropicProvider {
    /// HTTP client (reused across requests for connection pooling).
    client: reqwest::Client,
    /// Default API key (plain `sk-ant-…`). Used when OAuth credentials
    /// are absent and `StreamOptions.api_key` is not set.
    api_key: Option<String>,
    /// Shared OAuth credential slot. Starts `None`; populated by
    /// `with_oauth()` or by the login flow. Auto-refreshed before
    /// each request when present and expired.
    oauth: Rc<RefCell<Option<OAuthCredentials>>>,
    /// Optional path to a JSON file containing OAuth credentials.
    /// Re-read on every request so external tools (Pi, etc.) can
    /// keep the tokens fresh on disk.
    credential_file: Option<PathBuf>,
}

impl AnthropicProvider {
    pub fn new() -> Self {
        Self {
            client: reqwest::Client::new(),
            api_key: None,
            oauth: Rc::new(RefCell::new(None)),
            credential_file: None,
        }
    }

    /// Set a JSON credential file to re-read on every request.
    ///
    /// The file should contain an object with an `"anthropic"` key:
    /// ```json
    /// { "anthropic": { "type": "oauth", "refresh": "...", "access": "...", "expires": 123 } }
    /// ```
    /// Compatible with Pi's `~/.pi/agent/auth.json`.
    pub fn with_credential_file(mut self, path: impl Into<PathBuf>) -> Self {
        self.credential_file = Some(path.into());
        self
    }

    /// Set a plain API key (`sk-ant-api01-…`).
    pub fn with_api_key(mut self, key: impl Into<String>) -> Self {
        self.api_key = Some(key.into());
        self
    }

    /// Set OAuth credentials for auto-refresh.
    ///
    /// The provider stores them in `Rc<RefCell<>>` and refreshes the
    /// access token automatically when it expires.  Call
    /// [`oauth_credentials`] after requests to read the (potentially
    /// updated) credentials for persistence.
    pub fn with_oauth(mut self, creds: OAuthCredentials) -> Self {
        self.oauth = Rc::new(RefCell::new(Some(creds)));
        self
    }

    /// Set a pre-shared credential slot. The login flow and provider
    /// share this `Rc<RefCell<Option<OAuthCredentials>>>`.
    pub fn with_oauth_shared(mut self, slot: Rc<RefCell<Option<OAuthCredentials>>>) -> Self {
        self.oauth = slot;
        self
    }

    /// Access the shared OAuth credential slot.
    ///
    /// After login or token refresh the inner value may have changed.
    /// Clone the `Rc` to read or persist credentials.
    pub fn oauth_slot(&self) -> &Rc<RefCell<Option<OAuthCredentials>>> {
        &self.oauth
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
        let oauth_creds = self.oauth.clone();
        let cred_file = self.credential_file.clone();
        let (tx, rx) = channel::channel();

        let model = req.model;
        let context = req.context;
        let options = req.options;
        let cancel = req.cancel;

        let task = Box::pin(async move {
            let api_key = resolve_api_key(
                &client,
                &options,
                default_api_key.as_deref(),
                &oauth_creds,
                cred_file.as_deref(),
            )
            .await?;

            let is_oauth = oauth::is_oauth_token(&api_key);

            let base_url = if model.base_url.is_empty() {
                "https://api.anthropic.com"
            } else {
                &model.base_url
            };
            let url = format!("{}/v1/messages", base_url.trim_end_matches('/'));

            let body = build_request_body(&model, &context, &options, is_oauth);

            // Debug: log request body to ~/.mage/request.log
            if let Some(home) = dirs::home_dir() {
                use std::io::Write;
                if let Ok(mut f) = std::fs::OpenOptions::new()
                    .create(true).append(true)
                    .open(home.join(".mage/request.log"))
                {
                    let _ = writeln!(f, "=== REQUEST ===");
                    if let Ok(json) = serde_json::to_string_pretty(&body) {
                        let _ = writeln!(f, "{json}");
                    }
                    let _ = writeln!(f);
                }
            }

            let mut request = client.post(&url)
                .header("content-type", "application/json")
                .header("anthropic-version", "2023-06-01");

            // Auth and identity headers depend on token type
            if is_oauth {
                request = request
                    .header("accept", "text/event-stream")
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
        // Check OAuth slot first.
        if let Some(creds) = self.oauth.borrow().as_ref() {
            if creds.is_expired() {
                return llm::AuthStatus::RefreshRequired;
            }
            return llm::AuthStatus::Authenticated;
        }
        // Fall back to plain API key.
        if self.api_key.is_some() {
            return llm::AuthStatus::Authenticated;
        }
        llm::AuthStatus::NotAuthenticated {
            message: "Not logged in. Run /login to authenticate with Anthropic.".into(),
        }
    }

    fn login(&self) -> llm::LoginReceiver {
        crate::login::start_login(self.client.clone(), self.oauth.clone())
    }
}

// ---------------------------------------------------------------------------
// API key resolution with auto-refresh
// ---------------------------------------------------------------------------

/// Resolve the API key to use for a request.
///
/// Priority:
/// 1. `StreamOptions.api_key` (per-request override)
/// 2. OAuth credentials (auto-refreshed if expired)
/// 3. `default_api_key` (plain API key from provider construction)
///
/// Returns the access/API key string, or an error.
async fn resolve_api_key(
    client: &reqwest::Client,
    options: &llm::StreamOptions,
    default_api_key: Option<&str>,
    oauth_slot: &Rc<RefCell<Option<OAuthCredentials>>>,
    credential_file: Option<&std::path::Path>,
) -> Result<String, ProviderError> {
    // Per-request override wins.
    if let Some(key) = options.api_key.as_ref() {
        return Ok(key.to_string());
    }

    // Re-read credential file if configured (Pi compatibility).
    if let Some(path) = credential_file {
        if let Some(creds) = read_credential_file(path) {
            *oauth_slot.borrow_mut() = Some(creds);
        }
    }

    // OAuth credentials with auto-refresh.
    {
        let slot = oauth_slot.borrow();
        if let Some(creds) = slot.as_ref() {
            if creds.is_expired() {
                let refresh_tok = creds.refresh_token.clone();
                drop(slot); // release borrow before await
                let new_creds = oauth::refresh_token(client, &refresh_tok)
                    .await
                    .map_err(|e| ProviderError::Other(format!("OAuth refresh failed: {e}")))?;
                *oauth_slot.borrow_mut() = Some(new_creds);
            } else {
                return Ok(creds.access_token.clone());
            }
            // Re-borrow after refresh
            let slot = oauth_slot.borrow();
            if let Some(creds) = slot.as_ref() {
                return Ok(creds.access_token.clone());
            }
        }
    }
    // Plain API key.
    default_api_key
        .map(|k| k.to_string())
        .ok_or_else(|| ProviderError::Other(
            "no API key: set via StreamOptions.api_key, with_oauth(), or with_api_key()\nRun /login to authenticate with your Anthropic account.".into(),
        ))
}

/// Read OAuth credentials from a Pi-compatible auth.json file.
///
/// Expected format:
/// ```json
/// { "anthropic": { "type": "oauth", "refresh": "...", "access": "...", "expires": 123 } }
/// ```
fn read_credential_file(path: &std::path::Path) -> Option<OAuthCredentials> {
    let content = std::fs::read_to_string(path).ok()?;
    let json: serde_json::Value = serde_json::from_str(&content).ok()?;
    let entry = json.get("anthropic")?;
    let refresh = entry.get("refresh")?.as_str()?;
    let access = entry.get("access")?.as_str().unwrap_or("");
    let expires = entry.get("expires")?.as_u64().unwrap_or(0);
    Some(OAuthCredentials {
        refresh_token: refresh.to_string(),
        access_token: access.to_string(),
        expires_at_ms: expires,
    })
}
