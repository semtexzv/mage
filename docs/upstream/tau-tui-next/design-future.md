> **Note:** This document describes a planned future architecture.
> It is NOT implemented in the current codebase.
> See the source code and `FIXES.md` for the current state of the API.

# Design: Push-Based Renderer

## Core Insight

Components emit **final decorated lines** inside `cached()` blocks. On cache
hit: zero allocation, zero formatting. The `format!()` cost is paid once per
content/width/theme change, never per frame.

The renderer is thin: width tracker + theme holder + cache + terminal differ.
No style stack. No per-line decoration. Components own their layout and style.

## Responsibilities

```
Component                          Renderer
─────────                          ────────
Owns content                       Owns terminal width
Owns local layout (padding)        Owns global layout (sub_render)
Knows its style markers            Owns theme (resolves markers → colors)
Emits final decorated lines        Caches lines (id + hash + width + theme)
Calls r.width(), r.theme()         Diffs prev vs current, writes terminal
```

## Renderer

```rust
pub struct Renderer {
    // ── Frame ──
    output: Vec<Line>,
    width: u16,               // current available width (narrowed by sub_render)
    cursor: Option<CursorPos>,
    seen: HashSet<Id>,

    // ── Cache ──
    cache: HashMap<Id, CachedEntry>,

    // ── Theme ──
    theme: Theme,
    theme_version: u64,

    // ── Terminal state ──
    prev_lines: Vec<Line>,
    prev_width: u16,
    hw_cursor_row: usize,
    cursor_visible: bool,
    // ...
}

struct CachedEntry {
    hash: u64,
    width: u16,
    theme: u64,
    lines: Vec<Line>,
}
```

### Public API

```rust
impl Renderer {
    // ── Context ──

    /// Available width. Narrowed by sub_render(), not by components.
    pub fn width(&self) -> u16;

    /// Current theme. Components read this for color resolution.
    pub fn theme(&self) -> &Theme;

    // ── Pushing lines ──

    /// Push a fully-decorated line. Truncated to width(). No other processing.
    pub fn raw(&mut self, line: impl Into<Line>);

    /// Push N blank lines.
    pub fn spacer(&mut self, n: u16);

    /// Set cursor position (row = output.len(), col given).
    pub fn set_cursor(&mut self, col: usize);

    // ── Caching ──

    /// Cache-or-rebuild. Key: (id, hash, width, theme_version).
    /// On hit: replay stored Rc<str>. On miss: call build, cache output.
    pub fn cached(&mut self, id: impl Into<Id>, hash: u64, build: impl FnOnce(&mut Self));

    // ── Composition ──

    /// Render into temp buffer at given width. Cache shared.
    pub fn sub_render(&mut self, width: u16, build: impl FnOnce(&mut Self)) -> Vec<Line>;

    // ── Formatting helpers ──

    /// Wrap content in SGR codes resolved from a style marker.
    pub fn styled(&self, content: &str, marker: Marker) -> String;

    /// Build a bg-filled blank line at given width.
    pub fn blank(&self, width: u16, marker: Marker) -> String;

    /// Build left-pad prefix: bg spaces.
    pub fn left_pad(&self, n: u8, marker: Marker) -> String;

    /// Build right-fill suffix: bg spaces to fill remaining width.
    pub fn right_fill(&self, content_w: usize, total_w: usize, marker: Marker) -> String;
}
```

`raw()` is the only way to push a line. No `emit()`, no decoration layer.
The line goes into `self.output` as-is (truncated to `width()`). Components
are responsible for producing the final string.

### Formatting helpers

Components call these inside `cached()` to build decorated strings.
They resolve style markers through the theme — components never write
raw SGR codes for themed colors.

