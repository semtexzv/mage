use std::rc::Rc;
use mage_tui::ansi::strip_ansi;
use mage_tui::renderer::{Line, Renderer};
use mage_tui::style::Color;
use mage_tui::testutil::TestTerminal;
use mage_tui::Padding;

// ── begin_frame + push + end_frame basic flow ───────────────────

#[test]
fn basic_frame_renders_lines() {
    let mut r = Renderer::new();
    let mut term = TestTerminal::new(80, 24);

    r.begin_frame(80, 24);
    r.push_line("hello");
    r.push_line("world");
    r.end_frame(&mut term);

    // Should contain our content
    assert!(term.output.contains("hello"), "missing 'hello': {}", term.output);
    assert!(term.output.contains("world"), "missing 'world': {}", term.output);
}

#[test]
fn push_blank_adds_empty_line() {
    let mut r = Renderer::new();
    let mut term = TestTerminal::new(80, 24);

    r.begin_frame(80, 24);
    r.push_line("above");
    r.push_blank();
    r.push_line("below");
    r.end_frame(&mut term);

    // Should have 3 lines in prev_lines
    assert_eq!(r.prev_lines.len(), 3);
    assert_eq!(r.prev_lines[1].as_ref(), "");
}

#[test]
fn push_lines_appends_slice() {
    let mut r = Renderer::new();
    let mut term = TestTerminal::new(80, 24);

    let cached: Vec<Line> = vec![
        Rc::from("line a"),
        Rc::from("line b"),
        Rc::from("line c"),
    ];

    r.begin_frame(80, 24);
    r.push_lines(&cached);
    r.end_frame(&mut term);

    assert_eq!(r.prev_lines.len(), 3);
    // Should be the same Rc instances
    assert!(Rc::ptr_eq(&r.prev_lines[0], &cached[0]));
    assert!(Rc::ptr_eq(&r.prev_lines[1], &cached[1]));
    assert!(Rc::ptr_eq(&r.prev_lines[2], &cached[2]));
}

#[test]
fn width_returns_frame_width() {
    let mut r = Renderer::new();
    r.begin_frame(120, 40);
    assert_eq!(r.width(), 120);
    assert_eq!(r.height(), 40);
}

// ── set_cursor positions hardware cursor ────────────────────────

#[test]
fn set_cursor_positions_cursor() {
    let mut r = Renderer::new();
    let mut term = TestTerminal::new(80, 24);

    r.begin_frame(80, 24);
    r.push_line("prompt> hello");
    r.set_cursor(0, 10);
    r.end_frame(&mut term);

    // Terminal output should contain show-cursor escape
    assert!(term.output.contains("\x1b[?25h"), "cursor not shown");
    // Should contain cursor column positioning (col 10 → ESC[11G, 1-indexed)
    assert!(term.output.contains("\x1b[11G"), "cursor col not set: {}", term.output);
}

// ── begin_frame resets state ────────────────────────────────────

#[test]
fn begin_frame_clears_accumulated_lines() {
    let mut r = Renderer::new();
    let mut term = TestTerminal::new(80, 24);

    // First frame
    r.begin_frame(80, 24);
    r.push_line("frame 1");
    r.end_frame(&mut term);

    // Second frame
    r.begin_frame(80, 24);
    r.push_line("frame 2");
    r.end_frame(&mut term);

    // After second frame, prev_lines should only have frame 2 content
    assert_eq!(r.prev_lines.len(), 1);
    assert_eq!(r.prev_lines[0].as_ref(), "frame 2");
}

#[test]
fn begin_frame_resets_cursor() {
    let mut r = Renderer::new();
    let mut term = TestTerminal::new(80, 24);

    // Frame with cursor
    r.begin_frame(80, 24);
    r.push_line("input line");
    r.set_cursor(0, 5);
    r.end_frame(&mut term);

    // Frame without cursor - should hide it
    term.output.clear();
    r.begin_frame(80, 24);
    r.push_line("no cursor");
    r.end_frame(&mut term);

    // Cursor should be hidden
    assert!(term.output.contains("\x1b[?25l"), "cursor not hidden: {}", term.output);
}

// ── Unchanged lines (same Rc) not repainted ─────────────────────

