//! Popup overlay components.
//!
//! [`SelectList`] is a filterable list of items rendered as a popup
//! above the editor. It handles up/down navigation, filtering, and
//! selection.

use crate::ansi::{visible_width, RESET};
use crate::style::{Color, Style, Theme};


/// Visual style for overlay popups.
#[derive(Debug, Clone)]
pub struct OverlayStyle {
    pub highlight_bg: Color,
    pub highlight_fg: Color,
    pub label_fg: Color,
    pub desc_fg: Color,
}

impl Default for OverlayStyle {
    fn default() -> Self {
        Self::from_theme(&Theme::default())
    }
}

impl OverlayStyle {
    /// Derive overlay style from a [`Theme`].
    pub fn from_theme(theme: &Theme) -> Self {
        Self {
            highlight_bg: theme.overlay_highlight_bg,
            highlight_fg: theme.overlay_highlight_fg,
            label_fg: theme.overlay_label_fg,
            desc_fg: theme.overlay_desc_fg,
        }
    }
}

// ── Item ────────────────────────────────────────────────────────

/// A single item in a [`SelectList`].
#[derive(Clone, Debug)]
pub struct SelectItem {
    /// Display label (e.g. `/help`, `file.rs`).
    pub label: String,
    /// Short description shown to the right of the label.
    pub description: String,
    /// Value returned on selection (may differ from label).
    pub value: String,
}

impl SelectItem {
    pub fn new(label: impl Into<String>, description: impl Into<String>) -> Self {
        let label = label.into();
        let value = label.clone();
        Self { label, description: description.into(), value }
    }

    pub fn with_value(mut self, value: impl Into<String>) -> Self {
        self.value = value.into();
        self
    }
}

// ── SelectList ──────────────────────────────────────────────────

/// A filterable popup list.
///
/// Call [`SelectList::filter`] to update the visible items, then
/// [`SelectList::render`] to get the output lines.
pub struct SelectList {
    items: Vec<SelectItem>,
    /// Indices into `items` matching the current filter.
    filtered: Vec<usize>,
    /// Index into `filtered` of the highlighted item.
    selected: usize,
    /// Maximum number of visible rows before scrolling.
    pub max_visible: usize,
    /// Scroll offset into `filtered`.
    scroll_offset: usize,

    /// Visual style for the overlay.
    pub style: OverlayStyle,
}

/// Result of [`SelectList::handle_key`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SelectAction {
    /// Key consumed (navigation).
    Consumed,
    /// An item was selected (Enter). Contains `value`.
    Selected(String),
    /// Tab completion — fill in the value but keep editing.
    Completed(String),
    /// Overlay should be dismissed.
    Dismissed,
    /// Key not handled by the list.
    Ignored,
}

impl SelectList {
    pub fn new(items: Vec<SelectItem>) -> Self {
        let filtered: Vec<usize> = (0..items.len()).collect();
        Self {
            items,
            filtered,
            selected: 0,
            max_visible: 8,
            scroll_offset: 0,
            style: OverlayStyle::default(),
        }
    }

    /// Update the filtered list based on a prefix string.
    /// Case-insensitive substring match on label.
    pub fn filter(&mut self, text: &str) {
        let lower = text.to_lowercase();
        self.filtered = self.items.iter().enumerate()
            .filter(|(_, item)| item.label.to_lowercase().contains(&lower))
            .map(|(i, _)| i)
            .collect();
        // Keep selected in bounds.
        if self.filtered.is_empty() {
            self.selected = 0;
        } else if self.selected >= self.filtered.len() {
            self.selected = self.filtered.len() - 1;
        }
        self.clamp_scroll();
    }

    pub fn move_up(&mut self) {
        if self.selected > 0 {
            self.selected -= 1;
            self.clamp_scroll();
        }
    }

    pub fn move_down(&mut self) {
        if !self.filtered.is_empty() && self.selected + 1 < self.filtered.len() {
            self.selected += 1;
            self.clamp_scroll();
        }
    }

    /// Select the currently highlighted item. Returns its `value`.
    pub fn select(&self) -> Option<String> {
        self.filtered.get(self.selected)
            .and_then(|&i| self.items.get(i))
            .map(|item| item.value.clone())
    }

    /// Longest common prefix of all filtered item values.
    pub fn common_prefix(&self) -> String {
        let mut iter = self.filtered.iter()
            .filter_map(|&i| self.items.get(i))
            .map(|item| &item.value);
        let first = match iter.next() {
            Some(v) => v.clone(),
            None => return String::new(),
        };
        let mut prefix = first;
        for value in iter {
            while !value.starts_with(&prefix) && !prefix.is_empty() {
                prefix.pop();
            }
        }
        prefix
    }

    pub fn is_empty(&self) -> bool {
        self.filtered.is_empty()
    }

    pub fn selected_index(&self) -> usize {
        self.selected
    }

    pub fn filtered_count(&self) -> usize {
        self.filtered.len()
    }

