//! Demonstrates markdown tables with cell wrapping, truncation, padding,
//! background fills, and inline styles at various terminal widths.
//! Resize-aware TUI. Press q or Esc to quit.

use mage_tui::renderer::Renderer;
use mage_tui::style::{Color, Style};
use mage_tui::ansi::visible_width;
use mage_tui::*;

// ── Sample tables ───────────────────────────────────────────────

const STYLED_TABLE: &str = "\
| Component | Status | Notes |
|:----------|:------:|------:|
| **Router** | `active` | handles all routes |
| *Logger* | `active` | ~~disabled~~ enabled |
| **Cache** | `stale` | needs refresh |
| ~~Legacy~~ | `off` | *deprecated* since v2 |";

const WIDE_TABLE: &str = "\
| Feature | Description | Platform | Since |
|:--------|:------------|:--------:|------:|
| Headings | h1 bold+underline, h2 bold, h3+ with ### prefix | All | v0.1 |
| Tables | Box drawing, column alignment, cell wrapping, shrinking | All | v0.2 |
| Code blocks | Fenced with language tag, partial render without closing | All | v0.1 |
| Lists | Ordered, unordered, nested, task lists with checkboxes | All | v0.1 |";

const SMALL_TABLE: &str = "\
| A | B | C |
|---|---|---|
| x | y | z |
| 1 | 2 | 3 |";

// ── Build sections ──────────────────────────────────────────────

