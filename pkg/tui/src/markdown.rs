//! Incremental markdown renderer — parses markdown to styled terminal lines.
//!
//! Designed for streaming LLM output: `append()` new text, call `lines()`.
//! Only the last (potentially incomplete) block is re-rendered on each update;
//! earlier blocks reuse cached `Rc<str>` lines for O(1) diff via `Rc::ptr_eq`.
//!
//! ```text
//! let mut md = Markdown::new(80);
//! md.append("# Hello\n\nSome **bold** text");
//! let lines = md.lines(); // Vec<Rc<str>>
//! md.append("\n\n```rust\nfn main() {}\n```");
//! let lines = md.lines(); // heading + paragraph reused, code block rendered fresh
//! ```

use std::fmt::Write;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::ops::Range;
use std::rc::Rc;

use pulldown_cmark::{Alignment, CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd};

use crate::ansi::{visible_width, RESET};
use crate::style::{Color, Padding, StyleStack, Style};
use crate::wrap::wrap_text;

/// Markdown-specific color palette, derived from [`Theme`](crate::style::Theme).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct MdColors {
    pub heading: Color,
    pub link: Color,
    pub link_url: Color,
    pub code: Color,
    pub code_block: Color,
    pub code_block_border: Color,
    pub quote: Color,
    pub quote_border: Color,
    pub hr: Color,
    pub list_bullet: Color,
}
impl Default for MdColors {
    fn default() -> Self {
        Self {
            heading: Color::Yellow,
            link: Color::Blue,
            link_url: Color::Ansi256(245),
            code: Color::Cyan,
            code_block: Color::Green,
            code_block_border: Color::Ansi256(245),
            quote: Color::Ansi256(245),
            quote_border: Color::Green,
            hr: Color::Ansi256(245),
            list_bullet: Color::Cyan,
        }
    }
}
pub(crate) use crate::renderer::Line;

const MD_OPTS: Options = Options::ENABLE_TABLES
    .union(Options::ENABLE_STRIKETHROUGH)
    .union(Options::ENABLE_TASKLISTS);

// ── Public API ────────────────────────────────────────────────────────────

struct CachedBlock {
    hash: u64,
    lines: Vec<Line>,
}

/// Incremental markdown renderer with block-level caching.
pub struct Markdown {
    source: String,
    blocks: Vec<CachedBlock>,
    output: Vec<Line>,
    /// Outer width (terminal or container).
    outer_w: u16,
    pad: Padding,
    bg: Option<Color>,
    dirty: bool,
    colors: MdColors,
}

impl Markdown {
    pub fn new(width: u16) -> Self {
        Self {
            source: String::new(),
            blocks: Vec::new(),
            output: Vec::new(),
            outer_w: width,
            pad: Padding::ZERO,
            bg: None,
            dirty: true,
            colors: MdColors::default(),
        }
    }

    pub fn with_pad(width: u16, pad: Padding) -> Self {
        Self {
            source: String::new(),
            blocks: Vec::new(),
            output: Vec::new(),
            outer_w: width,
            pad,
            bg: None,
            dirty: true,
            colors: MdColors::default(),
        }
    }

    /// Apply theme colors to markdown rendering.
    pub fn apply_theme(&mut self, theme: &crate::style::Theme) {
        let new = MdColors {
            heading: theme.md_heading,
            link: theme.md_link,
            link_url: theme.md_link_url,
            code: theme.md_code,
            code_block: theme.md_code_block,
            code_block_border: theme.md_code_block_border,
            quote: theme.md_quote,
            quote_border: theme.md_quote_border,
            hr: theme.md_hr,
            list_bullet: theme.md_list_bullet,
        };
        // Invalidate cache if colors changed.
        if new != self.colors {
            self.colors = new;
            self.blocks.clear();
            self.dirty = true;
        }
    }

    /// Set background color. Lines are filled to the outer width.
    pub fn set_bg(&mut self, bg: Option<Color>) {
        if bg != self.bg {
            self.bg = bg;
            self.dirty = true;
        }
    }

    /// Inner width after padding.
    fn inner_w(&self) -> u16 {
        self.outer_w.saturating_sub(self.pad.h())
    }

