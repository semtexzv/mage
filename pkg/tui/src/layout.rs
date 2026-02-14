//! Horizontal pane layout — compose side-by-side output for the renderer.
//!
//! `HStack` splits available width into fixed or flex panes. Each pane
//! accumulates lines independently. Call [`HStack::compose`] to merge
//! them horizontally into `Vec<Line>` ready for `Renderer::push_lines`.
//!
//! ```text
//! let mut hs = HStack::new(80);
//! let left = hs.pane(PaneSize::Fixed(30));
//! let right = hs.pane(PaneSize::Flex);
//! hs.get_mut(left).push_line("sidebar");
//! hs.get_mut(right).push_line("main content");
//! let lines = hs.compose();
//! renderer.push_lines(&lines);
//! ```
//!
//! Panes support optional padding. When padding is set, [`Pane::width`]
//! returns the available content width (allocated minus horizontal padding),
//! and [`HStack::compose`] applies the padding automatically.
//!
//! ```text
//! let center = hs.pane_with_padding(PaneSize::Flex, Padding::horizontal(2));
//! // center.available_width() == allocated - 4
//! ```
//!
//! Pane-level dirty tracking avoids recomposition when nothing changed.

use std::rc::Rc;

use crate::ansi::{truncate_line, visible_width, RESET};
use crate::renderer::Line;
use crate::style::Padding;

// ── Pane ─────────────────────────────────────────────────────────

/// A single vertical column of lines within an [`HStack`].
///
/// Each pane has an *allocated* width (set by the layout engine) and
/// optional [`Padding`]. The [`width`](Pane::width) method returns the
/// **available content width** — the space left after padding is
/// subtracted. Callers should use this to plan their content; padding
/// is applied automatically by [`HStack::compose`].
pub struct Pane {
    lines: Vec<Line>,
    /// Full column allocation from the layout engine (includes padding).
    allocated: usize,
    padding: Padding,
    dirty: bool,
}

impl Pane {
    fn new(allocated: usize, padding: Padding) -> Self {
        Self {
            lines: Vec::new(),
            allocated,
            padding,
            dirty: true,
        }
    }

    /// Available content width in visible columns.
    ///
    /// This is the allocated width minus horizontal padding. Use this
    /// value to size content — padding is applied by [`HStack::compose`].
    pub fn available_width(&self) -> usize {
        self.allocated.saturating_sub(self.padding.h() as usize)
    }

    /// Full allocated width including padding.
    pub fn allocated(&self) -> usize {
        self.allocated
    }

    /// Current padding.
    pub fn padding(&self) -> Padding {
        self.padding
    }

    /// Number of lines currently in this pane.
    pub fn line_count(&self) -> usize {
        self.lines.len()
    }

    /// Append a line. Callers are responsible for wrapping/truncation;
    /// [`compose`](HStack::compose) truncates to [`width`](Pane::width)
    /// automatically.
    pub fn push_line(&mut self, line: impl Into<Line>) {
        self.lines.push(line.into());
        self.dirty = true;
    }

    /// Append an empty line.
    pub fn push_blank(&mut self) {
        self.lines.push(Rc::from(""));
        self.dirty = true;
    }

    /// Replace all content. Useful for components that rebuild every frame.
    pub fn set_lines(&mut self, lines: Vec<Line>) {
        self.lines = lines;
        self.dirty = true;
    }

    /// Clear all lines.
    pub fn clear(&mut self) {
        self.lines.clear();
        self.dirty = true;
    }

    /// Get line at index (for inspection / testing).
    pub fn line(&self, idx: usize) -> Option<&Line> {
        self.lines.get(idx)
    }
}

// ── Size specification ───────────────────────────────────────────

/// How a pane's width is determined.
#[derive(Debug, Clone, Copy)]
pub enum PaneSize {
    /// Exact number of columns.
    Fixed(usize),
    /// Percentage of total width (0.0 – 1.0).
    Percent(f32),
    /// Takes remaining space after fixed and percent panes.
    /// Multiple flex panes split the remainder equally.
    Flex,
}

