//! Color showcase — all built-in colors, 256-palette, RGB gradients,
//! text attributes, box-drawing borders, and transparency.
//!
//! Rebuilds on resize. Press q or Esc to quit.

use tau_tui_next::style::Color;
use tau_tui_next::ansi::visible_width;
use tau_tui_next::*;

const RST: &str = "\x1b[0m";

// ── Helpers ─────────────────────────────────────────────────────

fn fg(c: &Color) -> String {
    format!("\x1b[{}m", c.fg_code())
}
fn bg(c: &Color) -> String {
    format!("\x1b[{}m", c.bg_code())
}
fn sgr(codes: &str) -> String {
    format!("\x1b[{codes}m")
}

/// Fill a line to `w` columns with bg color, placing content left-aligned.
fn bg_line(content: &str, w: usize, color: &Color) -> String {
    let vw = visible_width(content);
    let fill = w.saturating_sub(vw);
    format!("{}{}{}{}", bg(color), content, " ".repeat(fill), RST)
}

/// Pad/truncate a string to exactly `n` visible columns (space-padded).
fn pad_to(s: &str, n: usize) -> String {
    let vw = visible_width(s);
    if vw >= n {
        tau_tui_next::ansi::truncate_line(s, n)
    } else {
        format!("{}{}", s, " ".repeat(n - vw))
    }
}

// ── Section builders ────────────────────────────────────────────

fn section_header(title: &str, w: usize) -> Vec<String> {
    let bar = "─".repeat(w.saturating_sub(2));
    vec![
        String::new(),
        format!("\x1b[1;33m┌{bar}┐{RST}"),
        format!("\x1b[1;33m│{RST} \x1b[1;37m{} \x1b[1;33m│{RST}", pad_to(title, w.saturating_sub(4))),
        format!("\x1b[1;33m└{bar}┘{RST}"),
    ]
}

/// Section 1: The 8 standard colors + 8 bright colors as fg and bg.
fn standard_colors(w: usize) -> Vec<String> {
    let mut lines = section_header("Standard Colors (foreground + background)", w);

    let names = [
        ("Black", Color::Black),
        ("Red", Color::Red),
        ("Green", Color::Green),
        ("Yellow", Color::Yellow),
        ("Blue", Color::Blue),
        ("Magenta", Color::Magenta),
        ("Cyan", Color::Cyan),
        ("White", Color::White),
    ];

    // Foreground row
    lines.push(String::new());
    lines.push(format!("  \x1b[2mForeground:{RST}"));
    let mut row = String::from("  ");
    for (name, color) in &names {
        row.push_str(&format!("{} {} {RST} ", fg(color), pad_to(name, 9)));
    }
    lines.push(row);

    // Bright foreground row (via Ansi256 8-15)
    lines.push(format!("  \x1b[2mBright foreground:{RST}"));
    let bright_names = [
        "BrBlack", "BrRed", "BrGreen", "BrYellow",
        "BrBlue", "BrMagenta", "BrCyan", "BrWhite",
    ];
    let mut row = String::from("  ");
    for (i, name) in bright_names.iter().enumerate() {
        let c = Color::Ansi256(8 + i as u8);
        row.push_str(&format!("{} {} {RST} ", fg(&c), pad_to(name, 9)));
    }
    lines.push(row);

    // Background blocks
    lines.push(String::new());
    lines.push(format!("  \x1b[2mBackground:{RST}"));
    let mut row = String::from("  ");
    for (name, color) in &names {
        row.push_str(&format!("{}\x1b[30m {} {RST} ", bg(color), pad_to(name, 9)));
    }
    lines.push(row);

    // Bright background
    lines.push(format!("  \x1b[2mBright background:{RST}"));
    let mut row = String::from("  ");
    for (i, name) in bright_names.iter().enumerate() {
        let c = Color::Ansi256(8 + i as u8);
        row.push_str(&format!("{}\x1b[30m {} {RST} ", bg(&c), pad_to(name, 9)));
    }
    lines.push(row);

    lines
}