```rust
impl Renderer {
    pub fn styled(&self, content: &str, marker: Marker) -> String {
        let s = self.theme.resolve(marker);
        let mut out = String::new();
        if let Some(bg) = s.bg { out.push_str(&bg.bg_sgr()); }
        if let Some(fg) = s.fg { out.push_str(&fg.fg_sgr()); }
        if s.bold { out.push_str("\x1b[1m"); }
        if s.dim { out.push_str("\x1b[2m"); }
        if s.italic { out.push_str("\x1b[3m"); }
        out.push_str(content);
        out.push_str("\x1b[0m");
        out
    }

    pub fn left_pad(&self, n: u8, marker: Marker) -> String {
        if n == 0 { return String::new(); }
        let s = self.theme.resolve(marker);
        match s.bg {
            Some(c) => format!("{}{}\x1b[0m", c.bg_sgr(), " ".repeat(n as usize)),
            None => " ".repeat(n as usize),
        }
    }

    pub fn right_fill(&self, content_w: usize, total_w: usize, marker: Marker) -> String {
        let fill = total_w.saturating_sub(content_w);
        if fill == 0 { return String::new(); }
        let s = self.theme.resolve(marker);
        match s.bg {
            Some(c) => format!("{}{}\x1b[0m", c.bg_sgr(), " ".repeat(fill)),
            None => String::new(),  // no bg → no fill (width constraint only)
        }
    }

    pub fn blank(&self, width: u16, marker: Marker) -> String {
        let s = self.theme.resolve(marker);
        match s.bg {
            Some(c) => format!("{}{}\x1b[0m", c.bg_sgr(), " ".repeat(width as usize)),
            None => String::new(),
        }
    }
}
```

No bg → `right_fill` returns empty, `blank` returns empty, `left_pad`
returns plain spaces. Same "padding is a width constraint" rule as before.
Bg fill only materializes when the theme gives the marker a background.

### `cached()`

```rust
pub fn cached(&mut self, id: impl Into<Id>, hash: u64, build: impl FnOnce(&mut Self)) {
    let id = id.into();
    self.seen.insert(id);

    if let Some(e) = self.cache.get(&id) {
        if e.hash == hash && e.width == self.width && e.theme == self.theme_version {
            self.output.extend(e.lines.iter().cloned());  // Rc::clone
            return;
        }
    }

    let start = self.output.len();
    build(self);
    let lines = self.output[start..].to_vec();
    self.cache.insert(id, CachedEntry {
        hash, width: self.width, theme: self.theme_version, lines,
    });
}
```

On cache hit: replays the same `Rc<str>` objects. `Rc::ptr_eq` works
in the differ → no terminal repaint. Zero allocation.

On cache miss: `build()` calls `r.raw()` which pushes new `Rc<str>`.
These are captured and stored. Next frame they'll be replayed.

### `sub_render()`

```rust
pub fn sub_render(&mut self, width: u16, build: impl FnOnce(&mut Self)) -> Vec<Line> {
    let saved = (std::mem::take(&mut self.output), self.width);
    self.width = width;
    build(self);
    let result = std::mem::replace(&mut self.output, saved.0);
    self.width = saved.1;
    result
}
```

Narrows width. Cache shared (same `self`). Used by `hstack()`, `bordered()`,
`overlay()` — all free functions.

## Style Markers + Theme

### Markers

Semantic roles, not colors. Components say "I'm secondary", theme says what that looks like.

```rust
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Marker {
    Default,       // normal text
    Primary,       // main accent (headings, active items)
    Secondary,     // subtle accent (borders, metadata)
    Muted,         // dim text (hints, disabled)
    Surface,       // content area bg
    SurfaceAlt,    // alternate bg (code blocks, alternating rows)
    Success,       // green
    Warning,       // yellow
    Error,         // red
}
```

### Resolved style

```rust
#[derive(Clone, Copy, Default, PartialEq, Eq, Hash, Debug)]
pub struct Resolved {
    pub fg: Option<Color>,
    pub bg: Option<Color>,
    pub bold: bool,
    pub dim: bool,
    pub italic: bool,
}
```

### Theme

```rust
pub struct Theme {
    map: HashMap<Marker, Resolved>,
}

impl Theme {
    pub fn resolve(&self, marker: Marker) -> Resolved {
        self.map.get(&marker).copied().unwrap_or_default()
    }
}

impl Default for Theme {
    fn default() -> Self {
        // Dark theme
        let mut m = HashMap::new();
        m.insert(Marker::Primary, Resolved { fg: Some(Color::CYAN), bold: true, ..default() });
        m.insert(Marker::Secondary, Resolved { fg: Some(Color::Idx(244)), ..default() });
        m.insert(Marker::Muted, Resolved { dim: true, ..default() });
        m.insert(Marker::SurfaceAlt, Resolved { bg: Some(Color::Idx(236)), ..default() });
        m.insert(Marker::Success, Resolved { fg: Some(Color::GREEN), ..default() });
        m.insert(Marker::Warning, Resolved { fg: Some(Color::YELLOW), ..default() });
        m.insert(Marker::Error, Resolved { fg: Some(Color::RED), bold: true, ..default() });
        Theme { map: m }
    }
}
```

Theme change → bump `theme_version` → all `cached()` entries miss → rebuild.

## Components

