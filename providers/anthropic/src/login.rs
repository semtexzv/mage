//! PKCE authorization code login flow for Anthropic (Claude Pro/Max).
//!
//! Uses the OAuth 2.0 Authorization Code Grant with PKCE (RFC 7636):
//!   1. Generate a PKCE code verifier + SHA-256 challenge
//!   2. Build an authorization URL and open the user's browser
//!   3. User authorizes on claude.ai, gets redirected to a callback page
//!   4. User pastes the `code#state` string back into the TUI
//!   5. Exchange the code for OAuth tokens
//!
//! This matches the flow used by Pi Mono / Claude Code CLI.
//! The resulting access tokens work with Bearer auth and CC identity mode.

use std::cell::RefCell;
use std::rc::Rc;

use llm::channel::{self, Sender};
use llm::LoginStep;

use crate::oauth::{OAuthCredentials, CLIENT_ID, TOKEN_URL};

const AUTHORIZE_URL: &str = "https://claude.ai/oauth/authorize";
const REDIRECT_URI: &str = "https://console.anthropic.com/oauth/code/callback";
const SCOPES: &str = "org:create_api_key user:profile user:inference";

/// Start the PKCE authorization code login flow.
///
/// Returns a channel of [`LoginStep`]s. A background task drives the
/// flow and sends steps as they happen. The `Prompt` step waits for
/// the user to paste the authorization code.
pub fn start_login(
    client: reqwest::Client,
    creds: Rc<RefCell<Option<OAuthCredentials>>>,
) -> llm::LoginReceiver {
    let (tx, rx) = channel::channel::<LoginStep>();

    tokio::task::spawn_local(async move {
        if let Err(e) = run_login_flow(&client, &creds, &tx).await {
            let _ = tx.send(LoginStep::Failed(e));
        }
    });

    rx
}

/// Generate a PKCE code verifier (random 32 bytes, base64url-encoded).
fn generate_verifier() -> String {
    use rand::Rng;
    let bytes: [u8; 32] = rand::rng().random();
    base64url_encode(&bytes)
}

/// Compute SHA-256 of the verifier and base64url-encode it.
fn compute_challenge(verifier: &str) -> String {
    use sha2::Digest;
    let hash = sha2::Sha256::digest(verifier.as_bytes());
    base64url_encode(&hash)
}

/// Base64url encoding without padding (RFC 4648 §5).
fn base64url_encode(bytes: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

async fn run_login_flow(
    client: &reqwest::Client,
    creds: &Rc<RefCell<Option<OAuthCredentials>>>,
    tx: &Sender<LoginStep>,
) -> Result<(), String> {
    // 1. Generate PKCE verifier + challenge
    let verifier = generate_verifier();
    let challenge = compute_challenge(&verifier);

    // 2. Build authorization URL
    let auth_params = [
        ("code", "true"),
        ("client_id", CLIENT_ID),
        ("response_type", "code"),
        ("redirect_uri", REDIRECT_URI),
        ("scope", SCOPES),
        ("code_challenge", &challenge),
        ("code_challenge_method", "S256"),
        ("state", &verifier),
    ];
    let query = auth_params
        .iter()
        .map(|(k, v)| format!("{}={}", k, urlencoding::encode(v)))
        .collect::<Vec<_>>()
        .join("&");
    let auth_url = format!("{AUTHORIZE_URL}?{query}");

    // 3. Open browser
    let _ = tx.send(LoginStep::Message(
        "Opening browser for Anthropic login...".into(),
    ));
    let _ = tx.send(LoginStep::OpenUrl(auth_url));

    // 4. Prompt user to paste the code
    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    let _ = tx.send(LoginStep::Prompt {
        message: "Paste the authorization code (code#state):".into(),
        reply: reply_tx,
    });

    let auth_code = reply_rx
        .await
        .map_err(|_| "login cancelled — no code received".to_string())?;

    let auth_code = auth_code.trim().to_string();
    if auth_code.is_empty() {
        return Err("empty authorization code".into());
    }

    // Parse code#state format
    let (code, state) = if let Some((c, s)) = auth_code.split_once('#') {
        (c.to_string(), s.to_string())
    } else {
        // If no # separator, assume the whole thing is the code
        (auth_code, verifier.clone())
    };

    let _ = tx.send(LoginStep::Message("Exchanging code for tokens...".into()));

    // 5. Exchange authorization code for tokens
    let token_body = serde_json::json!({
        "grant_type": "authorization_code",
        "client_id": CLIENT_ID,
        "code": code,
        "state": state,
        "redirect_uri": REDIRECT_URI,
        "code_verifier": verifier,
    });

    let response = client
        .post(TOKEN_URL)
        .header("content-type", "application/json")
        .body(token_body.to_string())
        .send()
        .await
        .map_err(|e| format!("token exchange request failed: {e}"))?;

    if !response.status().is_success() {
        let status = response.status().as_u16();
        let text = response.text().await.unwrap_or_default();
        return Err(format!("token exchange failed ({status}): {text}"));
    }

    #[derive(serde::Deserialize)]
    struct TokenResponse {
        access_token: String,
        refresh_token: String,
        expires_in: u64,
    }

    let token: TokenResponse = response
        .json()
        .await
        .map_err(|e| format!("token response parse failed: {e}"))?;

    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;
    // 5-minute buffer before actual expiry, matching Pi Mono
    let expires_at_ms = now_ms + token.expires_in * 1000 - 5 * 60 * 1000;

    let new_creds = OAuthCredentials {
        refresh_token: token.refresh_token,
        access_token: token.access_token,
        expires_at_ms,
    };

    *creds.borrow_mut() = Some(new_creds);
    let _ = tx.send(LoginStep::Done);
    Ok(())
}