/// Section 2: Text attributes — bold, dim, italic, underline, strikethrough, combinations.
fn text_attributes(w: usize) -> Vec<String> {
    let mut lines = section_header("Text Attributes", w);
    lines.push(String::new());

    let attrs = [
        ("Normal",           "0"),
        ("Bold",             "1"),
        ("Dim",              "2"),
        ("Italic",           "3"),
        ("Underline",        "4"),
        ("Strikethrough",    "9"),
        ("Bold+Italic",      "1;3"),
        ("Bold+Underline",   "1;4"),
        ("Dim+Italic",       "2;3"),
        ("Bold+Dim+Strike",  "1;2;9"),
    ];

    for (name, code) in &attrs {
        lines.push(format!(
            "  {}{}{RST}  \x1b[2m(SGR {}){RST}",
            sgr(code),
            pad_to(name, 20),
            code
        ));
    }

    // Attributes + colors
    lines.push(String::new());
    lines.push(format!("  \x1b[2mAttributes with colors:{RST}"));
    let combos = [
        ("Bold Red",       "1;31",    ""),
        ("Dim Cyan",       "2;36",    ""),
        ("Italic Green",   "3;32",    ""),
        ("Underline Blue", "4;34",    ""),
        ("Bold on Bg",     "1;37",    "44"),
        ("Italic on Bg",   "3;33",    "45"),
    ];
    for (name, fg_code, bg_code) in &combos {
        let code = if bg_code.is_empty() {
            fg_code.to_string()
        } else {
            format!("{fg_code};{bg_code}")
        };
        lines.push(format!(
            "  {}{}{RST}",
            sgr(&code),
            pad_to(name, 24),
        ));
    }

    lines
}

/// Section 3: 256-color palette in a grid.
fn palette_256(w: usize) -> Vec<String> {
    let mut lines = section_header("256-Color Palette", w);
    lines.push(String::new());

    // System colors 0-15 (2 rows of 8)
    lines.push(format!("  \x1b[2mSystem colors (0–15):{RST}"));
    for row_start in [0u8, 8] {
        let mut row = String::from("  ");
        for i in row_start..row_start + 8 {
            let c = Color::Ansi256(i);
            let label = format!("{:>3}", i);
            // Use white text on dark colors, black on light
            let fg_code = if i < 8 && i != 7 { "37" } else { "30" };
            row.push_str(&format!("{}\x1b[{fg_code}m {label} {RST}", bg(&c)));
        }
        lines.push(row);
    }

    // 216 color cube (16-231) — show as rows of 36
    lines.push(String::new());
    lines.push(format!("  \x1b[2m216-color cube (16–231):{RST}"));
    for row_start in (16u16..232).step_by(36) {
        let mut row = String::from("  ");
        let row_end = (row_start + 36).min(232);
        for i in row_start..row_end {
            let c = Color::Ansi256(i as u8);
            // Use white fg on darker half, black on lighter half
            let offset = (i - 16) % 36;
            let fg_code = if offset < 18 { "37" } else { "30" };
            row.push_str(&format!("{}\x1b[{fg_code}m{:>3}{RST}", bg(&c), i));
        }
        lines.push(row);
    }

    // Grayscale ramp 232-255
    lines.push(String::new());
    lines.push(format!("  \x1b[2mGrayscale (232–255):{RST}"));
    let mut row = String::from("  ");
    for i in 232u8..=255 {
        let c = Color::Ansi256(i);
        let fg_code = if i < 244 { "37" } else { "30" };
        row.push_str(&format!("{}\x1b[{fg_code}m{:>2}{RST}", bg(&c), i));
    }
    lines.push(row);

    lines
}

