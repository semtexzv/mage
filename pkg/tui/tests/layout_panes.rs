use std::rc::Rc;

use tau_tui_next::ansi::{strip_ansi, visible_width};
use tau_tui_next::layout::{HStack, PaneSize};
use tau_tui_next::style::Padding;

#[test]
fn two_fixed_panes() {
    let mut hs = HStack::new(20);
    let left = hs.pane(PaneSize::Fixed(8));
    let right = hs.pane(PaneSize::Fixed(12));

    hs.get_mut(left).push_line("hello");
    hs.get_mut(right).push_line("world");

    let lines = hs.compose();
    assert_eq!(lines.len(), 1);
    let plain = strip_ansi(&lines[0]);
    assert_eq!(plain, "hello   world       ");
}

#[test]
fn fixed_plus_flex() {
    let mut hs = HStack::new(30);
    let left = hs.pane(PaneSize::Fixed(10));
    let right = hs.pane(PaneSize::Flex);

    hs.get_mut(left).push_line("sidebar");
    hs.get_mut(right).push_line("main");

    let lines = hs.compose();
    let plain = strip_ansi(&lines[0]);
    // left=10, right=20 (30-10)
    assert_eq!(plain.len(), 30);
    assert!(plain.starts_with("sidebar"));
    assert!(plain.contains("main"));
}

#[test]
fn separator() {
    let mut hs = HStack::new(21);
    hs.set_separator(Some("│"));
    let left = hs.pane(PaneSize::Fixed(10));
    let right = hs.pane(PaneSize::Flex);

    hs.get_mut(left).push_line("left");
    hs.get_mut(right).push_line("right");

    let lines = hs.compose();
    let plain = strip_ansi(&lines[0]);
    // 10 + 1 (sep) + 10 (flex) = 21
    assert_eq!(visible_width(&plain), 21);
    assert!(plain.contains("│"));
}

#[test]
fn unequal_row_counts() {
    let mut hs = HStack::new(20);
    let left = hs.pane(PaneSize::Fixed(10));
    let right = hs.pane(PaneSize::Fixed(10));

    hs.get_mut(left).push_line("a");
    hs.get_mut(left).push_line("b");
    hs.get_mut(left).push_line("c");
    hs.get_mut(right).push_line("x");

    let lines = hs.compose();
    assert_eq!(lines.len(), 3);
    // Row 2: left has "c", right is blank (spaces)
    let plain = strip_ansi(&lines[2]);
    assert!(plain.starts_with("c"));
    assert_eq!(visible_width(&plain), 20);
}

#[test]
fn percentage_panes() {
    let mut hs = HStack::new(100);
    let left = hs.pane(PaneSize::Percent(0.3));
    let right = hs.pane(PaneSize::Percent(0.7));

    hs.get_mut(left).push_line("30%");
    hs.get_mut(right).push_line("70%");

    let lines = hs.compose();
    let plain = strip_ansi(&lines[0]);
    assert_eq!(visible_width(&plain), 100);
}

#[test]
fn caching_returns_same_lines() {
    let mut hs = HStack::new(20);
    let left = hs.pane(PaneSize::Fixed(10));
    let _right = hs.pane(PaneSize::Flex);

    hs.get_mut(left).push_line("cached");

    let first = hs.compose().to_vec();
    let second = hs.compose().to_vec();

    // Same content (and should be the cached vec, not recomposed).
    assert_eq!(first.len(), second.len());
    for (a, b) in first.iter().zip(second.iter()) {
        assert!(Rc::ptr_eq(a, b), "cache should return identical Rc pointers");
    }
}

#[test]
fn dirty_after_push_recomposes() {
    let mut hs = HStack::new(20);
    let left = hs.pane(PaneSize::Fixed(10));
    let _right = hs.pane(PaneSize::Flex);

    hs.get_mut(left).push_line("v1");
    let first = hs.compose().to_vec();

    hs.get_mut(left).clear();
    hs.get_mut(left).push_line("v2");
    let second = hs.compose().to_vec();

    let p1 = strip_ansi(&first[0]);
    let p2 = strip_ansi(&second[0]);
    assert!(p1.starts_with("v1"));
    assert!(p2.starts_with("v2"));
}

#[test]
fn truncation_of_long_content() {
    let mut hs = HStack::new(10);
    let p = hs.pane(PaneSize::Fixed(5));
    let _r = hs.pane(PaneSize::Flex);

    hs.get_mut(p).push_line("this is way too long");

    let lines = hs.compose();
    let plain = strip_ansi(&lines[0]);
    assert_eq!(visible_width(&plain), 10);
}

#[test]
fn empty_panes_produce_no_lines() {
    let mut hs = HStack::new(20);
    let _left = hs.pane(PaneSize::Fixed(10));
    let _right = hs.pane(PaneSize::Flex);

    let lines = hs.compose();
    assert_eq!(lines.len(), 0);
}

