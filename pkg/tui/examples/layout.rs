//! Layout demo — side-by-side panes with independent animations.
//!
//! Left pane: fixed-width activity log with timestamps.
//! Center pane: flex-width spinner animation.
//! Right pane: fixed-width live stats counter.
//!
//! Demonstrates HStack with Fixed/Flex panes, separators, and
//! pane-level dirty tracking. q/Esc to quit.

use crossterm::event::{KeyCode, KeyModifiers};
use mage_tui::layout::{HStack, PaneSize};
use mage_tui::style::Padding;
use mage_tui::{run_with_messages, App, Event, RESET};

const SPINNER: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
const BOLD: &str = "\x1b[1m";
const DIM: &str = "\x1b[2m";
const CYAN: &str = "\x1b[36m";
const GREEN: &str = "\x1b[32m";
const YELLOW: &str = "\x1b[33m";
const RED: &str = "\x1b[31m";
const MAGENTA: &str = "\x1b[35m";
const BLUE: &str = "\x1b[34m";

struct LayoutApp {
    tick: u64,
    events: Vec<(u64, &'static str, &'static str)>, // (tick, color, message)
    hstack: HStack,
    left: mage_tui::layout::PaneId,
    center: mage_tui::layout::PaneId,
    right: mage_tui::layout::PaneId,
}

enum Msg {
    Tick,
}

impl LayoutApp {
    fn new() -> Self {
        let mut hstack = HStack::new(80);
        hstack.set_separator(Some("│"));
        let left = hstack.pane_with_padding(PaneSize::Fixed(28), Padding::horizontal(1));
        let center = hstack.pane_with_padding(PaneSize::Flex, Padding::horizontal(2));
        let right = hstack.pane_with_padding(PaneSize::Fixed(20), Padding::horizontal(1));

        Self {
            tick: 0,
            events: Vec::new(),
            hstack,
            left,
            center,
            right,
        }
    }

    fn add_event(&mut self) {
        let messages: &[(&str, &str)] = &[
            (GREEN, "Build succeeded"),
            (YELLOW, "Cache miss"),
            (CYAN, "Fetching deps"),
            (RED, "Lint warning"),
            (MAGENTA, "Deploy started"),
            (BLUE, "Test passed"),
            (GREEN, "Health check OK"),
            (YELLOW, "Retry attempt"),
        ];
        let idx = (self.tick / 15) as usize % messages.len();
        let (color, msg) = messages[idx];
        self.events.push((self.tick, color, msg));
        // Keep last 20 events
        if self.events.len() > 20 {
            self.events.remove(0);
        }
    }

    fn rebuild_panes(&mut self) {
        let tick = self.tick;

        // ── Left pane: activity log ──
        let left = self.hstack.get_mut(self.left);
        left.clear();
        left.push_line(format!("{BOLD}{CYAN} Activity Log{RESET}"));
        left.push_line(format!("{DIM} ────────────────────────{RESET}"));
        for (t, color, msg) in &self.events {
            let age = tick - t;
            let age_str = if age < 60 {
                format!("{age:>3}t")
            } else {
                format!("{:>3}m", age / 60)
            };
            left.push_line(format!("{DIM} {age_str}{RESET} {color}●{RESET} {msg}"));
        }

        // ── Center pane: animated display ──
        let center = self.hstack.get_mut(self.center);
        center.clear();

        let frame = SPINNER[tick as usize % SPINNER.len()];
        let phase = (tick / 8) % 4;

        center.push_line(format!("{BOLD}{CYAN} Dashboard{RESET}"));
        center.push_line(format!("{DIM} ─────────────────────────────{RESET}"));
        center.push_blank();

        // Animated bars
        let tasks = [
            ("Compile", GREEN, 0.8 + 0.2 * ((tick as f64 * 0.05).sin())),
            ("Test   ", YELLOW, 0.5 + 0.5 * ((tick as f64 * 0.03).sin())),
            ("Deploy ", MAGENTA, 0.3 + 0.7 * ((tick as f64 * 0.07).sin())),
            ("Monitor", BLUE, 0.6 + 0.4 * ((tick as f64 * 0.04).sin())),
        ];

        for (name, color, progress) in &tasks {
            let bar_w = (center.available_width().saturating_sub(16)) as f64;
            let filled = (bar_w * progress.clamp(0.0, 1.0)) as usize;
            let empty = bar_w as usize - filled;
            let pct = (progress * 100.0) as u8;
            center.push_line(format!(
                "  {name} {color}{}{}  {pct:>3}%{RESET}",
                "█".repeat(filled),
                "░".repeat(empty),
            ));
        }

        center.push_blank();

        let status = match phase {
            0 => format!("  {GREEN}{frame} Running pipeline...{RESET}"),
            1 => format!("  {YELLOW}{frame} Waiting for workers...{RESET}"),
            2 => format!("  {CYAN}{frame} Syncing state...{RESET}"),
            _ => format!("  {MAGENTA}{frame} Optimizing...{RESET}"),
        };
        center.push_line(status);

        // ── Right pane: stats ──
        let right = self.hstack.get_mut(self.right);
        right.clear();
        right.push_line(format!("{BOLD}{CYAN} Stats{RESET}"));
        right.push_line(format!("{DIM} ──────────────────{RESET}"));
        right.push_blank();

        let builds = tick / 20;
        let tests = tick * 3;
        let uptime_s = tick / 60;
        let uptime_m = uptime_s / 60;

        right.push_line(format!(" {DIM}Builds:{RESET}  {GREEN}{builds:>6}{RESET}"));
        right.push_line(format!(" {DIM}Tests:{RESET}   {YELLOW}{tests:>6}{RESET}"));
        right.push_line(format!(" {DIM}Errors:{RESET}  {RED}{:>6}{RESET}", tick % 3));
        right.push_blank();
        right.push_line(format!(" {DIM}Uptime:{RESET}"));
        right.push_line(format!("  {CYAN}{uptime_m:>2}m {s:>2}s{RESET}", s = uptime_s % 60));
        right.push_blank();
        right.push_line(format!(" {DIM}Tick:{RESET} {tick:>8}"));
    }
}

impl App for LayoutApp {
    type Message = Msg;

    fn render(&mut self, r: &mut mage_tui::renderer::Renderer) {
        self.hstack.set_width(r.width() as usize);
        self.rebuild_panes();

        r.push_blank();
        r.push_line(format!(
            " {BOLD}{CYAN}Layout Demo{RESET}  {DIM}(q to quit, resize terminal to see flex){RESET}"
        ));
        r.push_blank();

        r.push_lines(self.hstack.compose());

        r.push_blank();
        r.push_line(format!(
            " {DIM}Panes: left=28 fixed │ center=flex │ right=20 fixed │ total={}{RESET}",
            r.width()
        ));
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
                if self.tick % 15 == 0 {
                    self.add_event();
                }
            }
            _ => {}
        }
        false
    }
}

#[tokio::main]
async fn main() {
    let (tx, rx) = tokio::sync::mpsc::channel(64);

    tokio::spawn(async move {
        loop {
            tokio::time::sleep(std::time::Duration::from_millis(16)).await;
            if tx.send(Msg::Tick).await.is_err() {
                break;
            }
        }
    });

    run_with_messages(LayoutApp::new(), rx).await;
}
