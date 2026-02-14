//! Keymap — packed u64 key bindings, const constructors.
//!
//! ```ignore
//! use mage_tui::keymap::*;
//!
//! let km = Keymap::from([
//!     (ctrl(ch('c')), Act::Quit),
//!     (ESC,           Act::Quit),
//!     (alt(LEFT),     Act::WordLeft),
//! ]);
//! ```

use crossterm::event::{KeyCode, KeyEvent};

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct KeyBind(u64);

const MOD_CTRL: u64 = 0x02 << 32;
const MOD_ALT: u64 = 0x04 << 32;
const MOD_SUPER: u64 = 0x08 << 32;

const fn encode(code: KeyCode) -> u64 {
    match code {
        KeyCode::Char(c) => c as u64,
        KeyCode::F(n) => 0x11_0000 + n as u64,
        KeyCode::Backspace => 0x20_0001,
        KeyCode::Enter => 0x20_0002,
        KeyCode::Left => 0x20_0003,
        KeyCode::Right => 0x20_0004,
        KeyCode::Up => 0x20_0005,
        KeyCode::Down => 0x20_0006,
        KeyCode::Home => 0x20_0007,
        KeyCode::End => 0x20_0008,
        KeyCode::PageUp => 0x20_0009,
        KeyCode::PageDown => 0x20_000A,
        KeyCode::Tab => 0x20_000B,
        KeyCode::Delete => 0x20_000C,
        KeyCode::Insert => 0x20_000D,
        KeyCode::Esc => 0x20_000E,
        _ => 0x30_0000,
    }
}

impl KeyBind {
    pub fn of(e: &KeyEvent) -> Self {
        Self(((e.modifiers.bits() as u64) << 32) | encode(e.code))
    }
}

// ── Constructors: ch + 3 modifiers, all composable ──────────────

pub const fn ch(c: char) -> KeyBind {
    KeyBind(encode(KeyCode::Char(c)))
}
pub const fn f(n: u8) -> KeyBind {
    KeyBind(encode(KeyCode::F(n)))
}
pub const fn ctrl(kb: KeyBind) -> KeyBind {
    KeyBind(kb.0 | MOD_CTRL)
}
pub const fn alt(kb: KeyBind) -> KeyBind {
    KeyBind(kb.0 | MOD_ALT)
}
pub const fn sup(kb: KeyBind) -> KeyBind {
    KeyBind(kb.0 | MOD_SUPER)
}

// ── Named keys ──────────────────────────────────────────────────

pub const ENTER: KeyBind = KeyBind(encode(KeyCode::Enter));
pub const ESC: KeyBind = KeyBind(encode(KeyCode::Esc));
pub const BACKSPACE: KeyBind = KeyBind(encode(KeyCode::Backspace));
pub const DELETE: KeyBind = KeyBind(encode(KeyCode::Delete));
pub const TAB: KeyBind = KeyBind(encode(KeyCode::Tab));
pub const LEFT: KeyBind = KeyBind(encode(KeyCode::Left));
pub const RIGHT: KeyBind = KeyBind(encode(KeyCode::Right));
pub const UP: KeyBind = KeyBind(encode(KeyCode::Up));
pub const DOWN: KeyBind = KeyBind(encode(KeyCode::Down));
pub const HOME: KeyBind = KeyBind(encode(KeyCode::Home));
pub const END: KeyBind = KeyBind(encode(KeyCode::End));
pub const PGUP: KeyBind = KeyBind(encode(KeyCode::PageUp));
pub const PGDN: KeyBind = KeyBind(encode(KeyCode::PageDown));

// ── Keymap ──────────────────────────────────────────────────────

pub struct Keymap<A>(Vec<(KeyBind, A)>);

impl<A: Clone> Default for Keymap<A> {
    fn default() -> Self {
        Self::new()
    }
}

impl<A: Clone> Keymap<A> {
    pub fn new() -> Self {
        Self(Vec::new())
    }

    pub fn bind(&mut self, kb: KeyBind, action: A) -> &mut Self {
        self.0.push((kb, action));
        self
    }

    pub fn lookup(&self, event: &KeyEvent) -> Option<A> {
        let k = KeyBind::of(event);
        self.0
            .iter()
            .find(|(kb, _)| *kb == k)
            .map(|(_, a)| a.clone())
    }
}

impl<A: Clone, const N: usize> From<[(KeyBind, A); N]> for Keymap<A> {
    fn from(arr: [(KeyBind, A); N]) -> Self {
        Self(arr.into())
    }
}
