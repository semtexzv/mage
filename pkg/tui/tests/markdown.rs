use std::rc::Rc;
use tau_tui_next::ansi::{strip_ansi, visible_width};
use tau_tui_next::{Markdown, Padding, Renderer};

fn lines(md: &str, width: u16) -> Vec<String> {
    let mut m = Markdown::new(width);
    m.append(md);
    m.lines().iter().map(|l| l.to_string()).collect()
}

fn stripped(md: &str, width: u16) -> Vec<String> {
    lines(md, width)
        .into_iter()
        .map(|l| tau_tui_next::ansi::strip_ansi(&l))
        .collect()
}

// ── Block rendering ───────────────────────────────────────────────

#[test]
fn heading_levels() {
    let s = stripped("# H1\n\n## H2\n\n### H3", 80);
    assert_eq!(s[0], "H1"); // h1: no prefix, bold+underline
    assert_eq!(s[1], ""); // blank separator
    assert_eq!(s[2], "H2"); // h2: no prefix, just bold
    assert_eq!(s[3], ""); // blank separator
    assert_eq!(s[4], "### H3"); // h3+: ### prefix
}

#[test]
fn paragraph_wrapping() {
    let s = stripped("the quick brown fox jumps over the lazy dog", 20);
    assert!(s.len() >= 2, "should wrap: {:?}", s);
    assert!(s[0].len() <= 20);
}

#[test]
fn code_block() {
    let s = stripped("```rust\nfn main() {}\n```", 80);
    assert_eq!(s[0], "```rust");
    assert_eq!(s[1], "  fn main() {}");
    assert_eq!(s[2], "```"); // real closing fence
}

#[test]
fn unordered_list() {
    let s = stripped("- alpha\n- beta\n- gamma", 80);
    assert_eq!(s[0], "- alpha");
    assert_eq!(s[1], "- beta");
    assert_eq!(s[2], "- gamma");
}

#[test]
fn ordered_list() {
    let s = stripped("1. first\n2. second\n3. third", 80);
    assert_eq!(s[0], "1. first");
    assert_eq!(s[1], "2. second");
    assert_eq!(s[2], "3. third");
}

#[test]
fn blockquote() {
    let s = stripped("> quoted text", 80);
    assert_eq!(s[0], "│ quoted text");
}

#[test]
fn horizontal_rule() {
    let s = stripped("---", 80);
    assert!(s[0].starts_with("─"), "should be hr: {:?}", s[0]);
}

#[test]
fn inline_bold_italic() {
    let raw = lines("**bold** and *italic*", 80);
    // Should contain ANSI bold codes
    assert!(raw[0].contains("\x1b[1m"), "missing bold");
    assert!(raw[0].contains("\x1b[3m"), "missing italic");
}

#[test]
fn inline_code() {
    let raw = lines("use `println!`", 80);
    assert!(raw[0].contains("\x1b[36m"), "missing cyan for code");
    let s = stripped("use `println!`", 80);
    assert_eq!(s[0], "use `println!`");
}

#[test]
fn link_same_text_and_url() {
    // Autolink: text == URL, should NOT duplicate
    let s = stripped("<https://example.com>", 80);
    // Should contain the URL once, not twice
    let count = s[0].matches("example.com").count();
    assert_eq!(count, 1, "autolink should not duplicate URL: {:?}", s[0]);
}

#[test]
fn link_different_text() {
    let s = stripped("[click here](https://example.com)", 80);
    assert!(s[0].contains("click here"), "missing link text");
    assert!(s[0].contains("example.com"), "missing URL");
}

#[test]
fn table_basic() {
    let md = "| A | B |\n|---|---|\n| 1 | 2 |\n| 3 | 4 |";
    let s = stripped(md, 80);
    // Should have box-drawing characters
    assert!(s[0].contains("┌"), "missing top border: {:?}", s);
    assert!(s.iter().any(|l| l.contains("│")), "missing column separators");
    assert!(s.last().unwrap().contains("┘"), "missing bottom border");
}

// ── Incremental caching ───────────────────────────────────────────

