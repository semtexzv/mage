# Design: Style State Stack

## Problem

The current rendering pipeline concatenates raw ANSI escape sequences as
strings with no semantic state tracking. This creates several issues:

1. **No composability**: When markdown content is wrapped in a bg container,
   inline resets (even SOFT_RESET) kill attributes from outer contexts.
   `wrap_text` carries stale SGR prefixes to continuation lines (e.g.
   `\x1b[36m\x1b[39m` no-op on line 5 of a wrapped table cell).

2. **Fragile wrap/truncate**: `SgrState` in `wrap.rs` is a fixed 8-slot
   array that records raw SGR code strings. It doesn't understand semantics —
   `\x1b[22m` (NOBOLD) also kills DIM, but the tracker doesn't know that.
   `truncate_line` appends `\x1b[0m` (hard RESET), which kills bg from
   outer containers.

3. **No restore on pop**: When bold ends inside a cell that's inside a
   bold header row, emitting `\x1b[22m` kills the outer bold too. There's
   no way to "return to parent state."

4. **Scattered constants**: Markdown uses 16+ ANSI constants (`BOLD`,
   `NOBOLD`, `CYAN`, `DEFFG`, `SOFT_RESET`, etc.) with manual pairing.
   Getting a pair wrong silently corrupts all subsequent output.

## Design: `StyleState` + `StyleStack`

### `StyleState` — flat snapshot of all active attributes

```rust
/// Complete terminal style state — all attributes + colors.
/// Cheap to copy (32 bytes). No heap allocation.
#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub struct StyleState {
    pub bold: bool,
    pub dim: bool,
    pub italic: bool,
    pub underline: bool,
    pub strikethrough: bool,
    pub fg: Option<Color>,  // None = terminal default
    pub bg: Option<Color>,  // None = terminal default
}
```

Key methods:

```rust
impl StyleState {
    /// Emit the SGR sequence that fully establishes this state from
    /// a reset terminal.  E.g. "\x1b[1;3;36;48;2;20;20;35m".
    /// Returns "" if all attributes are default (no-op).
    pub fn to_sgr(&self) -> String;

    /// Emit the minimal SGR sequence to transition FROM `from` TO `self`.
    /// Only attributes that differ are emitted.
    /// E.g. transitioning from {bold,fg=cyan} to {bold,fg=default}
    /// emits just "\x1b[39m".
    pub fn transition_from(&self, from: &StyleState) -> String;

    /// Merge: apply non-default fields from `overlay` on top of `self`.
    pub fn merge(&self, overlay: &StyleState) -> StyleState;
}
```

### `StyleStack` — push/pop with pre-built SGR strings

```rust
/// Nested style context with O(1) push/pop and pre-built SGR sequences.
///
/// Each frame records a style delta. The stack maintains the combined
/// current state and a cached SGR string ready to append to output.
///
/// ```
/// let mut ss = StyleStack::new();
/// ss.set_base(StyleState { bg: Some(Rgb(20,20,35)), ..default() });
///
/// buf.push_str(ss.sgr());                   // emit bg
/// ss.push(StyleState { bold: true, .. });
/// buf.push_str(ss.last_transition());        // emit "\x1b[1m"
/// buf.push_str("Header");
/// ss.pop();
/// buf.push_str(ss.last_transition());        // emit "\x1b[22m" (restore)
/// ```
pub struct StyleStack {
    /// Stack of combined states.  frames[0] = base, frames[last] = current.
    frames: Vec<StyleState>,

    /// Pre-built full SGR for the current state.
    /// Ready to prepend to continuation lines after a wrap break.
    current_sgr: String,

    /// SGR emitted by the last push() or pop() — the diff/transition.
    /// Append this to the output buffer after each push/pop call.
    last_transition: String,
}

impl StyleStack {
    pub fn new() -> Self;

    /// Set the base (outermost) state — e.g. container bg color.
    /// This is the state restored when all frames are popped.
    pub fn set_base(&mut self, base: StyleState);

    /// Push a style layer. Computes the transition SGR from current
    /// to current.merge(delta).  Access via `last_transition()`.
    pub fn push(&mut self, delta: StyleState);

    /// Pop the top layer. Computes the transition SGR from current
    /// back to parent.  Access via `last_transition()`.
    pub fn pop(&mut self);

    /// Full SGR for the current combined state — for line prefixes,
    /// continuation lines, or re-establishing state after unknown content.
    pub fn sgr(&self) -> &str;

    /// The transition SGR from the last push/pop. Append to output.
    pub fn last_transition(&self) -> &str;