/// Section 4: RGB gradients.
fn rgb_gradients(w: usize) -> Vec<String> {
    let mut lines = section_header("RGB True-Color Gradients", w);
    lines.push(String::new());

    let bar_w = w.saturating_sub(4).min(72);

    // Red gradient
    lines.push(format!("  \x1b[2mRed:{RST}"));
    let mut row = String::from("  ");
    for i in 0..bar_w {
        let r = (i * 255 / bar_w.max(1)) as u8;
        let c = Color::Rgb(r, 0, 0);
        row.push_str(&format!("{}█{RST}", fg(&c)));
    }
    lines.push(row);

    // Green gradient
    lines.push(format!("  \x1b[2mGreen:{RST}"));
    let mut row = String::from("  ");
    for i in 0..bar_w {
        let g = (i * 255 / bar_w.max(1)) as u8;
        let c = Color::Rgb(0, g, 0);
        row.push_str(&format!("{}█{RST}", fg(&c)));
    }
    lines.push(row);

    // Blue gradient
    lines.push(format!("  \x1b[2mBlue:{RST}"));
    let mut row = String::from("  ");
    for i in 0..bar_w {
        let b = (i * 255 / bar_w.max(1)) as u8;
        let c = Color::Rgb(0, 0, b);
        row.push_str(&format!("{}█{RST}", fg(&c)));
    }
    lines.push(row);

    // Rainbow gradient (hue sweep)
    lines.push(format!("  \x1b[2mRainbow:{RST}"));
    let mut row = String::from("  ");
    for i in 0..bar_w {
        let hue = i as f64 / bar_w as f64 * 360.0;
        let (r, g, b) = hue_to_rgb(hue);
        let c = Color::Rgb(r, g, b);
        row.push_str(&format!("{}█{RST}", fg(&c)));
    }
    lines.push(row);

    // Background rainbow
    lines.push(format!("  \x1b[2mRainbow (bg):{RST}"));
    let mut row = String::from("  ");
    for i in 0..bar_w {
        let hue = i as f64 / bar_w as f64 * 360.0;
        let (r, g, b) = hue_to_rgb(hue);
        let c = Color::Rgb(r, g, b);
        row.push_str(&format!("{} {RST}", bg(&c)));
    }
    lines.push(row);

    lines
}

/// Section 5: Transparency / default background.
fn transparency(w: usize) -> Vec<String> {
    let mut lines = section_header("Default/Transparent Background", w);
    lines.push(String::new());

    lines.push(format!(
        "  \x1b[2mColor::Default uses SGR 49 (bg) / 39 (fg) — inherits terminal theme:{RST}"
    ));
    lines.push(String::new());

    // Show text with explicit bg vs default bg
    let dark_bg = Color::Rgb(30, 30, 50);
    lines.push(bg_line(
        &format!("  {}This has an explicit RGB background{RST}", fg(&Color::White)),
        w, &dark_bg,
    ));
    lines.push(format!(
        "  {}This uses Color::Default — transparent to your theme{RST}",
        fg(&Color::Cyan),
    ));
    lines.push(String::new());

    // Side by side: default bg vs explicit bg
    let half = w.saturating_sub(4) / 2;
    let explicit = Color::Rgb(40, 20, 40);
    let left = format!(
        "{}{} Default bg (transparent) {RST}",
        fg(&Color::Green),
        " ".repeat(half.saturating_sub(25)),
    );
    let right = bg_line(
        &format!("  {}Explicit Rgb(40,20,40){RST}", fg(&Color::White)),
        half,
        &explicit,
    );
    lines.push(format!("  {left}{right}"));

    lines
}

