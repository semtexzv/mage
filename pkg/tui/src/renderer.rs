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
use crate::style::{Color, Padding, Theme};
use crate::wrap::wrap_text;

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
    pub fn push_blank(&mut self) {
        self.lines.push(Rc::from(""));
    }

    /// Append a slice of lines to the current frame (for cached widget output).
    pub fn push_lines(&mut self, lines: &[Line]) {
        self.lines.extend_from_slice(lines);
    }

    /// Set the cursor position for an Input view.
    pub fn set_cursor(&mut self, row: usize, col: usize) {
        self.cursor = Some(CursorPos { row, col });
    }

    /// Overwrite an already-pushed line for overlay compositing.
    /// The diff engine sees the changed line and repaints it.
    pub fn overwrite_line(&mut self, row: usize, content: impl Into<Line>) {
        if row < self.lines.len() {
            self.lines[row] = content.into();
        }
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

    /// Push word-wrapped text with padding and optional background fill.
    pub fn push_text_styled(&mut self, content: &str, padding: &Padding, bg: Option<Color>) {
        let w = self.width as usize;
        let left = padding.left as usize;
        let right = padding.right as usize;
        let inner_width = w.saturating_sub(left).saturating_sub(right);
        let left_prefix = if left > 0 { " ".repeat(left) } else { String::new() };
        for _ in 0..padding.top {
            if let Some(color) = bg {
                self.push_line(Self::bg_filled_line("", w, color));
            } else if left > 0 {
                self.push_line(left_prefix.as_str());
            } else {
                self.push_blank();
            }
        }
        // Word-wrap and emit content lines
        if inner_width > 0 && !content.is_empty() {
            let wrapped = wrap_text(content, inner_width);
            for line in &wrapped {
                let left_pad = " ".repeat(left);
                let padded = format!("{}{}", left_pad, line);
                if let Some(color) = bg {
                    self.push_line(Self::bg_filled_line(&padded, w, color));
                } else {
                    self.push_line(padded);
                }
            }
        } else if !content.is_empty() {
            // Width too small, just push content
            self.push_line(content);
        }
        // Emit bottom padding
        for _ in 0..padding.bottom {
            if let Some(color) = bg {
                self.push_line(Self::bg_filled_line("", w, color));
            } else if left > 0 {
                self.push_line(left_prefix.as_str());
            } else {
                self.push_blank();
            }
        }
    }

    /// Push an input line with prompt and set cursor position.
    pub fn push_input(&mut self, prompt: &str, content: &str, cursor: usize) {
        let line = format!("{}{}", prompt, content);
        let row = self.lines.len();
        self.push_line(line);
        let col = visible_width(prompt) + cursor;
        self.set_cursor(row, col);
    }
    /// Create a line filled to `width` with a background color.
    /// The visible content is padded with spaces to fill the full width.
    pub fn bg_filled_line(content: &str, width: usize, color: Color) -> String {
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
        let shrunk = !is_first && lines.len() < self.prev_lines.len();

        if is_first || width_changed || shrunk {
            self.full_render(term, &lines, w, th, true);
        } else {
            self.diff_render(term, &lines, w, th);
        }

        self.position_cursor(term, cursor, lines.len());
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
            buf.push_str(CLEAR_SCROLLBACK);
            buf.push_str(CLEAR_SCREEN);
            buf.push_str(CURSOR_HOME);
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

    // ── Differential render ─────────────────────────────────────

    fn diff_render(&mut self, term: &mut dyn Terminal, lines: &[Line], width: u16, th: usize) {
        let old_len = self.prev_lines.len();
        let max_len = old_len.max(lines.len());

        // Find changed range using Rc::ptr_eq for O(1) per-line comparison.
        let mut first: Option<usize> = None;
        let mut last: Option<usize> = None;
        for i in 0..max_len {
            let same = match (self.prev_lines.get(i), lines.get(i)) {
                (Some(a), Some(b)) => Rc::ptr_eq(a, b),
                (None, None) => true,
                _ => false,
            };
            if !same {
                if first.is_none() {
                    first = Some(i);
                }
                last = Some(i);
            }
        }

        // Detect appended lines.
        if lines.len() > old_len {
            if first.is_none() {
                first = Some(old_len);
            }
            last = Some(lines.len().saturating_sub(1));
        }

        // Nothing changed.
        if first.is_none() {
            self.max_lines = self.max_lines.max(lines.len());
            self.prev_vp_top = self.max_lines.saturating_sub(th);
            self.prev_lines = lines.to_vec();
            if self.prev_lines.len() > MAX_PREV_LINES {
                let drain = self.prev_lines.len() - MAX_PREV_LINES;
                self.prev_lines.drain(..drain);
            }
            self.prev_width = width;
            return;
        }

        let first = first.unwrap();
        let last = last.unwrap_or(first);

        // If first change is above what was previously visible, full re-render.
        let prev_content_vp = old_len.saturating_sub(th);
        if first < prev_content_vp {
            self.full_render(term, lines, width, th, true);
            return;
        }

        // All changes in deleted tail — clear those lines.
        if first >= lines.len() {
            if old_len > lines.len() {
                let extra = old_len - lines.len();
                if extra > th {
                    self.full_render(term, lines, width, th, true);
                    return;
                }
                let target = lines.len().saturating_sub(1);
                let mut buf = String::new();
                buf.push_str(SYNC_BEGIN);
                self.move_cursor_to(&mut buf, target, th);
                buf.push_str(CR);
                if extra > 0 {
                    buf.push_str(&cursor_down(1));
                }
                for i in 0..extra {
                    buf.push_str(CR);
                    buf.push_str(CLEAR_LINE);
                    if i < extra - 1 {
                        buf.push_str(&cursor_down(1));
                    }
                }
                if extra > 0 {
                    buf.push_str(&cursor_up(extra));
                }
                buf.push_str(SYNC_END);
                term.write(&buf);
                term.flush();
                self.hw_cursor_row = target;
            }
            self.max_lines = self.max_lines.max(lines.len());
            self.prev_vp_top = self.max_lines.saturating_sub(th);
            self.prev_lines = lines.to_vec();
            if self.prev_lines.len() > MAX_PREV_LINES {
                let drain = self.prev_lines.len() - MAX_PREV_LINES;
                self.prev_lines.drain(..drain);
            }
            self.prev_width = width;
            return;
        }

        // ── Normal diff path ────────────────────────────────────
        let appended = lines.len() > old_len;
        let append_start = appended && first == old_len && first > 0;
        let move_target = if append_start { first - 1 } else { first };

        let mut buf = String::new();
        buf.push_str(SYNC_BEGIN);

        let prev_vp_bottom = self.prev_vp_top + th.saturating_sub(1);
        if th > 0 && move_target > prev_vp_bottom {
            let cur_screen = self
                .hw_cursor_row
                .saturating_sub(self.prev_vp_top)
                .min(th.saturating_sub(1));
            let to_bottom = th.saturating_sub(1).saturating_sub(cur_screen);
            if to_bottom > 0 {
                buf.push_str(&cursor_down(to_bottom));
            }
            let scroll = move_target - prev_vp_bottom;
            for _ in 0..scroll {
                buf.push_str(CRLF);
            }
            self.hw_cursor_row = move_target;
            self.prev_vp_top += scroll;
        }

        self.move_cursor_to(&mut buf, move_target, th);

        if append_start {
            buf.push_str(CRLF);
        } else {
            buf.push_str(CR);
        }

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

        if old_len > lines.len() {
            if render_end < lines.len().saturating_sub(1) {
                let down = lines.len().saturating_sub(1) - render_end;
                buf.push_str(&cursor_down(down));
                final_row = lines.len().saturating_sub(1);
            }
            let extra = old_len - lines.len();
            for _ in lines.len()..old_len {
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

        self.hw_cursor_row = final_row;
        self.max_lines = self.max_lines.max(lines.len());
        self.prev_vp_top = self.max_lines.saturating_sub(th);
        self.prev_lines = lines.to_vec();
        if self.prev_lines.len() > MAX_PREV_LINES {
            let drain = self.prev_lines.len() - MAX_PREV_LINES;
            self.prev_lines.drain(..drain);
        }
        self.prev_width = width;
    }

    // ── Helpers ─────────────────────────────────────────────────

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
        let mut buf = String::new();
        if target_row > self.hw_cursor_row {
            buf.push_str(&cursor_down(target_row - self.hw_cursor_row));
        } else if target_row < self.hw_cursor_row {
            buf.push_str(&cursor_up(self.hw_cursor_row - target_row));
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

    fn move_cursor_to(&mut self, buf: &mut String, target: usize, th: usize) {
        let vp_top = self.max_lines.saturating_sub(th);
        let max_s = th.saturating_sub(1);
        let cur_s = self
            .hw_cursor_row
            .saturating_sub(self.prev_vp_top)
            .min(max_s);
        let tgt_s = target.saturating_sub(vp_top).min(max_s);
        let delta = tgt_s as isize - cur_s as isize;
        if delta > 0 {
            buf.push_str(&cursor_down(delta as usize));
        } else if delta < 0 {
            buf.push_str(&cursor_up((-delta) as usize));
        }
        self.hw_cursor_row = target;
    }
}