#[test]
fn incremental_append_reuses_cache() {
    let mut md = Markdown::new(80);
    md.append("# Hello\n\nFirst paragraph.");
    let lines1: Vec<Rc<str>> = md.lines().to_vec();

    // Append a new block
    md.append("\n\nSecond paragraph.");
    let lines2: Vec<Rc<str>> = md.lines().to_vec();

    assert!(lines2.len() > lines1.len(), "should have more lines");

    // The heading lines should be the exact same Rc (ptr_eq = cache hit)
    assert!(
        Rc::ptr_eq(&lines1[0], &lines2[0]),
        "heading should be cached (same Rc pointer)"
    );
}

#[test]
fn incremental_append_rerenders_last_block() {
    let mut md = Markdown::new(80);
    md.append("# Hello\n\nPartial");
    let lines1: Vec<Rc<str>> = md.lines().to_vec();
    // [0]="Hello" [1]="" [2]="Partial"

    // Extend the last paragraph (still same block)
    md.append(" more text");
    let lines2: Vec<Rc<str>> = md.lines().to_vec();
    // [0]="Hello" [1]="" [2]="Partial more text"

    // Heading + blank should be cached (same Rc pointers)
    assert!(Rc::ptr_eq(&lines1[0], &lines2[0]), "heading should be cached");
    assert!(Rc::ptr_eq(&lines1[1], &lines2[1]), "blank should be cached");

    // But the paragraph should differ (re-rendered)
    let para1 = tau_tui_next::ansi::strip_ansi(&lines1[2]);
    let para2 = tau_tui_next::ansi::strip_ansi(&lines2[2]);
    assert!(!para1.contains("more text"));
    assert!(para2.contains("more text"), "last block re-rendered: {para2}");
}

#[test]
fn width_change_invalidates_all() {
    let mut md = Markdown::new(80);
    md.append("# Hello\n\nSome text here.");
    let lines1: Vec<Rc<str>> = md.lines().to_vec();

    md.set_width(40);
    let lines2: Vec<Rc<str>> = md.lines().to_vec();

    // After width change, nothing should be ptr_eq (all re-rendered)
    if !lines1.is_empty() && !lines2.is_empty() {
        assert!(
            !Rc::ptr_eq(&lines1[0], &lines2[0]),
            "width change should invalidate cache"
        );
    }
}

#[test]
fn set_source_non_append_clears_cache() {
    let mut md = Markdown::new(80);
    md.append("# Hello\n\nParagraph.");
    let _ = md.lines();

    // Completely different source
    md.set_source("# Different\n\nOther text.".to_string());
    let lines = md.lines();
    let s = tau_tui_next::ansi::strip_ansi(&lines[0]);
    assert!(s.contains("Different"));
}

#[test]
fn empty_source() {
    let mut md = Markdown::new(80);
    assert!(md.lines().is_empty());
}

#[test]
fn unclosed_code_fence() {
    // Incomplete markdown — no fake closing fence
    let s = stripped("```rust\nfn main() {}", 80);
    assert_eq!(s.len(), 2);
    assert_eq!(s[0], "```rust");
    assert_eq!(s[1], "  fn main() {}");
    // No closing ``` — partial render
}

#[test]
fn fence_only() {
    let s = stripped("```rust", 80);
    assert_eq!(s, vec!["```rust"]);
}

#[test]
fn partial_code_then_complete() {
    let mut md = tau_tui_next::Markdown::new(80);
    md.append("```rust\nfn foo()");
    let s1: Vec<String> = md.lines().iter().map(|l| tau_tui_next::ansi::strip_ansi(l)).collect();
    assert_eq!(s1.len(), 2); // no closing fence

    md.append("\n```");
    let s2: Vec<String> = md.lines().iter().map(|l| tau_tui_next::ansi::strip_ansi(l)).collect();
    assert_eq!(s2[2], "```"); // now has closing fence
}

// ── Nested structures ─────────────────────────────────────────────

#[test]
fn nested_list() {
    let md = "- outer\n  - inner\n  - inner2\n- outer2";
    let s = stripped(md, 80);
    // Inner items should be indented
    assert!(
        s.iter().any(|l| l.starts_with("  ")),
        "nested list should be indented: {:?}",
        s
    );
}

