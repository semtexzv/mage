//! Default application entry point.
//!
//! Wires up the Anthropic provider, credentials, model defaults, agent loop,
//! and TUI. This is the function that the generated `main.rs` calls.

use std::cell::RefCell;
use std::rc::Rc;

use refstr::Str;

use llm::Provider as _;

use mage_core::module::Module;

use crate::app::App;

/// Run the full mage application with the given modules (interactive TUI).
pub async fn run_default(modules: Vec<Rc<dyn Module>>) {
    // Register exit hook so process::exit() (e.g., from Recompile) restores terminal.
    mage_core::upgrade::set_exit_hook(mage_tui::restore_terminal);
    let (provider, authenticator, available_models, default_model) = setup_provider();

    let model_name = default_model.name.to_string();

    let providers: Vec<(Str, Rc<dyn llm::Provider>)> =
        vec![("anthropic".into(), provider.clone() as Rc<dyn llm::Provider>)];

    let (agent_loop, event_rx) = mage_core::agent_loop::AgentLoop::new(
        system_prompt(),
        default_model,
        llm::StreamOptions::default(),
        providers,
        modules,
    );

    let handle = mage_core::session::spawn(agent_loop);
    let app = App::new(handle);

    let on_cred_save = make_cred_save_callback(&provider);

    crate::tui::run(
        app,
        event_rx,
        Some(authenticator),
        on_cred_save,
        available_models,
        model_name,
    )
    .await;
}

/// Run in non-interactive mode: send a single prompt, print output, exit.
pub async fn run_print(modules: Vec<Rc<dyn Module>>, prompt: String) {
    let (provider, _authenticator, _available_models, default_model) = setup_provider();

    let providers: Vec<(Str, Rc<dyn llm::Provider>)> =
        vec![("anthropic".into(), provider as Rc<dyn llm::Provider>)];

    let (mut agent_loop, mut event_rx) = mage_core::agent_loop::AgentLoop::new(
        system_prompt(),
        default_model,
        llm::StreamOptions::default(),
        providers,
        modules,
    );

    // Spawn a task to print streaming events to stdout.
    tokio::task::spawn_local({
        async move {
            while let Some(ev) = event_rx.recv().await {
                match ev {
                    mage_core::types::AgentEvent::MessageDelta { event } => {
                        match &event {
                            llm::AssistantMessageEvent::TextDelta { delta, .. } => {
                                print!("{delta}");
                            }
                            _ => {}
                        }
                    }
                    mage_core::types::AgentEvent::ToolExecStart { tool_name, args, .. } => {
                        let summary = summarize_args(&args);
                        eprintln!("\x1b[2m> {tool_name} {summary}\x1b[0m");
                    }
                    mage_core::types::AgentEvent::ToolExecEnd { tool_name, result, .. } => {
                        if result.is_error {
                            eprintln!("\x1b[31m> {tool_name} failed\x1b[0m");
                        }
                    }
                    mage_core::types::AgentEvent::AgentEnd { .. } => break,
                    mage_core::types::AgentEvent::AgentError { message } => {
                        eprintln!("\x1b[31merror: {message}\x1b[0m");
                        break;
                    }
                    _ => {}
                }
            }
        }
    });

    let msg = mage_core::types::Message::user_text(prompt);
    if let Err(e) = agent_loop.run(msg).await {
        eprintln!("\x1b[31merror: {e}\x1b[0m");
    }
    println!();
}

// ---------------------------------------------------------------------------
// Internals
// ---------------------------------------------------------------------------