/// Section 6: Box-drawing characters showcase.
fn box_drawing(w: usize) -> Vec<String> {
    let mut lines = section_header("Box-Drawing Characters", w);
    lines.push(String::new());

    let box_w = w.saturating_sub(4).min(50);
    let inner = box_w.saturating_sub(2);

    // ── Light box ──
    lines.push(format!("  \x1b[2mLight:{RST}"));
    lines.push(format!("  \x1b[36m┌{}┐{RST}", "─".repeat(inner)));
    lines.push(format!("  \x1b[36m│{RST}{}\x1b[36m│{RST}", pad_to(" Light box (U+250x)", inner)));
    lines.push(format!("  \x1b[36m│{RST}{}\x1b[36m│{RST}", pad_to(" ┌─┬─┐ nested", inner)));
    lines.push(format!("  \x1b[36m│{RST}{}\x1b[36m│{RST}", pad_to(" ├─┼─┤ grid", inner)));
    lines.push(format!("  \x1b[36m│{RST}{}\x1b[36m│{RST}", pad_to(" └─┴─┘", inner)));
    lines.push(format!("  \x1b[36m└{}┘{RST}", "─".repeat(inner)));

    // ── Heavy box ──
    lines.push(format!("  \x1b[2mHeavy:{RST}"));
    lines.push(format!("  \x1b[33m┏{}┓{RST}", "━".repeat(inner)));
    lines.push(format!("  \x1b[33m┃{RST}{}\x1b[33m┃{RST}", pad_to(" Heavy box (U+254x)", inner)));
    lines.push(format!("  \x1b[33m┃{RST}{}\x1b[33m┃{RST}", pad_to(" ┏━┳━┓ nested", inner)));
    lines.push(format!("  \x1b[33m┃{RST}{}\x1b[33m┃{RST}", pad_to(" ┣━╋━┫ grid", inner)));
    lines.push(format!("  \x1b[33m┃{RST}{}\x1b[33m┃{RST}", pad_to(" ┗━┻━┛", inner)));
    lines.push(format!("  \x1b[33m┗{}┛{RST}", "━".repeat(inner)));

    // ── Double box ──
    lines.push(format!("  \x1b[2mDouble:{RST}"));
    lines.push(format!("  \x1b[35m╔{}╗{RST}", "═".repeat(inner)));
    lines.push(format!("  \x1b[35m║{RST}{}\x1b[35m║{RST}", pad_to(" Double box (U+255x)", inner)));
    lines.push(format!("  \x1b[35m║{RST}{}\x1b[35m║{RST}", pad_to(" ╔═╦═╗ nested", inner)));
    lines.push(format!("  \x1b[35m║{RST}{}\x1b[35m║{RST}", pad_to(" ╠═╬═╣ grid", inner)));
    lines.push(format!("  \x1b[35m║{RST}{}\x1b[35m║{RST}", pad_to(" ╚═╩═╝", inner)));
    lines.push(format!("  \x1b[35m╚{}╝{RST}", "═".repeat(inner)));

    // ── Rounded box ──
    lines.push(format!("  \x1b[2mRounded:{RST}"));
    lines.push(format!("  \x1b[32m╭{}╮{RST}", "─".repeat(inner)));
    lines.push(format!("  \x1b[32m│{RST}{}\x1b[32m│{RST}", pad_to(" Rounded corners (U+256x)", inner)));
    lines.push(format!("  \x1b[32m╰{}╯{RST}", "─".repeat(inner)));

    // ── Styled box with bg fill ──
    lines.push(String::new());
    lines.push(format!("  \x1b[2mWith background fill:{RST}"));
    let fill_bg = Color::Rgb(25, 35, 55);
    let bgs = bg(&fill_bg);
    lines.push(format!("  {bgs}\x1b[36m╭{}╮{RST}", "─".repeat(inner)));
    lines.push(format!("  {bgs}\x1b[36m│\x1b[37m{}\x1b[36m│{RST}",
        pad_to(" Rounded box with RGB bg fill", inner)));
    lines.push(format!("  {bgs}\x1b[36m│\x1b[37m{}\x1b[36m│{RST}",
        pad_to(&format!(" bg: Rgb(25, 35, 55){}",  " ".repeat(inner.saturating_sub(30))), inner)));
    lines.push(format!("  {bgs}\x1b[36m╰{}╯{RST}", "─".repeat(inner)));

    lines
}

