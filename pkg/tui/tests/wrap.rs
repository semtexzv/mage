use tau_tui_next::ansi::{split_line_at_col, strip_ansi, visible_width};
use tau_tui_next::wrap::wrap_text;

#[test]
fn no_wrap_needed() {
    assert_eq!(wrap_text("hello", 80), vec!["hello"]);
}

#[test]
fn word_wrap() {
    assert_eq!(wrap_text("hello world foo", 11), vec!["hello world", "foo"]);
}

#[test]
fn long_word_break() {
    assert_eq!(wrap_text("abcdefghij", 5), vec!["abcde", "fghij"]);
}

#[test]
fn preserves_newlines() {
    assert_eq!(wrap_text("a\nb\nc", 80), vec!["a", "b", "c"]);
}

#[test]
fn empty_input() {
    assert!(wrap_text("", 80).is_empty());
}

#[test]
fn visible_width_plain() {
    assert_eq!(visible_width("hello"), 5);
}

#[test]
fn visible_width_ansi() {
    assert_eq!(visible_width("\x1b[31mhello\x1b[0m"), 5);
}

#[test]
fn split_line_at_col_preserves_sgr() {
    // Bold text: "\x1b[1m" + "hello world" + "\x1b[0m"
    let styled = "\x1b[1mhello world\x1b[0m";
    let (before, after) = split_line_at_col(styled, 5);
    // `before` should contain "hello" with reset
    assert!(before.contains("hello"), "before: {before:?}");
    // `after` should start with a valid SGR (bold re-established), not double-escaped
    assert!(!after.contains("\x1b[\x1b"), "double-escaped SGR in after: {after:?}");
    // `after` should contain the remaining text
    assert!(after.contains("world"), "after missing content: {after:?}");
    // If bold was active, after should start with \x1b[1m (re-establish bold)
    assert!(after.starts_with("\x1b[1m"), "after should re-establish bold: {after:?}");
}

// ── split_line_at_col: plain text ────────────────────────────────

#[test]
fn split_plain_at_start() {
    let (before, after) = split_line_at_col("hello world", 0);
    assert_eq!(before, "");
    assert_eq!(after, "hello world");
}

#[test]
fn split_plain_at_end() {
    let (before, after) = split_line_at_col("hello", 5);
    assert_eq!(strip_ansi(&before), "hello");
    assert_eq!(after, "");
}

#[test]
fn split_plain_middle() {
    let (before, after) = split_line_at_col("hello world", 5);
    assert_eq!(strip_ansi(&before), "hello");
    assert_eq!(strip_ansi(&after), " world");
}

#[test]
fn split_plain_beyond_end() {
    let (before, after) = split_line_at_col("hi", 100);
    assert_eq!(strip_ansi(&before), "hi");
    assert_eq!(after, "");
}

// ── split_line_at_col: styled text ───────────────────────────────

#[test]
fn split_bold_text_middle() {
    let (before, after) = split_line_at_col("\x1b[1mhello world\x1b[0m", 5);
    assert_eq!(strip_ansi(&before), "hello");
    assert!(before.ends_with("\x1b[0m"), "before should end with reset: {before:?}");
    assert!(after.starts_with("\x1b[1m"), "after should re-establish bold: {after:?}");
    assert_eq!(strip_ansi(&after), " world");
}

#[test]
fn split_multi_style() {
    // bold + cyan
    let (before, after) = split_line_at_col("\x1b[1;36mhello world\x1b[0m", 5);
    assert_eq!(strip_ansi(&before), "hello");
    assert!(before.ends_with("\x1b[0m"), "before should end with reset: {before:?}");
    // after should re-establish both bold AND cyan
    assert!(after.contains("\x1b["), "after should have SGR prefix: {after:?}");
    assert!(after.contains("1"), "after SGR should include bold (1): {after:?}");
    assert!(after.contains("36"), "after SGR should include cyan (36): {after:?}");
    assert_eq!(strip_ansi(&after), " world");
}

#[test]
fn split_at_style_boundary() {
    // Style changes exactly at col 5
    let (before, after) = split_line_at_col("hello\x1b[1m world\x1b[0m", 5);
    assert_eq!(strip_ansi(&before), "hello");
    // No SGR was active during before, so no reset should be appended
    assert!(!before.contains("\x1b["), "before should have no SGR: {before:?}");
    // Bold activates at the boundary, should be in after
    assert!(after.starts_with("\x1b[1m"), "after should start with bold: {after:?}");
    assert_eq!(strip_ansi(&after), " world");
}

#[test]
fn split_preserves_bg() {
    // Blue background
    let (before, after) = split_line_at_col("\x1b[44mhello world\x1b[0m", 3);
    assert_eq!(strip_ansi(&before), "hel");
    assert!(before.ends_with("\x1b[0m"), "before should end with reset: {before:?}");
    assert!(after.contains("44"), "after should re-establish blue bg (44): {after:?}");
    assert_eq!(strip_ansi(&after), "lo world");
}

