//! TUI integration — bridges the session event stream to [`mage_tui`].
//!
//! Chat-style UI with a widget-per-entry model. Each log entry owns a
//! persistent rendering widget (`Text`, `Markdown`) that caches its output
//! lines across frames. The render pass just calls `widget.render(r)` —
//! no manual ANSI or `push_line` in the app layer.

use std::rc::Rc;

use crossterm::event::{KeyCode, KeyModifiers};

use mage_core::event_stream::AgentEventReceiver;
use mage_core::types::ToolResult;
use mage_core::types::AgentEvent;
use mage_tui::renderer::Renderer;
use mage_tui::style::{Color, Padding, Style, Theme};
use mage_tui::text::Text;
use mage_tui::{Editor, Event, HRule, KeyResult, Markdown};

use crate::app::App;

// ── Theme ──────────────────────────────────────────────────────
const PAD_BLOCK: Padding = Padding::new(1, 1, 1, 1);
const PAD_MD: Padding = Padding::new(0, 1, 0, 1);
// Backgrounds — only for user input and tool blocks.
const BG_USER: Color = Color::Rgb(30, 35, 50);
const BG_TOOL: Color = Color::Rgb(25, 30, 25);
const BG_TOOL_ERR: Color = Color::Rgb(45, 25, 25);
// Foreground accents.
const FG_DIM: Color = Color::Rgb(100, 100, 110);
const FG_BORDER: Color = Color::Rgb(50, 50, 58);
const SPINNER_FRAMES: &[char] = &['⠈', '⠘', '⠰', '⠠', '⠤', '⠦', '⠇', '⠏'];

// ── Messages ───────────────────────────────────────────────────

enum Msg {
    Agent(AgentEvent),
    Login(llm::LoginStep),
    CommandError(String),
    Tick,
}

// ── Log entry widgets ──────────────────────────────────────────

/// Each entry in the conversation log owns its rendering widget(s).
/// Widgets are created once and mutated in place; the diff renderer
/// detects unchanged cached lines via `Rc::ptr_eq`.
enum Widget {
    /// User input. Immutable after creation.
    User(Text),
    /// Streaming assistant response.
    Assistant(Markdown),
    /// Streaming thinking block (subdued markdown).
    Thinking(Markdown),
    /// Tool invocation (running or completed).
    Tool(ToolWidget),
    /// Non-fatal error from the agent loop.
    Error(Text),
    /// Informational message (e.g. login flow status).
    Info(Text),
}

/// Composite widget for a tool invocation block.
struct ToolWidget {
    call_id: String,
    name: String,
    header: Text,
    /// Streaming output — updated as the tool runs.
    streaming: Option<Text>,
    /// Final output — replaces streaming on completion.
    output: Option<Text>,
    done: bool,
}

impl ToolWidget {
    fn new(call_id: &str, name: &str, args_summary: &str) -> Self {
        let mut header = Text::empty();
        header.push("⏵ ", Style::new().dim());
        header.push(name, Style::new().bold());
        if !args_summary.is_empty() {
            header.push(&format!("  {args_summary}"), Style::new().dim());
        }
        header.set_bg(Some(BG_TOOL));
        header.set_padding(Padding::new(1, 1, 1, 1));

        Self {
            call_id: call_id.to_string(),
            name: name.to_string(),
            header,
            streaming: None,
            output: None,
            done: false,
        }
    }

    /// Replace the streaming view with the tool's current state.
    fn update(&mut self, text: &str) {
        if self.done || text.is_empty() {
            return;
        }
        self.streaming = Some(
            Text::new(text)
                .style(Style::new().dim())
                .bg(BG_TOOL)
                .padding(Padding::new(0, 1, 0, 1)),
        );
    }

    fn complete(&mut self, is_error: bool, summary: &str) {
        self.done = true;
        self.streaming = None; // drop streaming widget

        let bg = if is_error { BG_TOOL_ERR } else { BG_TOOL };
        let icon = if is_error { "✗ " } else { "✓ " };
        let icon_style = if is_error {
            Style::new().bold().fg(Color::Red)
        } else {
            Style::new().fg(Color::Green)
        };

        self.header = Text::empty();
        self.header.push(icon, icon_style);
        self.header.push(&self.name, Style::new().bold());
        self.header.set_bg(Some(bg));
        self.header.set_padding(Padding::new(1, 1, 1, 1));

        if !summary.is_empty() {
            let lines: Vec<&str> = summary.lines().collect();
            let show = lines.len().min(8);
            let display: String = lines[..show].join("\n");
            let content = if lines.len() > 8 {
                format!("{display}\n… {} more lines", lines.len() - 8)
            } else {
                display
            };
            let text = Text::new(content)
                .style(Style::new().dim())
                .bg(bg)
                .padding(Padding::new(0, 1, 1, 1));
            self.output = Some(text);
        }
    }