/// Section 7: Rounded corner techniques sampler.
///
/// Shows every Unicode approach for making rounded/smooth card corners
/// so you can see which ones your terminal + font actually render well.
fn rounded_sampler(w: usize) -> Vec<String> {
    let mut lines = section_header("Rounded Corner Techniques", w);

    let card_bg = Color::Rgb(44, 44, 56);
    let cbg = bg(&card_bg);
    let cfg = fg(&card_bg);
    let card_w = w.saturating_sub(6).min(50);
    let inner = card_w.saturating_sub(2);
    let body = &[" Hello from this card", " Second line of content"];

    // Helper: render a card with given corner chars
    let card = |tl: &str, tr: &str, bl: &str, br: &str,
                horiz: &str, _vert: &str, label: &str| -> Vec<String> {
        let mut out = Vec::new();
        let cbg_ = &cbg;
        let cfg_ = &cfg;

        out.push(format!("   \x1b[2m{label}{RST}"));

        // Top edge
        let h_top: String = horiz.repeat(inner);
        out.push(format!("   {cfg_}{tl}{h_top}{tr}{RST}"));

        // Body lines
        for line in body {
            let vw = visible_width(line);
            let fill = card_w.saturating_sub(vw);
            out.push(format!("   {cbg_}{line}{}{RST}", " ".repeat(fill)));
        }
        // Padding line
        out.push(format!("   {cbg_}{}{RST}", " ".repeat(card_w)));

        // Bottom edge
        let h_bot: String = horiz.repeat(inner);
        out.push(format!("   {cfg_}{bl}{h_bot}{br}{RST}"));

        out.push(String::new());
        out
    };

    // ── A: Quadrant blocks (▗▖▝▘ + ▄▀) — current approach ──
    lines.extend(card("▗", "▖", "▝", "▘", "▄", " ",
        "A. Quadrant blocks: ▗▄▄▄▖ / ▝▀▀▀▘  (fg=card)"));

    // For the remaining ones, use fg=card on corners, bg=card on body,
    // but top/bottom edge chars vary.

    // ── B: Rounded box-drawing (╭╮╰╯) — no bg on border row ──
    {
        lines.push(format!("   \x1b[2mB. Rounded box-drawing: ╭──╮ / ╰──╯  (fg=card on border){RST}"));
        let h = "─".repeat(inner);
        lines.push(format!("   {cfg}╭{h}╮{RST}"));
        for line in body {
            let vw = visible_width(line);
            let fill = card_w.saturating_sub(vw);
            lines.push(format!("   {cbg}{line}{}{RST}", " ".repeat(fill)));
        }
        lines.push(format!("   {cbg}{}{RST}", " ".repeat(card_w)));
        lines.push(format!("   {cfg}╰{h}╯{RST}"));
        lines.push(String::new());
    }

    // ── C: Rounded box-drawing WITH bg fill everywhere ──
    {
        lines.push(format!("   \x1b[2mC. Rounded box-drawing + bg fill everywhere{RST}"));
        let h = "─".repeat(inner);
        lines.push(format!("   {cbg}{cfg}╭{h}╮{RST}"));
        for line in body {
            let vw = visible_width(line);
            let fill = card_w.saturating_sub(vw);
            lines.push(format!("   {cbg}{line}{}{RST}", " ".repeat(fill)));
        }
        lines.push(format!("   {cbg}{}{RST}", " ".repeat(card_w)));
        lines.push(format!("   {cbg}{cfg}╰{h}╯{RST}"));
        lines.push(String::new());
    }

    // ── D: Filled triangles (◢◣◤◥) ──
    lines.extend(card("◢", "◣", "◥", "◤", "▄", " ",
        "D. Filled triangles: ◢▄▄▄◣ / ◥▀▀▀◤  (fg=card)"));

    // ── E: Quarter arcs (◜◝◞◟) ──
    lines.extend(card("◜", "◝", "◟", "◞", "▄", " ",
        "E. Quarter arcs: ◜▄▄▄◝ / ◟▀▀▀◞  (fg=card)"));

    // ── F: Half circles (◠◡) as top/bottom ──
    {
        lines.push(format!("   \x1b[2mF. Half circles for edges: ◠◠◠ / ◡◡◡  (fg=card){RST}"));
        let mut top = String::from("   ");
        let mut bot = String::from("   ");
        for _ in 0..card_w {
            top.push_str(&format!("{cfg}◠{RST}"));
            bot.push_str(&format!("{cfg}◡{RST}"));
        }
        lines.push(top);
        for line in body {
            let vw = visible_width(line);
            let fill = card_w.saturating_sub(vw);
            lines.push(format!("   {cbg}{line}{}{RST}", " ".repeat(fill)));
        }
        lines.push(format!("   {cbg}{}{RST}", " ".repeat(card_w)));
        lines.push(bot);
        lines.push(String::new());
    }

    // ── G: Eighth blocks for thin edge (▔ top, ▁ bottom) ──
    {
        lines.push(format!("   \x1b[2mG. Eighth blocks: ▁▁▁ top / ▔▔▔ bottom  (fg=card){RST}"));
        let mut top = String::from("   ");
        let mut bot = String::from("   ");
        for _ in 0..card_w {
            top.push_str(&format!("{cfg}▁{RST}"));
            bot.push_str(&format!("{cfg}▔{RST}"));
        }
        lines.push(top);
        for line in body {
            let vw = visible_width(line);
            let fill = card_w.saturating_sub(vw);
            lines.push(format!("   {cbg}{line}{}{RST}", " ".repeat(fill)));
        }
        lines.push(format!("   {cbg}{}{RST}", " ".repeat(card_w)));
        lines.push(bot);
        lines.push(String::new());
    }

    // ── H: Legacy computing smooth corners (U+1FB7C-1FB7F) ──
    {
        lines.push(format!("   \x1b[2mH. Legacy computing (U+1FB7C-7F) — needs Unicode 13+ font{RST}"));
        // 🭼 = U+1FB7C lower left arc, 🭽 = U+1FB7D lower right arc
        // 🭾 = U+1FB7E upper left arc, 🭿 = U+1FB7F upper right arc
        let h = "▄".repeat(inner);
        lines.push(format!("   {cfg}\u{1FB7C}{h}\u{1FB7D}{RST}"));
        for line in body {
            let vw = visible_width(line);
            let fill = card_w.saturating_sub(vw);
            lines.push(format!("   {cbg}{line}{}{RST}", " ".repeat(fill)));
        }
        lines.push(format!("   {cbg}{}{RST}", " ".repeat(card_w)));
        let h = "▀".repeat(inner);
        lines.push(format!("   {cfg}\u{1FB7E}{h}\u{1FB7F}{RST}"));
        lines.push(String::new());
    }

    // ── I: Powerline semicircles (Nerd Font required) ──
    {
        lines.push(format!("   \x1b[2mI. Powerline semicircles (needs Nerd Font){RST}"));
        // U+E0B6 = left semicircle, U+E0B4 = right semicircle
        let left = "\u{E0B6}";
        let right = "\u{E0B4}";
        let fill_line = " ".repeat(inner);
        lines.push(format!("   {cfg}{left}{cbg}{fill_line}{RST}{cfg}{right}{RST}"));
        for line in body {
            let vw = visible_width(line);
            let fill = card_w.saturating_sub(vw);
            lines.push(format!("   {cbg}{line}{}{RST}", " ".repeat(fill)));
        }
        lines.push(format!("   {cbg}{}{RST}", " ".repeat(card_w)));
        lines.push(format!("   {cfg}{left}{cbg}{fill_line}{RST}{cfg}{right}{RST}"));
        lines.push(String::new());
    }

    // ── J: No border, just color contrast + half-block edges ──
    {
        lines.push(format!("   \x1b[2mJ. No border — just half-block soft edges  (fg=card){RST}"));
        let mut top = String::from("   ");
        let mut bot = String::from("   ");
        for _ in 0..card_w {
            top.push_str(&format!("{cfg}▄{RST}"));
            bot.push_str(&format!("{cfg}▀{RST}"));
        }
        lines.push(top);
        for line in body {
            let vw = visible_width(line);
            let fill = card_w.saturating_sub(vw);
            lines.push(format!("   {cbg}{line}{}{RST}", " ".repeat(fill)));
        }
        lines.push(format!("   {cbg}{}{RST}", " ".repeat(card_w)));
        lines.push(bot);
        lines.push(String::new());
    }

    lines
}


