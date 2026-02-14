/// Terminal colors for styling text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Color {
    Black,
    Red,
    Green,
    Yellow,
    Blue,
    Magenta,
    Cyan,
    White,
    Default,
    Ansi256(u8),
    Rgb(u8, u8, u8),
}

impl Color {
    /// Returns the SGR parameter for foreground color (e.g. `"31"`, `"38;5;42"`).
    pub fn fg_code(&self) -> String {
        match self {
            Color::Black => "30".into(),
            Color::Red => "31".into(),
            Color::Green => "32".into(),
            Color::Yellow => "33".into(),
            Color::Blue => "34".into(),
            Color::Magenta => "35".into(),
            Color::Cyan => "36".into(),
            Color::White => "37".into(),
            Color::Default => "39".into(),
            Color::Ansi256(n) => format!("38;5;{n}"),
            Color::Rgb(r, g, b) => format!("38;2;{r};{g};{b}"),
        }
    }

    /// Returns the SGR parameter for background color (e.g. `"41"`, `"48;5;42"`).
    pub fn bg_code(&self) -> String {
        match self {
            Color::Black => "40".into(),
            Color::Red => "41".into(),
            Color::Green => "42".into(),
            Color::Yellow => "43".into(),
            Color::Blue => "44".into(),
            Color::Magenta => "45".into(),
            Color::Cyan => "46".into(),
            Color::White => "47".into(),
            Color::Default => "49".into(),
            Color::Ansi256(n) => format!("48;5;{n}"),
            Color::Rgb(r, g, b) => format!("48;2;{r};{g};{b}"),
        }
    }
}

impl Color {
    /// Parse a `#RRGGBB` hex string into `Color::Rgb`.
    ///
    /// Returns `None` if the string is not exactly 7 chars or has invalid hex.
    pub fn from_hex(s: &str) -> Option<Self> {
        let s = s.strip_prefix('#')?;
        if s.len() != 6 {
            return None;
        }
        let r = u8::from_str_radix(&s[0..2], 16).ok()?;
        let g = u8::from_str_radix(&s[2..4], 16).ok()?;
        let b = u8::from_str_radix(&s[4..6], 16).ok()?;
        Some(Color::Rgb(r, g, b))
    }
}

// ── Style ──────────────────────────────────────────────────

/// Complete snapshot of terminal text attributes + colors.
///
/// 24 bytes, `Copy`. Represents the full ANSI SGR state at a point in the
/// output stream. Used by [`StyleStack`] for push/pop and by `wrap_text`
/// for continuation-line prefixes.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Style {
    pub bold: bool,
    pub dim: bool,
    pub italic: bool,
    pub underline: bool,
    pub strikethrough: bool,
    pub fg: Option<Color>,
    pub bg: Option<Color>,
}

impl Style {
    /// All attributes off, default colors.
    pub const NONE: Self = Self {
        bold: false,
        dim: false,
        italic: false,
        underline: false,
        strikethrough: false,
        fg: None,
        bg: None,
    };

    /// Create an empty style (all defaults). Alias for `NONE`.
    pub fn new() -> Self {
        Self::NONE
    }

    /// Enable bold.
    pub fn bold(mut self) -> Self { self.bold = true; self }
    /// Enable dim.
    pub fn dim(mut self) -> Self { self.dim = true; self }
    /// Enable italic.
    pub fn italic(mut self) -> Self { self.italic = true; self }
    /// Enable underline.
    pub fn underline(mut self) -> Self { self.underline = true; self }
    /// Enable strikethrough.
    pub fn strikethrough(mut self) -> Self { self.strikethrough = true; self }
    /// Set foreground color.
    pub fn fg(mut self, color: Color) -> Self { self.fg = Some(color); self }
    /// Set background color.
    pub fn bg(mut self, color: Color) -> Self { self.bg = Some(color); self }

    /// True when all attributes are at their default values.
    pub fn is_empty(&self) -> bool {
        *self == Self::NONE
    }

