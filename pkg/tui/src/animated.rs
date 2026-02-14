//! Animated text widget — per-character color effects driven by a tick counter.
//!
//! ```rust,no_run
//! use tau_tui_next::animated::{AnimatedText, Effect};
//! use tau_tui_next::renderer::LineSink;
//!
//! let mut anim = AnimatedText::new("Hello, world!", Effect::rainbow());
//!
//! // In your tick handler:
//! anim.tick();
//!
//! // In render:
//! // anim.render(&mut r);
//! ```

use std::rc::Rc;

use crate::renderer::{Line, LineSink, View};
use crate::style::Color;

// ── Effect ──────────────────────────────────────────────────────

/// Per-character color effect applied to animated text.
#[derive(Clone)]
pub struct Effect {
    kind: EffectKind,
    /// Characters per full wave cycle.
    pub wavelength: f64,
    /// Phase advance per tick (character positions).
    pub speed: f64,
    /// Whether to render text bold.
    pub bold: bool,
}

#[derive(Clone)]
enum EffectKind {
    /// Gray ↔ white sine wave.
    Shimmer { lo: u8 },
    /// Full hue rotation.
    Rainbow,
    /// Whole text pulses between dim and bright.
    Pulse { lo: u8 },
    /// Characters reveal left-to-right, `chars_per_tick` at a time.
    Typewriter { chars_per_tick: f64 },
}

impl Effect {
    /// Gray-to-white-to-gray shimmer wave. Bold by default.
    pub fn shimmer() -> Self {
        Self {
            kind: EffectKind::Shimmer { lo: 120 },
            wavelength: 40.0,
            speed: 0.6,
            bold: true,
        }
    }

    /// Full rainbow hue sweep.
    pub fn rainbow() -> Self {
        Self {
            kind: EffectKind::Rainbow,
            wavelength: 40.0,
            speed: 0.6,
            bold: false,
        }
    }

    /// Whole text pulses between `lo` gray and white.
    pub fn pulse() -> Self {
        Self {
            kind: EffectKind::Pulse { lo: 100 },
            wavelength: 60.0,
            speed: 1.0,
            bold: true,
        }
    }

    /// Typewriter reveal — characters appear one by one.
    pub fn typewriter() -> Self {
        Self {
            kind: EffectKind::Typewriter { chars_per_tick: 1.0 },
            wavelength: 1.0, // unused
            speed: 1.0,      // unused
            bold: false,
        }
    }

    /// Override wavelength (characters per full cycle).
    pub fn wavelength(mut self, w: f64) -> Self { self.wavelength = w; self }
    /// Override speed (phase advance per tick).
    pub fn speed(mut self, s: f64) -> Self { self.speed = s; self }
    /// Override bold.
    pub fn bold(mut self, b: bool) -> Self { self.bold = b; self }

    /// Set the low-end gray for shimmer/pulse.
    pub fn lo(mut self, v: u8) -> Self {
        match &mut self.kind {
            EffectKind::Shimmer { lo } | EffectKind::Pulse { lo } => *lo = v,
            _ => {}
        }
        self
    }

    /// Set reveal speed for typewriter (characters per tick).
    pub fn chars_per_tick(mut self, n: f64) -> Self {
        if let EffectKind::Typewriter { chars_per_tick } = &mut self.kind {
            *chars_per_tick = n;
        }
        self
    }
}

// ── Color computation ───────────────────────────────────────────

fn hue_to_rgb(hue: f64) -> (u8, u8, u8) {
    let h = (hue % 360.0 + 360.0) % 360.0 / 60.0;
    let x = 1.0 - (h % 2.0 - 1.0).abs();
    let (r, g, b) = match h as u8 {
        0 => (1.0, x, 0.0),
        1 => (x, 1.0, 0.0),
        2 => (0.0, 1.0, x),
        3 => (0.0, x, 1.0),
        4 => (x, 0.0, 1.0),
        _ => (1.0, 0.0, x),
    };
    ((r * 255.0) as u8, (g * 255.0) as u8, (b * 255.0) as u8)
}

/// Compute the fg color for a character at visible column `col` given the effect and phase.
fn char_color(effect: &Effect, col: usize, phase: f64) -> Option<Color> {
    match &effect.kind {
        EffectKind::Shimmer { lo } => {
            let t = ((col as f64 + phase) / effect.wavelength * std::f64::consts::TAU).sin();
            let v = (*lo as f64 + (255.0 - *lo as f64) * (t * 0.5 + 0.5)) as u8;
            Some(Color::Rgb(v, v, v))
        }
        EffectKind::Rainbow => {
            let hue = (col as f64 + phase) / effect.wavelength * 360.0;
            let (r, g, b) = hue_to_rgb(hue);
            Some(Color::Rgb(r, g, b))
        }
        EffectKind::Pulse { lo } => {
            // Phase drives the whole text uniformly — col is ignored.
            let t = (phase / effect.wavelength * std::f64::consts::TAU).sin();
            let v = (*lo as f64 + (255.0 - *lo as f64) * (t * 0.5 + 0.5)) as u8;
            Some(Color::Rgb(v, v, v))
        }
        EffectKind::Typewriter { .. } => None, // handled separately
    }
}