#[test]
fn unchanged_lines_not_repainted() {
    let mut r = Renderer::new();
    let mut term = TestTerminal::new(80, 24);

    let line_a: Line = Rc::from("unchanged line");
    let line_b: Line = Rc::from("also unchanged");

    // First frame - full render
    r.begin_frame(80, 24);
    r.push_lines(&[line_a.clone(), line_b.clone()]);
    r.end_frame(&mut term);

    // Second frame - same Rc pointers
    term.output.clear();
    r.begin_frame(80, 24);
    r.push_lines(&[line_a.clone(), line_b.clone()]);
    r.end_frame(&mut term);

    // Diff render should not contain the line content since nothing changed
    // (it may contain sync escapes but not the actual text)
    assert!(!term.output.contains("unchanged line"),
        "unchanged line was repainted: {}", term.output);
    assert!(!term.output.contains("also unchanged"),
        "unchanged line was repainted: {}", term.output);
}

#[test]
fn changed_line_is_repainted() {
    let mut r = Renderer::new();
    let mut term = TestTerminal::new(80, 24);

    let line_a: Line = Rc::from("stays same");
    let line_b: Line = Rc::from("will change");

    // First frame
    r.begin_frame(80, 24);
    r.push_lines(&[line_a.clone(), line_b]);
    r.end_frame(&mut term);

    // Second frame - line_b is different Rc
    let line_b2: Line = Rc::from("has changed");
    term.output.clear();
    r.begin_frame(80, 24);
    r.push_lines(&[line_a.clone(), line_b2]);
    r.end_frame(&mut term);

    // Should contain the new line but not the unchanged one
    assert!(term.output.contains("has changed"),
        "changed line not repainted: {}", term.output);
}

// ── finalize moves cursor past content ──────────────────────────

#[test]
fn finalize_moves_past_content() {
    let mut r = Renderer::new();
    let mut term = TestTerminal::new(80, 24);

    r.begin_frame(80, 24);
    r.push_line("line 1");
    r.push_line("line 2");
    r.end_frame(&mut term);

    term.output.clear();
    r.finalize(&mut term);

    // Should contain CRLF to move past content
    assert!(term.output.contains("\r\n"), "no CRLF in finalize: {:?}", term.output);
}

// ── push / text tests ───────────────────────────────────────────

#[test]
fn string_pushes_single_line() {
    let mut r = Renderer::new();
    r.begin_frame(80, 24);
    r.push_line("hello world");
    assert_eq!(r.prev_lines.len(), 0); // not end_frame yet, check lines via end_frame
    let mut term = TestTerminal::new(80, 24);
    r.end_frame(&mut term);
    assert_eq!(r.prev_lines.len(), 1);
    assert_eq!(r.prev_lines[0].as_ref(), "hello world");
}

#[test]
fn text_with_padding_renders_indentation_and_blanks() {
    let mut r = Renderer::new();
    let mut term = TestTerminal::new(80, 24);

    r.begin_frame(80, 24);
    r.push_text_styled(
        "hello",
        &Padding::new(1, 0, 2, 4),
        None,
    );
    r.end_frame(&mut term);
    // 1 top blank + 1 content + 2 bottom blanks = 4 lines
    assert_eq!(r.prev_lines.len(), 4);
    // Top padding: left-padded blank line
    assert_eq!(r.prev_lines[0].as_ref(), "    ");
    // Content with left padding of 4
    assert_eq!(r.prev_lines[1].as_ref(), "    hello");
    // Bottom padding: left-padded blank lines
    assert_eq!(r.prev_lines[2].as_ref(), "    ");
    assert_eq!(r.prev_lines[3].as_ref(), "    ");
}

#[test]
fn text_with_bg_fills_full_width() {
    let mut r = Renderer::new();
    let mut term = TestTerminal::new(40, 24);

    r.begin_frame(40, 24);
    r.push_text_styled(
        "hi",
        &Padding::new(0, 0, 0, 2),
        Some(Color::Blue),
    );
    r.end_frame(&mut term);

    assert_eq!(r.prev_lines.len(), 1);
    let line = r.prev_lines[0].as_ref();
    // Should start with blue bg ANSI code
    assert!(line.contains("\x1b[44m"), "missing bg code: {}", line);
    // Should end with reset
    assert!(line.ends_with("\x1b[0m"), "missing reset: {}", line);
    // Content should have "  hi" (2 left pad) then spaces to fill width=40
    // "  hi" is 4 visible chars, so 36 spaces fill
    assert!(line.contains("  hi"), "missing padded content: {}", line);
}