#[test]
fn task_list() {
    let md = "- [ ] unchecked\n- [x] checked";
    let s = stripped(md, 80);
    assert!(s.iter().any(|l| l.contains("☐")), "missing unchecked: {:?}", s);
    assert!(s.iter().any(|l| l.contains("☑")), "missing checked: {:?}", s);
}

// ── Renderer integration ──────────────────────────────────────────

#[test]
fn render_pushes_correct_lines_into_renderer() {
    let mut md = Markdown::new(80);
    md.append("# Hello\n\nSome **bold** text.\n\n- item one\n- item two");
    let cached_lines = md.lines().to_vec();
    let expected_count = cached_lines.len();

    // Render into a Renderer and verify the same lines appear
    let mut r = Renderer::new();
    r.begin_frame(80, 24);
    md.render(&mut r);

    assert_eq!(
        r.line_count(),
        expected_count,
        "render() should push exactly the cached line count into the renderer"
    );
}

// ── Table wrapping ────────────────────────────────────────────────

const WIDE_TABLE: &str = "\
| Feature | Description | Since |
|:--------|:------------|------:|
| Headings | h1 bold+underline, h2 bold | v0.1 |
| Tables | Box drawing, alignment, wrapping | v0.2 |";

#[test]
fn table_cells_wrap_at_narrow_width() {
    // At width 50 the Description column wraps "wrapping" to a 2nd row
    let s = stripped(WIDE_TABLE, 50);
    // Find the data row for "Tables" — it should span 2 visual rows
    let table_row_lines: Vec<&String> = s
        .iter()
        .filter(|l| l.contains("Tables") || l.contains("wrapping"))
        .collect();
    assert!(
        table_row_lines.len() >= 2,
        "Tables row should wrap to ≥2 visual lines at w=50: {table_row_lines:?}"
    );
    // The wrapped continuation should contain the overflow text
    assert!(
        table_row_lines.iter().any(|l| l.contains("wrapping")),
        "wrapped cell must contain overflow text 'wrapping': {table_row_lines:?}"
    );
}

#[test]
fn table_wrap_preserves_borders() {
    let s = stripped(WIDE_TABLE, 50);
    for l in &s {
        if l.is_empty() {
            continue;
        }
        // Every non-blank table line should start and end with │
        assert!(l.starts_with('│') || l.starts_with('┌') || l.starts_with('├') || l.starts_with('└'),
            "table line missing left border: {l:?}");
        assert!(l.ends_with('│') || l.ends_with('┐') || l.ends_with('┤') || l.ends_with('┘'),
            "table line missing right border: {l:?}");
    }
}

#[test]
fn table_wrap_all_lines_same_width() {
    // When cells wrap, every line of the table should have the same visible width
    let raw = lines(WIDE_TABLE, 50);
    let widths: Vec<usize> = raw.iter().filter(|l| !l.is_empty()).map(|l| visible_width(l)).collect();
    let first = widths[0];
    for (i, &w) in widths.iter().enumerate() {
        assert_eq!(w, first, "line {i} has width {w}, expected {first}");
    }
}

#[test]
fn table_deeply_wrapped_cells() {
    // At width 35, most cells wrap; ensure structure is intact
    let s = stripped(WIDE_TABLE, 35);
    // Count border rows (┌, ├, └)
    let borders: Vec<&String> = s.iter().filter(|l| l.starts_with('┌') || l.starts_with('├') || l.starts_with('└')).collect();
    // top + 2 separators + bottom = 4 border rows
    assert_eq!(borders.len(), 4, "expected 4 border rows: {borders:?}");

    // All visible lines should be exactly width 35
    let raw = lines(WIDE_TABLE, 35);
    for l in &raw {
        if l.is_empty() { continue; }
        let vw = visible_width(l);
        assert_eq!(vw, 35, "line should be w=35, got {vw}: {}", strip_ansi(l));
    }
}

