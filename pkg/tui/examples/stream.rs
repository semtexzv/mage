//! Streaming log with input — appends lines rapidly.
//!
//! Background task pushes 5 lines every 16ms (~300 lines/sec).
//! Type in the input row, Enter to send, Esc/Ctrl-C to quit.

use tau_tui_next::*;
use tokio::sync::mpsc;

struct StreamApp {
    lines: Vec<String>,
    editor: Editor,
    width: u16,
}

enum Msg {
    Lines(Vec<String>),
}

impl App for StreamApp {
    type Message = Msg;

    fn render(&mut self, r: &mut tau_tui_next::renderer::Renderer) {
        r.push_line(format!(
            "\x1b[1m\x1b[36mstreaming log\x1b[0m  ({} lines)",
            self.lines.len(),
        ));
        r.push_blank();
        for line in self.lines.iter() {
            r.push_line(line.as_str());
        }
        r.push_blank();
        self.editor.render(r, "\x1b[32m> \x1b[0m");
    }

    fn update(&mut self, event: Event<Msg>) -> bool {
        match event {
            Event::Key(k) => {
                use crossterm::event::{KeyCode, KeyModifiers};
                let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
                if k.code == KeyCode::Esc || (ctrl && k.code == KeyCode::Char('c')) {
                    return true;
                }
                if self.editor.handle_key(k, self.width as usize) == KeyResult::Submit {
                    let text = self.editor.take();
                    let trimmed = text.trim();
                    if !trimmed.is_empty() {
                        self.lines
                            .push(format!("\x1b[1m\x1b[35m[you]\x1b[0m {}", trimmed));
                    }
                }
            }
            Event::Message(Msg::Lines(batch)) => self.lines.extend(batch),
            Event::Resize(w, _) => { self.width = w; }
            _ => {}
        }
        false
    }
}

#[tokio::main]
async fn main() {
    let (w, _h) = crossterm::terminal::size().unwrap_or((80, 24));
    let (tx, rx) = mpsc::channel(512);
    tokio::spawn(async move {
        let mut i: u64 = 0;
        loop {
            let mut batch = Vec::with_capacity(5);
            for _ in 0..5 {
                i += 1;
                batch.push(format!(
                    "\x1b[33m[{:>6}]\x1b[0m  the quick brown fox jumps over the lazy dog — line {}",
                    i, i,
                ));
            }
            if tx.send(Msg::Lines(batch)).await.is_err() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(16)).await;
        }
    });
    run_with_messages(
        StreamApp {
            lines: Vec::new(),
            editor: Editor::new(),
            width: w,
        },
        rx,
    )
    .await;
}