    fn render(&mut self, r: &mut Renderer) {
        self.header.render(r);
        if let Some(stream) = &mut self.streaming {
            stream.render(r);
        }
        if let Some(output) = &mut self.output {
            output.render(r);
        }
    }
}

// ── Factory helpers ────────────────────────────────────────────

fn make_user_text(content: &str) -> Text {
    let mut t = Text::new(content);
    t = t.style(Style::new().bold());
    t = t.bg(BG_USER);
    t = t.padding(PAD_BLOCK);
    t
}

fn make_assistant_md(width: u16) -> Markdown {
    Markdown::with_pad(width, PAD_MD)
}

fn make_error_text(message: &str) -> Text {
    let mut t = Text::empty();
    t.push("error: ", Style::new().bold().fg(Color::Red));
    t.push_plain(message);
    t.set_padding(PAD_BLOCK);
    t
}
fn make_info_text(message: &str) -> Text {
    let mut t = Text::new(message);
    t = t.style(Style::new().fg(FG_DIM));
    t = t.padding(PAD_BLOCK);
    t
}
fn make_thinking_md(width: u16) -> Markdown {
    let mut md = Markdown::with_pad(width, PAD_MD);
    md.apply_theme(&Theme::thinking());
    md
}

// ── TUI state ──────────────────────────────────────────────────

struct MageTui {
    app: App,
    editor: Editor,
    log: Vec<Widget>,
    running: bool,
    /// Braille spinner frame counter (cycles through SPINNER_FRAMES).
    spinner_frame: usize,
    width: u16,
    /// Optional authenticator for /login command.
    authenticator: Option<Rc<dyn llm::Authenticator>>,
    /// Message sender - login flow relay tasks send Msg through here.
    msg_tx: tokio::sync::mpsc::Sender<Msg>,
    /// Called after credentials change (login success or token refresh).
    on_cred_save: Option<Rc<dyn Fn()>>,
    /// Pending login prompt — when set, the next submit goes here instead of the session.
    login_prompt_reply: Option<tokio::sync::oneshot::Sender<String>>,
    /// Known models from the provider, for /model command.
    available_models: Vec<llm::Model>,
    /// Current model display name for the status bar.
    current_model_name: String,
    /// Cumulative token usage across all turns.
    total_usage: llm::Usage,
    hr: HRule,
    spinner: Text,
    status: Text,
    /// Content line count from last frame (for spacer calculation).
    last_content_lines: usize,
}

impl MageTui {
    fn new(
        app: App,
        authenticator: Option<Rc<dyn llm::Authenticator>>,
        msg_tx: tokio::sync::mpsc::Sender<Msg>,
        on_cred_save: Option<Rc<dyn Fn()>>,
        available_models: Vec<llm::Model>,
        initial_model_name: String,
    ) -> Self {
        let mut editor = Editor::new();

        // Register slash commands for autocomplete overlay.
        let mut commands = vec![
            mage_tui::SelectItem::new("/login", "Login to your Anthropic account"),
            mage_tui::SelectItem::new("/model", "Switch model (provider/model-id)"),
        ];
        // Add module-registered commands.
        for name in app.commands.names() {
            let desc = app.commands.get(name)
                .and_then(|c| c.description.as_deref())
                .unwrap_or("");
            commands.push(mage_tui::SelectItem::new(
                format!("/{name}"),
                desc,
            ));
        }
        editor.set_commands(commands);

        // Register model sub-completions for /model command.
        let model_items: Vec<mage_tui::SelectItem> = available_models.iter().map(|m| {
            mage_tui::SelectItem::new(
                format!("{}/{}", m.provider, m.id),
                m.name.as_ref(),
            )
        }).collect();
        editor.set_command_completions("model", model_items);

        let mut tui = Self {
            app,
            editor,
            log: Vec::new(),
            running: false,
            spinner_frame: 0,
            width: 80,
            authenticator,
            msg_tx,
            on_cred_save,
            login_prompt_reply: None,
            available_models,
            current_model_name: initial_model_name,
            total_usage: llm::Usage::default(),
            hr: HRule::new('─', FG_BORDER),
            spinner: Text::empty(),
            status: Text::empty(),
            last_content_lines: 0,
        };
        tui.rebuild_status();
        tui
    }