#[test]
fn split_nested_styles() {
    // bold starts, then cyan added at col 2
    let (before, after) = split_line_at_col("\x1b[1mhe\x1b[36mllo world\x1b[0m", 4);
    assert_eq!(strip_ansi(&before), "hell");
    assert!(before.ends_with("\x1b[0m"), "before should end with reset: {before:?}");
    // At col 4, both bold and cyan are active — check the SGR contains both codes
    assert!(after.contains("1") && after.contains("36"),
        "after should re-establish bold+cyan: {after:?}");
    assert_eq!(strip_ansi(&after), "o world");
}

#[test]
fn split_mid_reset() {
    // bold "hello", then reset, then plain " world"
    let (before, after) = split_line_at_col("\x1b[1mhello\x1b[0m world", 7);
    assert_eq!(strip_ansi(&before), "hello w");
    // At col 7, no SGR is active (reset at col 5)
    // after should have no SGR prefix since nothing is active
    let after_stripped = strip_ansi(&after);
    assert_eq!(after_stripped, "orld");
    // Check that after does NOT start with an SGR sequence
    assert!(!after.starts_with("\x1b["), "after should have no SGR prefix when no style active: {after:?}");
}

// ── split_line_at_col: wide characters ───────────────────────────

#[test]
fn split_wide_char_boundary() {
    // 世 (U+4E16) has display width 2
    let s = "a\u{4e16}b";
    assert_eq!(visible_width(s), 4); // a(1) + 世(2) + b(1)

    // Split at col 1: 'a' in before, '世b' in after
    let (before, after) = split_line_at_col(s, 1);
    assert_eq!(strip_ansi(&before), "a");
    assert_eq!(strip_ansi(&after), "\u{4e16}b");

    // Split at col 2: wide char would cross boundary (starts at col 1, ends at col 3)
    // So before should still be just "a" (wide char stays in after)
    let (before2, after2) = split_line_at_col(s, 2);
    // Actually, 世 starts at col 1 and has width 2, so it occupies cols 1-2.
    // w(1) + gw(2) = 3 > col(2), so it breaks before adding 世.
    // Wait: the loop condition is `w < col` and then `w + gw > col`.
    // At col boundary: w=1, col=2, gw=2. w < col (1 < 2) is true.
    // Then w + gw > col: 1 + 2 > 2 = true, so break. before = "a".
    assert_eq!(strip_ansi(&before2), "a");
    assert_eq!(strip_ansi(&after2), "\u{4e16}b");

    // Split at col 3: 世 fits (w=1, col=3, 1+2=3 <= 3). before = "a世"
    let (before3, after3) = split_line_at_col(s, 3);
    assert_eq!(strip_ansi(&before3), "a\u{4e16}");
    assert_eq!(strip_ansi(&after3), "b");
}

#[test]
fn split_emoji() {
    let s = "hi \u{1f600} bye";
    // 😀 has display width 2
    // h(1) i(1) ' '(1) 😀(2) ' '(1) b(1) y(1) e(1) = 9
    let w = visible_width(s);
    assert!(w >= 8, "unexpected width: {w}");

    // Split after emoji: col = 5 (h=0, i=1, ' '=2, 😀=3-4, next char at col 5)
    let (before, after) = split_line_at_col(s, 5);
    let before_text = strip_ansi(&before);
    let after_text = strip_ansi(&after);
    // Emoji should be intact in before
    assert!(before_text.contains('\u{1f600}'), "emoji should be in before: {before_text:?}");
    assert_eq!(after_text, " bye");
}

// ── split_line_at_col: reconstruction invariant ──────────────────

#[test]
fn split_and_reconstruct() {
    let test_cases = vec![
        "hello world",
        "\x1b[1mhello world\x1b[0m",
        "\x1b[1;36mhello world\x1b[0m",
        "hello\x1b[1m world\x1b[0m",
        "\x1b[44mhello world\x1b[0m",
        "\x1b[1mhe\x1b[36mllo world\x1b[0m",
        "\x1b[1mhello\x1b[0m world",
        "a\u{4e16}b",
        "plain text no style",
    ];

    for original in &test_cases {
        let orig_text = strip_ansi(original);
        let orig_width = visible_width(original);

        for col in 0..=orig_width + 2 {
            let (before, after) = split_line_at_col(original, col);
            let reconstructed = format!("{}{}", strip_ansi(&before), strip_ansi(&after));
            assert_eq!(
                reconstructed, orig_text,
                "reconstruction failed for {:?} at col={}: before={:?} after={:?}",
                original, col, strip_ansi(&before), strip_ansi(&after)
            );
        }
    }
}