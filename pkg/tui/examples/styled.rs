//! Styled boxes scrolling demo — boxes with backgrounds, padding, borders,
//! and markdown tables arrive periodically and scroll upward.
//!
//! Press q or Esc to quit.

use tau_tui_next::style::{Color, Style};
use tau_tui_next::ansi::visible_width;
use tau_tui_next::*;
use tokio::sync::mpsc;

// ── Theme ───────────────────────────────────────────────────────

const RST: &str = "\x1b[0m";
const DIM: &str = "\x1b[2m";
const BOLD: &str = "\x1b[1m";

struct Theme {
    title: Style,
    subtitle: Style,
    status_ok: Style,
    status_warn: Style,
    status_err: Style,
    footer: Style,
    box_bgs: [Color; 4],
}

impl Theme {
    fn new() -> Self {
        Self {
            title: Style::new().bold().fg(Color::Yellow),
            subtitle: Style::new().bold().fg(Color::Cyan),
            status_ok: Style::new().bold().fg(Color::Green),
            status_warn: Style::new().bold().fg(Color::Yellow),
            status_err: Style::new().bold().fg(Color::Red),
            footer: Style::new().dim(),
            box_bgs: [
                Color::Rgb(25, 25, 50),
                Color::Rgb(20, 40, 30),
                Color::Rgb(45, 25, 25),
                Color::Rgb(35, 30, 15),
            ],
        }
    }
}

// ── Styled box builder ──────────────────────────────────────────

/// A rendered box: pre-built lines ready for push_line().
struct StyledBox {
    lines: Vec<String>,
}

/// Build a box with bg fill, padding, and content lines.
fn make_box(width: u16, bg: Color, pad: Padding, content: &[String]) -> StyledBox {
    let w = width as usize;
    let bg_s = Style::new().bg(bg).to_sgr();
    let left = " ".repeat(pad.left as usize);
    let mut lines = Vec::new();

    // top padding
    for _ in 0..pad.top {
        lines.push(format!("{bg_s}{}{}", " ".repeat(w), RESET));
    }

    for l in content {
        let vw = visible_width(l);
        let inner_avail = w.saturating_sub(pad.left as usize).saturating_sub(pad.right as usize);
        let text = if vw > inner_avail {
            tau_tui_next::ansi::truncate_line(l, inner_avail)
        } else {
            l.clone()
        };
        let text_vw = visible_width(&text);
        let fill = w.saturating_sub(pad.left as usize).saturating_sub(text_vw);
        lines.push(format!("{bg_s}{left}{text}{}{}", " ".repeat(fill), RESET));
    }

    // bottom padding
    for _ in 0..pad.bottom {
        lines.push(format!("{bg_s}{}{}", " ".repeat(w), RESET));
    }

    StyledBox { lines }
}

// ── Box content generators ──────────────────────────────────────

fn box_status_card(theme: &Theme, idx: usize, width: u16) -> StyledBox {
    let services = [
        ("api-gateway", "active", 142),
        ("auth-service", "active", 87),
        ("cache-layer", "degraded", 312),
        ("worker-pool", "down", 0),
        ("cdn-edge", "active", 1049),
    ];
    let (name, status, rps) = services[idx % services.len()];

    let status_style = match status {
        "active" => &theme.status_ok,
        "degraded" => &theme.status_warn,
        _ => &theme.status_err,
    };

    let title_s = theme.subtitle.to_sgr();
    let status_s = status_style.to_sgr();

    let content = vec![
        format!("{title_s}  ■ {name}{RST}"),
        format!("  status: {status_s}{status}{RST}   rps: {BOLD}{rps}{RST}"),
    ];

    let bg = theme.box_bgs[idx % theme.box_bgs.len()];
    make_box(width, bg, Padding::new(1, 2, 1, 0), &content)
}

fn box_markdown_table(theme: &Theme, idx: usize, width: u16) -> StyledBox {
    let tables = [
        "| Metric | Value | Trend |\n|:-------|------:|:-----:|\n| Latency | 42ms | ↓ |\n| Throughput | 1.2k | ↑ |\n| Errors | 0.3% | → |",
        "| Region | Load | Health |\n|:-------|-----:|:------:|\n| us-east | 78% | ✅ |\n| eu-west | 62% | ✅ |\n| ap-south | 91% | ⚠️ |",
        "| Task | Progress | ETA |\n|:-----|:---------|----:|\n| Build | ████░░ 67% | 12s |\n| Test | ██████ 100% | — |\n| Deploy | ░░░░░░ 0% | 45s |",
    ];

    let inner_w = width.saturating_sub(4); // 2 left + 2 right padding
    let mut md = Markdown::with_pad(inner_w, Padding::ZERO);
    md.append(tables[idx % tables.len()]);
    let md_lines: Vec<String> = md.lines().iter().map(|l| l.to_string()).collect();

    let bg = theme.box_bgs[(idx + 1) % theme.box_bgs.len()];
    make_box(width, bg, Padding::new(1, 2, 1, 2), &md_lines)
}

