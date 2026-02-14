//! Demo: multiple spinners that sequentially grow and shrink in height.
//!
//! Each spinner cycles through phases with different line counts,
//! testing the diff renderer's ability to handle content that changes size.
//! Ctrl-C or 'q' to quit.

use crossterm::event::{KeyCode, KeyModifiers};
use mage_tui::{run_with_messages, App, Event};
use tokio::sync::mpsc;

const SPINNER_FRAMES: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

const COLORS: &[&str] = &[
    "\x1b[31m", // red
    "\x1b[32m", // green
    "\x1b[33m", // yellow
    "\x1b[34m", // blue
    "\x1b[35m", // magenta
    "\x1b[36m", // cyan
];

struct Spinner {
    label: &'static str,
    frame: usize,
    phase: usize,
    detail_lines: Vec<String>,
}

impl Spinner {
    fn new(label: &'static str) -> Self {
        Self {
            label,
            frame: 0,
            phase: 0,
            detail_lines: Vec::new(),
        }
    }

    fn tick(&mut self) {
        self.frame = (self.frame + 1) % SPINNER_FRAMES.len();

        // Advance phase every 3 frames (~48ms) for rapid height changes.
        if self.frame.is_multiple_of(3) {
            self.phase = (self.phase + 1) % 8;
        }

        // Build detail lines based on phase — wild height swings.
        self.detail_lines.clear();
        let n = match self.phase {
            0 => 0,
            1 => 3,
            2 => 1,
            3 => 5,
            4 => 0,
            5 => 2,
            6 => 6,
            7 => 1,
            _ => 0,
        };
        for i in 0..n {
            self.detail_lines.push(format!(
                "    step {}/{}: {}",
                i + 1,
                n,
                [
                    "scanning...",
                    "compiling...",
                    "linking...",
                    "optimizing...",
                    "bundling...",
                    "uploading..."
                ][i % 6],
            ));
        }
    }

    fn render(&self, idx: usize, r: &mut mage_tui::renderer::Renderer) {
        let color = COLORS[idx % COLORS.len()];
        let frame = SPINNER_FRAMES[self.frame];
        let reset = "\x1b[0m";
        let dim = "\x1b[2m";
        r.push_line(
            format!(
                "  {color}{frame}{reset} \x1b[1m{}\x1b[0m  {dim}(phase {}, {} sub-lines){reset}",
                self.label,
                self.phase,
                self.detail_lines.len()
            ),
        );
        // Detail lines.
        for line in self.detail_lines.iter() {
            r.push_line(
                format!("{dim}{line}{reset}"),
            );
        }
    }
}

struct SpinnersApp {
    spinners: Vec<Spinner>,
    tick: u64,
}

enum Msg {
    Tick,
}

impl App for SpinnersApp {
    type Message = Msg;

    fn render(&mut self, r: &mut mage_tui::renderer::Renderer) {
        r.push_line(
            format!(
                "\x1b[1m\x1b[36m┌─ spinners demo ─┐\x1b[0m  tick {}",
                self.tick,
            ),
        );
        r.push_line("\x1b[2m│\x1b[0m");
        let n = self.spinners.len();
        for (i, spinner) in self.spinners.iter().enumerate() {
            spinner.render(i, r);
            if i < n - 1 {
                r.push_line("\x1b[2m│\x1b[0m");
            }
        }

        r.push_line("\x1b[2m│\x1b[0m");
        r.push_line("\x1b[2m└─ q to quit ─────┘\x1b[0m");
    }

    fn update(&mut self, event: Event<Msg>) -> bool {
        match event {
            Event::Key(key) => match key.code {
                KeyCode::Char('q') | KeyCode::Esc => return true,
                KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => return true,
                _ => {}
            },
            Event::Message(Msg::Tick) => {
                self.tick += 1;
                for spinner in self.spinners.iter_mut() {
                    spinner.tick();
                }
            }
            Event::Resize(_, _) => {}
            _ => {}
        }
        false
    }
}

#[tokio::main]
async fn main() {
    let (tx, rx) = mpsc::channel(64);

    tokio::spawn(async move {
        loop {
            tokio::time::sleep(std::time::Duration::from_millis(16)).await;
            if tx.send(Msg::Tick).await.is_err() {
                break;
            }
        }
    });

    let app = SpinnersApp {
        spinners: vec![
            Spinner::new("Installing dependencies"),
            Spinner::new("Building workspace"),
            Spinner::new("Running tests"),
            Spinner::new("Deploying artifacts"),
            Spinner::new("Verifying health checks"),
            Spinner::new("Cleaning up"),
        ],
        tick: 0,
    };

    run_with_messages(app, rx).await;
}