    /// Emit the SGR escape sequence that fully establishes this state
    /// from a reset terminal. Returns `""` if all attributes are default.
    ///
    /// Example: `"\x1b[1;3;36;48;2;20;20;35m"` for bold+italic+cyan fg+rgb bg.
    pub fn to_sgr(&self) -> String {
        let mut params = String::new();
        let mut sep = false;
        let mut p = |s: &str| {
            if sep { params.push(';'); }
            params.push_str(s);
            sep = true;
        };
        if self.bold { p("1"); }
        if self.dim { p("2"); }
        if self.italic { p("3"); }
        if self.underline { p("4"); }
        if self.strikethrough { p("9"); }
        if let Some(c) = &self.fg { p(&c.fg_code()); }
        if let Some(c) = &self.bg { p(&c.bg_code()); }
        if params.is_empty() {
            String::new()
        } else {
            format!("\x1b[{params}m")
        }
    }

    /// Emit the minimal SGR sequence to transition from `from` to `self`.
    ///
    /// Only the attributes that differ are emitted. If nothing changed,
    /// returns `""`.
    pub fn transition_from(&self, from: &Style) -> String {
        if *self == *from {
            return String::new();
        }

        let mut codes: Vec<&str> = Vec::new();
        let mut owned: Vec<String> = Vec::new();

        // Bold and dim share SGR 22 (turns off both). Handle them together.
        let bold_changed = self.bold != from.bold;
        let dim_changed = self.dim != from.dim;

        if bold_changed || dim_changed {
            // If either is being turned off, we might need SGR 22 which
            // kills both. Then re-enable whichever should stay on.
            let need_22 = (!self.bold && from.bold) || (!self.dim && from.dim);
            if need_22 {
                codes.push("22");
                // Re-enable bold/dim if they should survive the 22.
                if self.bold { codes.push("1"); }
                if self.dim { codes.push("2"); }
            } else {
                // Only turning things ON, no conflict.
                if self.bold && !from.bold { codes.push("1"); }
                if self.dim && !from.dim { codes.push("2"); }
            }
        }

        if self.italic != from.italic {
            codes.push(if self.italic { "3" } else { "23" });
        }
        if self.underline != from.underline {
            codes.push(if self.underline { "4" } else { "24" });
        }
        if self.strikethrough != from.strikethrough {
            codes.push(if self.strikethrough { "9" } else { "29" });
        }
        if self.fg != from.fg {
            match &self.fg {
                Some(c) => owned.push(c.fg_code()),
                None => codes.push("39"),
            }
        }
        if self.bg != from.bg {
            match &self.bg {
                Some(c) => owned.push(c.bg_code()),
                None => codes.push("49"),
            }
        }

        if codes.is_empty() && owned.is_empty() {
            return String::new();
        }

        let all: Vec<&str> = codes
            .into_iter()
            .chain(owned.iter().map(|s| s.as_str()))
            .collect();
        format!("\x1b[{}m", all.join(";"))
    }

    /// Overlay: for each field set in `overlay`, replace the corresponding
    /// field in `self`. Fields that are `false`/`None` in `overlay` are
    /// NOT applied (they're treated as "don't change").
    ///
    pub fn with_overlay(&self, overlay: &Style) -> Style {
        Style {
            bold: overlay.bold || self.bold,
            dim: overlay.dim || self.dim,
            italic: overlay.italic || self.italic,
            underline: overlay.underline || self.underline,
            strikethrough: overlay.strikethrough || self.strikethrough,
            fg: overlay.fg.or(self.fg),
            bg: overlay.bg.or(self.bg),
        }
    }
}