#[test]
fn three_panes() {
    let mut hs = HStack::new(30);
    let a = hs.pane(PaneSize::Fixed(10));
    let b = hs.pane(PaneSize::Fixed(10));
    let c = hs.pane(PaneSize::Fixed(10));

    hs.get_mut(a).push_line("aaa");
    hs.get_mut(b).push_line("bbb");
    hs.get_mut(c).push_line("ccc");

    let lines = hs.compose();
    let plain = strip_ansi(&lines[0]);
    assert_eq!(visible_width(&plain), 30);
    assert!(plain.contains("aaa"));
    assert!(plain.contains("bbb"));
    assert!(plain.contains("ccc"));
}

#[test]
fn width_change_recomposes() {
    let mut hs = HStack::new(20);
    let left = hs.pane(PaneSize::Flex);
    let _right = hs.pane(PaneSize::Flex);

    hs.get_mut(left).push_line("x");
    let before = hs.compose().to_vec();
    let w1 = visible_width(&strip_ansi(&before[0]));
    assert_eq!(w1, 20);

    hs.set_width(40);
    let after = hs.compose().to_vec();
    let w2 = visible_width(&strip_ansi(&after[0]));
    assert_eq!(w2, 40);
}

#[test]
fn pane_width_available_immediately() {
    let mut hs = HStack::new(50);
    let left = hs.pane(PaneSize::Fixed(20));
    let right = hs.pane(PaneSize::Flex);

    // Widths should be resolved immediately after pane() calls.
    assert_eq!(hs.get(left).available_width(), 20);
    assert_eq!(hs.get(right).available_width(), 30);
}

#[test]
fn pane_width_updates_on_set_width() {
    let mut hs = HStack::new(40);
    let left = hs.pane(PaneSize::Fixed(10));
    let right = hs.pane(PaneSize::Flex);

    assert_eq!(hs.get(right).available_width(), 30);

    hs.set_width(60);
    assert_eq!(hs.get(left).available_width(), 10); // fixed stays 10
    assert_eq!(hs.get(right).available_width(), 50); // flex takes remaining
}

#[test]
fn padding_reduces_content_width() {
    let mut hs = HStack::new(40);
    let p = hs.pane_with_padding(PaneSize::Fixed(20), Padding::horizontal(3));
    let _r = hs.pane(PaneSize::Flex);

    // allocated=20, padding left+right=6, content width=14
    assert_eq!(hs.get(p).allocated(), 20);
    assert_eq!(hs.get(p).available_width(), 14);
}

#[test]
fn padding_applied_in_compose() {
    let mut hs = HStack::new(20);
    let p = hs.pane_with_padding(PaneSize::Fixed(20), Padding::new(0, 2, 0, 3));

    hs.get_mut(p).push_line("hi");

    let lines = hs.compose();
    let plain = strip_ansi(&lines[0]);
    // 3 left + "hi" + pad-to-15 content + 2 right = 20 total
    assert_eq!(visible_width(&plain), 20);
    assert!(plain.starts_with("   hi"), "expected 3-space left pad, got: [{}]", plain);
}

#[test]
fn set_padding_updates_width() {
    let mut hs = HStack::new(40);
    let p = hs.pane(PaneSize::Fixed(20));
    let _r = hs.pane(PaneSize::Flex);

    assert_eq!(hs.get(p).available_width(), 20);

    hs.set_padding(p, Padding::horizontal(2));
    assert_eq!(hs.get(p).available_width(), 16);
    assert_eq!(hs.get(p).allocated(), 20);
}

#[test]
fn top_bottom_padding_inserts_blank_rows() {
    let mut hs = HStack::new(20);
    let p = hs.pane_with_padding(PaneSize::Fixed(20), Padding::new(1, 0, 1, 0));

    hs.get_mut(p).push_line("content");

    let lines = hs.compose();
    // 1 top blank + 1 content + 1 bottom blank = 3
    assert_eq!(lines.len(), 3);
    let top = strip_ansi(&lines[0]);
    let mid = strip_ansi(&lines[1]);
    let bot = strip_ansi(&lines[2]);
    assert_eq!(top.trim(), "", "top row should be blank");
    assert!(mid.contains("content"));
    assert_eq!(bot.trim(), "", "bottom row should be blank");
}

#[test]
fn padding_with_separator() {
    let mut hs = HStack::new(31);
    hs.set_separator(Some("│"));
    let left = hs.pane_with_padding(PaneSize::Fixed(15), Padding::horizontal(1));
    let right = hs.pane_with_padding(PaneSize::Flex, Padding::horizontal(1));

    // left: alloc=15, content=13. right: alloc=15, content=13. sep=1. total=31.
    assert_eq!(hs.get(left).available_width(), 13);
    assert_eq!(hs.get(right).available_width(), 13);

    hs.get_mut(left).push_line("L");
    hs.get_mut(right).push_line("R");

    let lines = hs.compose();
    let plain = strip_ansi(&lines[0]);
    assert_eq!(visible_width(&plain), 31);
    // Left pad: " L" at start
    assert!(plain.starts_with(" L"), "got: [{}]", plain);
    assert!(plain.contains("│"));
}