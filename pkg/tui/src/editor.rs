//! Multi-line text editor with pill support.
//!
//! The [`Editor`] stores content as logical lines (`Vec<String>`).
//! Each line may contain pill sentinel characters (Unicode PUA) that
//! map to full content via a side table.
//!
//! Soft wrapping is derived at render time — it's never stored.
//! The cursor is `(line, col)` in logical coordinates, stable across
//! terminal resizes.
//!
//! # Pill creation
//!
//! Call [`Editor::paste`] with text longer than [`Editor::pill_threshold`]
//! characters and it automatically becomes a pill. Pills are displayed
//! as `label` inside a colored rounded element and deleted as one unit
//! with backspace/delete.
//!
//! # Usage
//!
//! ```ignore
//! let mut editor = Editor::new();
//! // In your App::update():
//! editor.handle_key(key, width);
//! // or for pastes:
//! editor.paste(&text);
//! // In your App::render():
//! editor.render(r, "  ❯ ");
//! ```

use crate::overlay::{SelectAction, SelectItem, SelectList};
use crate::ansi::{visible_width, RESET};
use crate::renderer::Renderer;
use crate::style::{Color, Theme};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::collections::HashMap;

/// Result of [`Editor::handle_key`].
#[must_use]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeyResult {
    /// The key was consumed by the editor (text changed or cursor moved).
    Consumed,
    /// Enter was pressed — the caller should submit/take the content.
    Submit,
    /// The key was not handled by the editor.
    Ignored,
    /// An overlay item was selected (e.g. slash command).
    Command(String),
}

// ── Pill rendering ──────────────────────────────────────────────


/// Pill data stored in the side table.
#[derive(Clone, Debug)]
struct PillData {
    label: String,
    content: String,
}

/// Start of the Private Use Area range for pill sentinels.
/// U+F0000 .. U+FFFFD (Supplementary PUA-A). One char per pill.
/// This limits us to ~65 534 pills per editor session, which is
/// acceptable: each pill represents a user paste action, and no
/// realistic session will reach this count. On overflow the
/// `expect` below panics — a deliberate choice over silent corruption.
const PUA_BASE: u32 = 0xF0000;
fn pill_sentinel(index: usize) -> char {
    char::from_u32(PUA_BASE + index as u32).expect("pill index overflow (>65 534 pills)")
}

fn is_pill_sentinel(c: char) -> bool {
    let v = c as u32;
    (PUA_BASE..0xFFFFE).contains(&v)
}

fn pill_index(c: char) -> usize {
    (c as u32 - PUA_BASE) as usize
}

fn fg(c: Color) -> String {
    format!("\x1b[{}m", c.fg_code())
}
fn bg_sgr(c: Color) -> String {
    format!("\x1b[{}m", c.bg_code())
}

fn render_pill(label: &str, pill_bg: Color, pill_fg: Color) -> String {
    let cfg = fg(pill_bg);
    let cbg = bg_sgr(pill_bg);
    let pfg = fg(pill_fg);
    format!("{cfg}\u{E0B6}{cbg}{pfg} {label} {RESET}{cfg}\u{E0B4}{RESET}")
}

fn pill_visual_width(label: &str) -> usize {
    4 + visible_width(label)
}

fn make_pill_label(content: &str, max_len: usize) -> String {
    let char_count = content.chars().count();
    if char_count <= max_len {
        content.to_string()
    } else {
        let prefix: String = content.chars().take(max_len - 5).collect();
        format!("{prefix}… ({char_count})")
    }
}

// ── Visual line map (for vertical cursor movement) ──────────────

/// A visual line segment — one row on screen.
#[derive(Debug)]
struct VisualLine {
    /// Index of the logical line this segment belongs to.
    logical: usize,
    /// Char offset where this segment starts within the logical line.
    start_col: usize,
    /// Char count of this segment.
    len: usize,
}