// ── Pane handle ──────────────────────────────────────────────────

/// Opaque handle returned by [`HStack::pane`]. Used with
/// [`HStack::get`] / [`HStack::get_mut`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PaneId(usize);

// ── HStack ───────────────────────────────────────────────────────

/// Horizontal stack of panes that compose into lines for the renderer.
pub struct HStack {
    panes: Vec<Pane>,
    sizes: Vec<PaneSize>,
    paddings: Vec<Padding>,
    total_width: usize,
    separator: Option<&'static str>,
    /// Cached composed output; invalidated when any pane is dirty.
    cache: Vec<Line>,
}

impl HStack {
    /// Create a new horizontal stack for the given total width.
    pub fn new(total_width: usize) -> Self {
        Self {
            panes: Vec::new(),
            sizes: Vec::new(),
            paddings: Vec::new(),
            total_width,
            separator: None,
            cache: Vec::new(),
        }
    }

    /// Set the separator string drawn between panes (e.g. `"│"`).
    /// Set to `None` to disable. Default: none.
    pub fn set_separator(&mut self, sep: Option<&'static str>) {
        self.separator = sep;
    }

    /// Add a pane with the given size specification and no padding.
    /// Returns a handle.
    pub fn pane(&mut self, size: PaneSize) -> PaneId {
        self.pane_with_padding(size, Padding::ZERO)
    }

    /// Add a pane with the given size specification and padding.
    ///
    /// [`Pane::width`] will return the available content width (allocated
    /// minus horizontal padding). Padding is applied automatically during
    /// [`compose`](HStack::compose) — callers fill content to
    /// [`Pane::width`] and never think about margins.
    pub fn pane_with_padding(&mut self, size: PaneSize, padding: Padding) -> PaneId {
        let id = PaneId(self.panes.len());
        self.panes.push(Pane::new(0, padding));
        self.sizes.push(size);
        self.paddings.push(padding);
        self.resolve_widths();
        id
    }

    /// Set padding for an existing pane.
    ///
    /// This does not change the pane's allocated width — only the content
    /// width returned by [`Pane::width`]. The pane is marked dirty so
    /// the next [`compose`](HStack::compose) call reapplies padding.
    pub fn set_padding(&mut self, id: PaneId, padding: Padding) {
        self.paddings[id.0] = padding;
        self.panes[id.0].padding = padding;
        self.panes[id.0].dirty = true;
    }

    /// Immutable access to a pane.
    pub fn get(&self, id: PaneId) -> &Pane {
        &self.panes[id.0]
    }

    /// Mutable access to a pane.
    pub fn get_mut(&mut self, id: PaneId) -> &mut Pane {
        &mut self.panes[id.0]
    }

    /// Number of panes.
    pub fn pane_count(&self) -> usize {
        self.panes.len()
    }

    /// Update total width (e.g. on terminal resize).
    /// Pane widths are resolved immediately so that [`Pane::width`]
    /// returns the correct value before content is pushed.
    pub fn set_width(&mut self, width: usize) {
        if width != self.total_width {
            self.total_width = width;
            self.resolve_widths();
            // Force recomposition.
            for p in &mut self.panes {
                p.dirty = true;
            }
        }
    }