    pub fn width(&self) -> u16 {
        self.outer_w
    }

    /// Set outer width. Inner width recomputed from padding.
    pub fn set_width(&mut self, w: u16) {
        if w != self.outer_w {
            self.outer_w = w;
            self.blocks.clear();
            self.dirty = true;
        }
    }

    pub fn source(&self) -> &str {
        &self.source
    }

    /// Append text (streaming). Only the last block is re-rendered.
    pub fn append(&mut self, text: &str) {
        if !text.is_empty() {
            self.source.push_str(text);
            self.dirty = true;
        }
    }

    /// Replace the full source. If it's a pure append, cached blocks are preserved.
    pub fn set_source(&mut self, text: String) {
        if text != self.source {
            if !text.starts_with(&self.source) {
                self.blocks.clear();
            }
            self.source = text;
            self.dirty = true;
        }
    }

    /// Clear all content and cached state.
    pub fn clear(&mut self) {
        self.source.clear();
        self.blocks.clear();
        self.output.clear();
        self.dirty = true;
    }

    /// Get rendered lines (with left padding already applied).
    /// Re-renders only changed blocks.
    pub fn lines(&mut self) -> &[Line] {
        if self.dirty {
            self.rebuild();
        }
        &self.output
    }

    /// Render into a `Renderer`. Call `lines()` first (e.g. in
    /// `App::update`) to ensure the cache is fresh.
    pub fn render(&mut self, r: &mut crate::renderer::Renderer) {
        if self.dirty {
            self.rebuild();
        }
        r.push_lines(&self.output);
    }

    fn rebuild(&mut self) {
        let iw = self.inner_w();
        let left = self.pad.left as usize;
        let ow = self.outer_w as usize;
        let prefix = if left > 0 { " ".repeat(left) } else { String::new() };

        // Base style for bg fill (if set).
        let base = self.bg.map(|c| Style { bg: Some(c), ..Style::NONE });
        let bg_sgr = base.as_ref().map(|s| s.to_sgr()).unwrap_or_default();

        // Wrap a raw inner line with padding + optional bg fill.
        let wrap_line = |content: &str| -> Line {
            if base.is_some() {
                let padded = format!("{prefix}{content}");
                let vw = visible_width(&padded);
                let fill = ow.saturating_sub(vw);
                // Re-establish bg before fill spaces (content may contain
                // transitions that affect fg/bold but never bg, so bg
                // survives — but re-emitting is cheap insurance).
                Rc::from(format!("{bg_sgr}{padded}{bg_sgr}{}{RESET}", " ".repeat(fill)).as_str())
            } else if left > 0 {
                Rc::from(format!("{prefix}{content}").as_str())
            } else {
                Rc::from(content)
            }
        };

        let ranges = detect_blocks(&self.source);
        let old_len = self.blocks.len();
        let mut new_blocks = Vec::with_capacity(ranges.len());
        let mut output = Vec::new();

        // Top padding
        let blank = wrap_line("");
        for _ in 0..self.pad.top {
            output.push(blank.clone());
        }

        for (i, range) in ranges.iter().enumerate() {
            let slice = &self.source[range.clone()];
            let hash = content_hash(slice, iw);

            // Reuse if: same index, same hash, NOT the last old block (might have been incomplete)
            let reuse = i < old_len.saturating_sub(1) && self.blocks[i].hash == hash;

            if reuse {
                let lines = self.blocks[i].lines.clone();
                output.extend_from_slice(&lines);
                new_blocks.push(CachedBlock { hash, lines });
            } else {
                let raw = render_block(slice, iw, self.colors);
                let lines: Vec<Line> = raw.iter().map(|l| wrap_line(l)).collect();
                output.extend_from_slice(&lines);
                new_blocks.push(CachedBlock { hash, lines });
            }
        }

        // Strip trailing blank line before bottom padding
        if output.last().is_some_and(|l| {
            let s: &str = l;
            s.is_empty() || visible_width(s) == 0
        }) {
            output.pop();
        }

        // Bottom padding
        for _ in 0..self.pad.bottom {
            output.push(blank.clone());
        }

        self.output = output;
        self.blocks = new_blocks;
        self.dirty = false;
    }
}