    /// The current combined state.
    pub fn current(&self) -> &StyleState;
}
```

### How it solves each problem

**Problem 1 — Composability**: The base state includes bg. All
transitions preserve attributes they don't touch. `pop()` computes the
exact SGR to restore the parent — not a blunt reset.

**Problem 2 — Wrapping**: `wrap_text` receives the current `StyleState`.
When it breaks a line, it prefixes the continuation with
`state.to_sgr()` — the full state, not a fragile code stack.
`truncate_line` returns `(truncated_line, StyleState)` instead of
appending `\x1b[0m`.

**Problem 3 — Restore on pop**: `pop()` calls
`parent.transition_from(current)` to compute the exact codes needed.
If parent was bold and child added italic, pop emits `\x1b[23m`
(no-italic) — bold survives.

**Problem 4 — No manual pairing**: Markdown pushes semantic styles
(`bold`, `fg=cyan`) and pops them. The stack handles the ANSI encoding.

## Integration points

### 1. Markdown `Ctx`

```rust
struct Ctx<'s> {
    styles: StyleStack,
    buf: String,
    // ...
}

// Bold span:
fn event(&mut self, ev: Event) {
    Event::Start(Tag::Strong) => {
        self.styles.push(StyleState { bold: true, ..default() });
        self.buf.push_str(self.styles.last_transition());
    }
    Event::End(TagEnd::Strong) => {
        self.styles.pop();
        self.buf.push_str(self.styles.last_transition());
    }
}

// Heading flush — no manual RESET:
fn flush_heading(&mut self) {
    self.styles.push(StyleState {
        bold: true,
        underline: level == 1,
        fg: Some(Color::Yellow),
        ..default()
    });
    let sgr = self.styles.last_transition().to_string();
    // ... emit line with sgr prefix ...
    self.styles.pop();
    // sgr_end = self.styles.last_transition()
}
```

### 2. `wrap_text`

```rust
/// Word-wrap with full style state tracking.
pub fn wrap_text(
    text: &str,
    width: usize,
    base: &StyleState,  // state to restore at start of each continuation
) -> Vec<String>;
```

Each continuation line starts with `base.to_sgr()`.  The wrapper tracks
style changes within the line so it knows the state at each break
candidate.

### 3. `truncate_line`

```rust
/// Truncate, returning active style state at cut point.
pub fn truncate_line(
    s: &str,
    max_width: usize,
) -> (String, StyleState);
```

No `\x1b[0m` appended.  The caller decides: renderer appends hard
reset; bg-fill re-emits bg.

### 4. `Markdown::rebuild` bg fill

```rust
fn rebuild(&mut self) {
    let base = StyleState { bg: self.bg, ..default() };
    // ...
    let wrap_line = |content: &str| -> Line {
        let padded = format!("{prefix}{content}");
        let vw = visible_width(&padded);
        let fill = ow.saturating_sub(vw);
        // base.to_sgr() re-establishes bg after any inline resets
        Rc::from(format!(
            "{}{padded}{}{}\x1b[0m",
            base.to_sgr(),
            base.to_sgr(),  // re-establish bg before fill spaces
            " ".repeat(fill),
        ).as_str())
    };
}
```

### 5. Renderer line emission

The renderer's `full_render` / `diff_render` continue to append
`\x1b[0m` after each line — that's the hard boundary between terminal
rows.  No change needed here.

## Performance

- `StyleState` is 32 bytes, `Copy`, zero allocation.
- `to_sgr()` builds a `~20-byte` string.  For the hot path (streaming
  append), only the last block is re-rendered.
- `transition_from()` is even cheaper — often just one SGR code.
- `StyleStack` is typically 1–3 frames deep (bg → bold → inline color).
- `current_sgr` is cached and only recomputed on push/pop.

## Migration plan

1. Add `StyleState` to `style.rs` (alongside existing `Style` + `Color`)
2. Add `StyleStack` to `style.rs`
3. Update `wrap.rs`: replace `SgrState` with `StyleState` tracking,
   add `base: &StyleState` param to `wrap_text`
4. Update `truncate_line` to return `(String, StyleState)` instead of
   appending `\x1b[0m`
5. Update `markdown.rs` `Ctx`: replace manual ANSI constants with
   `StyleStack` push/pop
6. Update `Markdown::rebuild()`: pass base state (with bg) through the
   block renderer
7. Renderer: no change (keeps appending `\x1b[0m` per line)

Steps 1–2 are additive (no breakage).  Steps 3–6 are a coordinated
refactor of the rendering pipeline.
