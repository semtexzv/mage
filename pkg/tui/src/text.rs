//! Simple text widget with caching, styling, padding, and word-wrapping.
//!
//! `Text` is the default element for rendering styled content. It handles
//! word-wrapping, padding, background fill, and caches its output as
//! `Rc<str>` lines for O(1) diff comparison.
//!
//! # Examples
//!
//! ```rust
//! use mage_tui::{Text, Color, Style, Padding};
//!
//! let mut t = Text::new("Hello, world!")
//!     .style(Style::new().bold().fg(Color::Cyan))
//!     .padding(Padding::horizontal(2));
//!
//! // In render():
//! // t.render(r);
//! ```

use std::rc::Rc;

use crate::ansi::RESET;
use crate::renderer::{Line, LineSink, View};
use crate::style::{Color, Padding, Style};
use crate::wrap::wrap_text;

/// A styled, word-wrapping text widget with line caching.
///
/// Content is stored as a list of spans, each with its own style.
/// On render, spans are concatenated into styled text, word-wrapped
/// within padding, and optionally background-filled. Output lines
/// are cached as `Rc<str>` and only recomputed when content or
/// dimensions change.
pub struct Text {
    spans: Vec<Span>,
    padding: Padding,
    bg: Option<Color>,
    /// Cached output lines.
    output: Vec<Line>,
    /// Width used for last render (from Renderer).
    last_width: u16,
    dirty: bool,
}

/// A single span of styled text.
struct Span {
    text: String,
    style: Style,
}

impl Text {
    /// Create a new text widget with plain (unstyled) content.
    pub fn new(content: impl Into<String>) -> Self {
        Self {
            spans: vec![Span { text: content.into(), style: Style::NONE }],
            padding: Padding::ZERO,
            bg: None,
            output: Vec::new(),
            last_width: 0,
            dirty: true,
        }
    }

    /// Create an empty text widget.
    pub fn empty() -> Self {
        Self {
            spans: Vec::new(),
            padding: Padding::ZERO,
            bg: None,
            output: Vec::new(),
            last_width: 0,
            dirty: true,
        }
    }

    /// Set the style for the entire content (replaces all spans with one).
    ///
    /// If the widget has multiple spans, this collapses them into one
    /// concatenated string with the given style.
    pub fn style(mut self, style: Style) -> Self {
        if self.spans.len() == 1 {
            self.spans[0].style = style;
        } else {
            let text = self.plain_text();
            self.spans = vec![Span { text, style }];
        }
        self.dirty = true;
        self
    }

    /// Set padding.
    pub fn padding(mut self, padding: Padding) -> Self {
        self.padding = padding;
        self.dirty = true;
        self
    }

    /// Set background color. Lines are filled to the full width.
    pub fn bg(mut self, color: Color) -> Self {
        self.bg = Some(color);
        self.dirty = true;
        self
    }

    /// Append a styled span.
    pub fn push(&mut self, text: impl Into<String>, style: Style) {
        self.spans.push(Span { text: text.into(), style });
        self.dirty = true;
    }

    /// Append an unstyled span (inherits no style — rendered as default).
    pub fn push_plain(&mut self, text: impl Into<String>) {
        self.push(text, Style::NONE);
    }

    /// Replace all content with a single unstyled string.
    pub fn set_content(&mut self, text: impl Into<String>) {
        self.spans = vec![Span { text: text.into(), style: Style::NONE }];
        self.dirty = true;
    }

    /// Replace all content with a single styled string.
    pub fn set_styled(&mut self, text: impl Into<String>, style: Style) {
        self.spans = vec![Span { text: text.into(), style }];
        self.dirty = true;
    }

    /// Clear all content.
    pub fn clear(&mut self) {
        self.spans.clear();
        self.dirty = true;
    }

    /// Set background color (mutable version).
    pub fn set_bg(&mut self, bg: Option<Color>) {
        if bg != self.bg {
            self.bg = bg;
            self.dirty = true;
        }
    }

    /// Set padding (mutable version).
    pub fn set_padding(&mut self, padding: Padding) {
        if padding != self.padding {
            self.padding = padding;
            self.dirty = true;
        }
    }

    /// Get cached output lines. Rebuilds if dirty or width changed.
    pub fn lines(&mut self, width: u16) -> &[Line] {
        if self.dirty || width != self.last_width {
            self.rebuild(width);
        }
        &self.output
    }