// ── Block detection ───────────────────────────────────────────────────────

/// Parse the full source and return byte ranges of top-level blocks.
#[allow(unused_assignments)]
fn detect_blocks(source: &str) -> Vec<Range<usize>> {
    let parser = Parser::new_ext(source, MD_OPTS).into_offset_iter();
    let mut blocks: Vec<Range<usize>> = Vec::new();
    let mut depth: usize = 0;
    let mut start: Option<usize> = None;
    let mut block_end: usize = source.len();

    for (event, range) in parser {
        if start.is_none() {
            start = Some(range.start);
        }
        block_end = range.end;

        match event {
            Event::Start(_) => depth += 1,
            Event::End(_) => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    if let Some(s) = start.take() {
                        blocks.push(s..block_end);
                    }
                }
            }
            // Standalone events (Rule, TaskListMarker at depth 0, etc.)
            _ if depth == 0 => {
                if let Some(s) = start.take() {
                    blocks.push(s..block_end);
                }
            }
            _ => {}
        }
    }

    // Incomplete block at end (unclosed fence, etc.)
    if let Some(s) = start {
        blocks.push(s..source.len());
    }

    blocks
}

// ── Block rendering ───────────────────────────────────────────────────────

fn render_block(source: &str, width: u16, colors: MdColors) -> Vec<Line> {
    let parser = Parser::new_ext(source, MD_OPTS);
    let mut ctx = Ctx::new(width as usize, source, colors);
    for ev in parser {
        ctx.event(ev);
    }
    ctx.finish()
}

fn content_hash(source: &str, width: u16) -> u64 {
    let mut h = DefaultHasher::new();
    source.hash(&mut h);
    width.hash(&mut h);
    h.finish()
}

// ── Style helpers ─────────────────────────────────────────────────────────

/// Style presets for inline markdown elements.
const SS_BOLD: Style = Style { bold: true, dim: false, italic: false, underline: false, strikethrough: false, fg: None, bg: None };
const SS_ITALIC: Style = Style { bold: false, dim: false, italic: true, underline: false, strikethrough: false, fg: None, bg: None };
const SS_STRIKE: Style = Style { bold: false, dim: false, italic: false, underline: false, strikethrough: true, fg: None, bg: None };

// ── Render context (state machine) ────────────────────────────────────────

struct ListInfo {
    ordered: bool,
    next_idx: u64,
}

struct TableCtx {
    aligns: Vec<Alignment>,
    head: Vec<String>,
    rows: Vec<Vec<String>>,
    row: Vec<String>,
    cell: String,
    cell_styles: StyleStack,
    in_head: bool,
}

struct Ctx<'s> {
    w: usize,
    /// Raw source text of the single block being rendered (NOT the full document).
    block_source: &'s str,
    out: Vec<Line>,
    /// Inline text accumulator
    buf: String,
    /// Style stack for inline formatting
    styles: StyleStack,
    /// Current heading level, if inside a heading
    heading: Option<HeadingLevel>,
    /// Code block language, if inside a code block
    code_block: Option<String>,
    code_buf: String,
    /// Stack of list contexts (for nesting)
    lists: Vec<ListInfo>,
    /// Table state, if inside a table
    table: Option<TableCtx>,
    /// Blockquote nesting depth
    quote_depth: usize,
    /// Link URL (set on Start(Link), consumed on End(Link))
    link_url: Option<String>,
    /// Buffer position at start of link text (for dedup)
    link_text_start: usize,
    /// Theme-derived colors for markdown elements.
    colors: MdColors,
}

impl<'s> Ctx<'s> {
    fn new(w: usize, block_source: &'s str, colors: MdColors) -> Self {
        Self {
            w,
            block_source,
            out: Vec::new(),
            buf: String::new(),
            styles: StyleStack::new(),
            heading: None,
            code_block: None,
            code_buf: String::new(),
            lists: Vec::new(),
            table: None,
            quote_depth: 0,
            link_url: None,
            link_text_start: 0,
            colors,
        }
    }

    fn finish(mut self) -> Vec<Line> {
        // Flush any remaining inline text
        if !self.buf.is_empty() {
            self.flush_text();
        }
        self.out
    }

