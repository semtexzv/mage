//! OAuth support for Claude Pro/Max subscriptions.
//!
//! The PKCE login flow (browser redirect, code exchange) belongs in the
//! CLI/UI layer. This module provides the infrastructure the provider needs:
//! token detection, token refresh, and Claude Code identity constants.

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

const OAUTH_TOKEN_PREFIX: &str = "sk-ant-oat";
pub(crate) const TOKEN_URL: &str = "https://console.anthropic.com/v1/oauth/token";
pub(crate) const CLIENT_ID: &str = "9d1c250a-e61b-44d9-88ed-5944d1962f5e";

/// Version string sent in `user-agent` when mimicking Claude Code.
pub const CLAUDE_CODE_VERSION: &str = "2.1.2";

/// System prompt prepended in OAuth mode. Must be the first system block.
pub const CLAUDE_CODE_SYSTEM_PROMPT: &str =
    "You are Claude Code, Anthropic's official CLI for Claude.";

/// `anthropic-beta` value for OAuth mode.
pub const OAUTH_BETA_FLAGS: &str =
    "claude-code-20250219,oauth-2025-04-20,fine-grained-tool-streaming-2025-05-14,interleaved-thinking-2025-05-14";

/// `anthropic-beta` value for regular API key mode.
pub const API_KEY_BETA_FLAGS: &str =
    "fine-grained-tool-streaming-2025-05-14,interleaved-thinking-2025-05-14";

// ---------------------------------------------------------------------------
// Tool name mapping
// ---------------------------------------------------------------------------

/// Claude Code 2.x canonical tool names.
/// Source: <https://cchistory.mariozechner.at/data/prompts-2.1.11.md>
const CC_TOOLS: &[&str] = &[
    "Read",
    "Write",
    "Edit",
    "Bash",
    "Grep",
    "Glob",
    "AskUserQuestion",
    "EnterPlanMode",
    "ExitPlanMode",
    "KillShell",
    "NotebookEdit",
    "Skill",
    "Task",
    "TaskOutput",
    "TodoWrite",
    "WebFetch",
    "WebSearch",
];

/// Map a tool name to Claude Code canonical casing (case-insensitive).
/// Returns the original name unchanged if no CC tool matches.
pub fn to_cc_tool_name(name: &str) -> &str {
    let lower = name.to_ascii_lowercase();
    for cc in CC_TOOLS {
        if cc.to_ascii_lowercase() == lower {
            return cc;
        }
    }
    name
}

/// Reverse-map a Claude Code tool name back to the caller's original name.
/// Matches case-insensitively against `original_names`. Returns the CC name
/// as-is if no original matches (shouldn't happen in practice).
pub fn from_cc_tool_name<'a>(cc_name: &'a str, original_names: &'a [String]) -> &'a str {
    let lower = cc_name.to_ascii_lowercase();
    for orig in original_names {
        if orig.to_ascii_lowercase() == lower {
            return orig;
        }
    }
    cc_name
}

// ---------------------------------------------------------------------------
// Token detection
// ---------------------------------------------------------------------------

/// Returns `true` if the key is an OAuth access token (Claude Pro/Max subscription).
pub fn is_oauth_token(api_key: &str) -> bool {
    api_key.starts_with(OAUTH_TOKEN_PREFIX)
}

// ---------------------------------------------------------------------------
// Credentials & refresh
// ---------------------------------------------------------------------------

/// Stored OAuth credentials. Persist these across sessions.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OAuthCredentials {
    /// Refresh token — used to obtain new access tokens.
    pub refresh_token: String,
    /// Current access token — sent as Bearer token in API requests.
    pub access_token: String,
    /// Expiry timestamp in milliseconds since epoch (with 5-minute buffer).
    pub expires_at_ms: u64,
}

impl OAuthCredentials {
    /// Whether the access token has expired.
    pub fn is_expired(&self) -> bool {
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        now_ms >= self.expires_at_ms
    }
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    refresh_token: String,
    expires_in: u64,
}

/// Refresh an expired OAuth token. Returns updated credentials to persist.
///
/// The caller owns credential storage and must update `StreamOptions.api_key`
/// with the new access token before calling `Provider::stream`.
pub async fn refresh_token(
    client: &reqwest::Client,
    refresh_token: &str,
) -> Result<OAuthCredentials, String> {
    let body = serde_json::json!({
        "grant_type": "refresh_token",
        "client_id": CLIENT_ID,
        "refresh_token": refresh_token,
    });

    let response = client
        .post(TOKEN_URL)
        .header("content-type", "application/json")
        .body(body.to_string())
        .send()
        .await
        .map_err(|e| format!("token refresh request failed: {e}"))?;

    if !response.status().is_success() {
        let status = response.status().as_u16();
        let text = response.text().await.unwrap_or_default();
        return Err(format!("token refresh failed ({status}): {text}"));
    }

    let data: TokenResponse = response
        .json()
        .await
        .map_err(|e| format!("token refresh parse failed: {e}"))?;

    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;
    // 5-minute buffer before actual expiry, matching pi-mono
    let expires_at_ms = now_ms + data.expires_in * 1000 - 5 * 60 * 1000;

    Ok(OAuthCredentials {
        refresh_token: data.refresh_token,
        access_token: data.access_token,
        expires_at_ms,
    })
}