Components own content + local layout + style marker. They read `r.width()`
and `r.theme()`, emit final decorated lines inside `r.cached()`.

### Text

```rust
pub struct Text {
    id: Id,
    content: String,
    pad: Padding,
    marker: Marker,
}

impl Text {
    pub fn new(id: impl Into<Id>, content: impl Into<String>) -> Self {
        Self { id: id.into(), content: content.into(), pad: Padding::ZERO, marker: Marker::Default }
    }
    pub fn pad(mut self, p: Padding) -> Self { self.pad = p; self }
    pub fn marker(mut self, m: Marker) -> Self { self.marker = m; self }
    pub fn set(&mut self, content: impl Into<String>) { self.content = content.into(); }

    pub fn render(&self, r: &mut Renderer) {
        let h = hash(&(&self.content, &self.pad, &self.marker));
        let outer_w = r.width();
        let inner_w = outer_w.saturating_sub(self.pad.h() as u16) as usize;
        let pad = self.pad;
        let marker = self.marker;

        r.cached(self.id, h, |r| {
            let lp = r.left_pad(pad.left, marker);
            let blank = r.blank(outer_w, marker);

            for _ in 0..pad.top { r.raw(blank.clone()); }

            for line in wrap_text(&self.content, inner_w) {
                let vw = visible_width(&line);
                let rf = r.right_fill(vw + pad.left as usize, outer_w as usize, marker);
                r.raw(format!("{lp}{line}{rf}"));
            }

            for _ in 0..pad.bottom { r.raw(blank.clone()); }
        });
    }
}
```

### Markdown

Markdown knows its padding. It computes inner width, renders at that width,
emits padded lines.

```rust
pub struct Markdown {
    id: Id,
    source: String,
    blocks: Vec<CachedBlock>,
    output: Vec<Line>,           // undecorated, at current inner_w
    width: u16,                  // last inner width
    pad: Padding,
    marker: Marker,              // for bg fill
    dirty: bool,
}

impl Markdown {
    pub fn new(id: impl Into<Id>) -> Self { ... }
    pub fn pad(mut self, p: Padding) -> Self { self.pad = p; self }
    pub fn marker(mut self, m: Marker) -> Self { self.marker = m; self }
    pub fn append(&mut self, text: &str) { ... }
    pub fn set_source(&mut self, text: String) { ... }

    pub fn render(&mut self, r: &mut Renderer) {
        let outer_w = r.width();
        let inner_w = outer_w.saturating_sub(self.pad.h() as u16);

        // Rebuild internal block cache if width changed
        if inner_w != self.width {
            self.width = inner_w;
            self.blocks.clear();
            self.dirty = true;
        }
        if self.dirty { self.rebuild(); }

        let h = hash(&(&self.source, inner_w));
        let pad = self.pad;
        let marker = self.marker;
        let output = &self.output;

        r.cached(self.id, h, |r| {
            let lp = r.left_pad(pad.left, marker);
            let blank = r.blank(outer_w, marker);

            for _ in 0..pad.top { r.raw(blank.clone()); }

            for line in output {
                let vw = visible_width(line);
                let rf = r.right_fill(vw + pad.left as usize, outer_w as usize, marker);
                r.raw(Rc::from(format!("{lp}{line}{rf}").as_str()));
            }

            for _ in 0..pad.bottom { r.raw(blank.clone()); }
        });
    }
}
```

Markdown controls its own padding decoration. `r.width()` gives the available
space from the renderer (which may be narrowed by `sub_render()`). Markdown
subtracts its own padding to get inner width. The renderer never manipulates
markdown's lines — it just caches the final output.

On cache hit: replays the same `Rc<str>`. Zero `format!()`.

### LineEditor

```rust
impl LineEditor {
    pub fn render(&self, r: &mut Renderer) {
        let display = format!("{}{}", self.prompt, self.buf);
        r.raw(display);
        r.set_cursor(visible_width(&self.prompt) + self.cursor_visible_col());
    }
}
```

No caching — changes every keystroke. Single line, one `format!()`.

### LimitText

