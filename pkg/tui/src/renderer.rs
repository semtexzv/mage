//! Differential terminal renderer — main terminal mode.
//!
//! Renders in the primary terminal buffer (not alternate screen).
//! Scrollback is preserved. First render appends from the current cursor
//! position. Subsequent renders diff and only repaint changed rows.
//!
//! The renderer accumulates lines during a frame via `push_line` /
//! `push_blank` / `push_lines`, then diffs against the previous frame
//! on `end_frame`.

use std::rc::Rc;

use crate::ansi::{
    truncate_line, visible_width, RESET,
    SYNC_BEGIN, SYNC_END, CLEAR_SCROLLBACK, CLEAR_SCREEN, CURSOR_HOME, CLEAR_LINE,
    CRLF, CR, SHOW_CURSOR, HIDE_CURSOR,
    cursor_up, cursor_down, cursor_col,
};
use crate::style::{Color, Theme};

const MAX_PREV_LINES: usize = 10_000;

/// A single rendered line — `Rc<str>` enables O(1) identity comparison.
pub type Line = Rc<str>;

/// Cursor position for Input views.
#[derive(Debug, Clone, Copy)]
pub struct CursorPos {
    pub row: usize,
    pub col: usize,
}

/// Terminal output abstraction.
pub trait Terminal {
    fn write(&mut self, s: &str);
    fn flush(&mut self);
    fn size(&self) -> (u16, u16); // (cols, rows)
    fn has_error(&self) -> bool { false }
}

/// Real terminal using stdout + crossterm.
pub struct ProcessTerminal {
    cols: u16,
    rows: u16,
    write_failed: bool,
}

impl Default for ProcessTerminal {
    fn default() -> Self {
        Self::new()
    }
}

impl ProcessTerminal {
    pub fn new() -> Self {
        let (cols, rows) = crossterm::terminal::size().unwrap_or((80, 24));
        Self { cols, rows, write_failed: false }
    }

    pub fn update_size(&mut self) {
        if let Ok((c, r)) = crossterm::terminal::size() {
            self.cols = c;
            self.rows = r;
        }
    }
}

impl Terminal for ProcessTerminal {
    fn write(&mut self, s: &str) {
        use std::io::Write;
        if std::io::stdout().write_all(s.as_bytes()).is_err() {
            self.write_failed = true;
        }
    }
    fn flush(&mut self) {
        use std::io::Write;
        if std::io::stdout().flush().is_err() {
            self.write_failed = true;
        }
    }
    fn size(&self) -> (u16, u16) {
        (self.cols, self.rows)
    }
    fn has_error(&self) -> bool {
        self.write_failed
    }
}

// ── Renderer state ──────────────────────────────────────────────

pub struct Renderer {
    /// Lines accumulated during the current frame.
    lines: Vec<Line>,
    /// Lines from the previous frame (for diffing).
    pub prev_lines: Vec<Line>,
    /// Cursor position requested during the current frame.
    cursor: Option<CursorPos>,
    /// Current frame width.
    width: u16,
    /// Current frame height.
    height: u16,

    // Internal diff state
    prev_width: u16,
    hw_cursor_row: usize,
    max_lines: usize,
    prev_vp_top: usize,
    cursor_visible: bool,
    /// Semantic theme for component styling.
    pub theme: Theme,
}

impl Default for Renderer {
    fn default() -> Self {
        Self::new()
    }
}

impl Renderer {
    pub fn new() -> Self {
        Self {
            lines: Vec::new(),
            prev_lines: Vec::new(),
            cursor: None,
            width: 0,
            height: 0,
            prev_width: 0,
            hw_cursor_row: 0,
            max_lines: 0,
            prev_vp_top: 0,
            cursor_visible: false,
            theme: Theme::default(),
        }
    }

    // ── Frame accumulation API ──────────────────────────────────

    /// Returns the current frame width. Views query this to know available width.
    pub fn width(&self) -> u16 {
        self.width
    }

    /// Returns the current frame height.
    pub fn height(&self) -> u16 {
        self.height
    }