/// Unified entry point. Checks for `-p` flag to decide mode.
///
/// - No args: interactive TUI
/// - `-p "prompt"`: non-interactive, print output to stdout
/// Unified entry point. Handles CLI flags and monitor wrapping.
///
/// - `bootstrap` subcommand: recompile with extensions from modroots
/// - `-p "prompt"`: non-interactive print mode
/// - No args: interactive TUI (wrapped in monitor for self-upgrade)
pub async fn run(modules: Vec<Rc<dyn Module>>) {
    let args: Vec<String> = std::env::args().skip(1).collect();

    // Handle subcommands first (before monitor wrapping).
    if let Some(cmd) = args.first().map(|s| s.as_str()) {
        match cmd {
            "rebuild" => {
                crate::rebuild::run_rebuild();
                return;
            }
            "snapshot" => {
                crate::snapshot_cmd::run_snapshot(&args[1..]);
                return;
            }
            _ => {}
        }
    }

    // Check for -p / --print flag.
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "-p" | "--print" => {
                let prompt = if i + 1 < args.len() {
                    args[i + 1..].join(" ")
                } else {
                    let mut buf = String::new();
                    std::io::Read::read_to_string(&mut std::io::stdin(), &mut buf).unwrap_or(0);
                    buf
                };
                if prompt.trim().is_empty() {
                    eprintln!("error: -p requires a prompt");
                    std::process::exit(1);
                }
                run_print(modules, prompt).await;
                return;
            }
            _ => i += 1,
        }
    }

    run_default(modules).await;
}

fn system_prompt() -> &'static str {
    "You are a coding assistant. You have access to tools for reading, writing, \
     and editing files, running bash commands, and searching codebases. \
     Use the tools to help the user with their task. \
     Be concise. Prefer editing existing files over creating new ones. \
     Read files before editing them."
}

fn setup_provider() -> (
    Rc<anthropic::AnthropicProvider>,
    Rc<dyn llm::Authenticator>,
    Vec<llm::Model>,
    llm::Model,
) {
    let cred_path = crate::credentials::default_path();
    let oauth_slot: Rc<RefCell<Option<anthropic::oauth::OAuthCredentials>>> =
        Rc::new(RefCell::new(None));

    if let Some(ref path) = cred_path {
        let store = crate::credentials::load(path);
        if let Some(crate::credentials::Credential::OAuth {
            refresh_token,
            access_token,
            expires_at_ms,
        }) = store.get("anthropic")
        {
            *oauth_slot.borrow_mut() = Some(anthropic::oauth::OAuthCredentials {
                refresh_token: refresh_token.clone(),
                access_token: access_token.clone(),
                expires_at_ms: *expires_at_ms,
            });
        }
    }

    let mut provider =
        anthropic::AnthropicProvider::new().with_oauth_shared(oauth_slot.clone());

    // Check for Pi's auth file — re-read on every request for live token sharing.
    let pi_auth = dirs::home_dir().map(|h| h.join(".pi/agent/auth.json"));
    if let Some(ref path) = pi_auth {
        if path.exists() {
            provider = provider.with_credential_file(path);
        }
    }

    // Env vars take priority over everything.
    if let Ok(api_key) = std::env::var("ANTHROPIC_API_KEY") {
        *oauth_slot.borrow_mut() = None;
        provider = provider.with_api_key(&api_key);
    }

    let provider = Rc::new(provider);
    let authenticator: Rc<dyn llm::Authenticator> = provider.clone();
    let available_models = provider.models();

    // Default model: first in the list (Claude Opus 4.6).
    let default_model = available_models
        .first()
        .cloned()
        .expect("anthropic provider should define at least one model");

    (provider, authenticator, available_models, default_model)
}

fn make_cred_save_callback(
    provider: &Rc<anthropic::AnthropicProvider>,
) -> Option<Rc<dyn Fn()>> {
    let cred_path = crate::credentials::default_path()?;
    let oauth_slot = provider.oauth_slot().clone();
    Some(Rc::new(move || {
        if let Some(ref creds) = *oauth_slot.borrow() {
            let _ = crate::credentials::save_oauth(
                &cred_path,
                "anthropic",
                &creds.refresh_token,
                &creds.access_token,
                creds.expires_at_ms,
            );
        }
    }) as Rc<dyn Fn()>)
}

fn summarize_args(args: &serde_json::Value) -> String {
    if let Some(cmd) = args.get("command").and_then(|v| v.as_str()) {
        let display = if cmd.len() > 80 { &cmd[..77] } else { cmd };
        return format!("$ {display}");
    }
    if let Some(path) = args.get("file_path").and_then(|v| v.as_str()) {
        return path.to_string();
    }
    if let Some(pattern) = args.get("pattern").and_then(|v| v.as_str()) {
        return format!("/{pattern}/");
    }
    String::new()
}