    /// Push a style onto the inline stack and append the transition to buf.
    fn push_style(&mut self, ss: Style) {
        let t = self.styles.push(ss).to_string();
        self.buf.push_str(&t);
    }

    /// Pop a style from the inline stack and append the transition to buf.
    fn pop_style(&mut self) {
        let t = self.styles.pop().to_string();
        self.buf.push_str(&t);
    }

    // ── Event dispatch ────────────────────────────────────────────────

    fn event(&mut self, ev: Event) {
        // Code block mode — only collect text
        if self.code_block.is_some() {
            match ev {
                Event::Text(t) => self.code_buf.push_str(&t),
                Event::End(TagEnd::CodeBlock) => self.flush_code_block(),
                _ => {}
            }
            return;
        }

        // Table mode — collect cells
        if self.table.is_some() {
            self.table_event(ev);
            return;
        }

        match ev {
            // ── Block starts ──────────────────────────────────────────
            Event::Start(Tag::Heading { level, .. }) => {
                self.heading = Some(level);
            }
            Event::Start(Tag::Paragraph) => {}
            Event::Start(Tag::CodeBlock(kind)) => {
                let lang = match kind {
                    CodeBlockKind::Fenced(l) => l.to_string(),
                    CodeBlockKind::Indented => String::new(),
                };
                self.code_block = Some(lang);
                self.code_buf.clear();
            }
            Event::Start(Tag::List(start)) => {
                // Flush parent item text before entering nested list
                if !self.buf.is_empty() {
                    self.flush_text();
                }
                self.lists.push(ListInfo {
                    ordered: start.is_some(),
                    next_idx: start.unwrap_or(0),
                });
            }
            Event::Start(Tag::Item) => {}
            Event::Start(Tag::BlockQuote(_)) => {
                self.quote_depth += 1;
            }
            Event::Start(Tag::Table(aligns)) => {
                self.table = Some(TableCtx {
                    aligns,
                    head: Vec::new(),
                    rows: Vec::new(),
                    row: Vec::new(),
                    cell: String::new(),
                    cell_styles: StyleStack::new(),
                    in_head: false,
                });
            }

            // ── Block ends ────────────────────────────────────────────
            Event::End(TagEnd::Heading(_)) => self.flush_heading(),
            Event::End(TagEnd::Paragraph) => self.flush_text(),
            Event::End(TagEnd::Item) => {
                // Tight lists: text arrives without Paragraph wrapper
                if !self.buf.is_empty() {
                    self.flush_text();
                }
                if let Some(list) = self.lists.last_mut() {
                    list.next_idx += 1;
                }
            }
            Event::End(TagEnd::List(_)) => {
                self.lists.pop();
                if self.lists.is_empty() {
                    self.push_blank();
                }
            }
            Event::End(TagEnd::BlockQuote(_)) => {
                self.quote_depth = self.quote_depth.saturating_sub(1);
                if self.quote_depth == 0 {
                    self.push_blank();
                }
            }

            // ── Inline formatting ─────────────────────────────────────
            Event::Text(t) => self.buf.push_str(&t),
            Event::Code(t) => {
                self.push_style(Style { fg: Some(self.colors.code), ..Style::NONE });
                self.buf.push('`');
                self.buf.push_str(&t);
                self.buf.push('`');
                self.pop_style();
            }
            Event::SoftBreak => self.buf.push(' '),
            Event::HardBreak => self.buf.push('\n'),

            Event::Start(Tag::Strong) => self.push_style(SS_BOLD),
            Event::End(TagEnd::Strong) => self.pop_style(),
            Event::Start(Tag::Emphasis) => self.push_style(SS_ITALIC),
            Event::End(TagEnd::Emphasis) => self.pop_style(),
            Event::Start(Tag::Strikethrough) => self.push_style(SS_STRIKE),
            Event::End(TagEnd::Strikethrough) => self.pop_style(),

            Event::Start(Tag::Link { dest_url, .. }) => {
                self.link_url = Some(dest_url.to_string());
                self.link_text_start = self.buf.len();
                self.push_style(Style {
                    underline: true,
                    fg: Some(self.colors.link),
                    ..Style::NONE
                });
            }
            Event::End(TagEnd::Link) => {
                self.pop_style();
                // Show URL if it differs from the visible link text
                if let Some(url) = self.link_url.take() {
                    let link_text = crate::ansi::strip_ansi(&self.buf[self.link_text_start..]);
                    let url_cmp = url.strip_prefix("mailto:").unwrap_or(&url);
                    if link_text != url_cmp {
                        self.push_style(Style { fg: Some(self.colors.link_url), ..Style::NONE });
                        write!(self.buf, " ({url})").ok();
                        self.pop_style();
                    }
                }
            }
            Event::Start(Tag::Image { dest_url, .. }) => {
                self.push_style(Style { fg: Some(self.colors.link_url), ..Style::NONE });
                self.buf.push_str("[img: ");
                self.link_url = Some(dest_url.to_string());
                self.link_text_start = self.buf.len();
            }
            Event::End(TagEnd::Image) => {
                self.buf.push(']');
                self.pop_style();
                self.link_url = None;
            }

            Event::TaskListMarker(checked) => {
                self.buf.push_str(if checked {
                    "\u{2611} " // ☑
                } else {
                    "\u{2610} " // ☐
                });
            }

            Event::Rule => {
                let rule_w = self.effective_width().min(80);
                let hr_style = Style { fg: Some(self.colors.hr), ..Style::NONE };
                let hr_on = hr_style.to_sgr();
                let hr_off = Style::NONE.transition_from(&hr_style);
                self.push_styled(&format!("{hr_on}{}{hr_off}", "─".repeat(rule_w)));
                self.push_blank();
            }

            // Ignore everything else (HTML, footnotes, metadata)
            _ => {}
        }
    }