    /// Returns the number of lines accumulated in the current frame.
    pub fn line_count(&self) -> usize {
        self.lines.len()
    }

    /// Append a rendered line to the current frame.
    pub fn push_line(&mut self, line: impl Into<Line>) {
        self.lines.push(line.into());
    }

    /// Append an empty line to the current frame.
    /// Uses a shared singleton so Rc::ptr_eq detects unchanged blanks.
    pub fn push_blank(&mut self) {
        self.lines.push(blank_line());
    }

    /// Append a slice of lines to the current frame (for cached widget output).
    pub fn push_lines(&mut self, lines: &[Line]) {
        self.lines.extend_from_slice(lines);
    }

    /// Set the cursor position for an Input view.
    pub fn set_cursor(&mut self, row: usize, col: usize) {
        self.cursor = Some(CursorPos { row, col });
    }

    /// Composite overlay content into an existing line at a column range.
    ///
    /// Splices `content` into line `row` starting at visible column `col`,
    /// spanning `overlay_width` columns. Content before `col` and after
    /// `col + overlay_width` is preserved from the original line.
    ///
    /// The overlay content is padded with spaces to fill `overlay_width`.
    pub fn composite_at(
        &mut self,
        row: usize,
        col: usize,
        content: &str,
        overlay_width: usize,
    ) {
        if row >= self.lines.len() { return; }

        let base_line = &*self.lines[row];

        // Split the base line into three zones:
        //   [left_of_overlay] [overlay_zone] [right_of_overlay]
        // split_line_at_col handles SGR: left ends with reset, right re-establishes SGR.
        let (left_of_overlay, _) = crate::ansi::split_line_at_col(base_line, col);
        let (_, right_of_overlay) = crate::ansi::split_line_at_col(base_line, col + overlay_width);

        // If the base line is shorter than `col`, pad left portion with spaces.
        let left_visible_w = crate::ansi::visible_width(&left_of_overlay);
        let left_pad = " ".repeat(col.saturating_sub(left_visible_w));

        // Pad overlay content with spaces to fill the full overlay width.
        let overlay_visible_w = crate::ansi::visible_width(content);
        let overlay_pad = " ".repeat(overlay_width.saturating_sub(overlay_visible_w));

        // Assemble: left | reset | overlay content + pad | reset | right
        // The resets ensure clean boundaries: overlay doesn't inherit base
        // styling, and right_of_overlay re-establishes its own SGR state.
        let spliced = format!(
            "{left_of_overlay}{left_pad}{RESET}{content}{overlay_pad}{RESET}{right_of_overlay}"
        );
        self.lines[row] = Rc::from(spliced.as_str());
    }

    /// Begin a new frame: clears accumulated lines and resets cursor.
    pub fn begin_frame(&mut self, width: u16, height: u16) {
        self.lines.clear();
        self.cursor = None;
        self.width = width;
        self.height = height;
    }
    /// Create a line filled to `width` with a background color.
    /// The visible content is padded with spaces to fill the full width.
    pub(crate) fn bg_filled_line(content: &str, width: usize, color: Color) -> String {
        let vis_width = visible_width(content);
        let fill = width.saturating_sub(vis_width);
        let bg_start = format!("\x1b[{}m", color.bg_code());
        format!(
            "{}{}{}{}{}",
            bg_start,
            content,
            bg_start,
            " ".repeat(fill),
            RESET
        )
    }

    /// End the current frame: diffs accumulated lines against previous frame,
    /// paints changes to the terminal, then swaps buffers.
    pub fn end_frame(&mut self, term: &mut dyn Terminal) {
        let lines = std::mem::take(&mut self.lines);
        let cursor = self.cursor.take();
        self.render(term, lines, cursor);
    }