/// Section 8: Pill / chip / tag styles for inline elements.
///
/// These are single-line rounded elements for displaying tags, file paths,
/// pasted content, status badges, etc. — like iOS/macOS pill buttons.
fn pills(_w: usize) -> Vec<String> {
    let mut lines = section_header("Pill / Tag Styles", _w);
    lines.push(String::new());

    // Pill color palette
    let pill_colors: &[(&str, Color, Color)] = &[
        ("src/main.rs",    Color::Rgb(50, 55, 80),  Color::Rgb(140, 160, 220)),
        ("feat/login",     Color::Rgb(35, 60, 45),  Color::Rgb(120, 200, 140)),
        ("v2.14.3",        Color::Rgb(65, 50, 35),  Color::Rgb(220, 180, 100)),
        ("3 errors",       Color::Rgb(65, 35, 35),  Color::Rgb(220, 120, 120)),
        ("deployed",       Color::Rgb(30, 55, 55),  Color::Rgb(100, 200, 200)),
    ];

    // ── Style A: Plain bg with space padding ──
    lines.push(format!("  \x1b[2mA. Plain bg + space padding:{RST}"));
    let mut row = String::from("   ");
    for (label, pill_bg, pill_fg) in pill_colors {
        row.push_str(&format!(
            "{}{} {} {RST} ",
            bg(pill_bg), fg(pill_fg), label,
        ));
    }
    lines.push(row);
    lines.push(String::new());

    // ── Style B: Half-block ends (▐ left, ▌ right) ──
    lines.push(format!("  \x1b[2mB. Half-block ends (▐ content ▌):{RST}"));
    let mut row = String::from("   ");
    for (label, pill_bg, pill_fg) in pill_colors {
        // ▐ with fg=pill_bg gives left-half colored
        // ▌ with fg=pill_bg gives right-half colored (after content)
        row.push_str(&format!(
            "{}▐{}{} {} {}▌{RST} ",
            fg(pill_bg), bg(pill_bg), fg(pill_fg), label, fg(pill_bg),
        ));
    }
    lines.push(row);
    lines.push(String::new());

    // ── Style C: Powerline semicircles (Nerd Font) ──
    lines.push(format!("  \x1b[2mC. Powerline semicircles (Nerd Font):{RST}"));
    let mut row = String::from("   ");
    for (label, pill_bg, pill_fg) in pill_colors {
        // U+E0B6 = left semicircle (fg fills right half)
        // U+E0B4 = right semicircle (fg fills left half)
        row.push_str(&format!(
            "{}\u{E0B6}{}{} {} {RST}{}\u{E0B4}{RST} ",
            fg(pill_bg), bg(pill_bg), fg(pill_fg), label, fg(pill_bg),
        ));
    }
    lines.push(row);
    lines.push(String::new());

    // ── Style D: Rounded parens with bg ──
    lines.push(format!("  \x1b[2mD. Rounded parens with bg:{RST}"));
    let mut row = String::from("   ");
    for (label, pill_bg, pill_fg) in pill_colors {
        row.push_str(&format!(
            "{}{}({} {} {}){RST} ",
            bg(pill_bg), fg(pill_fg),
            fg(pill_fg), label, fg(pill_fg),
        ));
    }
    lines.push(row);
    lines.push(String::new());

    // ── Style E: Bold text + dim border brackets ──
    lines.push(format!("  \x1b[2mE. Bold text + dim brackets (no bg):{RST}"));
    let mut row = String::from("   ");
    for (label, _pill_bg, pill_fg) in pill_colors {
        row.push_str(&format!(
            "\x1b[2m[{RST}\x1b[1m{}{}{RST}\x1b[2m]{RST} ",
            fg(pill_fg), label,
        ));
    }
    lines.push(row);
    lines.push(String::new());

    // ── Style F: Icon prefix pills ──
    lines.push(format!("  \x1b[2mF. With icon prefix:{RST}"));
    let icons: &[(&str, &str, Color, Color)] = &[
        ("📎", "paste.txt",    Color::Rgb(50, 55, 80),  Color::Rgb(140, 160, 220)),
        ("🔗", "github.com",   Color::Rgb(35, 60, 45),  Color::Rgb(120, 200, 140)),
        ("⚡", "cached",       Color::Rgb(65, 50, 35),  Color::Rgb(220, 180, 100)),
        ("✗",  "failed",       Color::Rgb(65, 35, 35),  Color::Rgb(220, 120, 120)),
        ("✓",  "passed",       Color::Rgb(30, 55, 55),  Color::Rgb(100, 200, 200)),
    ];
    let mut row = String::from("   ");
    for (icon, label, pill_bg, pill_fg) in icons {
        row.push_str(&format!(
            "{}{} {icon} {}{} {RST} ",
            bg(pill_bg), fg(pill_fg), fg(pill_fg), label,
        ));
    }
    lines.push(row);
    lines.push(String::new());

    // ── Style G: Composable input-style pill row ──
    lines.push(format!("  \x1b[2mG. Input box with pills (mockup):{RST}"));
    let input_bg = Color::Rgb(35, 35, 45);
    let ibg = bg(&input_bg);
    let prompt_fg = fg(&Color::Rgb(100, 100, 130));
    let mut input_row = format!("   {ibg} {prompt_fg}❯{RST}{ibg} ");
    // A few pills inline
    let inline_pills: &[(&str, Color, Color)] = &[
        ("src/lib.rs",  Color::Rgb(55, 55, 85), Color::Rgb(150, 170, 230)),
        ("README.md",   Color::Rgb(45, 60, 50), Color::Rgb(130, 210, 150)),
    ];
    for (label, pbg, pfg) in inline_pills {
        input_row.push_str(&format!(
            "{}{} {} {RST}{ibg} ",
            bg(pbg), fg(pfg), label,
        ));
    }
    // Cursor
    input_row.push_str(&format!("{}▏{RST}{ibg}", fg(&Color::Rgb(150, 150, 180))));
    // Fill to some width
    let vw = visible_width(&input_row);
    let fill_w = _w.saturating_sub(3).min(70);
    if vw < fill_w + 3 {
        input_row.push_str(&" ".repeat(fill_w + 3 - vw));
    }
    input_row.push_str(RST);
    lines.push(input_row);
    lines.push(String::new());

    lines
}