    /// Handle user pressing Enter (Submit from the editor).
    fn submit(&mut self) {
        let text = self.editor.take();
        let trimmed = text.trim();
        if trimmed.is_empty() {
            return;
        }
        // If a login prompt is pending, send the text there instead.
        if let Some(reply) = self.login_prompt_reply.take() {
            let _ = reply.send(trimmed.to_string());
            self.log.push(Widget::Info(make_info_text(&format!("Code: {}", &trimmed[..trimmed.len().min(20)]))));
            return;
        }
        // Slash commands that bypassed the overlay (e.g. user dismissed it).
        if trimmed.starts_with('/') {
            self.execute_command(trimmed.to_string());
            return;
        }
        self.log.push(Widget::User(make_user_text(trimmed)));
        self.set_running(true);
        self.app.handle.send_input(trimmed);
    }

    /// Handle a slash command selected via the editor overlay.
    fn execute_command(&mut self, cmd: String) {
        let cmd = cmd.trim().trim_start_matches('/');
        let (name, args) = match cmd.split_once(' ') {
            Some((n, a)) => (n, a.trim()),
            None => (cmd, ""),
        };

        match name {
            "login" => self.start_login(),
            "model" => self.handle_model_command(args),
            _ => {
                // Delegate to the command registry.
                let handle = self.app.handle.clone();
                let commands = self.app.commands.clone();
                let name = name.to_string();
                let args = args.to_string();
                let tx = self.msg_tx.clone();
                tokio::task::spawn_local(async move {
                    if let Err(e) = commands.execute(&name, &args, &handle).await {
                        let _ = tx.send(Msg::CommandError(format!("{e}"))).await;
                    }
                });
            }
        }
    }

    fn handle_model_command(&mut self, args: &str) {
        if args.is_empty() {
            // List available models.
            if self.available_models.is_empty() {
                self.log.push(Widget::Info(make_info_text("No models available.")));
                return;
            }
            let mut listing = String::from("Available models:");
            for m in &self.available_models {
                listing.push_str(&format!("\n  {}/{} — {}", m.provider, m.id, m.name));
            }
            self.log.push(Widget::Info(make_info_text(&listing)));
            return;
        }

        // Parse provider/model or just model-id.
        let (provider_filter, model_id) = if let Some((p, m)) = args.split_once('/') {
            (Some(p), m)
        } else {
            (None, args)
        };

        let found = self.available_models.iter().find(|m| {
            let id_match = m.id.as_ref() == model_id;
            let provider_match = provider_filter
                .map(|p| m.provider.as_ref() == p)
                .unwrap_or(true);
            id_match && provider_match
        });

        match found {
            Some(model) => {
                let desc = format!("Model set to {}/{} ({})", model.provider, model.id, model.name);
                self.app.handle.set_model(model.clone());
                self.current_model_name = model.name.to_string();
                self.rebuild_status();
                self.log.push(Widget::Info(make_info_text(&desc)));
            }
            None => {
                self.log.push(Widget::Error(make_error_text(
                    &format!("Unknown model '{}'. Use /model to list available models.", args),
                )));
            }
        }
    }

    fn start_login(&mut self) {
        let auth = match &self.authenticator {
            Some(a) => a.clone(),
            None => {
                self.log.push(Widget::Error(make_error_text(
                    "no authenticator configured for this provider",
                )));
                return;
            }
        };

        // Check current status.
        let status = auth.auth_status();
        if status == llm::AuthStatus::Authenticated {
            self.log
                .push(Widget::Info(make_info_text("Already authenticated.")));
            return;
        }

        self.log
            .push(Widget::Info(make_info_text("Starting login…")));

        // Start the login flow — get a LoginReceiver.
        let mut login_rx = auth.login();
        let tx = self.msg_tx.clone();

        // Relay login steps into the TUI message loop.
        tokio::task::spawn_local(async move {
            while let Some(step) = login_rx.recv().await {
                if tx.send(Msg::Login(step)).await.is_err() {
                    break;
                }
            }
        });
    }