fn build_sections(term_w: u16) -> Vec<String> {
    let mut views: Vec<String> = Vec::new();

    let section = |views: &mut Vec<String>, title: &str| {
        views.push(String::new());
        views.push(format!(
            "\x1b[1m\x1b[35m{title}\x1b[0m"
        ));
        views.push(String::new());
    };

    let print_box = |views: &mut Vec<String>, label: &str, lines: &[String], box_w: usize| {
        let bar = "─".repeat(box_w);
        views.push(format!("\x1b[1m\x1b[36m┌{bar}┐\x1b[0m"));
        views.push(format!(
            "\x1b[1m\x1b[36m│\x1b[0m {:^w$} \x1b[1m\x1b[36m│\x1b[0m",
            label,
            w = box_w - 2
        ));
        views.push(format!("\x1b[1m\x1b[36m├{bar}┤\x1b[0m"));
        for l in lines {
            let vw = visible_width(l);
            let fill = box_w.saturating_sub(vw);
            views.push(format!(
                "\x1b[1m\x1b[36m│\x1b[0m{l}{}\x1b[0m\x1b[1m\x1b[36m│\x1b[0m",
                " ".repeat(fill)
            ));
        }
        views.push(format!("\x1b[1m\x1b[36m└{bar}┘\x1b[0m"));
    };

    let render_md = |src: &str, w: u16| -> Vec<String> {
        let mut md = Markdown::new(w);
        md.append(src);
        md.lines().iter().map(|l| l.to_string()).collect()
    };

    let render_md_padded = |src: &str, outer: u16, pad: Padding| -> Vec<String> {
        let mut md = Markdown::with_pad(outer, pad);
        md.append(src);
        md.lines().iter().map(|l| l.to_string()).collect()
    };

    let render_lines = |lines: &[String], w: u16| -> Vec<String> {
        let mut r = Renderer::new();
        let mut term = mage_tui::testutil::TestTerminal::new(w, 200);
        r.begin_frame(w, 200);
        for l in lines {
            r.push_line(l.as_str());
        }
        r.end_frame(&mut term);
        r.prev_lines.iter().map(|l| l.to_string()).collect()
    };

    // Title
    let tw = (term_w as usize).min(64);
    let bar = "═".repeat(tw);
    views.push(format!("\x1b[1m\x1b[33m╔{bar}╗\x1b[0m"));
    views.push(format!(
        "\x1b[1m\x1b[33m║{:^tw$}║\x1b[0m",
        "Styled Tables: Wrapping, Truncation, Padding & Backgrounds"
    ));
    views.push(format!("\x1b[1m\x1b[33m╚{bar}╝\x1b[0m"));

    // 1. Inline styles in cells
    section(&mut views, "1. Inline styles in cells (bold, italic, code, strikethrough)");
    {
        let w = term_w.min(60);
        let lines = render_md(STYLED_TABLE, w);
        print_box(&mut views, &format!("width = {w}"), &lines, w as usize);
    }

    // 2. Cell wrapping at decreasing widths
    section(&mut views, "2. Cell wrapping at decreasing widths");
    for &w in &[
        term_w.min(80),
        term_w.min(55),
        term_w.min(40),
    ] {
        let lines = render_md(WIDE_TABLE, w);
        print_box(&mut views, &format!("width = {w}"), &lines, w as usize);
        views.push(String::new());
    }

    // 3. Extreme narrow fallback
    section(&mut views, "3. Extreme narrow — table falls back to plain text");
    {
        let lines = render_md(WIDE_TABLE, 12);
        let box_w = (term_w as usize).min(40);
        print_box(&mut views, "width = 12 (no borders)", &lines, box_w);
    }

    // 4. Table with padding
    section(&mut views, "4. Table with padding");
    {
        let outer = term_w.min(60);
        for (label, pad) in [
            ("Padding::left(4)", Padding::left(4)),
            ("Padding::horizontal(6)", Padding::horizontal(6)),
            ("Padding::all(2)", Padding::all(2)),
        ] {
            let lines = render_md_padded(STYLED_TABLE, outer, pad);
            print_box(&mut views, label, &lines, outer as usize);
            views.push(String::new());
        }
    }

    // 5. Table with background fill
    section(&mut views, "5. Table with background color");
    {
        let outer = term_w.min(50);
        let pad = Padding::new(1, 2, 1, 2);
        let mut md = Markdown::with_pad(outer, pad);
        md.append(SMALL_TABLE);
        let md_lines: Vec<String> = md.lines().iter().map(|l| l.to_string()).collect();

        let bg_style = Style::new().bg(Color::Rgb(30, 30, 50));
        let bg_start = bg_style.to_sgr();
        let mut styled_lines: Vec<String> = Vec::new();

        for _ in 0..pad.top {
            styled_lines.push(format!(
                "{}{}{}", bg_start, " ".repeat(outer as usize), RESET
            ));
        }
        for l in &md_lines {
            let vw = visible_width(l);
            let fill = (outer as usize).saturating_sub(vw);
            styled_lines.push(format!(
                "{}{}{}{}", bg_start, l, " ".repeat(fill), RESET
            ));
        }
        for _ in 0..pad.bottom {
            styled_lines.push(format!(
                "{}{}{}", bg_start, " ".repeat(outer as usize), RESET
            ));
        }

        let out = render_lines(&styled_lines, outer);
        print_box(
            &mut views,
            "bg = Rgb(30,30,50), pad = (1,2,1,2)",
            &out,
            outer as usize,
        );
    }

    // 6. Composed dashboard
    section(&mut views, "6. Composed: styled heading + table + footer");
    {
        let outer = term_w.min(56);
        let accent = Style::new().bold().fg(Color::Yellow);
        let dim = Style::new().dim();
        let success = Style::new().fg(Color::Green);

        let heading = format!(
            "{}  Component Status Dashboard{}",
            accent.to_sgr(),
            RESET
        );

        let mut md = Markdown::with_pad(outer, Padding::horizontal(2));
        md.append(STYLED_TABLE);
        let md_lines: Vec<String> = md.lines().iter().map(|l| l.to_string()).collect();

        let success_dot = format!("{}●{}", success.to_sgr(), RESET);
        let red_dot = format!(
            "{}●{}",
            Style::new().fg(Color::Red).to_sgr(),
            RESET
        );
        let footer = format!(
            "{}  {success_dot} 3 active   {red_dot} 1 deprecated{}",
            dim.to_sgr(),
            RESET
        );

        let mut children: Vec<String> = Vec::new();
        children.push(String::new());
        children.push(heading);
        children.push(String::new());
        for l in md_lines {
            children.push(l);
        }
        children.push(footer);
        children.push(String::new());

        let out = render_lines(&children, outer);
        print_box(
            &mut views,
            "Dashboard with styled header/footer",
            &out,
            outer as usize,
        );
    }

    // 7. Same table at 3 padding levels
    section(&mut views, "7. Same table at 3 padding levels");
    {
        let outer = term_w.min(44);
        for (label, pad) in [
            ("no padding", Padding::ZERO),
            ("Padding::left(4)", Padding::left(4)),
            ("Padding::horizontal(6)", Padding::horizontal(6)),
        ] {
            let lines = render_md_padded(SMALL_TABLE, outer, pad);
            print_box(&mut views, label, &lines, outer as usize);
            views.push(String::new());
        }
    }

    // 8. Aggressive wrapping
    section(&mut views, "8. Aggressive wrapping: wide table in 30-col container");
    {
        let w = term_w.min(30);
        let lines = render_md(WIDE_TABLE, w);
        print_box(&mut views, &format!("width = {w}"), &lines, w as usize);
    }

    // Footer
    views.push(String::new());
    views.push(
        "\x1b[2mPress q or Esc to quit\x1b[0m".to_string(),
    );

    views
}

// ── App ─────────────────────────────────────────────────────────

struct StyledTablesApp {
    sections: Vec<String>,
    keymap: Keymap<Act>,
}

#[derive(Clone)]
enum Act { Quit }
enum Msg {}

impl App for StyledTablesApp {
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

    run(StyledTablesApp {
        sections: build_sections(width),
        keymap: Keymap::from([
            (ch('q'), Act::Quit),
            (ESC, Act::Quit),
            (ctrl(ch('c')), Act::Quit),
        ]),
    })
    .await;
}