/// Build the visual line map by wrapping each logical line.
fn build_visual_lines(
    lines: &[String],
    width: usize,
    pills: &HashMap<usize, PillData>,
) -> Vec<VisualLine> {
    let w = if width == 0 { 80 } else { width };
    let mut out = Vec::new();

    for (i, line) in lines.iter().enumerate() {
        if line.is_empty() {
            out.push(VisualLine { logical: i, start_col: 0, len: 0 });
            continue;
        }

        let mut seg_start: usize = 0;
        let mut col: usize = 0;

        for (ci, ch) in line.chars().enumerate() {
            let cw = if is_pill_sentinel(ch) {
                let idx = pill_index(ch);
                pills.get(&idx).map(|p| pill_visual_width(&p.label)).unwrap_or(1)
            } else {
                unicode_width::UnicodeWidthChar::width(ch).unwrap_or(1)
            };

            if col + cw > w && col > 0 {
                out.push(VisualLine { logical: i, start_col: seg_start, len: ci - seg_start });
                seg_start = ci;
                col = 0;
            }
            col += cw;
        }

        out.push(VisualLine {
            logical: i,
            start_col: seg_start,
            len: line.chars().count() - seg_start,
        });
    }

    out
}

/// Find which visual line the cursor is on.
fn find_visual_line(vlines: &[VisualLine], cursor_line: usize, cursor_col: usize) -> usize {
    for (i, vl) in vlines.iter().enumerate() {
        if vl.logical != cursor_line { continue; }
        let is_last_seg = i + 1 >= vlines.len() || vlines[i + 1].logical != cursor_line;
        let in_range = if is_last_seg {
            cursor_col >= vl.start_col && cursor_col <= vl.start_col + vl.len
        } else {
            cursor_col >= vl.start_col && cursor_col < vl.start_col + vl.len
        };
        if in_range { return i; }
    }
    vlines.len().saturating_sub(1)
}

/// Compute the visual column of the cursor within a visual line segment.
fn visual_col_in_segment(
    line: &str,
    seg_start: usize,
    cursor_col: usize,
    pills: &HashMap<usize, PillData>,
) -> usize {
    let mut col = 0;
    for (ci, ch) in line.chars().enumerate().skip(seg_start) {
        if ci >= cursor_col { break; }
        col += if is_pill_sentinel(ch) {
            let idx = pill_index(ch);
            pills.get(&idx).map(|p| pill_visual_width(&p.label)).unwrap_or(1)
        } else {
            unicode_width::UnicodeWidthChar::width(ch).unwrap_or(1)
        };
    }
    col
}

/// Given a visual column target, find the char offset within a visual line segment.
fn char_at_visual_col(
    line: &str,
    seg_start: usize,
    seg_len: usize,
    target_vcol: usize,
    pills: &HashMap<usize, PillData>,
) -> usize {
    let mut col = 0;
    for (ci, ch) in line.chars().enumerate().skip(seg_start).take(seg_len) {
        let cw = if is_pill_sentinel(ch) {
            let idx = pill_index(ch);
            pills.get(&idx).map(|p| pill_visual_width(&p.label)).unwrap_or(1)
        } else {
            unicode_width::UnicodeWidthChar::width(ch).unwrap_or(1)
        };
        if col + cw > target_vcol { return ci; }
        col += cw;
    }
    seg_start + seg_len
}

// ── Layout for rendering ────────────────────────────────────────

struct LayoutLine {
    text: String,
    has_cursor: bool,
    cursor_vcol: usize,
}

