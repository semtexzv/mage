//! ANSI escape sequence handling — constants, parsing, stripping, measurement.
//!
//! All terminal escape codes and ANSI-aware text manipulation lives here.
//! Other modules import what they need instead of defining their own constants.

use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use crate::style::Style;

// ── SGR reset ───────────────────────────────────────────────────

/// ANSI SGR reset sequence.
pub const RESET: &str = "\x1b[0m";

// ── Terminal control sequences ──────────────────────────────────
// Used by the renderer for differential painting.

pub(crate) const SYNC_BEGIN: &str = "\x1b[?2026h";
pub(crate) const SYNC_END: &str = "\x1b[?2026l";
pub(crate) const CLEAR_SCROLLBACK: &str = "\x1b[3J";
pub(crate) const CLEAR_SCREEN: &str = "\x1b[2J";
pub(crate) const CURSOR_HOME: &str = "\x1b[H";
pub(crate) const CLEAR_LINE: &str = "\x1b[2K";
pub(crate) const CRLF: &str = "\r\n";
pub(crate) const CR: &str = "\r";
pub(crate) const SHOW_CURSOR: &str = "\x1b[?25h";
pub(crate) const HIDE_CURSOR: &str = "\x1b[?25l";

pub(crate) fn cursor_up(n: usize) -> String {
    format!("\x1b[{}A", n)
}
pub(crate) fn cursor_down(n: usize) -> String {
    format!("\x1b[{}B", n)
}
pub(crate) fn cursor_col(n: usize) -> String {
    format!("\x1b[{}G", n)
} // 1-indexed

// ── Low-level ANSI extraction ───────────────────────────────────

pub(crate) const ESC: u8 = 0x1b;

/// Extract an ANSI escape sequence starting at byte `pos`.
/// Returns `(code_str, byte_len)` or None.
pub(crate) fn extract_ansi(s: &str, pos: usize) -> Option<(&str, usize)> {
    let bytes = s.as_bytes();
    if pos >= bytes.len() || bytes[pos] != ESC || pos + 1 >= bytes.len() {
        return None;
    }
    match bytes[pos + 1] {
        b'[' => extract_csi(s, bytes, pos),
        b']' | b'_' => extract_string_seq(s, bytes, pos),
        _ => None,
    }
}

fn extract_csi<'a>(s: &'a str, bytes: &[u8], pos: usize) -> Option<(&'a str, usize)> {
    let mut i = pos + 2;
    while i < bytes.len() && (0x30..=0x3F).contains(&bytes[i]) {
        i += 1;
    }
    while i < bytes.len() && (0x20..=0x2F).contains(&bytes[i]) {
        i += 1;
    }
    if i < bytes.len() && (0x40..=0x7E).contains(&bytes[i]) {
        i += 1;
        Some((&s[pos..i], i - pos))
    } else {
        None
    }
}

fn extract_string_seq<'a>(s: &'a str, bytes: &[u8], pos: usize) -> Option<(&'a str, usize)> {
    let mut i = pos + 2;
    while i < bytes.len() {
        if bytes[i] == 0x07 {
            return Some((&s[pos..i + 1], i + 1 - pos));
        }
        if bytes[i] == ESC && i + 1 < bytes.len() && bytes[i + 1] == b'\\' {
            return Some((&s[pos..i + 2], i + 2 - pos));
        }
        i += 1;
    }
    None
}

// ── SGR parsing ─────────────────────────────────────────────────

