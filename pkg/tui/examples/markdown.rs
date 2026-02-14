//! Renders a markdown document in a TUI. Resize-aware.
//! Press q or Esc to quit.

use mage_tui::*;

const SAMPLE: &str = r#"# Markdown Renderer

Paragraph with **bold**, *italic*, `code`, and ~~strikethrough~~.
A [link](https://example.com) and an autolink: <https://rust-lang.org>.

## Code

```rust
fn main() {
    println!("Hello!");
}
```

## Lists

- Unordered **bold** item
  - Nested
- [x] Task done
- [ ] Task pending

1. First
2. Second

## Aligned Table

| Test Case          |  Time (ms) | Allocs | Status |
|:-------------------|-----------:|-------:|:------:|
| Parse 1KB doc      |       0.42 |     12 |   ✅   |
| Parse 100KB doc    |       3.81 |     84 |   ✅   |
| Incremental append |       0.03 |      2 |   ✅   |
| Full re-render     |       1.22 |     31 |   ✅   |

## Wide Table

| Feature     | Description                                              | Since |
|:------------|:---------------------------------------------------------|------:|
| Headings    | h1 bold+underline, h2 bold, h3+ with `###` prefix       |  v0.1 |
| Bold/Italic | Nested `**bold *italic***` with SGR toggle codes         |  v0.1 |
| Tables      | Box drawing, column alignment, cell wrapping, shrinking  |  v0.2 |

## Blockquote

> "Simplicity is the ultimate sophistication."
> — Leonardo da Vinci

---

*Done.*
"#;

struct MdApp {
    md: Markdown,
    keymap: Keymap<Act>,
}

#[derive(Clone)]
enum Act { Quit }
enum Msg {}

impl App for MdApp {
    type Message = Msg;

    fn render(&mut self, r: &mut renderer::Renderer) {
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
            Event::Resize(w, _h) => {
                self.md.set_width(w);
                self.md.lines();
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
    let mut md = Markdown::new(width);
    md.append(SAMPLE);
    md.lines();

    run(MdApp {
        md,
        keymap: Keymap::from([
            (ch('q'), Act::Quit),
            (ESC, Act::Quit),
            (ctrl(ch('c')), Act::Quit),
        ]),
    })
    .await;
}