fn layout_for_render(
    lines: &[String],
    width: usize,
    cursor_line: usize,
    cursor_col: usize,
    pills: &HashMap<usize, PillData>,
    pill_bg: Color,
    pill_fg: Color,
) -> Vec<LayoutLine> {
    let w = if width == 0 { 80 } else { width };
    let mut out = Vec::new();

    for (li, line) in lines.iter().enumerate() {
        let is_cursor_line = li == cursor_line;

        if line.is_empty() {
            out.push(LayoutLine {
                text: String::new(),
                has_cursor: is_cursor_line,
                cursor_vcol: 0,
            });
            continue;
        }

        // Build visual segments by walking chars.
        let chars: Vec<char> = line.chars().collect();
        let mut col: usize = 0;

        // We accumulate rendered text and cursor info per segment.
        let mut seg_rendered = String::new();
        let mut seg_cursor: Option<usize> = None; // visual col of cursor in this segment
        let mut seg_vcol: usize = 0;

        for (ci, &ch) in chars.iter().enumerate() {
            let (rendered, cw) = if is_pill_sentinel(ch) {
                let idx = pill_index(ch);
                if let Some(p) = pills.get(&idx) {
                    (render_pill(&p.label, pill_bg, pill_fg), pill_visual_width(&p.label))
                } else {
                    ("?".to_string(), 1)
                }
            } else {
                (ch.to_string(), unicode_width::UnicodeWidthChar::width(ch).unwrap_or(1))
            };

            if col + cw > w && col > 0 {
                // Emit current segment.
                out.push(LayoutLine {
                    text: std::mem::take(&mut seg_rendered),
                    has_cursor: seg_cursor.is_some(),
                    cursor_vcol: seg_cursor.unwrap_or(0),
                });
                col = 0;
                seg_vcol = 0;
                seg_cursor = None;
            }

            if is_cursor_line && ci == cursor_col {
                seg_cursor = Some(seg_vcol);
            }

            seg_rendered.push_str(&rendered);
            seg_vcol += cw;
            col += cw;
        }

        // Cursor can be at end of line (after last char).
        if is_cursor_line && cursor_col >= chars.len() {
            seg_cursor = Some(seg_vcol);
        }

        out.push(LayoutLine {
            text: seg_rendered,
            has_cursor: seg_cursor.is_some(),
            cursor_vcol: seg_cursor.unwrap_or(0),
        });
    }

    out
}

// ── Editor ──────────────────────────────────────────────────────

/// Tracks an active overlay popup.
struct ActiveOverlay {
    list: SelectList,
    /// The line the overlay was triggered on.
    trigger_line: usize,
    /// The column where the trigger text starts (e.g. where `/` was typed).
    trigger_col: usize,
}

/// Multi-line text editor with pill support and overlay popups.
pub struct Editor {
    /// Logical lines. Always at least one (empty string).
    lines: Vec<String>,
    /// Cursor logical line index.
    cursor_line: usize,
    /// Cursor char index within the logical line.
    cursor_col: usize,
    /// Sticky visual column for vertical movement.
    preferred_vcol: Option<usize>,
    /// Last known width (for vertical movement without explicit width).
    last_width: usize,

    /// Pill side table: sentinel index → pill data.
    pills: HashMap<usize, PillData>,
    /// Next pill index.
    pill_counter: usize,

    /// Slash command items. When set, typing `/` opens the overlay.
    commands: Vec<SelectItem>,
    /// Currently active overlay popup (if any).
    overlay: Option<ActiveOverlay>,

    pub pill_threshold: usize,
    pub pill_label_max: usize,
    pub pill_bg: Color,
    pub pill_fg: Color,
}

impl Default for Editor {
    fn default() -> Self {
        Self::new()
    }
}

impl Editor {
    pub fn new() -> Self {
        Self {
            lines: vec![String::new()],
            cursor_line: 0,
            cursor_col: 0,
            preferred_vcol: None,
            last_width: 80,
            pills: HashMap::new(),
            pill_counter: 0,
            commands: Vec::new(),
            overlay: None,
            pill_threshold: 50,
            pill_label_max: 25,
            pill_bg: Theme::default().pill_bg,
            pill_fg: Theme::default().pill_fg,
        }
    }

    /// Set available slash commands. Typing `/` will open an overlay.
    pub fn set_commands(&mut self, commands: Vec<SelectItem>) {
        self.commands = commands;
    }

    /// Apply theme colors to pill styling.
    pub fn apply_theme(&mut self, theme: &crate::style::Theme) {
        self.pill_bg = theme.pill_bg;
        self.pill_fg = theme.pill_fg;
    }

    /// Whether the overlay popup is currently visible.
    pub fn has_overlay(&self) -> bool {
        self.overlay.is_some()
    }

    /// Programmatically show a completion overlay at the current cursor.
    pub fn show_completions(&mut self, items: Vec<SelectItem>) {
        if items.is_empty() { return; }
        self.overlay = Some(ActiveOverlay {
            list: SelectList::new(items),
            trigger_line: self.cursor_line,
            trigger_col: self.cursor_col,
        });
    }