```rust
pub struct LimitText {
    id: Id,
    content: String,
    max_rows: usize,
    pad: Padding,
    marker: Marker,
}

impl LimitText {
    pub fn new(id: impl Into<Id>, max_rows: usize) -> Self { ... }
    pub fn set(&mut self, content: impl Into<String>) { ... }

    pub fn render(&self, r: &mut Renderer) {
        let h = hash(&(&self.content, self.max_rows, &self.pad, &self.marker));
        let outer_w = r.width();
        let inner_w = outer_w.saturating_sub(self.pad.h() as u16) as usize;
        let pad = self.pad;
        let marker = self.marker;
        let max = self.max_rows;

        r.cached(self.id, h, |r| {
            let lp = r.left_pad(pad.left, marker);
            let blank = r.blank(outer_w, marker);

            for _ in 0..pad.top { r.raw(blank.clone()); }

            let lines = wrap_text(&self.content, inner_w);
            let show = if lines.len() > max { max.saturating_sub(1) } else { lines.len() };

            for line in &lines[..show] {
                let vw = visible_width(line);
                let rf = r.right_fill(vw + pad.left as usize, outer_w as usize, marker);
                r.raw(format!("{lp}{line}{rf}"));
            }
            if lines.len() > max {
                let msg = r.styled(&format!("… {} more lines", lines.len() - show), Marker::Muted);
                let vw = visible_width(&msg);
                let rf = r.right_fill(vw + pad.left as usize, outer_w as usize, marker);
                r.raw(format!("{lp}{msg}{rf}"));
            }

            for _ in 0..pad.bottom { r.raw(blank.clone()); }
        });
    }
}
```

### LimitMarkdown

```rust
pub struct LimitMarkdown {
    md: Markdown,
    max_rows: usize,
}

impl LimitMarkdown {
    pub fn new(id: impl Into<Id>, max_rows: usize) -> Self { ... }
    pub fn append(&mut self, text: &str) { self.md.append(text); }

    pub fn render(&mut self, r: &mut Renderer) {
        // Use markdown's full render but cap output
        let outer_w = r.width();
        let inner_w = outer_w.saturating_sub(self.md.pad.h() as u16);
        self.md.ensure_width(inner_w);

        let h = hash(&(self.md.source(), inner_w, self.max_rows));
        let pad = self.md.pad;
        let marker = self.md.marker;
        let output = &self.md.output;
        let max = self.max_rows;

        r.cached(self.md.id, h, |r| {
            let lp = r.left_pad(pad.left, marker);
            let blank = r.blank(outer_w, marker);

            for _ in 0..pad.top { r.raw(blank.clone()); }

            let show = output.len().min(max);
            for line in &output[..show] {
                let vw = visible_width(line);
                let rf = r.right_fill(vw + pad.left as usize, outer_w as usize, marker);
                r.raw(Rc::from(format!("{lp}{line}{rf}").as_str()));
            }
            if output.len() > max {
                let msg = r.styled(&format!("… {} more lines", output.len() - show), Marker::Muted);
                let vw = visible_width(&msg);
                let rf = r.right_fill(vw + pad.left as usize, outer_w as usize, marker);
                r.raw(format!("{lp}{msg}{rf}"));
            }

            for _ in 0..pad.bottom { r.raw(blank.clone()); }
        });
    }
}
```

## App Trait

```rust
pub trait App: 'static {
    type Message: Send + 'static;
    fn render(&mut self, r: &mut Renderer);
    fn update(&mut self, event: Event<Self::Message>) -> bool;
}
```

## Examples

### Counter (dumb text, no components)

```rust
fn render(&mut self, r: &mut Renderer) {
    r.spacer(1);
    r.raw(r.styled(&format!("counter  (tick {})", self.ticks), Marker::Primary));
    r.spacer(1);
    r.raw(format!("  Count: {}", r.styled(&self.count.to_string(), Marker::Warning)));
    r.spacer(1);
    r.raw(r.styled("  Up/Down to change, q to quit", Marker::Muted));
}
```

### Styled markdown stream

```rust
struct MdApp {
    md: Markdown,
    editor: LineEditor,
}

// Init:
MdApp {
    md: Markdown::new("md")
        .pad(Padding::new(1, 2, 1, 2))
        .marker(Marker::SurfaceAlt),
    editor: LineEditor::new(),
}

// Render:
fn render(&mut self, r: &mut Renderer) {
    r.raw(r.styled("streaming markdown", Marker::Primary));
    r.spacer(1);
    self.md.render(r);    // owns its padding + bg, reads r.width()
    r.spacer(1);
    self.editor.render(r);
}
```

### Two-pane

```rust
fn render(&mut self, r: &mut Renderer) {
    hstack(r, |r| {
        bordered(r, Border::Rounded, |r| {
            self.left.render(r);    // left.pad + left.marker handle decoration
        });
    }, |r| {
        self.right.render(r);
    });
}
```

### Dumb text with themed colors