/// Parse a single CSI SGR escape sequence (e.g. `"\x1b[1;33m"`) and
/// apply its parameters to `state`. Returns `true` if the code was
/// recognized as SGR, `false` otherwise.
///
/// Used by `wrap_text` and `truncate_line` to track style state as they
/// scan through ANSI-decorated text.
pub fn apply_sgr(state: &mut Style, code: &str) -> bool {
    use crate::style::Color;

    if !code.starts_with("\x1b[") || !code.ends_with('m') {
        return false;
    }
    let params = &code[2..code.len() - 1]; // between \x1b[ and m
    if params.is_empty() {
        // \x1b[m is equivalent to \x1b[0m
        *state = Style::NONE;
        return true;
    }

    let mut nums = params.split(';').peekable();
    while let Some(s) = nums.next() {
        let n: u16 = s.parse().unwrap_or(0);
        match n {
            0 => *state = Style::NONE,
            1 => state.bold = true,
            2 => state.dim = true,
            3 => state.italic = true,
            4 => state.underline = true,
            9 => state.strikethrough = true,
            22 => { state.bold = false; state.dim = false; }
            23 => state.italic = false,
            24 => state.underline = false,
            29 => state.strikethrough = false,
            30..=37 => {
                state.fg = Some(match n {
                    30 => Color::Black, 31 => Color::Red, 32 => Color::Green,
                    33 => Color::Yellow, 34 => Color::Blue, 35 => Color::Magenta,
                    36 => Color::Cyan, 37 => Color::White, _ => unreachable!(),
                });
            }
            38 => {
                // Extended fg: 38;5;n or 38;2;r;g;b
                if let Some(mode) = nums.next() {
                    match mode {
                        "5" => {
                            if let Some(idx) = nums.next() {
                                if let Ok(v) = idx.parse::<u8>() {
                                    state.fg = Some(Color::Ansi256(v));
                                }
                            }
                        }
                        "2" => {
                            let r = nums.next().and_then(|s| s.parse::<u8>().ok()).unwrap_or(0);
                            let g = nums.next().and_then(|s| s.parse::<u8>().ok()).unwrap_or(0);
                            let b = nums.next().and_then(|s| s.parse::<u8>().ok()).unwrap_or(0);
                            state.fg = Some(Color::Rgb(r, g, b));
                        }
                        _ => {}
                    }
                }
            }
            39 => state.fg = None,
            40..=47 => {
                state.bg = Some(match n {
                    40 => Color::Black, 41 => Color::Red, 42 => Color::Green,
                    43 => Color::Yellow, 44 => Color::Blue, 45 => Color::Magenta,
                    46 => Color::Cyan, 47 => Color::White, _ => unreachable!(),
                });
            }
            48 => {
                // Extended bg: 48;5;n or 48;2;r;g;b
                if let Some(mode) = nums.next() {
                    match mode {
                        "5" => {
                            if let Some(idx) = nums.next() {
                                if let Ok(v) = idx.parse::<u8>() {
                                    state.bg = Some(Color::Ansi256(v));
                                }
                            }
                        }
                        "2" => {
                            let r = nums.next().and_then(|s| s.parse::<u8>().ok()).unwrap_or(0);
                            let g = nums.next().and_then(|s| s.parse::<u8>().ok()).unwrap_or(0);
                            let b = nums.next().and_then(|s| s.parse::<u8>().ok()).unwrap_or(0);
                            state.bg = Some(Color::Rgb(r, g, b));
                        }
                        _ => {}
                    }
                }
            }
            49 => state.bg = None,
            _ => {} // ignore unknown
        }
    }
    true
}

// ── ANSI-aware text manipulation ────────────────────────────────

/// Strip ANSI codes and measure visible terminal width.
/// Uses grapheme clusters so that emoji sequences (e.g. ⚠️ = U+26A0 + VS16)
/// are measured as a single unit by `UnicodeWidthStr`.
pub fn visible_width(s: &str) -> usize {
    let stripped = strip_ansi(s);
    stripped
        .graphemes(true)
        .map(|g| if g == "\t" { 3 } else { UnicodeWidthStr::width(g) })
        .sum()
}

/// Remove all ANSI escape sequences.
pub fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    let bytes = s.as_bytes();
    while i < bytes.len() {
        if bytes[i] == ESC {
            if let Some((_, len)) = extract_ansi(s, i) {
                i += len;
                continue;
            }
        }
        if let Some(ch) = s[i..].chars().next() {
            out.push(ch);
            i += ch.len_utf8();
        } else {
            i += 1;
        }
    }
    out
}

