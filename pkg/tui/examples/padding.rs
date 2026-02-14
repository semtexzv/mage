//! Padding showcase — left, right, horizontal on every element type.
//! Resize-aware TUI. Press q or Esc to quit.

use tau_tui_next::renderer::Renderer;
use tau_tui_next::style::{Color, Style};
use tau_tui_next::ansi::visible_width;
use tau_tui_next::*;

// ── Constants ───────────────────────────────────────────────────

const PROSE: &str = "The quick brown fox jumps over the lazy dog. \
Pack my box with five dozen liquor jugs. \
How vexingly quick daft zebras jump.";

const TABLE_MD: &str = "\
| Feature | Status | Notes |
|:--------|:------:|------:|
| Alpha | ✅ | works great |
| Beta | ⚠️ | wip |
| Gamma | ❌ | todo |";

const LIST_MD: &str = "\
- First item with **bold** text
- Second item with `inline code`
  - Nested child
- Third item with *italic*";

const CODE_MD: &str = "\
```rust
fn main() {
    println!(\"Hello padding!\");
}
```";

// ── Build all sections into lines ───────────────────────────────

fn build_sections(outer: u16) -> Vec<String> {
    let ow = outer as usize;
    let mut views: Vec<String> = Vec::new();

    let section = |views: &mut Vec<String>, title: &str| {
        let s = Style::new().bold().fg(Color::Yellow).to_sgr();
        views.push(format!("{s}── {title} ──{RESET}"));
        views.push(String::new());
    };

    let box_label = |views: &mut Vec<String>, label: &str, lines: &[String]| {
        let dim = "\x1b[2m";
        let rst = "\x1b[0m";
        let bar = "─".repeat(ow);
        views.push(format!("{dim}┌{bar}┐{rst}"));
        views.push(format!(
            "{dim}│{rst}\x1b[1m\x1b[36m{:^ow$}{rst}{dim}│{rst}",
            label
        ));
        views.push(format!("{dim}├{bar}┤{rst}"));
        for l in lines {
            let vw = visible_width(l);
            let fill = ow.saturating_sub(vw);
            views.push(format!(
                "{dim}│{rst}{l}{}{dim}│{rst}",
                " ".repeat(fill)
            ));
        }
        views.push(format!("{dim}└{bar}┘{rst}"));
        views.push(String::new());
    };

    // Helper: render text_styled and extract lines
    let render_styled = |content: &str, pad: Padding, bg: Option<Color>| -> Vec<String> {
        let mut r = Renderer::new();
        let mut term = tau_tui_next::testutil::TestTerminal::new(outer, 200);
        r.begin_frame(outer, 200);
        r.push_text_styled(content, &pad, bg);
        r.end_frame(&mut term);
        r.prev_lines.iter().map(|l| l.to_string()).collect()
    };

    // Helper: render Markdown and extract lines
    let render_md = |src: &str, pad: Padding| -> Vec<String> {
        let mut md = Markdown::with_pad(outer, pad);
        md.append(src);
        md.lines().iter().map(|l| l.to_string()).collect()
    };

    // Title
    let title = Style::new().bold().fg(Color::Yellow).to_sgr();
    let w = ow.min(60);
    let bar = "═".repeat(w);
    views.push(format!("{title}╔{bar}╗{RESET}"));
    views.push(format!(
        "{title}║{:^w$}║{}",
        "Padding Showcase",
        RESET
    ));
    views.push(format!("{title}╚{bar}╝{RESET}"));
    views.push(String::new());

    // 1. Text — native padding
    section(&mut views, "1. Text — native padding (word-wrapped)");
    for &(label, pad) in &[
        ("no padding", Padding::ZERO),
        ("Padding::left(6)", Padding::left(6)),
        ("Padding::horizontal(5)", Padding::horizontal(5)),
        ("Padding::all(2)", Padding::all(2)),
    ] {
        let lines = render_styled(PROSE, pad, None);
        box_label(&mut views, label, &lines);
    }

    // 2. text_styled with bg
    section(
        &mut views,
        "2. text_styled — padding + background fill",
    );
    let bg = Some(Color::Rgb(30, 30, 55));
    for &(label, pad) in &[
        ("bg, no padding", Padding::ZERO),
        ("bg + Padding::left(4)", Padding::left(4)),
        ("bg + Padding::horizontal(4)", Padding::horizontal(4)),
        ("bg + Padding::new(1,3,1,3)", Padding::new(1, 3, 1, 3)),
    ] {
        let lines = render_styled(PROSE, pad, bg);
        box_label(&mut views, label, &lines);
    }

    // 3. Markdown prose
    section(&mut views, "3. Markdown::with_pad — paragraphs + inline styles");
    let md_prose = "Here is **bold**, *italic*, `code`, and ~~strikethrough~~. \
Links work too: [Rust](https://www.rust-lang.org). Pack my box \
with five dozen liquor jugs.";
    for &(label, pad) in &[
        ("no padding", Padding::ZERO),
        ("Padding::left(6)", Padding::left(6)),
        ("Padding::horizontal(4)", Padding::horizontal(4)),
    ] {
        let lines = render_md(md_prose, pad);
        box_label(&mut views, label, &lines);
    }

    // 4. Markdown tables
    section(&mut views, "4. Markdown::with_pad — tables");
    for &(label, pad) in &[
        ("no padding", Padding::ZERO),
        ("Padding::left(4)", Padding::left(4)),
        ("Padding::horizontal(6)", Padding::horizontal(6)),
    ] {
        let lines = render_md(TABLE_MD, pad);
        box_label(&mut views, label, &lines);
    }

    // 5. Markdown lists
    section(&mut views, "5. Markdown::with_pad — lists");
    for &(label, pad) in &[
        ("no padding", Padding::ZERO),
        ("Padding::left(4)", Padding::left(4)),
        ("Padding::horizontal(6)", Padding::horizontal(6)),
    ] {
        let lines = render_md(LIST_MD, pad);
        box_label(&mut views, label, &lines);
    }

    // 6. Markdown code blocks
    section(&mut views, "6. Markdown::with_pad — code blocks");
    for &(label, pad) in &[
        ("no padding", Padding::ZERO),
        ("Padding::left(6)", Padding::left(6)),
        ("Padding::horizontal(8)", Padding::horizontal(8)),
    ] {
        let lines = render_md(CODE_MD, pad);
        box_label(&mut views, label, &lines);
    }

    // 7. Mixed: markdown inside bg container
    section(
        &mut views,
        "7. Mixed — Markdown inside bg-filled container",
    );
    {
        let bg_style = Style::new().bg(Color::Rgb(25, 25, 45)).to_sgr();
        let md_pad = Padding::horizontal(2);
        let mut md = Markdown::with_pad(outer, md_pad);
        md.append("## Status Report\n\n");
        md.append(TABLE_MD);
        md.append("\n\n> All systems nominal.\n");
        let md_lines: Vec<String> = md.lines().iter().map(|l| l.to_string()).collect();

        let mut bg_lines = Vec::new();
        bg_lines.push(format!("{bg_style}{}{RESET}", " ".repeat(ow)));
        for l in &md_lines {
            let vw = visible_width(l);
            let fill = ow.saturating_sub(vw);
            bg_lines.push(format!("{bg_style}{l}{}{RESET}", " ".repeat(fill)));
        }
        bg_lines.push(format!("{bg_style}{}{RESET}", " ".repeat(ow)));
        box_label(
            &mut views,
            "bg + Markdown::with_pad(horizontal(2))",
            &bg_lines,
        );
    }

    // 8. Input
    section(&mut views, "8. Input — padding via prompt prefix");
    for &(label, prompt, content) in &[
        ("no padding", "> ", "hello world"),
        ("left=4 via prompt", "    > ", "hello world"),
        ("left=8 via prompt", "        > ", "hello world"),
    ] {
        let lines = {
            let mut r = Renderer::new();
            let mut term = tau_tui_next::testutil::TestTerminal::new(outer, 200);
            r.begin_frame(outer, 200);
            r.push_input(prompt, content, content.len());
            r.end_frame(&mut term);
            r.prev_lines.iter().map(|l| l.to_string()).collect::<Vec<_>>()
        };
        box_label(&mut views, label, &lines);
    }

    // Footer
    views.push(
        "\x1b[2mPress q or Esc to quit\x1b[0m".to_string(),
    );

    views
}

// ── App ─────────────────────────────────────────────────────────

struct PaddingApp {
    sections: Vec<String>,
    keymap: Keymap<Act>,
}

#[derive(Clone)]
enum Act { Quit }
enum Msg {}

impl App for PaddingApp {
    type Message = Msg;

    fn render(&mut self, r: &mut renderer::Renderer) {
        for line in &self.sections {
            r.push_line(line.as_str());
        }
    }

    fn update(&mut self, event: Event<Msg>) -> bool {
        match event {
            Event::Key(k) => {
                if let Some(Act::Quit) = self.keymap.lookup(&k) {
                    return true;
                }
            }
            Event::Resize(w, _h) => {
                self.sections = build_sections(w);
            }
            Event::Message(_) => {}
            _ => {}
        }
        false
    }
}

#[tokio::main]
async fn main() {
    let width = crossterm::terminal::size().map(|(w, _)| w).unwrap_or(80);

    run(PaddingApp {
        sections: build_sections(width),
        keymap: Keymap::from([
            (ch('q'), Act::Quit),
            (ESC, Act::Quit),
            (ctrl(ch('c')), Act::Quit),
        ]),
    })
    .await;
}