    // ── Table events ──────────────────────────────────────────────────

    fn table_event(&mut self, ev: Event) {
        let tbl = self.table.as_mut().unwrap();
        match ev {
            Event::Start(Tag::TableHead) => tbl.in_head = true,
            Event::End(TagEnd::TableHead) => {
                tbl.in_head = false;
                tbl.head = std::mem::take(&mut tbl.row);
            }
            Event::Start(Tag::TableRow) => {}
            Event::End(TagEnd::TableRow) => {
                let row = std::mem::take(&mut tbl.row);
                tbl.rows.push(row);
            }
            Event::Start(Tag::TableCell) => {
                tbl.cell.clear();
                tbl.cell_styles = StyleStack::new();
            }
            Event::End(TagEnd::TableCell) => {
                let cell = std::mem::take(&mut tbl.cell);
                tbl.row.push(cell);
            }
            Event::End(TagEnd::Table) => self.flush_table(),
            // Inline events inside cells — use cell_styles stack
            Event::Text(t) => tbl.cell.push_str(&t),
            Event::Code(t) => {
                let t_on = tbl.cell_styles.push(Style { fg: Some(self.colors.code), ..Style::NONE }).to_string();
                tbl.cell.push_str(&t_on);
                tbl.cell.push('`');
                tbl.cell.push_str(&t);
                tbl.cell.push('`');
                let t_off = tbl.cell_styles.pop().to_string();
                tbl.cell.push_str(&t_off);
            }
            Event::Start(Tag::Strong) => {
                let t = tbl.cell_styles.push(SS_BOLD).to_string();
                tbl.cell.push_str(&t);
            }
            Event::End(TagEnd::Strong) => {
                let t = tbl.cell_styles.pop().to_string();
                tbl.cell.push_str(&t);
            }
            Event::Start(Tag::Emphasis) => {
                let t = tbl.cell_styles.push(SS_ITALIC).to_string();
                tbl.cell.push_str(&t);
            }
            Event::End(TagEnd::Emphasis) => {
                let t = tbl.cell_styles.pop().to_string();
                tbl.cell.push_str(&t);
            }
            Event::Start(Tag::Strikethrough) => {
                let t = tbl.cell_styles.push(SS_STRIKE).to_string();
                tbl.cell.push_str(&t);
            }
            Event::End(TagEnd::Strikethrough) => {
                let t = tbl.cell_styles.pop().to_string();
                tbl.cell.push_str(&t);
            }
            Event::SoftBreak | Event::HardBreak => tbl.cell.push(' '),
            _ => {}
        }
    }