fn box_log_entry(theme: &Theme, idx: usize, width: u16) -> StyledBox {
    let logs = [
        ("[INFO]  Connection pool initialized (32 workers)", Color::Cyan),
        ("[WARN]  Cache miss rate above threshold: 23%", Color::Yellow),
        ("[ERROR] Timeout connecting to redis-primary:6379", Color::Red),
        ("[INFO]  Deployment v2.14.3 rolled out successfully", Color::Green),
        ("[DEBUG] GC pause: 12ms (heap: 847MB)", Color::Magenta),
    ];
    let (msg, color) = logs[idx % logs.len()];
    let tag_style = Style::new().bold().fg(color).to_sgr();
    let ts = format!("2026-02-12 02:{:02}:{:02}", idx / 60 % 60, idx % 60);

    let content = vec![
        format!("{DIM}{ts}{RST}  {tag_style}{msg}{RST}"),
    ];

    let bg = theme.box_bgs[(idx + 2) % theme.box_bgs.len()];
    make_box(width, bg, Padding::new(1, 2, 1, 2), &content)
}

fn box_hero_banner(theme: &Theme, width: u16) -> StyledBox {
    let title_s = theme.title.to_sgr();
    let sub_s = theme.subtitle.to_sgr();
    let bar = "═".repeat(width.saturating_sub(6) as usize);
    let iw = width.saturating_sub(6) as usize;
    let content = vec![
        format!("{title_s}╔{bar}╗{RST}"),
        format!("{title_s}║{:^iw$}║{RST}", "Styled Boxes Demo"),
        format!("{title_s}╚{bar}╝{RST}"),
        String::new(),
        format!("{sub_s}  Boxes with backgrounds, padding, borders, and markdown tables{RST}"),
        format!("{sub_s}  scroll upward as new content arrives every 800ms.{RST}"),
    ];
    let bg = Color::Rgb(20, 20, 40);
    make_box(width, bg, Padding::new(1, 2, 1, 2), &content)
}

// ── App ─────────────────────────────────────────────────────────

struct StyledApp {
    boxes: Vec<StyledBox>,
    theme: Theme,
    width: u16,
    tick: u64,
    keymap: Keymap<Act>,
}

#[derive(Clone)]
enum Act {
    Quit,
}

enum Msg {
    AddBox,
}

impl StyledApp {
    fn generate_box(&self) -> StyledBox {
        let box_w = self.width.min(70);
        match self.tick % 3 {
            0 => box_status_card(&self.theme, self.tick as usize, box_w),
            1 => box_markdown_table(&self.theme, self.tick as usize, box_w),
            _ => box_log_entry(&self.theme, self.tick as usize, box_w),
        }
    }
}

impl App for StyledApp {
    type Message = Msg;

    fn render(&mut self, r: &mut renderer::Renderer) {
        let footer_s = self.theme.footer.to_sgr();

        // Header
        r.push_line(format!(
            "{}  styled boxes{RST}  {DIM}tick {}{RST}  {DIM}{} boxes{RST}",
            self.theme.title.to_sgr(),
            self.tick,
            self.boxes.len(),
        ));
        r.push_blank();

        // All boxes
        for b in &self.boxes {
            for l in &b.lines {
                r.push_line(l.as_str());
            }
            r.push_blank();
        }

        // Footer
        r.push_line(format!("{footer_s}  Press q or Esc to quit{RST}"));
    }

    fn update(&mut self, event: Event<Msg>) -> bool {
        match event {
            Event::Key(k) => {
                if let Some(Act::Quit) = self.keymap.lookup(&k) {
                    return true;
                }
            }
            Event::Message(Msg::AddBox) => {
                self.tick += 1;
                let b = self.generate_box();
                self.boxes.push(b);
                // Keep last ~30 boxes to prevent unbounded growth
                if self.boxes.len() > 30 {
                    self.boxes.drain(..self.boxes.len() - 30);
                }
            }
            Event::Resize(w, _h) => {
                self.width = w;
                // Rebuild hero + existing boxes at new width
                let box_w = w.min(70);
                let hero = box_hero_banner(&self.theme, box_w);
                self.boxes.clear();
                self.boxes.push(hero);
            }
            _ => {}
        }
        false
    }
}

#[tokio::main]
async fn main() {
    let (tx, rx) = mpsc::channel::<Msg>(64);

    tokio::spawn(async move {
        loop {
            tokio::time::sleep(std::time::Duration::from_millis(800)).await;
            if tx.send(Msg::AddBox).await.is_err() {
                break;
            }
        }
    });

    let width = crossterm::terminal::size().map(|(w, _)| w).unwrap_or(80);
    let theme = Theme::new();
    let box_w = width.min(70);
    let hero = box_hero_banner(&theme, box_w);

    run_with_messages(
        StyledApp {
            boxes: vec![hero],
            theme,
            width,
            tick: 0,
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