#[test]
fn text_with_bg_padding_fills_blank_lines() {
    let mut r = Renderer::new();
    let mut term = TestTerminal::new(20, 24);

    r.begin_frame(20, 24);
    r.push_text_styled(
        "x",
        &Padding::new(1, 0, 0, 0),
        Some(Color::Red),
    );
    r.end_frame(&mut term);
    assert_eq!(r.prev_lines.len(), 2);
    // Top padding line should also have bg fill
    let top = r.prev_lines[0].as_ref();
    assert!(top.contains("\x1b[41m"), "top padding missing bg: {}", top);
    assert!(top.ends_with("\x1b[0m"), "top padding missing reset: {}", top);
}

#[test]
fn spacer_pushes_blank_lines() {
    let mut r = Renderer::new();
    let mut term = TestTerminal::new(80, 24);

    r.begin_frame(80, 24);
    for _ in 0..3 { r.push_blank(); }
    r.end_frame(&mut term);

    assert_eq!(r.prev_lines.len(), 3);
    for line in &r.prev_lines {
        assert_eq!(line.as_ref(), "");
    }
}

#[test]
fn column_renders_children_sequentially() {
    let mut r = Renderer::new();
    let mut term = TestTerminal::new(80, 24);

    r.begin_frame(80, 24);
    r.push_line("first");
    r.push_line("second");
    r.push_line("third");
    r.end_frame(&mut term);

    assert_eq!(r.prev_lines.len(), 3);
    assert_eq!(r.prev_lines[0].as_ref(), "first");
    assert_eq!(r.prev_lines[1].as_ref(), "second");
    assert_eq!(r.prev_lines[2].as_ref(), "third");
}

#[test]
fn input_sets_cursor_at_correct_column() {
    let mut r = Renderer::new();
    let mut term = TestTerminal::new(80, 24);

    r.begin_frame(80, 24);
    r.push_input("> ", "hello", 3);
    r.end_frame(&mut term);

    assert_eq!(r.prev_lines.len(), 1);
    assert_eq!(r.prev_lines[0].as_ref(), "> hello");
    // Cursor should be at visible_width("> ") + 3 = 2 + 3 = 5
    // Terminal uses 1-indexed, so ESC[6G
    assert!(term.output.contains("\x1b[6G"), "cursor col not set correctly: {}", term.output);
    assert!(term.output.contains("\x1b[?25h"), "cursor not shown");
}

#[test]
fn text_word_wraps() {
    let mut r = Renderer::new();
    let mut term = TestTerminal::new(20, 24);
    // "hello world foo bar" at width 10 should wrap
    r.begin_frame(20, 24);
    r.push_text_styled(
        "hello world foo bar",
        &Padding::new(0, 5, 0, 5), // 5 left + 5 right = 10 inner width
        None,
    );
    r.end_frame(&mut term);
    // With inner width 10: "hello" "world foo" "bar" or similar wrapping
    assert!(r.prev_lines.len() >= 2, "expected wrapping, got {} lines", r.prev_lines.len());
    // Each line should be indented with 5 spaces
    for line in &r.prev_lines {
        assert!(line.starts_with("     "), "missing left padding: {:?}", line.as_ref());
    }
}

#[test]
fn bg_filled_line_survives_content_reset() {
    let mut r = Renderer::new();
    let mut term = TestTerminal::new(20, 24);
    r.begin_frame(20, 24);
    // Content with an inline reset
    r.push_text_styled(
        "\x1b[1mhi\x1b[0m there",
        &Padding::ZERO,
        Some(Color::Blue),
    );
    r.end_frame(&mut term);
    let line = r.prev_lines[0].as_ref();
    // The fill spaces should still have blue bg.
    // Count occurrences of the blue bg code \x1b[44m — should appear at least twice
    // (once before content, once before fill).
    let bg_count = line.matches("\x1b[44m").count();
    assert!(bg_count >= 2, "bg should be re-emitted before fill: {line:?} (count: {bg_count})");
}

// ── composite_at tests ──────────────────────────────────────────

#[test]
fn composite_at_plain_line() {
    let mut r = Renderer::new();
    r.begin_frame(80, 24);
    r.push_line("hello world test");
    r.composite_at(0, 6, "OVER", 4);

    // We need to end frame to inspect prev_lines
    let mut term = TestTerminal::new(80, 24);
    r.end_frame(&mut term);

    let composited = r.prev_lines[0].as_ref();
    let visible = strip_ansi(composited);
    // "hello " (6) + "OVER" (4) + "d test" (remaining from col 10)
    assert_eq!(visible, "hello OVERd test",
        "visible text mismatch: {visible:?}, raw: {composited:?}");
}