    // ── Flush helpers ─────────────────────────────────────────────────

    fn flush_heading(&mut self) {
        let text = std::mem::take(&mut self.buf);
        let level = self.heading.take().unwrap_or(HeadingLevel::H1);
        let num = level as u8;

        // Build heading style
        let mut heading_ss = Style {
            bold: true,
            fg: Some(self.colors.heading),
            ..Style::NONE
        };
        if num == 1 {
            heading_ss.underline = true;
        }
        let start = heading_ss.to_sgr();
        let end = Style::NONE.transition_from(&heading_ss);

        let styled = if num <= 2 {
            format!("{start}{text}{end}")
        } else {
            let prefix = "#".repeat(num as usize);
            format!("{start}{prefix} {text}{end}")
        };

        self.push_styled(&styled);
        self.push_blank();
    }

    fn flush_text(&mut self) {
        let text = std::mem::take(&mut self.buf);
        // Reset inline style stack for next block
        self.styles = StyleStack::new();
        if text.is_empty() {
            return;
        }

        if !self.lists.is_empty() {
            self.flush_list_item(&text);
        } else {
            let ew = self.effective_width();
            let lines = wrap_text(&text, ew);
            for line in &lines {
                self.push_styled(line);
            }
            self.push_blank();
        }
    }

    fn flush_list_item(&mut self, text: &str) {
        let depth = self.lists.len() - 1;
        let list = &self.lists[depth];
        let indent = "  ".repeat(depth);

        let bullet_style = Style { fg: Some(self.colors.list_bullet), ..Style::NONE };
        let cyan_on = bullet_style.to_sgr();
        let cyan_off = Style::NONE.transition_from(&bullet_style);

        let (bullet, bullet_vis_w) = if list.ordered {
            let idx = list.next_idx;
            (format!("{cyan_on}{idx}.{cyan_off} "), format!("{idx}. ").len())
        } else {
            (format!("{cyan_on}-{cyan_off} "), 2)
        };

        let content_w = self
            .effective_width()
            .saturating_sub(depth * 2 + bullet_vis_w);
        let cont_indent = format!("{indent}{}", " ".repeat(bullet_vis_w));

        let wrapped = wrap_text(text, content_w);
        for (j, line) in wrapped.iter().enumerate() {
            if j == 0 {
                self.push_styled(&format!("{indent}{bullet}{line}"));
            } else {
                self.push_styled(&format!("{cont_indent}{line}"));
            }
        }
    }

    fn flush_code_block(&mut self) {
        let lang = self.code_block.take().unwrap_or_default();
        let code = std::mem::take(&mut self.code_buf);

        let fence_style = Style { fg: Some(self.colors.code_block_border), ..Style::NONE };
        let fence_on = fence_style.to_sgr();
        let fence_off = Style::NONE.transition_from(&fence_style);

        let code_style = Style { fg: Some(self.colors.code_block), ..Style::NONE };
        let code_on = code_style.to_sgr();
        let code_off = Style::NONE.transition_from(&code_style);

        self.push_styled(&format!("{fence_on}```{lang}{fence_off}"));
        for code_line in code.lines() {
            self.push_styled(&format!("  {code_on}{code_line}{code_off}"));
        }
        // Only emit closing fence if the source actually has one (last line is a fence)
        let last_line = self.block_source.lines().next_back().unwrap_or("").trim();
        let has_fence = last_line.len() >= 3
            && (last_line.chars().all(|c| c == '`') || last_line.chars().all(|c| c == '~'));
        if has_fence {
            self.push_styled(&format!("{fence_on}```{fence_off}"));
            self.push_blank();
        }
    }