    /// Render lines to the terminal. Diffs against previous frame.
    /// If `cursor` is Some, positions the hardware cursor there and shows it.
    fn render(&mut self, term: &mut dyn Terminal, lines: Vec<Line>, cursor: Option<CursorPos>) {
        let width = self.width;
        let height = self.height;
        // If width/height not set via begin_frame, fall back to terminal size
        let (w, h) = if width > 0 && height > 0 {
            (width, height)
        } else {
            term.size()
        };
        let th = h as usize;

        let is_first = self.prev_lines.is_empty() && self.prev_width == 0;
        let width_changed = !is_first && w != self.prev_width;

        let changed = if is_first {
            self.full_render(term, &lines, w, th, false);
            true
        } else if width_changed {
            self.full_render(term, &lines, w, th, true);
            true
        } else {
            self.diff_render(term, &lines, w, th)
        };

        // Only touch the cursor if we actually painted something.
        // Otherwise cursor movement snaps the viewport when the user
        // is scrolling through terminal scrollback.
        if changed {
            self.position_cursor(term, cursor, lines.len());
        }
    }

    /// Move cursor to end of content and print a newline so the shell
    /// prompt appears below our output.
    pub fn finalize(&mut self, term: &mut dyn Terminal) {
        if self.prev_lines.is_empty() {
            return;
        }
        let target = self.prev_lines.len().saturating_sub(1);
        if target > self.hw_cursor_row {
            term.write(&cursor_down(target - self.hw_cursor_row));
        } else if target < self.hw_cursor_row {
            term.write(&cursor_up(self.hw_cursor_row - target));
        }
        term.write(CRLF);
        term.flush();
    }

    // ── Full render ─────────────────────────────────────────────

    fn full_render(
        &mut self,
        term: &mut dyn Terminal,
        lines: &[Line],
        width: u16,
        th: usize,
        clear: bool,
    ) {
        let mut buf = String::new();
        buf.push_str(SYNC_BEGIN);

        if clear {
            // Clear visible screen + scrollback to avoid stale content mixing
            // with new content when lines scroll past the terminal bottom.
            buf.push_str(CLEAR_SCREEN);
            buf.push_str(CURSOR_HOME);
            buf.push_str(CLEAR_SCROLLBACK);
        }

        let w = width as usize;
        for (i, line) in lines.iter().enumerate() {
            if i > 0 {
                buf.push_str(CRLF);
            }
            buf.push_str(&truncate_line(line, w));
            buf.push_str(RESET);
        }

        buf.push_str(SYNC_END);
        term.write(&buf);
        term.flush();

        let cursor = lines.len().saturating_sub(1);
        self.hw_cursor_row = cursor;
        if clear {
            self.max_lines = lines.len();
        } else {
            self.max_lines = self.max_lines.max(lines.len());
        }
        self.prev_vp_top = self.max_lines.saturating_sub(th);
        self.prev_lines = lines.to_vec();
        if self.prev_lines.len() > MAX_PREV_LINES {
            let drain = self.prev_lines.len() - MAX_PREV_LINES;
            self.prev_lines.drain(..drain);
        }
        self.prev_width = width;
    }

    // ── Differential render (Pi model) ───────────────────────────
    //
    // Uses local mutable variables for viewport/cursor tracking (like Pi).
    // All cursor movements are computed relative to where the hardware
    // cursor actually is (hw_cursor_row) and the current viewport top.

    /// Returns true if anything was painted, false if nothing changed.
    fn diff_render(&mut self, term: &mut dyn Terminal, lines: &[Line], width: u16, th: usize) -> bool {
        // Debug: log render decisions
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true).append(true)
            .open("/tmp/mage-render.log")
        {
            use std::io::Write;
            let _ = writeln!(f, "diff_render: lines={} prev={} hw_cursor={} vp_top={} th={}",
                lines.len(), self.prev_lines.len(), self.hw_cursor_row, self.prev_vp_top, th);
        }
        let old_len = self.prev_lines.len();
        let max_len = old_len.max(lines.len());

        // Find changed range. Try Rc::ptr_eq first (O(1)), fall back to
        // string comparison for lines from different allocations.
        let mut first_changed: isize = -1;
        let mut last_changed: isize = -1;
        for i in 0..max_len {
            let same = match (self.prev_lines.get(i), lines.get(i)) {
                (Some(a), Some(b)) => Rc::ptr_eq(a, b) || **a == **b,
                (None, None) => true,
                _ => false,
            };
            if !same {
                if first_changed == -1 {
                    first_changed = i as isize;
                }
                last_changed = i as isize;
            }
        }