/// Section 9: Fg on various bg combos — readability matrix.
fn fg_bg_matrix(w: usize) -> Vec<String> {
    let mut lines = section_header("Foreground × Background Matrix", w);
    lines.push(String::new());

    let colors = [
        ("Blk", Color::Black),
        ("Red", Color::Red),
        ("Grn", Color::Green),
        ("Yel", Color::Yellow),
        ("Blu", Color::Blue),
        ("Mag", Color::Magenta),
        ("Cyn", Color::Cyan),
        ("Wht", Color::White),
    ];

    // Header row
    let mut hdr = String::from("  bg\\fg ");
    for (name, _) in &colors {
        hdr.push_str(&format!(" {:<5}", name));
    }
    lines.push(format!("\x1b[2m{hdr}{RST}"));

    // Each bg row
    for (bg_name, bg_color) in &colors {
        let mut row = format!("  {:<5} ", bg_name);
        for (_fg_name, fg_color) in &colors {
            row.push_str(&format!(
                "{}{} Txt {RST} ",
                bg(bg_color),
                fg(fg_color),
            ));
        }
        lines.push(row);
    }

    lines
}

// ── HSV helper ──────────────────────────────────────────────────

fn hue_to_rgb(hue: f64) -> (u8, u8, u8) {
    let h = hue / 60.0;
    let c = 1.0_f64;
    let x = c * (1.0 - (h % 2.0 - 1.0).abs());
    let (r, g, b) = match h as u8 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    ((r * 255.0) as u8, (g * 255.0) as u8, (b * 255.0) as u8)
}