    /// Dismiss the overlay if active.
    pub fn dismiss_overlay(&mut self) {
        self.overlay = None;
    }

    // ── Helpers ─────────────────────────────────────────────────

    fn line_char_count(&self, li: usize) -> usize {
        self.lines[li].chars().count()
    }

    fn set_col(&mut self, col: usize) {
        self.cursor_col = col;
        self.preferred_vcol = None;
    }

    // ── Content access ──────────────────────────────────────────

    pub fn is_empty(&self) -> bool {
        self.lines.len() == 1 && self.lines[0].is_empty()
    }

    pub fn cursor(&self) -> (usize, usize) {
        (self.cursor_line, self.cursor_col)
    }

    /// Collect the plain text (pills expanded to full content).
    pub fn text(&self) -> String {
        let mut s = String::new();
        for (i, line) in self.lines.iter().enumerate() {
            if i > 0 { s.push('\n'); }
            for ch in line.chars() {
                if is_pill_sentinel(ch) {
                    let idx = pill_index(ch);
                    if let Some(p) = self.pills.get(&idx) {
                        s.push_str(&p.content);
                    }
                } else {
                    s.push(ch);
                }
            }
        }
        s
    }

    pub fn clear(&mut self) {
        self.lines = vec![String::new()];
        self.cursor_line = 0;
        self.cursor_col = 0;
        self.preferred_vcol = None;
        self.overlay = None;
        self.pills.clear();
        self.pill_counter = 0;
    }

    pub fn take(&mut self) -> String {
        let s = self.text();
        self.clear();
        s
    }

    pub fn stats(&self) -> (usize, usize, usize) {
        let mut chars = 0;
        let mut pills = 0;
        for line in &self.lines {
            for ch in line.chars() {
                if is_pill_sentinel(ch) { pills += 1; } else { chars += 1; }
            }
        }
        let newlines = self.lines.len().saturating_sub(1);
        (chars, pills, newlines)
    }

    // ── Editing operations ──────────────────────────────────────

    /// Insert a character at the cursor.
    pub fn insert_char(&mut self, c: char) {
        let byte_off = self.byte_offset(self.cursor_line, self.cursor_col);
        self.lines[self.cursor_line].insert(byte_off, c);
        self.set_col(self.cursor_col + 1);
    }

    /// Insert a newline, splitting the current line.
    pub fn insert_newline(&mut self) {
        let byte_off = self.byte_offset(self.cursor_line, self.cursor_col);
        let after = self.lines[self.cursor_line][byte_off..].to_string();
        self.lines[self.cursor_line].truncate(byte_off);
        self.lines.insert(self.cursor_line + 1, after);
        self.cursor_line += 1;
        self.set_col(0);
    }

    /// Insert a pill at the cursor.
    pub fn insert_pill(&mut self, content: String) {
        let label = make_pill_label(&content, self.pill_label_max);
        let idx = self.pill_counter;
        self.pill_counter += 1;
        self.pills.insert(idx, PillData { label, content });
        let sentinel = pill_sentinel(idx);
        let byte_off = self.byte_offset(self.cursor_line, self.cursor_col);
        self.lines[self.cursor_line].insert(byte_off, sentinel);
        self.set_col(self.cursor_col + 1);
    }

    /// Backspace: delete the char/pill before the cursor, or merge lines.
    pub fn backspace(&mut self) {
        if self.cursor_col > 0 {
            let byte_off = self.byte_offset(self.cursor_line, self.cursor_col - 1);
            let ch = self.lines[self.cursor_line][byte_off..].chars().next().unwrap();
            self.lines[self.cursor_line].drain(byte_off..byte_off + ch.len_utf8());
            if is_pill_sentinel(ch) {
                self.pills.remove(&pill_index(ch));
            }
            self.set_col(self.cursor_col - 1);
        } else if self.cursor_line > 0 {
            // Merge with previous line.
            let current = self.lines.remove(self.cursor_line);
            self.cursor_line -= 1;
            let new_col = self.line_char_count(self.cursor_line);
            self.lines[self.cursor_line].push_str(&current);
            self.set_col(new_col);
        }
    }