// ── StyleStack ──────────────────────────────────────────────────
///
/// Each `push()` records a **style overlay** (the delta — which attributes to
/// turn on or colors to set). The stack computes the **combined state** by
/// applying all overlays on top of the base, from bottom to top.
///
/// Internally, each frame stores both the original overlay delta and the
/// resulting combined state. This lets [`set_base`](StyleStack::set_base)
/// replace the base and recompute all combined states correctly.
///
/// Cached SGR strings:
/// - **`sgr()`** — full SGR to re-establish the current combined state
///   (for continuation lines after wrapping).
/// - **`last_transition()`** — the minimal diff SGR from the last
///   push or pop (append directly to output buffer).
///
/// ```text
/// let mut ss = StyleStack::new();
/// ss.set_base(Style { bg: Some(Color::Blue), ..Default::default() });
///
/// buf.push_str(ss.sgr());                   // emit bg
/// buf.push_str(ss.push(Style { bold: true, ..Default::default() }));
/// buf.push_str("Header");
/// buf.push_str(ss.pop());                   // "\x1b[22m" — restore
/// ```
///
/// # Frame layout
///
/// ```text
/// frames[0] = base        (delta: NONE, combined: base)
/// frames[1] = push(bold)  (delta: bold, combined: base + bold)
/// frames[2] = push(cyan)  (delta: cyan, combined: base + bold + cyan)
/// ```
///
/// `set_base(new_base)` replaces `frames[0]` and recomputes all combined
/// states by replaying each frame's delta on top of the new base.
pub struct StyleStack {
    /// Each frame is `(delta, combined)`. Frame 0 is the base:
    /// its delta is `Style::NONE` (unused) and its combined IS the base.
    frames: Vec<(Style, Style)>,
    /// Pre-built full SGR for the current combined state.
    current_sgr: String,
    /// SGR emitted by the last push()/pop().
    last_transition: String,
}
impl Default for StyleStack {
    fn default() -> Self {
        Self::new()
    }
}
impl StyleStack {
    /// Create an empty stack (all defaults).
    pub fn new() -> Self {
        Self {
            frames: vec![(Style::NONE, Style::NONE)],
            current_sgr: String::new(),
            last_transition: String::new(),
        }
    }
    /// Create with an initial base state (e.g. container bg color).
    pub fn with_base(base: Style) -> Self {
        let sgr = base.to_sgr();
        Self {
            frames: vec![(Style::NONE, base)],
            current_sgr: sgr.clone(),
            last_transition: sgr,
        }
    }
    /// Set the base (outermost) state, replacing frame 0.
    ///
    /// All pushed overlays are replayed on top of the new base so that
    /// the combined state at each level is correct.
    pub fn set_base(&mut self, base: Style) {
        self.frames[0] = (Style::NONE, base);
        self.recompute();
    }
    /// Push a style overlay. Returns the transition SGR to emit.
    ///
    /// The overlay's `true`/`Some` fields are added on top of the current
    /// state. `false`/`None` fields mean "keep current".
    pub fn push(&mut self, overlay: Style) -> &str {
        let prev = self.current();
        let next = prev.with_overlay(&overlay);
        self.last_transition = next.transition_from(&prev);
        self.frames.push((overlay, next));
        self.current_sgr = next.to_sgr();
        &self.last_transition
    }
    /// Pop the top style layer. Returns the transition SGR to restore
    /// the parent state.
    ///
    /// Panics if only the base frame remains (nothing to pop).
    pub fn pop(&mut self) -> &str {
        assert!(self.frames.len() > 1, "StyleStack: cannot pop the base frame");
        let (_, old_combined) = self.frames.pop().unwrap();
        let parent = self.current();
        self.last_transition = parent.transition_from(&old_combined);
        self.current_sgr = parent.to_sgr();
        &self.last_transition
    }
    /// Full SGR for the current combined state — for line prefixes,
    /// continuation lines, or re-establishing state after unknown content.
    pub fn sgr(&self) -> &str {
        &self.current_sgr
    }
    /// The transition SGR from the last push/pop.
    pub fn last_transition(&self) -> &str {
        &self.last_transition
    }
    /// The current combined state (top of the stack).
    pub fn current(&self) -> Style {
        self.frames.last().unwrap().1
    }
    /// Number of frames (including base). Base = 1, one push = 2, etc.
    pub fn depth(&self) -> usize {
        self.frames.len()
    }