    fn handle_login_step(&mut self, step: llm::LoginStep) {
        match step {
            llm::LoginStep::Message(msg) => {
                self.log.push(Widget::Info(make_info_text(&msg)));
            }
            llm::LoginStep::OpenUrl(url) => {
                self.log
                    .push(Widget::Info(make_info_text(&format!("Open: {url}"))));
                // Try to open the browser.
                let _ = open_url(&url);
            }
            llm::LoginStep::Prompt { message, reply } => {
                self.log.push(Widget::Info(make_info_text(&message)));
                self.login_prompt_reply = Some(reply);
            }
            llm::LoginStep::Done => {
                self.log
                    .push(Widget::Info(make_info_text("Login successful.")));
                if let Some(f) = &self.on_cred_save {
                    f();
                }
            }
            llm::LoginStep::Failed(msg) => {
                self.log
                    .push(Widget::Error(make_error_text(&format!("Login failed: {msg}"))));
                // Clear any pending prompt on failure.
                self.login_prompt_reply = None;
            }
        }
    }

    fn set_running(&mut self, running: bool) {
        self.running = running;
        if running {
            let ch = SPINNER_FRAMES[self.spinner_frame % SPINNER_FRAMES.len()];
            self.spinner.set_styled(format!(" {ch}"), Style::new().fg(FG_DIM));
        } else {
            self.spinner.clear();
        }
    }

    fn rebuild_status(&mut self) {
        let u = &self.total_usage;
        let ctx = format_tokens(u.input + u.output);
        let cache = format_tokens(u.cache_read + u.cache_write);
        let cost = format_cost(u.cost.total);
        let label = format!(" {}  ·  ctx {}  ·  cache {}  ·  {}", self.current_model_name, ctx, cache, cost);
        self.status = Text::new(label).style(Style::new().fg(FG_DIM));
    }

    /// Get or create the current streaming assistant Markdown entry.
    fn current_assistant(&mut self) -> &mut Markdown {
        let need_new = !matches!(self.log.last(), Some(Widget::Assistant(_)));
        if need_new {
            self.log
                .push(Widget::Assistant(make_assistant_md(self.width)));
        }
        match self.log.last_mut().unwrap() {
            Widget::Assistant(md) => md,
            _ => unreachable!(),
        }
    }

    /// Get or create the current streaming thinking Markdown entry.
    fn current_thinking(&mut self) -> &mut Markdown {
        let need_new = !matches!(self.log.last(), Some(Widget::Thinking(_)));
        if need_new {
            self.log
                .push(Widget::Thinking(make_thinking_md(self.width)));
        }
        match self.log.last_mut().unwrap() {
            Widget::Thinking(md) => md,
            _ => unreachable!(),
        }
    }

    /// Find a tool widget by call_id.
    fn find_tool(&mut self, call_id: &str) -> Option<&mut ToolWidget> {
        for entry in self.log.iter_mut().rev() {
            if let Widget::Tool(tw) = entry {
                if tw.call_id == call_id {
                    return Some(tw);
                }
            }
        }
        None
    }

    fn update_widths(&mut self, w: u16) {
        if w == self.width {
            return;
        }
        self.width = w;
        for entry in &mut self.log {
            match entry {
                Widget::Assistant(md) | Widget::Thinking(md) => md.set_width(w),
                _ => {}
            }
        }
    }
}

// ── Rendering ──────────────────────────────────────────────────

impl mage_tui::App for MageTui {
    type Message = Msg;