    /// Delete: delete the char/pill after the cursor, or merge lines.
    pub fn delete(&mut self) {
        let cc = self.line_char_count(self.cursor_line);
        if self.cursor_col < cc {
            let byte_off = self.byte_offset(self.cursor_line, self.cursor_col);
            let ch = self.lines[self.cursor_line][byte_off..].chars().next().unwrap();
            self.lines[self.cursor_line].drain(byte_off..byte_off + ch.len_utf8());
            if is_pill_sentinel(ch) {
                self.pills.remove(&pill_index(ch));
            }
            self.preferred_vcol = None;
        } else if self.cursor_line + 1 < self.lines.len() {
            let next = self.lines.remove(self.cursor_line + 1);
            self.lines[self.cursor_line].push_str(&next);
            self.preferred_vcol = None;
        }
    }

    /// Paste text. If char count > `pill_threshold`, creates a pill.
    pub fn paste(&mut self, text: &str) {
        if text.chars().count() > self.pill_threshold {
            self.insert_pill(text.to_string());
        } else {
            for c in text.chars() {
                if c == '\n' || c == '\r' {
                    self.insert_newline();
                } else if !c.is_control() {
                    self.insert_char(c);
                }
            }
        }
    }

    /// Convert char offset to byte offset within a line.
    fn byte_offset(&self, line: usize, char_col: usize) -> usize {
        self.lines[line]
            .char_indices()
            .nth(char_col)
            .map(|(i, _)| i)
            .unwrap_or(self.lines[line].len())
    }

    // ── Cursor movement ─────────────────────────────────────────

    pub fn move_left(&mut self) {
        if self.cursor_col > 0 {
            self.set_col(self.cursor_col - 1);
        } else if self.cursor_line > 0 {
            self.cursor_line -= 1;
            self.set_col(self.line_char_count(self.cursor_line));
        }
    }

    pub fn move_right(&mut self) {
        let cc = self.line_char_count(self.cursor_line);
        if self.cursor_col < cc {
            self.set_col(self.cursor_col + 1);
        } else if self.cursor_line + 1 < self.lines.len() {
            self.cursor_line += 1;
            self.set_col(0);
        }
    }

    pub fn move_home(&mut self) {
        self.set_col(0);
    }

    pub fn move_end(&mut self) {
        self.set_col(self.line_char_count(self.cursor_line));
    }

    pub fn move_up(&mut self, width: usize) {
        let w = if width == 0 { self.last_width } else { width };
        let vlines = build_visual_lines(&self.lines, w, &self.pills);
        let vi = find_visual_line(&vlines, self.cursor_line, self.cursor_col);
        if vi == 0 { return; }

        let cur_vl = &vlines[vi];
        let vcol = visual_col_in_segment(
            &self.lines[cur_vl.logical], cur_vl.start_col, self.cursor_col, &self.pills,
        );
        let target_vcol = self.preferred_vcol.unwrap_or(vcol);

        let target_vl = &vlines[vi - 1];
        let new_col = char_at_visual_col(
            &self.lines[target_vl.logical], target_vl.start_col, target_vl.len, target_vcol, &self.pills,
        );
        self.cursor_line = target_vl.logical;
        self.cursor_col = new_col;
        self.preferred_vcol = Some(target_vcol);
    }

    pub fn move_down(&mut self, width: usize) {
        let w = if width == 0 { self.last_width } else { width };
        let vlines = build_visual_lines(&self.lines, w, &self.pills);
        let vi = find_visual_line(&vlines, self.cursor_line, self.cursor_col);
        if vi + 1 >= vlines.len() { return; }

        let cur_vl = &vlines[vi];
        let vcol = visual_col_in_segment(
            &self.lines[cur_vl.logical], cur_vl.start_col, self.cursor_col, &self.pills,
        );
        let target_vcol = self.preferred_vcol.unwrap_or(vcol);

        let target_vl = &vlines[vi + 1];
        let new_col = char_at_visual_col(
            &self.lines[target_vl.logical], target_vl.start_col, target_vl.len, target_vcol, &self.pills,
        );
        self.cursor_line = target_vl.logical;
        self.cursor_col = new_col;
        self.preferred_vcol = Some(target_vcol);
    }