    fn flush_table(&mut self) {
        let tbl = match self.table.take() {
            Some(t) => t,
            None => return,
        };

        let ncols = tbl.aligns.len().max(tbl.head.len());
        if ncols == 0 {
            return;
        }

        let ew = self.effective_width();
        // Border overhead: "│ " + (n-1) * " │ " + " │" = 3n + 1
        let border_overhead = 3 * ncols + 1;
        let avail = ew.saturating_sub(border_overhead);

        if avail < ncols {
            // Too narrow for any table — fall back to wrapped plain text
            let ew = self.effective_width();
            for cell in &tbl.head {
                let stripped = crate::ansi::strip_ansi(cell);
                if !stripped.is_empty() {
                    for line in wrap_text(cell, ew) {
                        self.push_styled(&line);
                    }
                }
            }
            for row in &tbl.rows {
                let joined = row.iter().map(|s| s.as_str()).collect::<Vec<_>>().join("  ");
                for line in wrap_text(&joined, ew) {
                    self.push_styled(&line);
                }
            }
            self.push_blank();
            return;
        }

        // ── Natural widths and minimum widths (longest word per column) ──
        let mut natural: Vec<usize> = vec![0; ncols];
        let mut min_word: Vec<usize> = vec![1; ncols];

        let measure = |cell: &str, nat: &mut usize, mw: &mut usize| {
            let vw = visible_width(cell);
            *nat = (*nat).max(vw);
            let stripped = crate::ansi::strip_ansi(cell);
            for word in stripped.split_whitespace() {
                *mw = (*mw).max(visible_width(word).min(20)); // cap word width at 20
            }
        };

        for (i, cell) in tbl.head.iter().enumerate() {
            measure(cell, &mut natural[i], &mut min_word[i]);
        }
        for row in &tbl.rows {
            for (i, cell) in row.iter().enumerate() {
                if i < ncols {
                    measure(cell, &mut natural[i], &mut min_word[i]);
                }
            }
        }

        // ── Compute final column widths ──
        let total_natural: usize = natural.iter().sum();
        let col_w: Vec<usize> = if total_natural <= avail {
            // Everything fits — use natural widths
            natural.clone()
        } else {
            // Need to shrink — distribute available space proportionally
            let min_total: usize = min_word.iter().sum();
            if min_total <= avail {
                // Min widths fit — distribute extra above minimums proportionally
                let mut widths = min_word.clone();
                let extra = avail - min_total;
                let grow_potential: Vec<usize> = natural.iter().zip(min_word.iter())
                    .map(|(n, m)| n.saturating_sub(*m))
                    .collect();
                let total_grow: usize = grow_potential.iter().sum();
                if total_grow > 0 && extra > 0 {
                    let mut allocated = 0;
                    for (i, gp) in grow_potential.iter().enumerate() {
                        let grow = (*gp as f64 / total_grow as f64 * extra as f64) as usize;
                        widths[i] += grow;
                        allocated += grow;
                    }
                    let mut leftover = extra - allocated;
                    for i in 0..ncols {
                        if leftover == 0 { break; }
                        if widths[i] < natural[i] {
                            widths[i] += 1;
                            leftover -= 1;
                        }
                    }
                }
                widths
            } else {
                // Even minimums don't fit — force proportional distribution
                let mut widths = vec![1usize; ncols];
                let remaining = avail.saturating_sub(ncols);
                if remaining > 0 {
                    let total_weight: usize = natural.iter().sum::<usize>().max(1);
                    let mut allocated = 0;
                    for (i, &nw) in natural.iter().enumerate() {
                        let share = (nw as f64 / total_weight as f64 * remaining as f64) as usize;
                        widths[i] += share;
                        allocated += share;
                    }
                    let mut leftover = remaining - allocated;
                    for w in widths.iter_mut() {
                        if leftover == 0 { break; }
                        *w += 1;
                        leftover -= 1;
                    }
                }
                widths
            }
        };

        // ── Helpers ──
        let aligns = &tbl.aligns;

        // Pad a single line to column width respecting alignment.
        let align_pad = |line: &str, w: usize, align: Alignment| -> String {
            let vw = visible_width(line);
            if vw >= w {
                return crate::ansi::truncate_line(line, w);
            }
            let gap = w - vw;
            match align {
                Alignment::Right => format!("{}{line}", " ".repeat(gap)),
                Alignment::Center => {
                    let left = gap / 2;
                    let right = gap - left;
                    format!("{}{line}{}", " ".repeat(left), " ".repeat(right))
                }
                _ => format!("{line}{}", " ".repeat(gap)), // Left or None
            }
        };

        // Wrap cell text to column width, returning one or more aligned lines.
        let wrap_cell = |text: &str, w: usize, align: Alignment| -> Vec<String> {
            if w == 0 {
                return vec![String::new()];
            }
            let wrapped = wrap_text(text, w);
            if wrapped.is_empty() {
                return vec![" ".repeat(w)];
            }
            wrapped
                .into_iter()
                .map(|l| align_pad(&l, w, align))
                .collect()
        };

        let bold_on = SS_BOLD.to_sgr();
        let bold_off = Style::NONE.transition_from(&SS_BOLD);

        // Emit one (possibly multi-line) table row.
        let emit_row =
            |out: &mut Self, cells: &[String], col_w: &[usize], bold: bool| {
                let cell_lines: Vec<Vec<String>> = (0..ncols)
                    .map(|i| {
                        let text = cells.get(i).map(|s| s.as_str()).unwrap_or("");
                        let align = aligns.get(i).copied().unwrap_or(Alignment::None);
                        wrap_cell(text, col_w[i], align)
                    })
                    .collect();
                let height = cell_lines.iter().map(|c| c.len()).max().unwrap_or(1);
                for row_line in 0..height {
                    let parts: Vec<String> = (0..ncols)
                        .map(|i| {
                            let align = aligns.get(i).copied().unwrap_or(Alignment::None);
                            let empty = " ".repeat(col_w[i]);
                            let text = cell_lines[i].get(row_line).unwrap_or(&empty);
                            if bold {
                                format!("{bold_on}{text}{bold_off}")
                            } else {
                                // Continuation lines of wrapped cells: pad with alignment
                                if cell_lines[i].get(row_line).is_none() {
                                    align_pad("", col_w[i], align)
                                } else {
                                    text.clone()
                                }
                            }
                        })
                        .collect();
                    out.push_styled(&format!("│ {} │", parts.join(" │ ")));
                }
            };

        let border_cells: Vec<String> = col_w.iter().map(|w| "─".repeat(*w)).collect();

        // Top border
        self.push_styled(&format!("┌─{}─┐", border_cells.join("─┬─")));

        // Header
        emit_row(&mut *self, &tbl.head, &col_w, true);

        // Separator
        self.push_styled(&format!("├─{}─┤", border_cells.join("─┼─")));

        // Rows
        for (ri, row) in tbl.rows.iter().enumerate() {
            emit_row(&mut *self, row, &col_w, false);
            if ri < tbl.rows.len() - 1 {
                self.push_styled(&format!("├─{}─┤", border_cells.join("─┼─")));
            }
        }

        // Bottom border
        self.push_styled(&format!("└─{}─┘", border_cells.join("─┴─")));
        self.push_blank();
    }