    /// Replay all overlay deltas on top of the current base.
    fn recompute(&mut self) {
        let mut combined = self.frames[0].1; // base combined state
        for frame in &mut self.frames[1..] {
            combined = combined.with_overlay(&frame.0); // apply original delta
            frame.1 = combined; // update combined
        }
        self.current_sgr = combined.to_sgr();
    }
}

/// Semantic color slots for theming. Components read these instead of
/// hardcoding ANSI colors. All fields have sensible defaults.
///
/// Colors can be loaded from JSON files (see [`Theme::from_json`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Theme {
    // ── Generic UI ──────────────────────────────────────────
    /// Headings, active items, main accent.
    pub accent: Color,
    /// Borders, metadata, secondary text.
    pub border: Color,
    /// Accented/highlighted borders.
    pub border_accent: Color,
    /// Subtle/muted borders.
    pub border_muted: Color,
    /// Hints, disabled text, dim content.
    pub muted: Color,
    /// Very dimmed text (more subtle than muted).
    pub dim: Color,
    /// Default text color (None = terminal default).
    pub text: Option<Color>,
    /// Content area background (None = terminal default).
    pub surface_bg: Option<Color>,
    /// Alternate surface (code blocks, alternating rows).
    pub surface_alt_bg: Option<Color>,
    /// Success indicators.
    pub success: Color,
    /// Warning indicators.
    pub warning: Color,
    /// Error indicators.
    pub error: Color,
    /// Heading text color.
    pub md_heading: Color,
    /// Link text color.
    pub md_link: Color,
    /// Link URL color (shown after link text).
    pub md_link_url: Color,
    /// Inline code color.
    pub md_code: Color,
    /// Code block content color.
    pub md_code_block: Color,
    /// Code block fence (```) border color.
    pub md_code_block_border: Color,
    /// Blockquote text color.
    pub md_quote: Color,
    /// Blockquote border color.
    pub md_quote_border: Color,
    /// Horizontal rule color.
    pub md_hr: Color,
    /// List bullet/number color.
    pub md_list_bullet: Color,
    // ── Overlay / SelectList ────────────────────────────────
    /// Highlighted row background.
    pub overlay_highlight_bg: Color,
    /// Highlighted row foreground.
    pub overlay_highlight_fg: Color,
    /// Normal label foreground.
    pub overlay_label_fg: Color,
    /// Description foreground.
    pub overlay_desc_fg: Color,
    /// Pill background.
    pub pill_bg: Color,
    /// Pill foreground.
    pub pill_fg: Color,
}
impl Default for Theme {
    fn default() -> Self {
        Self {
            accent: Color::Cyan,
            border: Color::Ansi256(244),
            border_accent: Color::Cyan,
            border_muted: Color::Ansi256(240),
            muted: Color::Ansi256(245),
            dim: Color::Ansi256(240),
            text: None,
            surface_bg: None,
            surface_alt_bg: Some(Color::Ansi256(236)),
            success: Color::Green,
            warning: Color::Yellow,
            error: Color::Red,
            md_heading: Color::Yellow,
            md_link: Color::Blue,
            md_link_url: Color::Ansi256(245),
            md_code: Color::Cyan,
            md_code_block: Color::Green,
            md_code_block_border: Color::Ansi256(245),
            md_quote: Color::Ansi256(245),
            md_quote_border: Color::Green,
            md_hr: Color::Ansi256(245),
            md_list_bullet: Color::Cyan,

            overlay_highlight_bg: Color::Rgb(40, 50, 70),
            overlay_highlight_fg: Color::Rgb(200, 220, 255),
            overlay_label_fg: Color::Rgb(180, 190, 210),
            overlay_desc_fg: Color::Rgb(100, 110, 130),
            pill_bg: Color::Rgb(50, 55, 80),
            pill_fg: Color::Rgb(140, 160, 220),
        }
    }
}

impl Theme {
    /// Wrap `text` in the foreground color for `slot`, resetting fg after.
    pub fn foreground(&self, slot: ThemeColor, text: &str) -> String {
        let color = self.resolve(slot);
        format!("\x1b[{}m{}\x1b[39m", color.fg_code(), text)
    }