    /// Handle up/down/enter/esc/tab. Returns action.
    pub fn handle_key(&mut self, key: &crossterm::event::KeyEvent) -> SelectAction {
        use crossterm::event::KeyCode;
        match key.code {
            KeyCode::Up => { self.move_up(); SelectAction::Consumed }
            KeyCode::Down => { self.move_down(); SelectAction::Consumed }
            KeyCode::Enter => {
                if let Some(value) = self.select() {
                    SelectAction::Selected(value)
                } else {
                    SelectAction::Dismissed
                }
            }
            KeyCode::Tab => {
                if let Some(value) = self.select() {
                    SelectAction::Completed(value)
                } else {
                    SelectAction::Dismissed
                }
            }
            KeyCode::Esc => SelectAction::Dismissed,
            _ => SelectAction::Ignored,
        }
    }

    fn clamp_scroll(&mut self) {
        if self.selected < self.scroll_offset {
            self.scroll_offset = self.selected;
        }
        if self.selected >= self.scroll_offset + self.max_visible {
            self.scroll_offset = self.selected + 1 - self.max_visible;
        }
    }

    /// Render the list into lines. Returns empty vec if no items match.
    pub fn render(&self, width: usize) -> Vec<String> {
        render_select_list(self, width)
    }

    /// Render the list constrained to at most `max_height` output lines.
    /// Re-clamps the scroll window so the selected item is always visible
    /// within the available space.
    pub fn render_constrained(&mut self, width: usize, max_height: usize) -> Vec<String> {
        if max_height == 0 || self.filtered.is_empty() {
            return Vec::new();
        }
        // Reserve up to 2 lines for scroll indicators (↑/↓).
        let item_budget = max_height.saturating_sub(2).max(1);
        let effective_visible = self.filtered.len().min(item_budget).min(self.max_visible);

        // Re-clamp scroll so selected is within the effective window.
        if self.selected < self.scroll_offset {
            self.scroll_offset = self.selected;
        }
        if self.selected >= self.scroll_offset + effective_visible {
            self.scroll_offset = self.selected + 1 - effective_visible;
        }

        // Save and temporarily override max_visible for rendering.
        let saved = self.max_visible;
        self.max_visible = effective_visible;
        let lines = render_select_list(self, width);
        self.max_visible = saved;

        // Final safety: truncate to max_height (shouldn't happen, but defensive).
        if lines.len() > max_height {
            lines.into_iter().take(max_height).collect()
        } else {
            lines
        }
    }
}

/// Render a SelectList into styled lines for overlay display.
pub fn render_select_list(list: &SelectList, width: usize) -> Vec<String> {
    if list.filtered.is_empty() {
        return Vec::new();
    }

    let visible_count = list.filtered.len().min(list.max_visible);
    let end = (list.scroll_offset + visible_count).min(list.filtered.len());
    let start = end.saturating_sub(visible_count);

    let hl_bg = Style::new().bg(list.style.highlight_bg).to_sgr();
    let hl_fg = Style::new().fg(list.style.highlight_fg).to_sgr();
    let lbl_fg = Style::new().fg(list.style.label_fg).to_sgr();
    let dsc_fg = Style::new().fg(list.style.desc_fg).to_sgr();

    // Compute column widths using display labels (strip leading /).
    fn display_label(item: &SelectItem) -> &str {
        item.label.strip_prefix('/').unwrap_or(&item.label)
    }
    let max_label_w: usize = list.filtered[start..end].iter()
        .filter_map(|&i| list.items.get(i))
        .map(|item| visible_width(display_label(item)))
        .max()
        .unwrap_or(0);

    let mut lines = Vec::with_capacity(visible_count + 2);

    // Top scroll indicator.
    if start > 0 {
        lines.push(format!("{dsc_fg} ↑ {} more{RESET}", start));
    }

    for vi in start..end {
        let idx = list.filtered[vi];
        let item = &list.items[idx];
        let is_selected = vi == list.selected;
        let dlabel = display_label(item);

        let label_w = visible_width(dlabel);
        let gap = max_label_w.saturating_sub(label_w) + 2;
        let gap_str = " ".repeat(gap);

        let desc = if item.description.is_empty() {
            String::new()
        } else {
            let avail = width.saturating_sub(max_label_w + gap + 1);
            truncate_str(&item.description, avail)
        };

        if is_selected {
            let content = format!("{dlabel}{gap_str}{dsc_fg}{desc}");
            let content_w = visible_width(&content);
            let fill = " ".repeat(width.saturating_sub(content_w));
            lines.push(format!("{hl_bg}{hl_fg}{content}{fill}{RESET}"));
        } else {
            lines.push(format!("{lbl_fg}{dlabel}{RESET}{gap_str}{dsc_fg}{desc}{RESET}"));
        }
    }

    // Bottom scroll indicator.
    let remaining = list.filtered.len().saturating_sub(end);
    if remaining > 0 {
        lines.push(format!("{dsc_fg} ↓ {} more{RESET}", remaining));
    }

    lines
}

fn truncate_str(s: &str, max_width: usize) -> String {
    if max_width == 0 { return String::new(); }
    let mut w = 0;
    let mut result = String::new();
    for ch in s.chars() {
        let cw = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(1);
        if w + cw > max_width { break; }
        result.push(ch);
        w += cw;
    }
    result
}
