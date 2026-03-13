use mage_tui::renderer::Renderer;
use mage_tui::testutil::TestTerminal;
use mage_tui::ansi::{strip_ansi, truncate_line, visible_width};
use mage_tui::{Markdown, Padding};

// ── Helpers ─────────────────────────────────────────────────────

fn render_text(content: &str, padding: &Padding, bg: Option<mage_tui::Color>, width: u16) -> Vec<String> {
    let mut r = Renderer::new();
    let mut term = TestTerminal::new(width, 24);
    r.begin_frame(width, 24);
    let mut t = mage_tui::Text::new(content);
    t = t.padding(*padding);
    if let Some(color) = bg {
        t = t.bg(color);
    }
    t.render(&mut r);
    r.end_frame(&mut term);
    r.prev_lines.iter().map(|l| strip_ansi(l)).collect()
}

fn render_lines(lines: &[String], width: u16) -> Vec<String> {
    let mut r = Renderer::new();
    let mut term = TestTerminal::new(width, 24);
    r.begin_frame(width, 24);
    for l in lines {
        r.push_line(l.as_str());
    }
    r.end_frame(&mut term);
    r.prev_lines.iter().map(|l| strip_ansi(l)).collect()
}

// ── Text padding ────────────────────────────────────────────────

#[test]
fn text_left_pad() {
    let lines = render_text("hello", &Padding::left(4), None, 40);
    assert_eq!(lines.len(), 1);
    assert!(lines[0].starts_with("    hello"), "got: {:?}", lines[0]);
}

#[test]
fn text_right_pad_constrains_wrap() {
    // 20 cols, 5 right padding → inner width 15
    let lines_padded = render_text("aaaa bbbb cccc dddd", &Padding::new(0, 5, 0, 0), None, 20);

    let lines_full = render_text("aaaa bbbb cccc dddd", &Padding::ZERO, None, 20);

    assert!(
        lines_padded.len() > lines_full.len(),
        "right pad should constrain: padded={} full={}",
        lines_padded.len(),
        lines_full.len()
    );
    for l in &lines_padded {
        assert!(visible_width(l) <= 15, "too wide: {l:?}");
    }
}

#[test]
fn horizontal_pad() {
    let lines = render_text("hello world", &Padding::horizontal(3), None, 40);
    assert!(lines[0].starts_with("   "), "left pad missing: {:?}", lines[0]);
    for l in &lines {
        assert!(visible_width(l) <= 40 - 3, "too wide: {l:?}");
    }
}

#[test]
fn top_bottom_pad() {
    let lines = render_text("hello", &Padding::new(2, 0, 3, 0), None, 40);
    assert_eq!(lines.len(), 6, "lines: {lines:?}");
    assert_eq!(lines[0], "");
    assert_eq!(lines[1], "");
    assert_eq!(lines[2], "hello");
    assert_eq!(lines[3], "");
    assert_eq!(lines[4], "");
    assert_eq!(lines[5], "");
}

#[test]
fn all_sides_pad() {
    let lines = render_text("hi", &Padding::all(1), None, 20);
    assert_eq!(lines.len(), 3);
    assert_eq!(lines[0], " ");
    assert_eq!(lines[1], " hi");
    assert_eq!(lines[2], " ");
}

// ── String (no padding, truncation only) ────────────────────────

#[test]
fn string_truncates_at_width() {
    let s = "a".repeat(50);
    let truncated = truncate_line(&s, 30);
    assert_eq!(visible_width(&truncated), 30);
}

// ── Markdown with padding (component-owned) ─────────────────────

#[test]
fn markdown_left_pad() {
    let pad = Padding::left(2);
    let mut md = Markdown::with_pad(40, pad);
    md.append("# Hello\n\nSome text here.");
    let md_lines: Vec<String> = md.lines().iter().map(|l| l.to_string()).collect();
    let lines = render_lines(&md_lines, 40);
    for l in &lines {
        if !l.is_empty() {
            assert!(l.starts_with("  "), "missing left pad: {l:?}");
        }
    }
}

#[test]
fn markdown_horizontal_pad() {
    let pad = Padding::horizontal(4);
    let mut md = Markdown::with_pad(60, pad);
    md.append("A paragraph with enough words to wrap at fifty-two columns but not at sixty.");
    let md_lines: Vec<String> = md.lines().iter().map(|l| l.to_string()).collect();
    let lines = render_lines(&md_lines, 60);
    for l in &lines {
        if l.trim().is_empty() {
            continue;
        }
        assert!(l.starts_with("    "), "missing left pad: {l:?}");
        assert!(visible_width(l) <= 56, "too wide ({}): {l:?}", visible_width(l));
    }
}

#[test]
fn markdown_table_respects_padding() {
    let pad = Padding::horizontal(2);
    let outer = 40u16;
    let mut md = Markdown::with_pad(outer, pad);
    md.append(
        "| Feature | Status | Notes |\n|---------|--------|-------|\n| Alpha | ✅ | works |\n| Beta | ⚠️ | wip |",
    );
    let md_lines: Vec<String> = md.lines().iter().map(|l| l.to_string()).collect();
    let lines = render_lines(&md_lines, outer);
    for l in &lines {
        let vw = visible_width(l);
        assert!(
            vw <= outer as usize,
            "table line exceeds width ({vw} > {outer}): {l:?}",
        );
    }
}

#[test]
fn markdown_table_borders_intact() {
    let pad = Padding::horizontal(4);
    let outer = 50u16;
    let mut md = Markdown::with_pad(outer, pad);
    md.append(
        "| Feature | Description | Status |\n\
         |---------|-------------|--------|\n\
         | Tables | Box drawing, alignment, cell wrapping | ✅ |\n\
         | Lists | Ordered, unordered, nested, tasks | ✅ |",
    );
    let md_lines: Vec<String> = md.lines().iter().map(|l| l.to_string()).collect();
    let lines = render_lines(&md_lines, outer);
    for l in &lines {
        if l.contains('┌') {
            assert!(l.contains('┐'), "broken top border: {l:?}");
        }
        if l.contains('├') {
            assert!(l.contains('┤'), "broken separator: {l:?}");
        }
        if l.contains('└') {
            assert!(l.contains('┘'), "broken bottom border: {l:?}");
        }
    }
}

// ── Cache / identity tests ──────────────────────────────────────

#[test]
fn different_padding_produces_different_output() {
    let lines1 = render_text("hello world", &Padding::ZERO, None, 40);

    let lines2 = render_text("hello world", &Padding::left(4), None, 40);

    assert_ne!(lines1, lines2);
    assert!(lines2[0].starts_with("    "));
}

// ── Input cursor ────────────────────────────────────────────────

#[test]
fn input_cursor_position() {
    let mut r = Renderer::new();
    let mut term = TestTerminal::new(40, 24);
    r.begin_frame(40, 24);
    let line = format!("{}{}", "> ", "hello");
    let row = r.line_count();
    r.push_line(line);
    r.set_cursor(row, mage_tui::ansi::visible_width("> ") + 3);
    r.end_frame(&mut term);
    // cursor at: prompt_width(2) + 3 = 5, terminal 1-indexed → ESC[6G
    assert!(term.output.contains("\x1b[6G"), "cursor col not set: {}", term.output);
}

#[test]
fn zero_pad_is_noop() {
    let lines1 = render_text("hello", &Padding::ZERO, None, 40);

    let lines2 = render_text("hello", &Padding::ZERO, None, 40);

    // Both should produce same content
    assert_eq!(lines1, lines2);
}
