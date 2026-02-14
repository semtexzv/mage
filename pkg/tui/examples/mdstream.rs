//! Streaming markdown TUI — simulates LLM output arriving token by token.
//!
//! A background task streams a markdown document character by character.
//! The TUI renders it incrementally with full formatting.
//! Press q or Esc to quit.

use tau_tui_next::style::Color;
use tau_tui_next::*;
use tokio::sync::mpsc;

const MD_PAD: Padding = Padding::new(1, 2, 1, 2);
const MD_BG: Color = Color::Rgb(20, 20, 35);

const SAMPLE: &str = r#"# Streaming Markdown Demo

Here is a paragraph with **bold**, *italic*, `inline code`, and ~~strikethrough~~.
Links work too: [Rust](https://www.rust-lang.org).

## Code Block

```rust
fn main() {
    println!("Hello from streaming markdown!");
    let scores = vec![97, 42, 88, 73, 31];
    let avg = scores.iter().sum::<i32>() / scores.len() as i32;
    println!("Average: {avg}");
}
```

## Lists

Unordered with nesting:

- First item
- Second with **bold** and `code`
  - Nested bullet
  - Another nested
- Third item

Ordered:

1. Headings (h1–h6)
2. Inline formatting
3. Fenced code blocks
4. Tables with alignment

Task list:

- [x] Markdown parser
- [x] Incremental rendering
- [x] Table column alignment
- [ ] Syntax highlighting

## Benchmark Results

| Test Case          |  Time (ms) | Allocs | Status |
|:-------------------|-----------:|-------:|:------:|
| Parse 1KB doc      |       0.42 |     12 |   ✅   |
| Parse 100KB doc    |       3.81 |     84 |   ✅   |
| Incremental append |       0.03 |      2 |   ✅   |
| Full re-render     |       1.22 |     31 |   ✅   |
| Table (10 cols)    |       0.89 |     47 |   ⚠️   |

## Wide Table

| Feature     | Description                                              | Since |
|:------------|:---------------------------------------------------------|------:|
| Headings    | h1 bold+underline, h2 bold, h3+ with `###` prefix       |  v0.1 |
| Bold/Italic | Nested `**bold *italic***` with SGR toggle codes         |  v0.1 |
| Code blocks | Fenced with language tag, partial render without closing |  v0.1 |
| Lists       | Ordered, unordered, nested, task lists with checkboxes   |  v0.1 |
| Tables      | Box drawing, column alignment, cell wrapping, shrinking  |  v0.2 |
| Blockquotes | Green `│` border, italic text, nested depth support      |  v0.1 |
| Links       | Blue underline, URL shown if different from text         |  v0.1 |
| Images      | `[img: alt]` placeholder in magenta                      |  v0.1 |

## Blockquote

> "Any sufficiently advanced technology is indistinguishable from magic."
> — Arthur C. Clarke

---

*Stream complete — looping in 2s…*
"#;

struct MdApp {
    md: Markdown,
    round: u64,
    keymap: Keymap<Act>,
}

#[derive(Clone)]
enum Act {
    Quit,
}

enum Msg {
    Chunk(String),
    Reset(u64),
}

impl App for MdApp {
    type Message = Msg;

    fn render(&mut self, r: &mut tau_tui_next::renderer::Renderer) {
        let left = " ".repeat(MD_PAD.left as usize);

        r.push_line(format!(
            "{left}\x1b[1m\x1b[36mstreaming markdown\x1b[0m  round {} · {} bytes",
            self.round,
            self.md.source().len(),
        ));
        r.push_blank();
        self.md.render(r);
        r.push_blank();
        r.push_line(format!("{left}\x1b[2mPress q or Esc to quit\x1b[0m"));
    }

    fn update(&mut self, event: Event<Msg>) -> bool {
        match event {
            Event::Key(k) => {
                if let Some(Act::Quit) = self.keymap.lookup(&k) {
                    return true;
                }
            }
            Event::Message(Msg::Chunk(text)) => {
                self.md.append(&text);
            }
            Event::Message(Msg::Reset(round)) => {
                self.round = round;
                self.md.set_source(String::new());
            }
            Event::Resize(w, _h) => {
                self.md.set_width(w);
            }
            _ => {}
        }
        // Rebuild the line cache so render() has fresh output.
        self.md.lines();
        false
    }
}

#[tokio::main]
async fn main() {
    let (tx, rx) = mpsc::channel::<Msg>(256);

    tokio::spawn(async move {
        let chars: Vec<char> = SAMPLE.chars().collect();
        let mut round = 0u64;
        loop {
            if tx.send(Msg::Reset(round)).await.is_err() {
                break;
            }
            if round > 0 {
                tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            }

            let mut pos = 0;
            while pos < chars.len() {
                let chunk_size = 3 + (pos % 6);
                let end = (pos + chunk_size).min(chars.len());
                let text: String = chars[pos..end].iter().collect();
                if tx.send(Msg::Chunk(text)).await.is_err() {
                    return;
                }
                pos = end;
                tokio::time::sleep(std::time::Duration::from_millis(16)).await;
            }

            round += 1;
            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
        }
    });

    let initial_width = crossterm::terminal::size().map(|(w, _)| w).unwrap_or(80);

    let mut md = Markdown::with_pad(initial_width, MD_PAD);
    md.set_bg(Some(MD_BG));

    run_with_messages(
        MdApp {
            md,
            round: 0,
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