#[test]
fn table_multi_row_cell_alignment() {
    // Right-aligned "Since" column: wrapped lines should be right-aligned
    let s = stripped(WIDE_TABLE, 35);
    // Find rows containing "v0." — these are from the Since column
    // In the wrapped table the Since values span multiple rows like "v0." / "  1"
    let since_rows: Vec<&String> = s.iter().filter(|l| l.contains("v0.")).collect();
    assert!(!since_rows.is_empty(), "no Since values found: {:?}", s);
}

// ── Table truncation / fallback ───────────────────────────────────

#[test]
fn table_extreme_narrow_falls_back_to_text() {
    // At width 10 the table can't fit even 1 char per column with borders,
    // so it should fall back to plain text rendering
    let s = stripped(WIDE_TABLE, 10);
    // Should NOT have box-drawing table borders
    let has_box = s.iter().any(|l| l.contains('┌') || l.contains('└'));
    assert!(!has_box, "width=10 should fall back to text, not box table: {s:?}");
    // But should still contain the cell text
    assert!(s.iter().any(|l| l.contains("Feature")), "cell text should still appear");
    assert!(s.iter().any(|l| l.contains("Headings")), "cell text should still appear");
}

#[test]
fn table_just_barely_fits() {
    // 3-column table needs at least 3*3+1=10 border chars + 3 content chars = 13
    let small = "| A | B | C |\n|---|---|---|\n| x | y | z |";
    let s = stripped(small, 13);
    // Should render as a table (has borders)
    assert!(s.iter().any(|l| l.contains('┌')), "should fit as table at w=13: {s:?}");
}

#[test]
fn table_one_char_too_narrow_for_table() {
    let small = "| A | B | C |\n|---|---|---|\n| x | y | z |";
    let s = stripped(small, 12);
    // Border overhead = 3*3+1=10, avail=12-10=2, need 3 (ncols) → falls back
    // Actually avail=2 < ncols=3, so it falls back to text
    let has_table = s.iter().any(|l| l.contains('┌'));
    assert!(!has_table, "should fall back at w=12: {s:?}");
}

// ── Table with inline styles ──────────────────────────────────────

#[test]
fn table_cells_with_bold_and_code() {
    let md = "| Name | Status |\n|------|--------|\n| **Alpha** | `pass` |\n| ~~Beta~~ | *fail* |";
    let raw = lines(md, 40);
    let s = stripped(md, 40);

    // Check that ANSI bold is present in the Alpha cell
    let alpha_line = raw.iter().find(|l| l.contains("Alpha")).expect("Alpha row");
    assert!(alpha_line.contains("\x1b[1m"), "Alpha cell missing bold ANSI");

    // Check that inline code styling is present in the pass cell
    let pass_line = raw.iter().find(|l| l.contains("pass")).expect("pass row");
    assert!(pass_line.contains("\x1b[36m"), "pass cell missing cyan ANSI for code");

    // Check strikethrough on Beta
    let beta_line = raw.iter().find(|l| l.contains("Beta")).expect("Beta row");
    assert!(beta_line.contains("\x1b[9m"), "Beta cell missing strikethrough ANSI");

    // Check italic on fail
    let fail_line = raw.iter().find(|l| l.contains("fail")).expect("fail row");
    assert!(fail_line.contains("\x1b[3m"), "fail cell missing italic ANSI");

    // Stripped text should be clean
    assert!(s.iter().any(|l| l.contains("Alpha")));
    assert!(s.iter().any(|l| l.contains("`pass`")));
}

#[test]
fn table_styled_cells_wrap_correctly() {
    // Bold/code text in narrow cells should wrap preserving ANSI across lines
    let md = "\
| Key | Value |
|-----|-------|
| **very long bold key** | `some inline code value` |";
    let raw = lines(md, 30);
    let s: Vec<String> = raw.iter().map(|l| strip_ansi(l)).collect();

    // Should have box borders
    assert!(s.iter().any(|l| l.contains('┌')), "missing table borders: {s:?}");

    // All table lines same width
    let widths: Vec<usize> = raw.iter().filter(|l| !l.is_empty()).map(|l| visible_width(l)).collect();
    let first = widths[0];
    for (i, &w) in widths.iter().enumerate() {
        assert_eq!(w, first, "table line {i} has width {w}, expected {first}");
    }
}