```rust
r.raw(format!(
    "{} {} {}",
    r.styled("INFO", Marker::Primary),
    r.styled("2024-01-15", Marker::Muted),
    "Server started on port 8080",
));
```

## Composition Helpers

Free functions using `sub_render()`.

### `bordered()`

```rust
pub fn bordered(r: &mut Renderer, border: Border, build: impl FnOnce(&mut Renderer)) {
    let inner_w = r.width().saturating_sub(border.cols());
    let inner = r.sub_render(inner_w, build);
    let (tl, tr, bl, br, h, v) = border.chars();
    let bar = h.to_string().repeat(inner_w as usize);
    r.raw(format!("{tl}{bar}{tr}"));
    for line in &inner {
        let vw = visible_width(line);
        let pad = (inner_w as usize).saturating_sub(vw);
        r.raw(format!("{v}{line}\x1b[0m{}{v}", " ".repeat(pad)));
    }
    r.raw(format!("{bl}{bar}{br}"));
}
```

### `hstack()`

```rust
pub fn hstack(
    r: &mut Renderer,
    left: impl FnOnce(&mut Renderer),
    right: impl FnOnce(&mut Renderer),
) {
    let lw = r.width() / 2;
    let rw = r.width() - lw;
    let left_lines = r.sub_render(lw, left);
    let right_lines = r.sub_render(rw, right);
    let max = left_lines.len().max(right_lines.len());
    for i in 0..max {
        let l = left_lines.get(i).map(|s| s.as_ref()).unwrap_or("");
        let rv = right_lines.get(i).map(|s| s.as_ref()).unwrap_or("");
        let lt = truncate_line(l, lw as usize);
        let pad = (lw as usize).saturating_sub(visible_width(&lt));
        r.raw(format!("{lt}\x1b[0m{}{rv}", " ".repeat(pad)));
    }
}
```

### `overlay()`

```rust
pub fn overlay(
    r: &mut Renderer,
    base: impl FnOnce(&mut Renderer),
    popup_w: u16, row: usize, col: usize,
    popup: impl FnOnce(&mut Renderer),
) {
    base(r);
    let popup_lines = r.sub_render(popup_w, popup);
    for (i, pl) in popup_lines.iter().enumerate() {
        let t = row + i;
        if t < r.output.len() {
            r.output[t] = Rc::from(splice_line(&r.output[t], col, pl, popup_w as usize));
        }
    }
}
```

## Color

```rust
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Color {
    Ansi(u8),           // 0-15
    Idx(u8),            // 0-255
    Rgb(u8, u8, u8),
}

impl Color {
    pub fn fg_sgr(&self) -> String;
    pub fn bg_sgr(&self) -> String;
    pub const BLACK: Self   = Color::Ansi(0);
    pub const RED: Self     = Color::Ansi(1);
    pub const GREEN: Self   = Color::Ansi(2);
    pub const YELLOW: Self  = Color::Ansi(3);
    pub const BLUE: Self    = Color::Ansi(4);
    pub const MAGENTA: Self = Color::Ansi(5);
    pub const CYAN: Self    = Color::Ansi(6);
    pub const WHITE: Self   = Color::Ansi(7);
}
```

## What's Gone

- `View` enum — components push directly
- `Reconciler` — merged into Renderer
- Style stack on renderer — components own their decoration
- `emit()` decoration layer — `raw()` is pass-through
- `pad_lines()` — components pad their own output
- `view_lines()` / `lines_snapshot()` — components call `r.raw()` directly

## What Stays

- `Rc<str>` lines + `Rc::ptr_eq` diffing
- `cached()` with (id, hash, width, theme) key
- `wrap_text()` / `truncate_line()` / `SgrState`
- Markdown's internal block-level cache
- Diff renderer logic (same as current)
- `Padding` struct (owned by components, not renderer)

## Implementation Order

1. `Color`, `Marker`, `Resolved`, `Theme` — new `style.rs`
2. Merge Reconciler + Renderer into unified `Renderer`
   - `raw()`, `spacer()`, `set_cursor()`, `cached()`, `sub_render()`
   - `styled()`, `left_pad()`, `right_fill()`, `blank()` helpers
   - `begin_frame()`, `end_frame()` with diff + cache cleanup
3. `App` trait → `render(&mut self, r: &mut Renderer)`
4. `Text` component with pad + marker
5. Port `Markdown` — owns pad + marker, uses `r.cached()` + `r.raw()`
6. Port `LineEditor`
7. `LimitText`, `LimitMarkdown`
8. Port examples
9. `sub_render()` + `bordered()` + `hstack()` + `overlay()`
