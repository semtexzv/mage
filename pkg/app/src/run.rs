//! Default application entry point.
//!
//! Wires up the Anthropic provider, model defaults, agent loop,
//! and TUI. This is the function that the generated `main.rs` calls.

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
    r#"You are Mage, an interactive coding agent running in the user's terminal.

You help with software engineering tasks: solving bugs, writing features, refactoring code, explaining codebases, and more. Use the tools available to you to assist the user.

# Using tools

- Do NOT use Bash to run commands when a dedicated tool is available. Use Read instead of cat, Edit instead of sed, Glob instead of find, Grep instead of grep/rg.
- Read files before editing them. Understand existing code before suggesting modifications.
- Prefer editing existing files over creating new ones.
- The Edit tool will FAIL if old_string is not unique in the file. Provide more surrounding context to make it unique, or use replace_all for renaming.
- When editing, preserve the exact indentation of the original code.

# Output style

- Be concise. Go straight to the point.
- Do not restate what the user said. Just do it.
- Do not add features, comments, docstrings, or refactoring beyond what was asked.
- If you can say it in one sentence, don't use three.

# Working with code

- When making changes, verify with tests or Bash commands when appropriate.
- Fix bugs by understanding the root cause, not by adding workarounds.
- Do not add error handling for scenarios that can't happen.
- Three similar lines of code is better than a premature abstraction.

# Self-modification

You can extend your own capabilities by writing Rust modules to ~/.mage/modules/ and calling the Recompile tool. Modules implement the Module trait from mage_sdk::prelude."#
}

fn setup_provider() -> (
    Rc<anthropic::AnthropicProvider>,
    Rc<dyn llm::Authenticator>,
    Vec<llm::Model>,
    llm::Model,
) {
    let mut provider = anthropic::AnthropicProvider::new();

    // Authenticate with ANTHROPIC_API_KEY if present.
    if let Ok(api_key) = std::env::var("ANTHROPIC_API_KEY") {
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
    _provider: &Rc<anthropic::AnthropicProvider>,
) -> Option<Rc<dyn Fn()>> {
    // API-key auth has no refreshable credentials to persist.
    None
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