    /// Wrap `text` in the background color for `slot`, resetting bg after.
    pub fn background(&self, slot: ThemeColor, text: &str) -> String {
        let color = self.resolve(slot);
        format!("\x1b[{}m{}\x1b[49m", color.bg_code(), text)
    }

    /// Resolve a slot to its `Color` value.
    pub fn resolve(&self, slot: ThemeColor) -> Color {
        match slot {
            ThemeColor::Accent => self.accent,
            ThemeColor::Border => self.border,
            ThemeColor::BorderAccent => self.border_accent,
            ThemeColor::BorderMuted => self.border_muted,
            ThemeColor::Muted => self.muted,
            ThemeColor::Dim => self.dim,
            ThemeColor::Success => self.success,
            ThemeColor::Warning => self.warning,
            ThemeColor::Error => self.error,
            ThemeColor::MdHeading => self.md_heading,
            ThemeColor::MdLink => self.md_link,
            ThemeColor::MdLinkUrl => self.md_link_url,
            ThemeColor::MdCode => self.md_code,
            ThemeColor::MdCodeBlock => self.md_code_block,
            ThemeColor::MdCodeBlockBorder => self.md_code_block_border,
            ThemeColor::MdQuote => self.md_quote,
            ThemeColor::MdQuoteBorder => self.md_quote_border,
            ThemeColor::MdHr => self.md_hr,
            ThemeColor::MdListBullet => self.md_list_bullet,
            ThemeColor::OverlayHighlightBg => self.overlay_highlight_bg,
            ThemeColor::OverlayHighlightFg => self.overlay_highlight_fg,
            ThemeColor::OverlayLabelFg => self.overlay_label_fg,
            ThemeColor::OverlayDescFg => self.overlay_desc_fg,
            ThemeColor::PillBg => self.pill_bg,
            ThemeColor::PillFg => self.pill_fg,
        }
    }

    /// Load a theme from a JSON object.
    ///
    /// The JSON shape follows pi-mono's theme format:
    /// ```json
    /// {
    ///   "vars": { "cyan": "#00d7ff", "gray": 244 },
    ///   "colors": {
    ///     "accent": "cyan",
    ///     "border": 244,
    ///     "muted": "#808080",
    ///     ...
    ///   }
    /// }
    /// ```
    ///
    /// Color values are:
    /// - `"#RRGGBB"` hex string → `Color::Rgb`
    /// - integer 0-255 → `Color::Ansi256`
    /// - string matching a `vars` key → resolved recursively
    /// - `""` empty string → `None` (terminal default)
    ///
    /// Unknown keys are ignored. Missing keys keep their defaults.
    #[cfg(feature = "json-theme")]
    pub fn from_json(json: &serde_json::Value) -> Self {
        use serde_json::Value;
        let mut theme = Self::default();
        let vars = json.get("vars")
            .and_then(|v| v.as_object())
            .cloned()
            .unwrap_or_default();
        let colors = match json.get("colors").and_then(|v| v.as_object()) {
            Some(c) => c,
            None => return theme,
        };

        // Resolve a value that may be a hex string, integer, or variable
        // reference. Variable references are resolved recursively (with
        // cycle detection via a depth limit of 10).
        let resolve_value = |val: &Value| -> Option<Color> {
            fn resolve_inner(
                val: &Value,
                vars: &serde_json::Map<String, Value>,
                depth: usize,
            ) -> Option<Color> {
                if depth > 10 {
                    return None; // circular reference guard
                }
                match val {
                    Value::String(s) if s.is_empty() => None,
                    Value::String(s) if s.starts_with('#') => Color::from_hex(s),
                    Value::String(s) => {
                        // Variable reference — resolve recursively
                        vars.get(s.as_str())
                            .and_then(|v| resolve_inner(v, vars, depth + 1))
                    }
                    Value::Number(n) => n.as_u64().map(|v| Color::Ansi256(v as u8)),
                    _ => None,
                }
            }
            resolve_inner(val, &vars, 0)
        };
        macro_rules! set {
            ($field:ident, $key:expr) => {
                if let Some(val) = colors.get($key) {
                    if let Some(c) = resolve_value(val) {
                        theme.$field = c;
                    }
                }
            };
            (opt $field:ident, $key:expr) => {
                if let Some(val) = colors.get($key) {
                    theme.$field = resolve_value(val);
                }
            };
        }

        // ── Core UI ──
        set!(accent, "accent");
        set!(border, "border");
        set!(border_accent, "borderAccent");
        set!(border_muted, "borderMuted");
        set!(muted, "muted");
        set!(dim, "dim");
        set!(opt text, "text");
        set!(opt surface_bg, "surfaceBg");
        set!(opt surface_alt_bg, "surfaceAltBg");
        set!(success, "success");
        set!(warning, "warning");
        set!(error, "error");
        // ── Markdown ──
        set!(md_heading, "mdHeading");
        set!(md_link, "mdLink");
        set!(md_link_url, "mdLinkUrl");
        set!(md_code, "mdCode");
        set!(md_code_block, "mdCodeBlock");
        set!(md_code_block_border, "mdCodeBlockBorder");
        set!(md_quote, "mdQuote");
        set!(md_quote_border, "mdQuoteBorder");
        set!(md_hr, "mdHr");
        set!(md_list_bullet, "mdListBullet");

        // ── Overlay ──
        set!(overlay_highlight_bg, "overlayHighlightBg");
        set!(overlay_highlight_fg, "overlayHighlightFg");
        set!(overlay_label_fg, "overlayLabelFg");
        set!(overlay_desc_fg, "overlayDescFg");
        // Pi-mono uses "selectedBg" for the overlay highlight background
        set!(overlay_highlight_bg, "selectedBg");
        // ── Editor pills ──
        set!(pill_bg, "pillBg");
        set!(pill_fg, "pillFg");
        theme
    }
}

