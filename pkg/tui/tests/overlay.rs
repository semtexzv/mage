use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};
use tau_tui_next::overlay::{SelectAction, SelectItem, SelectList};

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent {
        code,
        modifiers: KeyModifiers::NONE,
        kind: KeyEventKind::Press,
        state: KeyEventState::NONE,
    }
}

fn sample_items() -> Vec<SelectItem> {
    vec![
        SelectItem::new("/help", "Show help"),
        SelectItem::new("/clear", "Clear screen"),
        SelectItem::new("/history", "Show history"),
        SelectItem::new("/quit", "Exit"),
        SelectItem::new("/theme", "Change theme"),
    ]
}

#[test]
fn filter_narrows_results() {
    let mut list = SelectList::new(sample_items());
    assert_eq!(list.filtered_count(), 5);
    list.filter("he");
    // "help" and "theme" both contain "he"
    assert!(list.filtered_count() < 5);
    assert!(list.filtered_count() > 0);
}

#[test]
fn empty_filter_shows_all() {
    let mut list = SelectList::new(sample_items());
    list.filter("");
    assert_eq!(list.filtered_count(), sample_items().len());
}

#[test]
fn navigation_up_down() {
    let mut list = SelectList::new(sample_items());
    assert_eq!(list.selected_index(), 0);
    list.move_down();
    list.move_down();
    assert_eq!(list.selected_index(), 2);
    list.move_up();
    assert_eq!(list.selected_index(), 1);
}

#[test]
fn select_returns_value() {
    let mut list = SelectList::new(sample_items());
    list.move_down(); // now on "/clear"
    let val = list.select();
    assert_eq!(val, Some("/clear".to_string()));
}

#[test]
fn enter_selects() {
    let mut list = SelectList::new(sample_items());
    list.move_down(); // "/clear"
    let action = list.handle_key(&key(KeyCode::Enter));
    assert_eq!(action, SelectAction::Selected("/clear".to_string()));
}

#[test]
fn esc_dismisses() {
    let mut list = SelectList::new(sample_items());
    let action = list.handle_key(&key(KeyCode::Esc));
    assert_eq!(action, SelectAction::Dismissed);
}

#[test]
fn tab_completes() {
    let mut list = SelectList::new(sample_items());
    // First item is "/help"
    let action = list.handle_key(&key(KeyCode::Tab));
    assert_eq!(action, SelectAction::Completed("/help".to_string()));
}

#[test]
fn render_produces_lines() {
    let list = SelectList::new(sample_items());
    let lines = list.render(40);
    assert!(!lines.is_empty());
    // Should render at most the number of items (possibly fewer if scroll window)
    assert!(lines.len() <= sample_items().len());
}

#[test]
fn common_prefix() {
    let items = vec![
        SelectItem::new("/help", "Help"),
        SelectItem::new("/history", "History"),
    ];
    let list = SelectList::new(items);
    let prefix = list.common_prefix();
    assert_eq!(prefix, "/h");
}
