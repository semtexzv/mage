//! Minimal counter — Up/Down to change, q/Esc/Ctrl-C to quit.

use mage_tui::*;
use tokio::sync::mpsc;

struct Counter {
    count: i64,
    ticks: u64,
    keymap: Keymap<Act>,
}

#[derive(Clone)]
enum Act {
    Quit,
    Up,
    Down,
}
enum Msg {
    Tick,
}

impl App for Counter {
    type Message = Msg;

    fn render(&mut self, r: &mut mage_tui::renderer::Renderer) {
        r.push_blank();
        r.push_line(format!("\x1b[1m\x1b[36mcounter demo\x1b[0m  (tick {})", self.ticks));
        r.push_blank();
        r.push_line(format!("  Count: \x1b[1m\x1b[33m{}\x1b[0m", self.count));
        r.push_blank();
        r.push_line("\x1b[2m  Up/Down to change, q to quit\x1b[0m");
    }

    fn update(&mut self, event: Event<Msg>) -> bool {
        match event {
            Event::Key(k) => match self.keymap.lookup(&k) {
                Some(Act::Quit) => return true,
                Some(Act::Up) => self.count += 1,
                Some(Act::Down) => self.count -= 1,
                None => {}
            },
            Event::Message(Msg::Tick) => self.ticks += 1,
            Event::Resize(_, _) => {}
            _ => {}
        }
        false
    }
}

#[tokio::main]
async fn main() {
    let (tx, rx) = mpsc::channel(256);
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
            if tx.send(Msg::Tick).await.is_err() {
                break;
            }
        }
    });
    run_with_messages(
        Counter {
            count: 0,
            ticks: 0,
            keymap: Keymap::from([
                (ch('q'), Act::Quit),
                (ESC, Act::Quit),
                (ctrl(ch('c')), Act::Quit),
                (UP, Act::Up),
                (DOWN, Act::Down),
            ]),
        },
        rx,
    )
    .await;
}