    /// Move cursor left by one word.
    pub fn word_left(&mut self) {
        if self.cursor_col == 0 {
            if self.cursor_line > 0 {
                self.cursor_line -= 1;
                self.set_col(self.line_char_count(self.cursor_line));
            }
            return;
        }

        let line = &self.lines[self.cursor_line];
        let chars: Vec<char> = line.chars().collect();
        let mut pos = self.cursor_col;

        // Skip whitespace.
        while pos > 0 && chars[pos - 1].is_whitespace() { pos -= 1; }
        // Skip word (non-whitespace), but pills are single atoms.
        if pos > 0 && is_pill_sentinel(chars[pos - 1]) {
            pos -= 1;
        } else {
            while pos > 0 && !chars[pos - 1].is_whitespace() && !is_pill_sentinel(chars[pos - 1]) {
                pos -= 1;
            }
        }

        self.set_col(pos);
    }

    /// Move cursor right by one word.
    pub fn word_right(&mut self) {
        let cc = self.line_char_count(self.cursor_line);
        if self.cursor_col >= cc {
            if self.cursor_line + 1 < self.lines.len() {
                self.cursor_line += 1;
                self.set_col(0);
            }
            return;
        }

        let line = &self.lines[self.cursor_line];
        let chars: Vec<char> = line.chars().collect();
        let mut pos = self.cursor_col;

        // Skip current word/pill.
        if pos < chars.len() && is_pill_sentinel(chars[pos]) {
            pos += 1;
        } else {
            while pos < chars.len() && !chars[pos].is_whitespace() && !is_pill_sentinel(chars[pos]) {
                pos += 1;
            }
        }
        // Skip trailing whitespace.
        while pos < chars.len() && chars[pos].is_whitespace() { pos += 1; }

        self.set_col(pos);
    }

    // ── Overlay helpers ────────────────────────────────────────

    /// Extract the current filter text from the trigger point to cursor.
    fn overlay_filter_text(&self) -> String {
        if let Some(ov) = &self.overlay {
            if ov.trigger_line == self.cursor_line && self.cursor_col > ov.trigger_col {
                let line = &self.lines[self.cursor_line];
                let start = line.char_indices()
                    .nth(ov.trigger_col)
                    .map(|(i, _)| i)
                    .unwrap_or(line.len());
                let end = line.char_indices()
                    .nth(self.cursor_col)
                    .map(|(i, _)| i)
                    .unwrap_or(line.len());
                return line[start..end].to_string();
            }
        }
        String::new()
    }

    /// Sync the overlay filter with current editor text.
    fn update_overlay_filter(&mut self) {
        let filter = self.overlay_filter_text();
        if let Some(ov) = &mut self.overlay {
            ov.list.filter(&filter);
        }
    }

    /// Check if cursor has moved before or to the trigger point, or
    /// the `/` trigger character was deleted → dismiss.
    fn check_overlay_dismiss(&mut self) {
        let dismiss = if let Some(ov) = &self.overlay {
            if self.cursor_line != ov.trigger_line || self.cursor_col < ov.trigger_col {
                true
            } else {
                // Check that the `/` is still there at trigger_col.
                let line = &self.lines[ov.trigger_line];
                line.chars().nth(ov.trigger_col) != Some('/')
            }
        } else {
            false
        };
        if dismiss {
            self.overlay = None;
        }
    }

    /// Try to open a slash command overlay. Called after `/` is inserted.
    fn try_open_slash(&mut self) {
        if self.commands.is_empty() { return; }

        // `/` must be at column 0 or preceded by whitespace.
        let col = self.cursor_col; // cursor is now after the `/`
        if col < 1 { return; }
        let slash_col = col - 1;
        if slash_col > 0 {
            let line = &self.lines[self.cursor_line];
            let prev = line.chars().nth(slash_col - 1);
            if prev.is_some_and(|c| !c.is_whitespace()) { return; }
        }

        let mut list = SelectList::new(self.commands.clone());
        list.max_visible = 5;
        list.filter(""); // show all
        self.overlay = Some(ActiveOverlay {
            list,
            trigger_line: self.cursor_line,
            trigger_col: slash_col,
        });
    }