    /// Resolve individual pane widths from total width and size specs.
    fn resolve_widths(&mut self) {
        let sep_w = self.separator.map_or(0, visible_width);
        let sep_count = if self.panes.len() > 1 { self.panes.len() - 1 } else { 0 };
        let sep_total = sep_w * sep_count;
        let available = self.total_width.saturating_sub(sep_total);

        let mut widths: Vec<usize> = vec![0; self.panes.len()];
        let mut used = 0usize;
        let mut flex_count = 0usize;

        // Pass 1: fixed + percent
        for (i, sz) in self.sizes.iter().enumerate() {
            match sz {
                PaneSize::Fixed(w) => {
                    widths[i] = (*w).min(available.saturating_sub(used));
                    used += widths[i];
                }
                PaneSize::Percent(pct) => {
                    let w = (available as f32 * pct.clamp(0.0, 1.0)).round() as usize;
                    widths[i] = w.min(available.saturating_sub(used));
                    used += widths[i];
                }
                PaneSize::Flex => {
                    flex_count += 1;
                }
            }
        }

        // Pass 2: flex
        let remaining = available.saturating_sub(used);
        if let Some(per_flex) = remaining.checked_div(flex_count) {
            let mut leftover = remaining % flex_count;
            for (i, sz) in self.sizes.iter().enumerate() {
                if matches!(sz, PaneSize::Flex) {
                    widths[i] = per_flex + if leftover > 0 { leftover -= 1; 1 } else { 0 };
                }
            }
        }

        // Update pane allocated widths.
        for (p, &w) in self.panes.iter_mut().zip(widths.iter()) {
            if p.allocated != w {
                p.allocated = w;
            }
        }
    }

    /// Compose all panes horizontally into output lines.
    ///
    /// Returns a cached `&[Line]` — only recomposes when a pane
    /// changed since the last call.
    ///
    /// Each pane's content is truncated or padded to its
    /// [`width`](Pane::width). Left/right [`Padding`] is applied as
    /// leading/trailing spaces. Top/bottom padding inserts blank rows.
    pub fn compose(&mut self) -> &[Line] {
        let any_dirty = self.panes.iter().any(|p| p.dirty);
        if !any_dirty && !self.cache.is_empty() {
            return &self.cache;
        }

        // Ensure widths are up to date.
        self.resolve_widths();

        let sep = self.separator.unwrap_or("");

        // Build per-pane row slices with top/bottom padding applied.
        // Each entry is a Vec of Option<&Line> where None means blank.
        let pane_rows: Vec<Vec<Option<&Line>>> = self.panes.iter().map(|pane| {
            let pad = &pane.padding;
            let top = pad.top as usize;
            let bottom = pad.bottom as usize;
            let mut rows: Vec<Option<&Line>> = Vec::with_capacity(top + pane.lines.len() + bottom);
            for _ in 0..top {
                rows.push(None);
            }
            for line in &pane.lines {
                rows.push(Some(line));
            }
            for _ in 0..bottom {
                rows.push(None);
            }
            rows
        }).collect();

        let max_rows = pane_rows.iter().map(|r| r.len()).max().unwrap_or(0);

        let mut out = Vec::with_capacity(max_rows);
        for row in 0..max_rows {
            let mut composed = String::new();
            for (i, pane) in self.panes.iter().enumerate() {
                if i > 0 {
                    composed.push_str(sep);
                }

                let alloc = pane.allocated;
                let left = pane.padding.left as usize;
                let content_w = pane.available_width(); // allocated - h padding
                // Left padding.
                for _ in 0..left {
                    composed.push(' ');
                }

                if let Some(Some(line)) = pane_rows[i].get(row) {
                    let vw = visible_width(line);
                    if vw <= content_w {
                        // Content fits — pad right to content width.
                        composed.push_str(line);
                        composed.push_str(RESET);
                        for _ in 0..(content_w - vw) {
                            composed.push(' ');
                        }
                    } else {
                        // Content too wide — truncate.
                        composed.push_str(&truncate_line(line, content_w));
                        composed.push_str(RESET);
                    }
                } else {
                    // No content or blank padding row — fill content area.
                    for _ in 0..content_w {
                        composed.push(' ');
                    }
                }

                // Right padding.
                // If allocated > left + content_w + right (rounding), fill
                // any remaining columns too.
                let used = left + content_w;
                let trail = alloc.saturating_sub(used);
                for _ in 0..trail {
                    composed.push(' ');
                }
            }
            out.push(Rc::from(composed.as_str()));
        }

        // Mark all panes clean.
        for p in &mut self.panes {
            p.dirty = false;
        }

        self.cache = out;
        &self.cache
    }
}