    fn render(&mut self, r: &mut Renderer) {
        self.update_widths(r.width());

        // Bottom chrome height: blank + spinner + hr + editor + hr + status.
        // Editor is typically 1 line but can be multi-line.
        let chrome_lines = 6usize; // approximate

        // Spacer: push the editor to the bottom when the log is short.
        // Uses last frame's content line count to estimate.
        let content_lines = self.last_content_lines;
        let term_h = r.height() as usize;
        let spacer = term_h.saturating_sub(content_lines + chrome_lines);
        for _ in 0..spacer {
            r.push_blank();
        }

        // Chat log — each entry renders itself.
        for entry in &mut self.log {
            match entry {
                Widget::User(text) => text.render(r),
                Widget::Assistant(md) => md.render(r),
                Widget::Thinking(md) => md.render(r),
                Widget::Tool(tw) => tw.render(r),
                Widget::Error(text) => text.render(r),
                Widget::Info(text) => text.render(r),
            }
        }

        // Spinner line — always present, active or blank.
        r.push_blank();
        if self.running {
            self.spinner.render(r);
        } else {
            r.push_blank();
        }
        self.hr.render(r);
        self.editor.render(r, " ");
        self.hr.render(r);

        // Status bar: model · context · cache · cost
        self.status.render(r);

        // Track content lines for next frame's spacer calculation.
        self.last_content_lines = r.line_count().saturating_sub(spacer);
    }
    fn update(&mut self, event: Event<Msg>) -> bool {
        match event {
            Event::Key(key) => {
                if key.modifiers.contains(KeyModifiers::CONTROL)
                    && key.code == KeyCode::Char('c')
                {
                    if self.running {
                        self.app.handle.abort();
                        return false;
                    }
                    self.app.handle.shutdown();
                    return true;
                }

                if key.modifiers.contains(KeyModifiers::CONTROL)
                    && key.code == KeyCode::Char('d')
                {
                    self.app.handle.shutdown();
                    return true;
                }

                match self.editor.handle_key(key, self.width as usize) {
                    KeyResult::Submit => self.submit(),
                    KeyResult::Command(cmd) => self.execute_command(cmd),
                    KeyResult::Consumed | KeyResult::Ignored => {}
                }
                false
            }
            Event::Paste(text) => {
                self.editor.paste(&text);
                false
            }
            Event::Message(Msg::Agent(ev)) => {
                self.handle_agent_event(ev);
                false
            }
            Event::Message(Msg::Login(step)) => {
                self.handle_login_step(step);
                false
            }
            Event::Message(Msg::CommandError(msg)) => {
                self.log.push(Widget::Error(make_error_text(&msg)));
                false
            }
            Event::Message(Msg::Tick) => {
                if self.running {
                    self.spinner_frame = self.spinner_frame.wrapping_add(1);
                    let ch = SPINNER_FRAMES[self.spinner_frame % SPINNER_FRAMES.len()];
                    self.spinner.set_styled(format!(" {ch}"), Style::new().fg(FG_DIM));
                }
                false
            }
            Event::Resize(w, _) => {
                self.width = w;
                false
            }
        }
    }
}

impl MageTui {
    fn handle_agent_event(&mut self, ev: AgentEvent) {
        match ev {
            AgentEvent::AgentStart => {}
            AgentEvent::MessageDelta {
                event,
            } => {
                match &event {
                    llm::AssistantMessageEvent::TextDelta { delta, .. } => {
                        self.current_assistant().append(delta);
                    }
                    llm::AssistantMessageEvent::ThinkingStart { .. } => {
                        // Force a new thinking block.
                        self.log.push(Widget::Thinking(make_thinking_md(self.width)));
                    }
                    llm::AssistantMessageEvent::ThinkingDelta { delta, .. } => {
                        self.current_thinking().append(delta);
                    }
                    llm::AssistantMessageEvent::ThinkingEnd { .. } => {
                        // Nothing to do — block is complete, stays in log.
                    }
                    _ => {}
                }
            }
            AgentEvent::ToolExecStart {
                tool_call_id, tool_name, args, ..
            } => {
                let args_summary = summarize_args(&args);
                self.log
                    .push(Widget::Tool(ToolWidget::new(&tool_call_id, &tool_name, &args_summary)));
            }
            AgentEvent::ToolExecUpdate {
                tool_call_id, update, ..
            } => {
                if let Some(tw) = self.find_tool(&tool_call_id) {
                    tw.update(&update.text);
                }
            }
            AgentEvent::ToolExecEnd {
                tool_call_id, tool_name, result, ..
            } => {
                let summary = tool_result_text(&result);
                if let Some(tw) = self.find_tool(&tool_call_id) {
                    tw.complete(result.is_error, &summary);
                } else {
                    let mut tw = ToolWidget::new(&tool_call_id, &tool_name, "");
                    tw.complete(result.is_error, &summary);
                    self.log.push(Widget::Tool(tw));
                }
            }
            AgentEvent::AgentError { message } => {
                self.log.push(Widget::Error(make_error_text(&message)));
            }
            AgentEvent::TurnEnd { message, .. } => {
                // Accumulate usage from the completed turn.
                if let mage_core::types::MessageBody::Assistant { usage, model, .. } = &message.body {
                    self.total_usage.input += usage.input;
                    self.total_usage.output += usage.output;
                    self.total_usage.cache_read += usage.cache_read;
                    self.total_usage.cache_write += usage.cache_write;
                    self.total_usage.total_tokens += usage.total_tokens;
                    // Compute cost from the model's per-token rates.
                    let rates = self.available_models.iter()
                        .find(|m| m.id.as_ref() == model.as_ref())
                        .map(|m| &m.cost);
                    if let Some(rates) = rates {
                        self.total_usage.cost.input += usage.input as f64 * rates.input / 1_000_000.0;
                        self.total_usage.cost.output += usage.output as f64 * rates.output / 1_000_000.0;
                        self.total_usage.cost.cache_read += usage.cache_read as f64 * rates.cache_read / 1_000_000.0;
                        self.total_usage.cost.cache_write += usage.cache_write as f64 * rates.cache_write / 1_000_000.0;
                        self.total_usage.cost.total = self.total_usage.cost.input
                            + self.total_usage.cost.output
                            + self.total_usage.cost.cache_read
                            + self.total_usage.cost.cache_write;
                    }
                    self.rebuild_status();
                }
            }
            AgentEvent::AgentEnd { .. } => {
                self.set_running(false);
            }
            _ => {}
        }
    }
}