    /// Render into any line sink. Uses `sink.width()` for layout.
    pub fn render(&mut self, r: &mut impl LineSink) {
        let w = r.width();
        if self.dirty || w != self.last_width {
            self.rebuild(w);
        }
        r.push_lines(&self.output);
    }

    // ── Internal ──

    fn plain_text(&self) -> String {
        self.spans.iter().map(|s| s.text.as_str()).collect()
    }

    /// Build the styled string from all spans.
    ///
    /// When a background color is set, span resets re-establish the bg
    /// so the fill color is never interrupted by `\x1b[0m`.
    fn build_styled_content(&self) -> String {
        // If bg is set, after each styled span we reset then immediately
        // re-establish the bg so unstyled text and fill spaces keep it.
        let reset_seq: String = match self.bg {
            Some(color) => format!("{}\x1b[{}m", RESET, color.bg_code()),
            None => RESET.to_string(),
        };
        let mut buf = String::new();
        // When bg is set, establish it at the start so unstyled leading
        // spans also render on the correct background.
        if let Some(color) = self.bg {
            buf.push_str(&format!("\x1b[{}m", color.bg_code()));
        }
        for span in &self.spans {
            if span.text.is_empty() {
                continue;
            }
            let sgr = span.style.to_sgr();
            if !sgr.is_empty() {
                buf.push_str(&sgr);
                buf.push_str(&span.text);
                buf.push_str(&reset_seq);
            } else {
                buf.push_str(&span.text);
            }
        }
        buf
    }

    fn rebuild(&mut self, width: u16) {
        self.output.clear();
        self.last_width = width;
        self.dirty = false;

        let w = width as usize;
        if w == 0 {
            return;
        }

        let left = self.padding.left as usize;
        let right = self.padding.right as usize;
        let inner_width = w.saturating_sub(left).saturating_sub(right);
        let left_prefix = if left > 0 { " ".repeat(left) } else { String::new() };

        // Top padding
        for _ in 0..self.padding.top {
            self.push_padded_line("", w, &left_prefix);
        }

        // Content
        if inner_width > 0 && !self.spans.is_empty() {
            let styled = self.build_styled_content();
            if !styled.is_empty() {
                let wrapped = wrap_text(&styled, inner_width);
                for line in &wrapped {
                    let padded = if left > 0 {
                        format!("{}{}", left_prefix, line)
                    } else {
                        line.clone()
                    };
                    self.push_padded_line(&padded, w, &left_prefix);
                }
            }
        }

        // Bottom padding
        for _ in 0..self.padding.bottom {
            self.push_padded_line("", w, &left_prefix);
        }
    }

    /// Push a line into output, applying bg fill if set.
    fn push_padded_line(&mut self, content: &str, width: usize, left_prefix: &str) {
        let line: Line = if let Some(color) = self.bg {
            Rc::from(crate::renderer::Renderer::bg_filled_line(content, width, color).as_str())
        } else if content.is_empty() {
            if !left_prefix.is_empty() {
                Rc::from(left_prefix)
            } else {
                crate::renderer::blank_line()
            }
        } else {
            Rc::from(content)
        };
        self.output.push(line);
    }
}

impl View for Text {
    fn render(&mut self, sink: &mut impl LineSink) {
        Text::render(self, sink);
    }
}

/// A full-width horizontal rule widget. Renders a single line of repeated characters.
pub struct HRule {
    ch: char,
    style: Style,
    cached: Option<(u16, Line)>,
}

impl HRule {
    pub fn new(ch: char, fg: Color) -> Self {
        Self {
            ch,
            style: Style::new().fg(fg),
            cached: None,
        }
    }

    /// Render into any line sink. Uses `sink.width()` for layout.
    pub fn render(&mut self, sink: &mut impl LineSink) {
        let w = sink.width();
        // Check cache
        if let Some((cached_w, ref line)) = self.cached {
            if cached_w == w {
                sink.push_lines(&[line.clone()]);
                return;
            }
        }
        let sgr = self.style.to_sgr();
        let content = self.ch.to_string().repeat(w as usize);
        let line: Line = if sgr.is_empty() {
            Rc::from(content.as_str())
        } else {
            Rc::from(format!("{sgr}{content}{RESET}").as_str())
        };
        sink.push_lines(&[line.clone()]);
        self.cached = Some((w, line));
    }
}

impl View for HRule {
    fn render(&mut self, sink: &mut impl LineSink) {
        HRule::render(self, sink);
    }
}