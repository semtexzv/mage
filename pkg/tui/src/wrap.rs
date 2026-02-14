//! Word wrapping with ANSI code preservation.

use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use crate::ansi::{apply_sgr, extract_ansi, ESC};
use crate::style::Style;

// ── Word wrapping ───────────────────────────────────────────────

/// Word-wrap text preserving ANSI codes across line breaks.
/// Hard newlines are preserved. Returns empty vec for empty input.
///
/// Style state is tracked using [`Style`] so that continuation
/// lines after a break are prefixed with the correct SGR sequence.
pub fn wrap_text(text: &str, width: usize) -> Vec<String> {
    if text.is_empty() || width == 0 {
        return vec![];
    }
    let mut result = Vec::new();
    let mut sgr = Style::NONE;
    for line in text.split('\n') {
        wrap_line(line, width, &mut sgr, &mut result);
    }
    result
}

fn wrap_line(text: &str, width: usize, sgr: &mut Style, out: &mut Vec<String>) {
    let bytes = text.as_bytes();
    let mut i = 0;
    let mut cur = sgr.to_sgr();
    let mut cur_w: usize = 0;
    let mut brk_pos: Option<usize> = None;
    let mut brk_w: usize = 0;
    let mut brk_sgr = *sgr;
    let initial = out.len();

    while i < bytes.len() {
        if bytes[i] == ESC {
            if let Some((code, len)) = extract_ansi(text, i) {
                apply_sgr(sgr, code);
                cur.push_str(code);
                i += len;
                continue;
            }
        }
        let remaining = &text[i..];
        let grapheme = match remaining.graphemes(true).next() {
            Some(g) => g,
            None => {
                i += 1;
                continue;
            }
        };
        let gw = if grapheme == "\t" {
            3
        } else {
            UnicodeWidthStr::width(grapheme)
        };
        let glen = grapheme.len();

        if grapheme == " " {
            if cur_w + 1 > width && cur_w > 0 {
                out.push(cur);
                cur = sgr.to_sgr();
                cur_w = 0;
                brk_pos = None;
            } else {
                brk_pos = Some(cur.len());
                brk_w = cur_w;
                brk_sgr = *sgr;
                cur.push(' ');
                cur_w += 1;
            }
            i += glen;
            continue;
        }

        if cur_w + gw > width {
            if let Some(bp) = brk_pos {
                let after = cur[bp + 1..].to_string();
                cur.truncate(bp);
                out.push(cur);
                let prefix = brk_sgr.to_sgr();
                cur = format!("{}{}", prefix, after);
                cur_w = cur_w - brk_w - 1;
                brk_pos = None;
            } else if cur_w == 0 {
                cur.push_str(grapheme);
                out.push(cur);
                cur = sgr.to_sgr();
                i += glen;
                continue;
            } else {
                out.push(cur);
                cur = sgr.to_sgr();
                cur_w = 0;
                brk_pos = None;
            }
        }

        cur.push_str(grapheme);
        cur_w += gw;
        i += glen;
    }

    if cur_w > 0 || out.len() == initial {
        out.push(cur);
    }
}
