//! Chat-style editor demo.
//!
//! Messages scroll upward. Editor sits at the bottom edge-to-edge.
//! Enter submits, Shift+Enter inserts a newline.
//! Paste >50 chars becomes a pill.
//!
//! Esc or Ctrl+C to quit.

use mage_tui::style::Color;
use mage_tui::wrap::wrap_text;
use mage_tui::overlay::SelectItem;
use mage_tui::*;

const RST: &str = "\x1b[0m";
const DIM: &str = "\x1b[2m";

// ── Message ─────────────────────────────────────────────────────

struct Message {
    text: String,
    color: Color,
}

// ── App ─────────────────────────────────────────────────────────

struct ChatApp {
    messages: Vec<Message>,
    editor: Editor,
    width: u16,
    msg_idx: usize,
}

const MSG_COLORS: [Color; 4] = [
    Color::Rgb(180, 200, 230),
    Color::Rgb(200, 220, 180),
    Color::Rgb(220, 190, 180),
    Color::Rgb(190, 210, 200),
];

impl ChatApp {
    fn submit(&mut self) {
        let text = self.editor.take();
        let trimmed = text.trim();
        if trimmed.is_empty() {
            return;
        }
        let color = MSG_COLORS[self.msg_idx % MSG_COLORS.len()];
        self.msg_idx += 1;
        self.messages.push(Message {
            text: trimmed.to_string(),
            color,
        });
    }
}

impl App for ChatApp {
    type Message = ();

    fn render(&mut self, r: &mut renderer::Renderer) {
        let w = r.width() as usize;

        // ── Messages ──
        for msg in &self.messages {
            let fg_code = format!("\x1b[{}m", msg.color.fg_code());
            let inner_w = w.saturating_sub(1);
            if inner_w == 0 {
                r.push_line(format!("{fg_code}{}{RST}", &msg.text));
                continue;
            }
            for paragraph in msg.text.split('\n') {
                if paragraph.is_empty() {
                    r.push_blank();
                } else {
                    for line in wrap_text(paragraph, inner_w) {
                        r.push_line(format!(" {fg_code}{line}{RST}"));
                    }
                }
            }
            r.push_blank();
        }

        // ── Separator ──
        r.push_line(format!("{DIM}{}{RST}", "─".repeat(w)));

        // ── Editor (edge to edge) ──
        self.editor.render(r, "");
    }

    fn update(&mut self, event: Event<()>) -> bool {
        match event {
            Event::Key(key) => {
                use crossterm::event::KeyCode;
                let ctrl = key.modifiers.contains(crossterm::event::KeyModifiers::CONTROL);
                // Esc dismisses overlay first, then quits.
                if key.code == KeyCode::Esc {
                    if self.editor.has_overlay() {
                        self.editor.dismiss_overlay();
                    } else {
                        return true;
                    }
                    return false;
                }
                if ctrl && key.code == KeyCode::Char('c') {
                    return true;
                }

                let editor_w = self.width as usize;
                match self.editor.handle_key(key, editor_w) {
                    KeyResult::Submit => self.submit(),
                    KeyResult::Command(cmd) => {
                        let color = MSG_COLORS[self.msg_idx % MSG_COLORS.len()];
                        self.msg_idx += 1;
                        self.messages.push(Message {
                            text: format!("Command: {cmd}"),
                            color,
                        });
                        self.editor.clear();
                    }
                    _ => {}
                }
            }
            Event::Paste(text) => {
                self.editor.paste(&text);
            }
            Event::Resize(w, _h) => {
                self.width = w;
            }
            _ => {}
        }
        false
    }
}

#[tokio::main]
async fn main() {
    let (w, _h) = crossterm::terminal::size().unwrap_or((80, 24));

    let welcome = vec![
        Message { text: "Welcome to the chat editor demo!".into(), color: MSG_COLORS[0] },
        Message { text: "Type a message and press Enter to send.".into(), color: MSG_COLORS[1] },
        Message { text: "Option+Enter for newlines. Type / for commands. Tab to complete. Esc to quit.".into(), color: MSG_COLORS[2] },
    ];

    let mut editor = Editor::new();
    editor.set_commands(vec![
        SelectItem::new("/help", "Show help information"),
        SelectItem::new("/clear", "Clear all messages"),
        SelectItem::new("/model", "Switch the active model"),
        SelectItem::new("/compact", "Compact conversation history"),
        SelectItem::new("/export", "Export chat to file"),
        SelectItem::new("/theme", "Change color theme"),
        SelectItem::new("/quit", "Exit the application"),
    ]);

    let msg_idx = welcome.len();
    run(ChatApp {
        messages: welcome,
        editor,
        width: w,
        msg_idx,
    })
    .await;
}