// ── Widget ──────────────────────────────────────────────────────

/// Animated text widget. Call [`tick()`](AnimatedText::tick) each frame,
/// then [`render()`](AnimatedText::render) into any [`LineSink`].
pub struct AnimatedText {
    lines: Vec<String>,
    effect: Effect,
    tick: u64,
    last_tick: u64,
    last_width: u16,
    output: Vec<Line>,
}

impl AnimatedText {
    /// Create with content and effect. Content may contain newlines.
    pub fn new(content: impl Into<String>, effect: Effect) -> Self {
        let text = content.into();
        let lines: Vec<String> = text.lines().map(String::from).collect();
        Self {
            lines,
            effect,
            tick: 0,
            last_tick: u64::MAX, // force first rebuild
            last_width: 0,
            output: Vec::new(),
        }
    }

    /// Advance the animation by one frame.
    pub fn tick(&mut self) {
        self.tick += 1;
    }

    /// Set the tick counter directly (useful for syncing multiple widgets).
    pub fn set_tick(&mut self, tick: u64) {
        self.tick = tick;
    }

    /// Current tick value.
    pub fn current_tick(&self) -> u64 {
        self.tick
    }

    /// Replace the text content.
    pub fn set_content(&mut self, content: impl Into<String>) {
        let text = content.into();
        self.lines = text.lines().map(String::from).collect();
        self.last_tick = u64::MAX; // force rebuild
    }

    /// Replace the effect.
    pub fn set_effect(&mut self, effect: Effect) {
        self.effect = effect;
        self.last_tick = u64::MAX;
    }

    /// Render into a line sink.
    pub fn render(&mut self, sink: &mut impl LineSink) {
        let w = sink.width();
        if self.tick != self.last_tick || w != self.last_width {
            self.rebuild(w);
        }
        sink.push_lines(&self.output);
    }

    /// Get rendered lines directly (for inspection/testing).
    pub fn rendered(&mut self, width: u16) -> &[Line] {
        if self.tick != self.last_tick || width != self.last_width {
            self.rebuild(width);
        }
        &self.output
    }

    fn rebuild(&mut self, _width: u16) {
        self.last_tick = self.tick;
        self.last_width = _width;
        self.output.clear();

        let phase = self.tick as f64 * self.effect.speed;

        for text_line in &self.lines {
            let rendered = match &self.effect.kind {
                EffectKind::Typewriter { chars_per_tick } => {
                    render_typewriter(text_line, self.tick, *chars_per_tick, self.effect.bold)
                }
                _ => render_colored(text_line, &self.effect, phase),
            };
            self.output.push(Rc::from(rendered.as_str()));
        }
    }
}

impl View for AnimatedText {
    fn render(&mut self, sink: &mut impl LineSink) {
        AnimatedText::render(self, sink);
    }
}

// ── Line rendering ──────────────────────────────────────────────

fn render_colored(text: &str, effect: &Effect, phase: f64) -> String {
    use std::fmt::Write;
    let mut buf = String::with_capacity(text.len() * 20);
    if effect.bold {
        buf.push_str("\x1b[1m");
    }
    let mut col = 0usize;
    for ch in text.chars() {
        if ch == ' ' || ch.is_ascii_control() {
            buf.push(ch);
            col += 1;
            continue;
        }
        if let Some(color) = char_color(effect, col, phase) {
            let _ = write!(buf, "\x1b[{}m{ch}", color.fg_code());
        } else {
            buf.push(ch);
        }
        col += 1;
    }
    buf.push_str("\x1b[0m");
    buf
}

fn render_typewriter(text: &str, tick: u64, chars_per_tick: f64, bold: bool) -> String {
    let visible_count = (tick as f64 * chars_per_tick) as usize;
    let mut buf = String::with_capacity(text.len() + 16);
    if bold {
        buf.push_str("\x1b[1m");
    }
    let mut count = 0usize;
    for ch in text.chars() {
        if ch == ' ' || ch.is_ascii_control() {
            if count < visible_count {
                buf.push(ch);
            }
            count += 1;
            continue;
        }
        if count < visible_count {
            buf.push(ch);
        }
        count += 1;
    }
    if bold {
        buf.push_str("\x1b[0m");
    }
    buf
}