#[test]
fn composite_at_styled_base_restores_after() {
    let mut r = Renderer::new();
    r.begin_frame(80, 24);
    r.push_line("\x1b[1mhello world test\x1b[0m");
    r.composite_at(0, 6, "XX", 4);

    let mut term = TestTerminal::new(80, 24);
    r.end_frame(&mut term);

    let composited = r.prev_lines[0].as_ref();
    let visible = strip_ansi(composited);
    assert_eq!(visible, "hello XX  d test",
        "visible text mismatch: {visible:?}");

    // After the overlay portion (XX + padding + reset), the remaining "test"
    // should have bold re-established via \x1b[1m
    // There should be \x1b[1m somewhere after the overlay content
    let after_overlay = &composited[composited.find("XX").unwrap() + 2..];
    assert!(after_overlay.contains("\x1b[1m"),
        "bold should be re-established after overlay: {composited:?}");
}

#[test]
fn composite_at_styled_overlay_on_plain() {
    let mut r = Renderer::new();
    r.begin_frame(80, 24);
    r.push_line("aaaa bbbb cccc");
    r.composite_at(0, 5, "\x1b[31mRED\x1b[0m", 4);

    let mut term = TestTerminal::new(80, 24);
    r.end_frame(&mut term);

    let composited = r.prev_lines[0].as_ref();
    let visible = strip_ansi(composited);
    assert_eq!(visible, "aaaa RED  cccc",
        "visible text mismatch: {visible:?}");

    // The "cccc" portion should have no color leakage from the red overlay.
    // Find the last reset before "cccc" — everything after that should be plain.
    let cccc_pos = composited.rfind("cccc").expect("should contain 'cccc'");
    let after_portion = &composited[cccc_pos..];
    assert!(!after_portion.contains("\x1b[31m"),
        "red should not leak into after portion: {composited:?}");
}

#[test]
fn composite_at_beyond_line_length() {
    let mut r = Renderer::new();
    r.begin_frame(80, 24);
    r.push_line("short");
    r.composite_at(0, 10, "XXXXX", 5);

    let mut term = TestTerminal::new(80, 24);
    r.end_frame(&mut term);

    let composited = r.prev_lines[0].as_ref();
    let visible = strip_ansi(composited);
    // "short" (5) + 5 spaces padding to reach col 10 + "XXXXX" (5)
    assert_eq!(visible, "short     XXXXX",
        "visible text mismatch: {visible:?}");
}

#[test]
fn composite_at_full_width() {
    let mut r = Renderer::new();
    r.begin_frame(80, 24);
    r.push_line("0123456789");
    r.composite_at(0, 0, "XXXXX", 10);

    let mut term = TestTerminal::new(80, 24);
    r.end_frame(&mut term);

    let composited = r.prev_lines[0].as_ref();
    let visible = strip_ansi(composited);
    // Overlay is 5 chars in 10-wide slot: "XXXXX" + 5 spaces
    assert_eq!(visible, "XXXXX     ",
        "visible text mismatch: {visible:?}");
}

#[test]
fn composite_at_preserves_bg() {
    let mut r = Renderer::new();
    r.begin_frame(80, 24);
    r.push_line("\x1b[44mblue background here\x1b[0m");
    r.composite_at(0, 5, "hi", 4);

    let mut term = TestTerminal::new(80, 24);
    r.end_frame(&mut term);

    let composited = r.prev_lines[0].as_ref();
    let visible = strip_ansi(composited);
    // "blue " (5) + "hi" + 2 spaces padding + "ground here"
    assert_eq!(visible, "blue hi  ground here",
        "visible text mismatch: {visible:?}");

    // After overlay, remaining text should have blue bg re-established
    let after_hi = &composited[composited.find("hi").unwrap() + 2..];
    assert!(after_hi.contains("\x1b[") && after_hi.contains("44"),
        "blue bg should be re-established after overlay: {composited:?}");
}

#[test]
fn composite_at_row_out_of_bounds() {
    let mut r = Renderer::new();
    r.begin_frame(80, 24);
    r.push_line("line 0");
    r.push_line("line 1");
    // Should not panic
    r.composite_at(5, 0, "OVER", 4);
    assert_eq!(r.line_count(), 2, "out-of-bounds composite should be no-op");
}

#[test]
fn composite_at_multiple_rows() {
    let mut r = Renderer::new();
    r.begin_frame(80, 24);
    r.push_line("original content");
    r.push_line("second line");
    r.composite_at(1, 0, "MORE", 4);
    // Compositing same row again should still work
    r.composite_at(0, 5, "XX", 2);
    let mut term = TestTerminal::new(80, 24);
    r.end_frame(&mut term);

    // Both rows should have been composited
    assert_eq!(r.prev_lines.len(), 2);
}