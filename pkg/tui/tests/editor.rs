use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};
use mage_tui::editor::Editor;
use mage_tui::renderer::Renderer;
use mage_tui::KeyResult;

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent {
        code,
        modifiers: KeyModifiers::NONE,
        kind: KeyEventKind::Press,
        state: KeyEventState::NONE,
    }
}

fn key_mod(code: KeyCode, mods: KeyModifiers) -> KeyEvent {
    KeyEvent {
        code,
        modifiers: mods,
        kind: KeyEventKind::Press,
        state: KeyEventState::NONE,
    }
}

fn type_str(editor: &mut Editor, s: &str) {
    for c in s.chars() {
        let _ = editor.handle_key(key(KeyCode::Char(c)), 80);
    }
}

#[test]
fn typing_inserts_characters() {
    let mut ed = Editor::new();
    type_str(&mut ed, "hello");
    assert_eq!(ed.text(), "hello");
}

#[test]
fn backspace_deletes() {
    let mut ed = Editor::new();
    type_str(&mut ed, "abc");
    let _ = ed.handle_key(key(KeyCode::Backspace), 80);
    assert_eq!(ed.text(), "ab");
}

#[test]
fn enter_submits() {
    let mut ed = Editor::new();
    type_str(&mut ed, "hi");
    let result = ed.handle_key(key(KeyCode::Enter), 80);
    assert_eq!(result, KeyResult::Submit);
}

#[test]
fn alt_enter_inserts_newline() {
    let mut ed = Editor::new();
    type_str(&mut ed, "a");
    let _ = ed.handle_key(key_mod(KeyCode::Enter, KeyModifiers::ALT), 80);
    type_str(&mut ed, "b");
    assert_eq!(ed.text(), "a\nb");
}

#[test]
fn cursor_movement() {
    let mut ed = Editor::new();
    type_str(&mut ed, "abc");
    let _ = ed.handle_key(key(KeyCode::Left), 80);
    let _ = ed.handle_key(key(KeyCode::Left), 80);
    type_str(&mut ed, "x");
    assert_eq!(ed.text(), "axbc");
}

#[test]
fn home_end() {
    let mut ed = Editor::new();
    type_str(&mut ed, "hello");
    let _ = ed.handle_key(key(KeyCode::Home), 80);
    type_str(&mut ed, "x");
    assert_eq!(ed.text(), "xhello");
}

#[test]
fn delete_key() {
    let mut ed = Editor::new();
    type_str(&mut ed, "abc");
    let _ = ed.handle_key(key(KeyCode::Home), 80);
    let _ = ed.handle_key(key(KeyCode::Delete), 80);
    assert_eq!(ed.text(), "bc");
}

#[test]
fn multi_line_up_down() {
    let mut ed = Editor::new();
    type_str(&mut ed, "abc");
    let _ = ed.handle_key(key_mod(KeyCode::Enter, KeyModifiers::ALT), 80);
    type_str(&mut ed, "def");
    assert_eq!(ed.cursor().0, 1); // on line 1
    let _ = ed.handle_key(key(KeyCode::Up), 80);
    assert_eq!(ed.cursor().0, 0); // on line 0
}

#[test]
fn paste_short_inserts_text() {
    let mut ed = Editor::new();
    ed.paste("hello");
    assert_eq!(ed.text(), "hello");
}

#[test]
fn paste_long_creates_pill() {
    let mut ed = Editor::new();
    let long = "x".repeat(100);
    ed.paste(&long);
    assert_eq!(ed.text(), long);
    let (_chars, pills, _newlines) = ed.stats();
    assert_eq!(pills, 1);
}

#[test]
fn clear_resets() {
    let mut ed = Editor::new();
    type_str(&mut ed, "stuff");
    ed.clear();
    assert!(ed.is_empty());
    assert_eq!(ed.text(), "");
}

#[test]
fn render_produces_output() {
    let mut ed = Editor::new();
    type_str(&mut ed, "hello world");
    let mut r = Renderer::new();
    r.begin_frame(80, 24);
    ed.render(&mut r, "> ");
    assert!(r.line_count() > 0);
}

#[test]
fn editor_starts_empty() {
    let ed = Editor::new();
    assert!(ed.is_empty());
    assert_eq!(ed.text(), "");
}

#[test]
fn pill_removed_from_hashmap_on_backspace() {
    let mut ed = Editor::new();
    ed.pill_threshold = 5;
    ed.paste("long enough to become a pill");
    let (_, pills, _) = ed.stats();
    assert_eq!(pills, 1, "should have one pill");
    // Backspace deletes the pill sentinel
    ed.backspace();
    let (_, pills_after, _) = ed.stats();
    assert_eq!(pills_after, 0, "pill should be gone from text");
    // The text() should not contain the pill content anymore
    assert_eq!(ed.text(), "");
}

#[test]
fn pill_removed_from_hashmap_on_delete() {
    let mut ed = Editor::new();
    ed.pill_threshold = 5;
    ed.paste("long enough to become a pill");
    let (_, pills, _) = ed.stats();
    assert_eq!(pills, 1);
    // Move cursor left (before the pill), then delete forward
    ed.move_left();
    ed.delete();
    let (_, pills_after, _) = ed.stats();
    assert_eq!(pills_after, 0, "pill should be gone from text");
    assert_eq!(ed.text(), "");
}