// ── App ─────────────────────────────────────────────────────────

struct ColorsApp {
    sections: Vec<Vec<String>>,
    width: u16,
    keymap: Keymap<Act>,
}

#[derive(Clone)]
enum Act {
    Quit,
}

impl ColorsApp {
    fn rebuild(&mut self) {
        let w = self.width as usize;
        self.sections = vec![
            standard_colors(w),
            text_attributes(w),
            palette_256(w),
            rgb_gradients(w),
            transparency(w),
            box_drawing(w),
            rounded_sampler(w),
            pills(w),
            fg_bg_matrix(w),
        ];
    }
}

impl App for ColorsApp {
    type Message = ();

    fn render(&mut self, r: &mut renderer::Renderer) {
        r.push_line(format!(
            "\x1b[1;37m  Color Reference{RST}  \x1b[2m({}×{}){RST}",
            self.width,
            r.height()
        ));

        for section in &self.sections {
            for line in section {
                r.push_line(line.as_str());
            }
        }

        r.push_blank();
        r.push_line(format!("\x1b[2m  Press q or Esc to quit{RST}"));
    }

    fn update(&mut self, event: Event<()>) -> bool {
        match event {
            Event::Key(k) => {
                if let Some(Act::Quit) = self.keymap.lookup(&k) {
                    return true;
                }
            }
            Event::Resize(w, _h) => {
                self.width = w;
                self.rebuild();
            }
            _ => {}
        }
        false
    }
}

#[tokio::main]
async fn main() {
    let width = crossterm::terminal::size().map(|(w, _)| w).unwrap_or(80);

    let mut app = ColorsApp {
        sections: vec![],
        width,
        keymap: Keymap::from([
            (ch('q'), Act::Quit),
            (ESC, Act::Quit),
            (ctrl(ch('c')), Act::Quit),
        ]),
    };
    app.rebuild();

    run(app).await;
}