    /// Accept the selected overlay item: replace trigger..cursor with value, close overlay.
    fn accept_overlay(&mut self) -> Option<String> {
        let ov = self.overlay.take()?;
        let value = ov.list.select()?;

        // Erase from trigger_col to current cursor.
        let line = &self.lines[ov.trigger_line];
        let start_byte = line.char_indices()
            .nth(ov.trigger_col)
            .map(|(i, _)| i)
            .unwrap_or(line.len());
        let end_byte = line.char_indices()
            .nth(self.cursor_col)
            .map(|(i, _)| i)
            .unwrap_or(line.len());
        let mut new_line = String::with_capacity(line.len());
        new_line.push_str(&line[..start_byte]);
        new_line.push_str(&value);
        new_line.push_str(&line[end_byte..]);
        let new_col = ov.trigger_col + value.chars().count();
        self.lines[ov.trigger_line] = new_line;
        self.cursor_col = new_col;
        self.preferred_vcol = None;

        Some(value)
    }

    // ── Key handling ────────────────────────────────────────────

    /// Handle a key event.
    ///
    /// - **Enter**: submit (returns [`KeyResult::Submit`])
    /// - **Alt+Enter** / **Option+Enter**: insert newline (all terminals)
    /// - **Shift+Enter**: insert newline (kitty protocol only)
    /// - **`\` + Enter**: insert newline (all terminals, fallback)
    /// - **Alt+Left/Right**: line start / end
    /// - **Ctrl/Super+Left/Right**, **Alt+b/f**: word/token jump
    /// - **Ctrl+A / Ctrl+E**: line start / end
    /// - **`/`**: opens slash command overlay (if commands are set)
    /// - **Tab**: accepts overlay selection
    pub fn handle_key(&mut self, key: KeyEvent, width: usize) -> KeyResult {
        self.last_width = if width > 0 { width } else { self.last_width };

        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        let sup = key.modifiers.contains(KeyModifiers::SUPER);
        let shift = key.modifiers.contains(KeyModifiers::SHIFT);

        // ── Overlay active: route keys there first ──
        if self.overlay.is_some() {
            // Let the list handle navigation keys.
            let action = self.overlay.as_mut().unwrap().list.handle_key(&key);
            match action {
                SelectAction::Selected(value) => {
                    // Replace trigger text with the selected value.
                    self.accept_overlay();
                    if value.starts_with('/') {
                        return KeyResult::Command(value);
                    }
                    return KeyResult::Consumed;
                }
                SelectAction::Completed(_value) => {
                    // Tab: fill the selected item + space, close overlay.
                    self.accept_overlay();
                    self.insert_char(' ');
                    return KeyResult::Consumed;
                }
                SelectAction::Dismissed => {
                    self.overlay = None;
                    return KeyResult::Consumed;
                }
                SelectAction::Consumed => {
                    return KeyResult::Consumed;
                }
                SelectAction::Ignored => {
                    // Fall through — let the editor handle the key,
                    // then update the overlay filter.
                }
            }
        }

        // ── Enter: submit vs newline ──
        if key.code == KeyCode::Enter {
            if shift || alt {
                self.overlay = None;
                self.insert_newline();
                return KeyResult::Consumed;
            }
            // Backslash escape: `\` + Enter → newline.
            if self.cursor_col > 0 {
                let line = &self.lines[self.cursor_line];
                let prev_char = line.chars().nth(self.cursor_col - 1);
                if prev_char == Some('\\') {
                    self.overlay = None;
                    self.cursor_col -= 1;
                    self.delete();
                    self.insert_newline();
                    return KeyResult::Consumed;
                }
            }
            // If overlay is active, accept selection on Enter too.
            if self.overlay.is_some() {
                if let Some(value) = self.accept_overlay() {
                    if value.starts_with('/') {
                        return KeyResult::Command(value);
                    }
                    return KeyResult::Consumed;
                }
            }
            return KeyResult::Submit;
        }

        // ── Modifier + arrow combos ──

        if alt && key.code == KeyCode::Left  { self.overlay = None; self.move_home(); return KeyResult::Consumed; }
        if alt && key.code == KeyCode::Right { self.overlay = None; self.move_end();  return KeyResult::Consumed; }

        if (ctrl || sup) && key.code == KeyCode::Left  { self.overlay = None; self.word_left();  return KeyResult::Consumed; }
        if (ctrl || sup) && key.code == KeyCode::Right { self.overlay = None; self.word_right(); return KeyResult::Consumed; }
        if alt && key.code == KeyCode::Char('b') { self.overlay = None; self.word_left();  return KeyResult::Consumed; }
        if alt && key.code == KeyCode::Char('f') { self.overlay = None; self.word_right(); return KeyResult::Consumed; }

        if ctrl && key.code == KeyCode::Char('a') { self.overlay = None; self.move_home(); return KeyResult::Consumed; }
        if ctrl && key.code == KeyCode::Char('e') { self.overlay = None; self.move_end();  return KeyResult::Consumed; }

        let result = match key.code {
            KeyCode::Char(c) if !ctrl => {
                self.insert_char(c);
                // After inserting `/`, try to open slash commands.
                if c == '/' {
                    self.try_open_slash();
                }
                KeyResult::Consumed
            }
            KeyCode::Backspace => { self.backspace(); KeyResult::Consumed }
            KeyCode::Delete    => { self.delete();    KeyResult::Consumed }
            KeyCode::Left      => { self.move_left();  KeyResult::Consumed }
            KeyCode::Right     => { self.move_right(); KeyResult::Consumed }
            KeyCode::Up        => { self.move_up(width);   KeyResult::Consumed }
            KeyCode::Down      => { self.move_down(width); KeyResult::Consumed }
            KeyCode::Home      => { self.move_home(); KeyResult::Consumed }
            KeyCode::End       => { self.move_end();  KeyResult::Consumed }
            _ => KeyResult::Ignored,
        };

        // After any text change, update overlay filter or dismiss.
        if self.overlay.is_some() {
            self.check_overlay_dismiss();
            self.update_overlay_filter();
        }

        result
    }