        let appended = lines.len() > old_len;
        if appended {
            if first_changed == -1 {
                first_changed = old_len as isize;
            }
            last_changed = (lines.len() - 1) as isize;
        }

        // Nothing changed — no terminal writes at all.
        if first_changed == -1 {
            if let Ok(mut f) = std::fs::OpenOptions::new()
                .create(true).append(true).open("/tmp/mage-render.log")
            {
                use std::io::Write;
                let _ = writeln!(f, "  -> nothing changed, skip");
            }
            self.prev_lines = lines.to_vec();
            self.prev_width = width;
            return false;
        }

        let first = first_changed as usize;
        let last = last_changed as usize;
        let append_start = appended && first == old_len && first > 0;

        // If first change is above the previous viewport, full re-render.
        if first < self.prev_vp_top {
            self.full_render(term, lines, width, th, true);
            return true;
        }

        // All changes are in deleted tail (nothing new to render).
        if first >= lines.len() {
            if old_len > lines.len() {
                let extra = old_len - lines.len();
                if extra > th {
                    self.full_render(term, lines, width, th, true);
                    return true;
                }
                let target = lines.len().saturating_sub(1);
                if target < self.prev_vp_top {
                    self.full_render(term, lines, width, th, true);
                    return true;
                }
                let mut buf = String::new();
                buf.push_str(SYNC_BEGIN);
                let delta = self.compute_line_diff(target);
                if delta > 0 { buf.push_str(&cursor_down(delta as usize)); }
                else if delta < 0 { buf.push_str(&cursor_up((-delta) as usize)); }
                buf.push_str(CR);
                // Clear extra lines
                if extra > 0 { buf.push_str(&cursor_down(1)); }
                for i in 0..extra {
                    buf.push_str(CR);
                    buf.push_str(CLEAR_LINE);
                    if i < extra - 1 { buf.push_str(&cursor_down(1)); }
                }
                if extra > 0 { buf.push_str(&cursor_up(extra)); }
                buf.push_str(SYNC_END);
                term.write(&buf);
                term.flush();
                self.hw_cursor_row = target;
            }
            self.prev_vp_top = self.prev_vp_top.max(
                lines.len().saturating_sub(th),
            );
            self.prev_lines = lines.to_vec();
            self.prev_width = width;
            return true;
        }

        // ── Normal diff path ────────────────────────────────────
        let move_target = if append_start { first - 1 } else { first };

        let mut buf = String::new();
        buf.push_str(SYNC_BEGIN);

        // If move_target is below the visible viewport, scroll down.
        let prev_vp_bottom = self.prev_vp_top + th.saturating_sub(1);
        if th > 0 && move_target > prev_vp_bottom {
            // Move cursor to bottom of screen first.
            let cur_screen = self.hw_cursor_row
                .saturating_sub(self.prev_vp_top)
                .min(th.saturating_sub(1));
            let to_bottom = th.saturating_sub(1).saturating_sub(cur_screen);
            if to_bottom > 0 {
                buf.push_str(&cursor_down(to_bottom));
            }
            // Scroll by emitting newlines at the bottom.
            let scroll = move_target - prev_vp_bottom;
            for _ in 0..scroll {
                buf.push_str(CRLF);
            }
            // Update local tracking (like Pi's local variable mutation).
            self.hw_cursor_row = move_target;
            self.prev_vp_top += scroll;
        }

        // Move cursor to move_target.
        let delta = self.compute_line_diff(move_target);
        if delta > 0 { buf.push_str(&cursor_down(delta as usize)); }
        else if delta < 0 { buf.push_str(&cursor_up((-delta) as usize)); }
        self.hw_cursor_row = move_target;

        buf.push_str(if append_start { CRLF } else { CR });