    // ── Output helpers ────────────────────────────────────────────────

    fn effective_width(&self) -> usize {
        self.w.saturating_sub(self.quote_depth * 2)
    }

    fn push_styled(&mut self, text: &str) {
        if self.quote_depth > 0 {
            let qb_style = Style { fg: Some(self.colors.quote_border), ..Style::NONE };
            let green_on = qb_style.to_sgr();
            let green_off = Style::NONE.transition_from(&qb_style);
            let mut prefix = String::new();
            for _ in 0..self.quote_depth {
                write!(prefix, "{green_on}│{green_off} ").ok();
            }
            let qt_style = Style { fg: Some(self.colors.quote), ..Style::NONE };
            let qt_on = qt_style.to_sgr();
            let qt_off = Style::NONE.transition_from(&qt_style);
            self.out.push(Rc::from(format!("{prefix}{qt_on}{text}{qt_off}").as_str()));
        } else {
            self.out.push(Rc::from(text));
        }
    }

    fn push_blank(&mut self) {
        if self.quote_depth > 0 {
            let qb_style = Style { fg: Some(self.colors.quote_border), ..Style::NONE };
            let green_on = qb_style.to_sgr();
            let green_off = Style::NONE.transition_from(&qb_style);
            let mut prefix = String::new();
            for _ in 0..self.quote_depth {
                write!(prefix, "{green_on}│{green_off} ").ok();
            }
            self.out.push(Rc::from(prefix.as_str()));
        } else {
            self.out.push(Rc::from(""));
        }
    }
}