    // ── Rendering ───────────────────────────────────────────────

    /// Render the editor into the renderer.
    ///
    /// `left_margin` is prepended to the first line; continuation lines
    /// are indented by its visible width.
    ///
    /// If an overlay is active, it composites on top of the lines
    /// already in the renderer (appearing above the editor input).
    pub fn render(&self, r: &mut Renderer, left_margin: &str) {
        let margin_w = visible_width(left_margin);
        let editor_w = (r.width() as usize).saturating_sub(margin_w);

        // Remember where the editor starts — overlay goes above this.
        let editor_start_row = r.line_count();

        // ── Editor input ──
        let layout = layout_for_render(
            &self.lines, editor_w,
            self.cursor_line, self.cursor_col,
            &self.pills, self.pill_bg, self.pill_fg,
        );

        let base_row = r.line_count();
        let continuation = " ".repeat(margin_w);

        for (i, ll) in layout.iter().enumerate() {
            let prefix = if i == 0 { left_margin } else { &continuation };
            r.push_line(format!("{prefix}{}", ll.text));

            if ll.has_cursor {
                r.set_cursor(base_row + i, margin_w + ll.cursor_vcol);
            }
        }

        // ── Overlay compositing (on top of lines above the editor) ──
        if let Some(ov) = &self.overlay {
            // Overlay starts 1 col after the `/` trigger, so labels align
            // with the text the user is typing after `/`.
            let overlay_col = margin_w + 1;
            let overlay_w = editor_w.saturating_sub(1).min(r.width() as usize / 2).max(20);
            let overlay_lines = ov.list.render(overlay_w);
            if !overlay_lines.is_empty() {
                let overlay_h = overlay_lines.len();
                let start = editor_start_row.saturating_sub(overlay_h);
                for (i, line) in overlay_lines.into_iter().enumerate() {
                    let row = start + i;
                    if row < editor_start_row {
                        r.composite_at(row, overlay_col, &line, overlay_w);
                    }
                }
            }
        }
    }
}