        // Render changed lines (first to last).
        let render_end = last.min(lines.len().saturating_sub(1));
        let w = width as usize;
        for (idx, line) in lines[first..=render_end].iter().enumerate() {
            if idx > 0 {
                buf.push_str(CRLF);
            }
            buf.push_str(CLEAR_LINE);
            buf.push_str(&truncate_line(line, w));
            buf.push_str(RESET);
        }

        let mut final_row = render_end;

        // If content shrunk, clear the old tail lines.
        if old_len > lines.len() {
            if render_end < lines.len().saturating_sub(1) {
                let down = lines.len().saturating_sub(1) - render_end;
                buf.push_str(&cursor_down(down));
                final_row = lines.len().saturating_sub(1);
            }
            let extra = old_len - lines.len();
            for _ in 0..extra {
                buf.push_str(CRLF);
                buf.push_str(CLEAR_LINE);
            }
            if extra > 0 {
                buf.push_str(&cursor_up(extra));
            }
        }

        buf.push_str(SYNC_END);
        term.write(&buf);
        term.flush();

        // Update state — compute viewport from where cursor ended up (like Pi).
        self.hw_cursor_row = final_row;
        self.max_lines = self.max_lines.max(lines.len());
        self.prev_vp_top = self.prev_vp_top.max(
            final_row.saturating_sub(th.saturating_sub(1)),
        );
        self.prev_lines = lines.to_vec();
        if self.prev_lines.len() > MAX_PREV_LINES {
            let drain = self.prev_lines.len() - MAX_PREV_LINES;
            self.prev_lines.drain(..drain);
        }
        self.prev_width = width;
        true
    }

    // ── Helpers ─────────────────────────────────────────────────

    /// Compute cursor movement delta from current hw_cursor_row to target,
    /// accounting for the viewport. Positive = down, negative = up.
    fn compute_line_diff(&self, target: usize) -> isize {
        let cur_screen = self.hw_cursor_row as isize - self.prev_vp_top as isize;
        let tgt_screen = target as isize - self.prev_vp_top as isize;
        tgt_screen - cur_screen
    }

    /// Position the hardware cursor for an Input view, or hide it.
    fn position_cursor(
        &mut self,
        term: &mut dyn Terminal,
        cursor: Option<CursorPos>,
        total_lines: usize,
    ) {
        let Some(pos) = cursor else {
            if self.cursor_visible {
                term.write(HIDE_CURSOR);
                term.flush();
                self.cursor_visible = false;
            }
            return;
        };
        if total_lines == 0 {
            if self.cursor_visible {
                term.write(HIDE_CURSOR);
                term.flush();
                self.cursor_visible = false;
            }
            return;
        }
        let target_row = pos.row.min(total_lines - 1);
        let delta = self.compute_line_diff(target_row);
        let mut buf = String::new();
        if delta > 0 {
            buf.push_str(&cursor_down(delta as usize));
        } else if delta < 0 {
            buf.push_str(&cursor_up((-delta) as usize));
        }
        buf.push_str(&cursor_col(pos.col + 1));
        if !self.cursor_visible {
            buf.push_str(SHOW_CURSOR);
            self.cursor_visible = true;
        }
        term.write(&buf);
        term.flush();
        self.hw_cursor_row = target_row;
    }
}

// ── Widget traits ────────────────────────────────────────────────────────────

/// Minimal line-output interface that widgets render into.
///
/// `Renderer` implements this, but so can test harnesses or nested containers.
pub trait LineSink {
    /// Available width in terminal columns.
    fn width(&self) -> u16;
    /// Append a slice of pre-rendered lines.
    fn push_lines(&mut self, lines: &[Line]);
}

impl LineSink for Renderer {
    fn width(&self) -> u16 {
        self.width
    }
    fn push_lines(&mut self, lines: &[Line]) {
        self.lines.extend_from_slice(lines);
    }
}

/// A renderable widget. Implemented by `Text`, `Markdown`, `AnimatedText`, etc.
pub trait View {
    fn render(&mut self, sink: &mut impl LineSink);
}

/// A blank `Rc<str>` line — shared singleton for empty rows.
pub fn blank_line() -> Line {
    Rc::from("")
}