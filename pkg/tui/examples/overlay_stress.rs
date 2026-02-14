//! Overlay compositing stress test.
//!
//! Base content has aggressive styling: bold, italic, colored backgrounds,
//! foreground colors on every line. An overlay popup is composited on top
//! and must render cleanly without inheriting any base styles.
//!
//! Press q or Esc to quit. Up/Down navigates the overlay.

use tau_tui_next::style::{Color, Style};
use tau_tui_next::overlay::{render_select_list, SelectItem, SelectList};
use tau_tui_next::*;

const RST: &str = "\x1b[0m";

// ── Styled base lines ──────────────────────────────────────────

/// Generate aggressively styled base content: every line has a different
/// combination of bold, italic, dim, underline, strikethrough, fg, bg.
fn styled_base_lines(width: usize) -> Vec<String> {
    let combos: &[(Style, &str)] = &[
        (Style::new().bold().fg(Color::Red).bg(Color::Rgb(40, 0, 0)),
         "BOLD RED on dark-red bg "),
        (Style::new().italic().fg(Color::Green).bg(Color::Rgb(0, 30, 0)),
         "ITALIC GREEN on dark-green bg "),
        (Style::new().bold().italic().fg(Color::Yellow).bg(Color::Rgb(40, 40, 0)),
         "BOLD+ITALIC YELLOW on dark-yellow bg "),
        (Style::new().underline().fg(Color::Blue).bg(Color::Rgb(0, 0, 50)),
         "UNDERLINE BLUE on dark-blue bg "),
        (Style::new().dim().fg(Color::Magenta).bg(Color::Rgb(30, 0, 30)),
         "DIM MAGENTA on dark-magenta bg "),
        (Style::new().strikethrough().fg(Color::Cyan).bg(Color::Rgb(0, 30, 30)),
         "STRIKETHROUGH CYAN on dark-cyan bg "),
        (Style::new().bold().underline().fg(Color::Rgb(255, 128, 0)).bg(Color::Rgb(40, 20, 0)),
         "BOLD+UL ORANGE on dark-orange bg "),
        (Style::new().bold().italic().underline().fg(Color::Rgb(200, 200, 255)).bg(Color::Rgb(20, 20, 60)),
         "BOLD+IT+UL LAVENDER on navy bg "),
        (Style::new().bold().dim().fg(Color::White).bg(Color::Rgb(50, 50, 50)),
         "BOLD+DIM WHITE on gray bg "),
        (Style::new().italic().strikethrough().fg(Color::Rgb(255, 100, 200)).bg(Color::Rgb(40, 10, 30)),
         "IT+STRIKE PINK on dark-pink bg "),
    ];

    let mut lines = Vec::new();
    for (i, (style, label)) in combos.iter().enumerate() {
        let sgr = style.to_sgr();
        // Fill the full width with styled content
        let repeat_count = (width / label.len()).max(1);
        let content: String = label.repeat(repeat_count);
        let truncated = &content[..content.len().min(width)];
        lines.push(format!("{sgr}{truncated}{RST}"));

        // Also push a "mixed" line: starts with one style, changes mid-line
        if i + 1 < combos.len() {
            let (next_style, next_label) = &combos[i + 1];
            let half = width / 2;
            let left_text = &label.repeat(repeat_count)[..half.min(label.len() * repeat_count)];
            let right_text = &next_label.repeat(repeat_count)[..half.min(next_label.len() * repeat_count)];
            lines.push(format!(
                "{sgr}{left_text}{}{right_text}{RST}",
                next_style.to_sgr()
            ));
        }
    }
    lines
}

// ── App ─────────────────────────────────────────────────────────

struct OverlayStressApp {
    base_lines: Vec<String>,
    overlay: SelectList,
    width: u16,
    keymap: Keymap<Act>,
}

#[derive(Clone)]
enum Act {
    Quit,
}

impl App for OverlayStressApp {
    type Message = ();

    fn render(&mut self, r: &mut renderer::Renderer) {
        let w = r.width() as usize;

        // ── Title ──
        let title_style = Style::new().bold().fg(Color::Yellow);
        r.push_line(format!(
            "{}  Overlay Compositing Stress Test{RST}",
            title_style.to_sgr()
        ));
        r.push_line(format!(
            "  {}Base has aggressive styling; overlay must be clean.{RST}",
            Style::new().dim().to_sgr()
        ));
        r.push_blank();

        // ── Styled base content ──
        let base_start = r.line_count();
        for line in &self.base_lines {
            r.push_line(line.as_str());
        }

        // ── Overlay ──
        // Composite the overlay in the middle of the styled content
        let overlay_w = w.min(50);
        let overlay_col = 4; // slight indent
        let overlay_lines = render_select_list(&self.overlay, overlay_w);
        let overlay_start_row = base_start + 2; // start a few lines into the styled content

        for (i, line) in overlay_lines.iter().enumerate() {
            r.composite_at(overlay_start_row + i, overlay_col, line, overlay_w);
        }

        // ── Footer ──
        r.push_blank();
        r.push_line(format!(
            "  {}Up/Down to navigate overlay.  q / Esc to quit.{RST}",
            Style::new().dim().to_sgr()
        ));
    }

    fn update(&mut self, event: Event<()>) -> bool {
        match event {
            Event::Key(k) => {
                if let Some(Act::Quit) = self.keymap.lookup(&k) {
                    return true;
                }
                use crossterm::event::KeyCode;
                match k.code {
                    KeyCode::Up => { self.overlay.move_up(); }
                    KeyCode::Down => { self.overlay.move_down(); }
                    _ => {}
                }
            }
            Event::Resize(w, _h) => {
                self.width = w;
                self.base_lines = styled_base_lines(w as usize);
            }
            _ => {}
        }
        false
    }
}

#[tokio::main]
async fn main() {
    let (w, _h) = crossterm::terminal::size().unwrap_or((80, 24));

    let mut overlay = SelectList::new(vec![
        SelectItem::new("alpha",    "First item — should be clean"),
        SelectItem::new("beta",     "Second item — no base bleed"),
        SelectItem::new("gamma",    "Third item — styles isolated"),
        SelectItem::new("delta",    "Fourth item — bg is overlay's own"),
        SelectItem::new("epsilon",  "Fifth item — fg is overlay's own"),
        SelectItem::new("zeta",     "Sixth item — no underline leak"),
        SelectItem::new("eta",      "Seventh item — no bold leak"),
        SelectItem::new("theta",    "Eighth item — no italic leak"),
    ]);
    overlay.max_visible = 6;

    run(OverlayStressApp {
        base_lines: styled_base_lines(w as usize),
        overlay,
        width: w,
        keymap: Keymap::from([
            (ch('q'), Act::Quit),
            (ESC, Act::Quit),
            (ctrl(ch('c')), Act::Quit),
        ]),
    })
    .await;
}