/// Named color slots for [`Theme::foreground`] and [`Theme::background`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ThemeColor {
    Accent,
    Border,
    BorderAccent,
    BorderMuted,
    Muted,
    Dim,
    Success,
    Warning,
    Error,
    MdHeading,
    MdLink,
    MdLinkUrl,
    MdCode,
    MdCodeBlock,
    MdCodeBlockBorder,
    MdQuote,
    MdQuoteBorder,
    MdHr,
    MdListBullet,
    OverlayHighlightBg,
    OverlayHighlightFg,
    OverlayLabelFg,
    OverlayDescFg,
    PillBg,
    PillFg,
}

// ── Layout ──────────────────────────────────────────────────────

/// Padding in terminal cells. 8 bytes, `Copy`.
#[derive(Clone, Copy, Default, PartialEq, Eq, Hash, Debug)]
pub struct Padding {
    pub top: u16,
    pub bottom: u16,
    pub left: u16,
    pub right: u16,
}

impl Padding {
    pub const ZERO: Padding = Padding { top: 0, right: 0, bottom: 0, left: 0 };

    pub const fn new(top: u16, right: u16, bottom: u16, left: u16) -> Self {
        Self { top, right, bottom, left }
    }

    /// Uniform padding on all sides.
    pub const fn all(n: u16) -> Self {
        Self { top: n, right: n, bottom: n, left: n }
    }

    /// Horizontal (left + right) only.
    pub const fn horizontal(n: u16) -> Self {
        Self { top: 0, right: n, bottom: 0, left: n }
    }

    /// Vertical (top + bottom) only.
    pub const fn vertical(n: u16) -> Self {
        Self { top: n, right: 0, bottom: n, left: 0 }
    }

    /// Left only.
    pub const fn left(n: u16) -> Self {
        Self { top: 0, right: 0, bottom: 0, left: n }
    }

    /// Total horizontal cells consumed.
    pub const fn h(&self) -> u16 {
        self.left.saturating_add(self.right)
    }

    pub const fn is_zero(&self) -> bool {
        self.top == 0 && self.right == 0 && self.bottom == 0 && self.left == 0
    }
}