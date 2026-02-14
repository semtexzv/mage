//! Wrap stress test — verifies styled text wraps correctly.
//!
//! Each section shows a styled string wrapped at a narrow width.
//! Continuation lines must keep their formatting (bold, color, etc.).
//!
//! Press q or Esc to quit.

use tau_tui_next::style::{Color, Style};
use tau_tui_next::wrap::wrap_text;
use tau_tui_next::*;

const RST: &str = "\x1b[0m";

struct WrapStressApp {
    keymap: Keymap<Act>,
}

#[derive(Clone)]
enum Act {
    Quit,
}

/// A test case: label, input string, wrap width.
struct Case {
    label: &'static str,
    input: String,
    width: usize,
}

fn cases() -> Vec<Case> {
    let bold = Style::new().bold().to_sgr();
    let red = Style::new().fg(Color::Red).to_sgr();
    let bold_red = Style::new().bold().fg(Color::Red).to_sgr();
    let green = Style::new().fg(Color::Green).to_sgr();
    let ul_blue = Style::new().underline().fg(Color::Blue).to_sgr();
    let bold_ul_yellow = Style::new().bold().underline().fg(Color::Yellow).to_sgr();
    let dim = Style::new().dim().to_sgr();
    let bg = Style::new().bg(Color::Rgb(40, 0, 0)).fg(Color::White).to_sgr();

    vec![
        Case {
            label: "1. All-bold wraps — every line should be bold",
            input: format!("{bold}the quick brown fox jumps over the lazy dog{RST}"),
            width: 20,
        },
        Case {
            label: "2. Style starts after first space — continuation inherits",
            input: format!("plain {bold_red}styled text that wraps around{RST}"),
            width: 18,
        },
        Case {
            label: "3. Style changes mid-word at break — next line has new style",
            input: format!("{bold}hello {red}world beautiful day{RST}"),
            width: 12,
        },
        Case {
            label: "4. Reset at space — continuation should NOT be styled",
            input: format!("{bold_red}hello{RST} world should be plain"),
            width: 15,
        },
        Case {
            label: "5. Multiple style changes across breaks",
            input: format!(
                "{bold}bold {red}red {green}green {ul_blue}underline-blue {bold_ul_yellow}bold-ul-yellow{RST}"
            ),
            width: 14,
        },
        Case {
            label: "6. Long styled word forces mid-word break",
            input: format!("{bold_red}abcdefghijklmnopqrstuvwxyz{RST}"),
            width: 10,
        },
        Case {
            label: "7. Dim text wraps — all lines should be dim",
            input: format!("{dim}this is dim text that should stay dim across all wrapped lines{RST}"),
            width: 22,
        },
        Case {
            label: "8. Background color survives wrap",
            input: format!("{bg}white on dark red background that wraps{RST}"),
            width: 20,
        },
        Case {
            label: "9. Style change right at space boundary",
            input: format!("hello{bold} {red}world again{RST}"),
            width: 8,
        },
        Case {
            label: "10. Alternating styles with spaces",
            input: format!(
                "{bold}A{RST} {red}B{RST} {green}C{RST} {ul_blue}D{RST} {bold_red}E{RST} \
                 {bold}F{RST} {red}G{RST} {green}H{RST} {ul_blue}I{RST} {bold_red}J{RST}"
            ),
            width: 6,
        },
    ]
}

impl App for WrapStressApp {
    type Message = ();

    fn render(&mut self, r: &mut renderer::Renderer) {
        let w = r.width() as usize;
        let title_sgr = Style::new().bold().fg(Color::Yellow).to_sgr();
        let label_sgr = Style::new().bold().fg(Color::Cyan).to_sgr();
        let dim_sgr = Style::new().dim().to_sgr();
        let sep = format!("{dim_sgr}{}{RST}", "─".repeat(w));

        r.push_line(format!("{title_sgr}  Wrap Stress Test{RST}"));
        r.push_line(format!(
            "  {dim_sgr}Continuation lines must keep their styling.{RST}"
        ));
        r.push_blank();

        for case in &cases() {
            r.push_line(format!("{label_sgr}  {}{RST}", case.label));
            r.push_line(format!(
                "  {dim_sgr}width={}, input len={}{RST}",
                case.width,
                case.input.len()
            ));

            let wrapped = wrap_text(&case.input, case.width);
            for (i, line) in wrapped.iter().enumerate() {
                let marker = if i == 0 { "→" } else { "↳" };
                // Show the line with a visible boundary marker
                r.push_line(format!("    {dim_sgr}{marker}{RST} {line}"));
            }
            r.push_line(sep.as_str());
        }

        r.push_blank();
        r.push_line(format!(
            "  {dim_sgr}q / Esc to quit{RST}"
        ));
    }

    fn update(&mut self, event: Event<()>) -> bool {
        if let Event::Key(k) = event {
            if self.keymap.lookup(&k).is_some() {
                return true;
            }
        }
        false
    }
}

#[tokio::main]
async fn main() {
    run(WrapStressApp {
        keymap: Keymap::from([
            (ch('q'), Act::Quit),
            (ESC, Act::Quit),
            (ctrl(ch('c')), Act::Quit),
        ]),
    })
    .await;
}