/// Truncate a line to at most `max_width` visible columns.
/// ANSI escape sequences are preserved (they don't consume width).
/// Uses grapheme clusters so emoji sequences are treated as single units.
/// A reset is appended if any SGR codes were active at the cut point.
pub fn truncate_line(s: &str, max_width: usize) -> String {
    if max_width == 0 {
        return String::new();
    }
    let bytes = s.as_bytes();
    let mut out = String::with_capacity(s.len());
    let mut w: usize = 0;
    let mut i: usize = 0;
    let mut sgr = Style::NONE;

    while i < bytes.len() {
        // Pass through ANSI escape sequences (zero visible width)
        if bytes[i] == ESC {
            if let Some((code, len)) = extract_ansi(s, i) {
                out.push_str(code);
                apply_sgr(&mut sgr, code);
                i += len;
                continue;
            }
        }
        // Visible grapheme cluster
        let remaining = &s[i..];
        let grapheme = match remaining.graphemes(true).next() {
            Some(g) => g,
            None => break,
        };
        let gw = if grapheme == "\t" { 3 } else { UnicodeWidthStr::width(grapheme) };
        if w + gw > max_width {
            break;
        }
        out.push_str(grapheme);
        w += gw;
        i += grapheme.len();
    }

    if !sgr.is_empty() {
        out.push_str(RESET);
    }
    out
}

/// Split a line at a visible column boundary.
///
/// Returns `(before, after)` where:
/// - `before` contains content up to (not including) column `col`,
///   followed by a reset if any SGR was active.
/// - `after` contains content from column `col` onward,
///   prefixed with the SGR state active at the split point.
///
/// ANSI escape sequences are preserved and tracked through the split.
pub fn split_line_at_col(s: &str, col: usize) -> (String, String) {
    let bytes = s.as_bytes();
    let mut before = String::with_capacity(s.len());
    let mut w: usize = 0;
    let mut i: usize = 0;
    let mut sgr = Style::NONE;

    // Collect everything up to `col`.
    while i < bytes.len() && w < col {
        if bytes[i] == ESC {
            if let Some((code, len)) = extract_ansi(s, i) {
                before.push_str(code);
                apply_sgr(&mut sgr, code);
                i += len;
                continue;
            }
        }
        let remaining = &s[i..];
        let grapheme = match remaining.graphemes(true).next() {
            Some(g) => g,
            None => break,
        };
        let gw = if grapheme == "\t" { 3 } else { UnicodeWidthStr::width(grapheme) };
        if w + gw > col {
            break; // wide char would cross the boundary
        }
        before.push_str(grapheme);
        w += gw;
        i += grapheme.len();
    }

    // Reset before if SGR was active.
    let sgr_at_split = sgr;
    if !sgr.is_empty() {
        before.push_str(RESET);
    }

    // Build after: restore SGR state, then remaining content.
    let mut after = String::with_capacity(s.len() - i + 32);

    // Skip ANSI codes at the boundary (they're part of before's state tracking).
    // Actually, keep scanning — any ANSI codes between before and the next visible
    // char should update the state and be included in `after`.
    let mut after_sgr = sgr_at_split;
    while i < bytes.len() {
        if bytes[i] == ESC {
            if let Some((code, len)) = extract_ansi(s, i) {
                apply_sgr(&mut after_sgr, code);
                i += len;
                continue;
            }
        }
        break;
    }

    // Prefix `after` with the SGR state so it renders correctly standalone.
    let sgr_prefix = after_sgr.to_sgr();
    if !sgr_prefix.is_empty() {
        after.push_str(&sgr_prefix);
    }

    // Copy remaining content.
    after.push_str(&s[i..]);

    (before, after)
}
