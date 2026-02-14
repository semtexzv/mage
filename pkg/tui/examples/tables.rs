//! Streams a markdown table row-by-row in a TUI. Resize-aware.
//! Press q or Esc to quit.

use mage_tui::*;
use tokio::sync::mpsc;

const HEADER: &str = "| Name    | Score | Status  |\n|---------|-------|---------|\n";
const ROWS: &[&str] = &[
    "| Alice   | 97    | ✅ pass |\n",
    "| Bob     | 42    | ❌ fail |\n",
    "| Charlie | 88    | ✅ pass |\n",
    "| Diana   | 73    | ✅ pass |\n",
    "| Eve     | 31    | ❌ fail |\n",
];

struct TableApp {
    md: Markdown,
    rows_added: usize,
    keymap: Keymap<Act>,
}

#[derive(Clone)]
enum Act { Quit }

enum Msg { AddRow }

impl App for TableApp {
    type Message = Msg;

    fn render(&mut self, r: &mut renderer::Renderer) {
        r.push_line(format!(
            "\x1b[1m\x1b[36mstreaming table\x1b[0m  {} of {} rows",
            self.rows_added, ROWS.len(),
        ));
        r.push_blank();
        self.md.render(r);
        r.push_blank();
        r.push_line("\x1b[2mPress q or Esc to quit\x1b[0m");
    }

    fn update(&mut self, event: Event<Msg>) -> bool {
        match event {
            Event::Key(k) => {
                if let Some(Act::Quit) = self.keymap.lookup(&k) {
                    return true;
                }
            }
            Event::Message(Msg::AddRow) => {
                if self.rows_added < ROWS.len() {
                    self.md.append(ROWS[self.rows_added]);
                    self.rows_added += 1;
                }
            }
            Event::Resize(w, _h) => {
                self.md.set_width(w);
            }
            _ => {}
        }
        self.md.lines();
        false
    }
}

#[tokio::main]
async fn main() {
    let (tx, rx) = mpsc::channel::<Msg>(16);

    tokio::spawn(async move {
        // Add one row every 600ms
        for _ in 0..ROWS.len() {
            tokio::time::sleep(std::time::Duration::from_millis(600)).await;
            if tx.send(Msg::AddRow).await.is_err() {
                break;
            }
        }
    });

    let width = crossterm::terminal::size().map(|(w, _)| w).unwrap_or(80);
    let mut md = Markdown::new(width);
    md.append(HEADER);
    md.lines();

    run_with_messages(
        TableApp {
            md,
            rows_added: 0,
            keymap: Keymap::from([
                (ch('q'), Act::Quit),
                (ESC, Act::Quit),
                (ctrl(ch('c')), Act::Quit),
            ]),
        },
        rx,
    )
    .await;
}
