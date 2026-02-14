use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};
use tau_tui_next::{alt, ch, ctrl, f, Keymap, DOWN, ESC, LEFT, UP};

fn ev(code: KeyCode, mods: KeyModifiers) -> KeyEvent {
    KeyEvent {
        code,
        modifiers: mods,
        kind: KeyEventKind::Press,
        state: KeyEventState::NONE,
    }
}

#[derive(Clone, Debug, PartialEq)]
enum Act {
    Quit,
    Up,
    Down,
    Search,
}

#[test]
fn lookup_basics() {
    let km = Keymap::from([
        (ctrl(ch('c')), Act::Quit),
        (ESC, Act::Quit),
        (UP, Act::Up),
        (DOWN, Act::Down),
        (ctrl(ch('f')), Act::Search),
    ]);
    assert_eq!(
        km.lookup(&ev(KeyCode::Char('c'), KeyModifiers::CONTROL)),
        Some(Act::Quit)
    );
    assert_eq!(
        km.lookup(&ev(KeyCode::Esc, KeyModifiers::NONE)),
        Some(Act::Quit)
    );
    assert_eq!(
        km.lookup(&ev(KeyCode::Up, KeyModifiers::NONE)),
        Some(Act::Up)
    );
    assert_eq!(km.lookup(&ev(KeyCode::Char('x'), KeyModifiers::NONE)), None);
}

#[test]
fn alt_bindings() {
    let km = Keymap::from([(alt(LEFT), Act::Search)]);
    assert_eq!(
        km.lookup(&ev(KeyCode::Left, KeyModifiers::ALT)),
        Some(Act::Search)
    );
    assert_eq!(km.lookup(&ev(KeyCode::Left, KeyModifiers::NONE)), None);
}

#[test]
fn builder_style() {
    let mut km = Keymap::new();
    km.bind(f(5), Act::Search);
    assert_eq!(
        km.lookup(&ev(KeyCode::F(5), KeyModifiers::NONE)),
        Some(Act::Search)
    );
}