// ── Helpers ────────────────────────────────────────────────────


fn summarize_args(args: &serde_json::Value) -> String {
    if let Some(cmd) = args.get("command").and_then(|v| v.as_str()) {
        let display = if cmd.len() > 80 {
            format!("{}…", &cmd[..77])
        } else {
            cmd.to_string()
        };
        return format!("$ {display}");
    }
    if let Some(obj) = args.as_object() {
        for (k, v) in obj {
            if let Some(s) = v.as_str() {
                let display = if s.len() > 60 {
                    format!("{}…", &s[..57])
                } else {
                    s.to_string()
                };
                return format!("{k}: {display}");
            }
        }
    }
    String::new()
}

fn tool_result_text(result: &ToolResult) -> String {
    result.content.iter()
        .find_map(|uc| match uc {
            llm::UserContent::Text { text } => Some(text.clone()),
            _ => None,
        })
        .unwrap_or_default()
}

/// Try to open a URL in the user's default browser.
fn open_url(url: &str) -> std::io::Result<()> {
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open").arg(url).spawn()?;
    }
    #[cfg(target_os = "linux")]
    {
        std::process::Command::new("xdg-open").arg(url).spawn()?;
    }
    #[cfg(target_os = "windows")]
    {
        std::process::Command::new("cmd").args(["/C", "start", url]).spawn()?;
    }
    Ok(())
}

/// Format token count: 1234 → "1.2k", 1234567 → "1.2M".
fn format_tokens(n: u64) -> String {
    if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1_000_000.0)
    } else if n >= 1_000 {
        format!("{:.1}k", n as f64 / 1_000.0)
    } else {
        format!("{n}")
    }
}

/// Format cost: 0.0 → "$0.00", 0.1234 → "$0.12", 1.5 → "$1.50".
fn format_cost(c: f64) -> String {
    if c < 0.01 && c > 0.0 {
        format!("${c:.4}")
    } else {
        format!("${c:.2}")
    }
}

// ── Public entry-point ─────────────────────────────────────────

/// Run the full TUI application. Returns when the user quits.
///
/// `authenticator` is optional — when provided, the `/login` command
/// is available and delegates to the provider's auth flow.
pub async fn run(
    app: App,
    mut event_rx: AgentEventReceiver,
    authenticator: Option<Rc<dyn llm::Authenticator>>,
    on_cred_save: Option<Rc<dyn Fn()>>,
    available_models: Vec<llm::Model>,
    initial_model_name: String,
) {
    let (tx, tui_rx) = tokio::sync::mpsc::channel(256);

    // Relay agent events into the TUI message loop.
    let tx2 = tx.clone();
    tokio::task::spawn_local(async move {
        while let Some(ev) = event_rx.recv().await {
            if tx2.send(Msg::Agent(ev)).await.is_err() {
                break;
            }
        }
    });

    // Spinner tick — drives the Braille animation at ~10fps.
    let tx3 = tx.clone();
    tokio::task::spawn_local(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_millis(100));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            interval.tick().await;
            if tx3.send(Msg::Tick).await.is_err() {
                break;
            }
        }
    });

    let tui_app = MageTui::new(app, authenticator, tx, on_cred_save, available_models, initial_model_name);
    mage_tui::run_with_messages(tui_app, tui_rx).await;
}