// ── Table with padding ────────────────────────────────────────────

#[test]
fn table_with_horizontal_padding() {
    let pad = Padding::horizontal(4);
    let outer = 60u16;
    let mut md = Markdown::with_pad(outer, pad);
    md.append(WIDE_TABLE);
    let md_lines: Vec<String> = md.lines().iter().map(|l| l.to_string()).collect();

    for l in &md_lines {
        if l.trim().is_empty() { continue; }
        // Left padding: 4 spaces
        assert!(l.starts_with("    "), "missing left pad: {l:?}");
        // Total visible width should not exceed outer width
        let vw = visible_width(l);
        assert!(vw <= outer as usize, "line exceeds outer width ({vw} > {outer}): {l:?}");
    }
}

#[test]
fn table_with_padding_borders_intact() {
    let pad = Padding::horizontal(4);
    let outer = 50u16;
    let mut md = Markdown::with_pad(outer, pad);
    md.append(WIDE_TABLE);
    let md_lines: Vec<String> = md.lines().iter().map(|l| l.to_string()).collect();
    let stripped_lines: Vec<String> = md_lines.iter().map(|l| strip_ansi(l)).collect();

    for l in &stripped_lines {
        let trimmed = l.trim_start();
        if trimmed.starts_with('┌') { assert!(trimmed.ends_with('┐'), "broken top: {l:?}"); }
        if trimmed.starts_with('├') { assert!(trimmed.ends_with('┤'), "broken sep: {l:?}"); }
        if trimmed.starts_with('└') { assert!(trimmed.ends_with('┘'), "broken bot: {l:?}"); }
    }
}

#[test]
fn table_padding_narrows_inner_width() {
    // With large padding the table's inner width shrinks, forcing more wrapping
    let no_pad = {
        let mut md = Markdown::new(50);
        md.append(WIDE_TABLE);
        md.lines().len()
    };
    let with_pad = {
        let mut md = Markdown::with_pad(50, Padding::horizontal(8));
        md.append(WIDE_TABLE);
        md.lines().len()
    };
    assert!(
        with_pad >= no_pad,
        "padding should cause same or more lines: no_pad={no_pad}, with_pad={with_pad}"
    );
}

// ── Soft-reset / bg composability ─────────────────────────────────

#[test]
fn no_hard_reset_in_output() {
    // Markdown lines must never contain \x1b[0m (full RESET) because it
    // kills background color from outer containers.  SOFT_RESET
    // (\x1b[22;23;24;29;39m) is used instead.
    let md_src = "\
# Heading

Paragraph with **bold** and *italic*.

```rust
fn main() {}
```

---

- [x] checked
- [ ] unchecked

> blockquote

[link](https://example.com)

| A | B |
|---|---|
| 1 | 2 |";

    let raw = lines(md_src, 60);
    for (i, line) in raw.iter().enumerate() {
        assert!(
            !line.contains("\x1b[0m"),
            "line {i} contains hard RESET (\\x1b[0m), use SOFT_RESET: {line:?}",
        );
    }
}

#[test]
fn style_transitions_preserve_bg_in_composed_line() {
    // Simulate wrapping a markdown heading inside a bg container.
    // Style transitions should never emit \x1b[0m (hard reset), only
    // individual attribute-off codes, so outer bg is preserved.
    let mut md = Markdown::new(40);
    md.append("# Hello World");
    let heading = &md.lines()[0];

    // Wrap in bg
    let bg = "\x1b[44m"; // blue bg
    let composed = format!("{bg}{heading}{bg}{}\x1b[0m", " ".repeat(5));

    // No hard reset inside the heading itself — style transitions use
    // individual off-codes (e.g. \x1b[22;24;39m) that don't touch bg.
    assert!(!heading.contains("\x1b[0m"), "heading should not contain hard reset");

    // The only \x1b[0m should be at the very end of the composed line
    let first_reset = composed.find("\x1b[0m");
    assert!(first_reset.is_some(), "composed line should end with reset");
    assert!(
        composed.ends_with("\x1b[0m"),
        "hard reset should only appear at the very end: {composed:?}",
    );
}
