use mage_tui::style::{Color, Style, StyleStack, Theme, ThemeColor};
use mage_tui::ansi::{apply_sgr, RESET};

// ── Style builder / basic tests ──────────────────────────────────

#[test]
fn default_style_is_all_off() {
    let s = Style::default();
    assert!(!s.bold);
    assert!(!s.dim);
    assert!(!s.italic);
    assert!(!s.underline);
    assert!(!s.strikethrough);
    assert_eq!(s.fg, None);
    assert_eq!(s.bg, None);
}

#[test]
fn builder_methods() {
    let s = Style::new().bold().fg(Color::Red);
    assert!(s.bold);
    assert_eq!(s.fg, Some(Color::Red));
}

#[test]
fn to_sgr_bold() {
    let s = Style::new().bold();
    assert_eq!(s.to_sgr(), "\x1b[1m");
}

#[test]
fn to_sgr_fg_color() {
    let s = Style::new().fg(Color::Green);
    assert_eq!(s.to_sgr(), "\x1b[32m");
}

#[test]
fn to_sgr_bg_color() {
    let s = Style::new().bg(Color::Blue);
    assert_eq!(s.to_sgr(), "\x1b[44m");
}

#[test]
fn to_sgr_combined() {
    let s = Style::new().bold().italic().fg(Color::Red).bg(Color::White);
    assert_eq!(s.to_sgr(), "\x1b[1;3;31;47m");
}

#[test]
fn to_sgr_empty_for_default() {
    let s = Style::default();
    assert_eq!(s.to_sgr(), "");
}

#[test]
fn reset_is_reset() {
    assert_eq!(RESET, "\x1b[0m");
}

#[test]
fn to_sgr_ansi256() {
    let s = Style::new().fg(Color::Ansi256(42));
    assert_eq!(s.to_sgr(), "\x1b[38;5;42m");
}

#[test]
fn to_sgr_rgb() {
    let s = Style::new().fg(Color::Rgb(100, 200, 50));
    assert_eq!(s.to_sgr(), "\x1b[38;2;100;200;50m");
}

#[test]
fn to_sgr_all_attributes() {
    let s = Style::new()
        .bold()
        .dim()
        .italic()
        .underline()
        .strikethrough();
    assert_eq!(s.to_sgr(), "\x1b[1;2;3;4;9m");
}

#[test]
fn round_trip_styled_text() {
    let s = Style::new().bold().fg(Color::Red);
    let result = format!("{}hello{}", s.to_sgr(), RESET);
    assert_eq!(result, "\x1b[1;31mhello\x1b[0m");
}

#[test]
fn round_trip_bg_styled_text() {
    let s = Style::new().bg(Color::Cyan);
    let result = format!("{}world{}", s.to_sgr(), RESET);
    assert_eq!(result, "\x1b[46mworld\x1b[0m");
}

// ── Style state tests ────────────────────────────────────────────

#[test]
fn style_state_none_is_empty() {
    assert!(Style::NONE.is_empty());
    assert!(Style::default().is_empty());
}

#[test]
fn style_state_to_sgr_empty() {
    assert_eq!(Style::NONE.to_sgr(), "");
}

#[test]
fn style_state_to_sgr_bold() {
    let s = Style { bold: true, ..Default::default() };
    assert_eq!(s.to_sgr(), "\x1b[1m");
}

#[test]
fn style_state_to_sgr_combined() {
    let s = Style {
        bold: true,
        italic: true,
        fg: Some(Color::Cyan),
        bg: Some(Color::Rgb(20, 20, 35)),
        ..Default::default()
    };
    assert_eq!(s.to_sgr(), "\x1b[1;3;36;48;2;20;20;35m");
}

#[test]
fn style_state_transition_same_is_empty() {
    let s = Style { bold: true, ..Default::default() };
    assert_eq!(s.transition_from(&s), "");
}

#[test]
fn style_state_transition_add_bold() {
    let from = Style::NONE;
    let to = Style { bold: true, ..Default::default() };
    assert_eq!(to.transition_from(&from), "\x1b[1m");
}

#[test]
fn style_state_transition_remove_bold() {
    let from = Style { bold: true, ..Default::default() };
    let to = Style::NONE;
    // Should use individual off-code, not hard reset
    assert_eq!(to.transition_from(&from), "\x1b[22m");
}

#[test]
fn style_state_transition_change_fg() {
    let from = Style { fg: Some(Color::Cyan), ..Default::default() };
    let to = Style { fg: Some(Color::Red), ..Default::default() };
    assert_eq!(to.transition_from(&from), "\x1b[31m");
}

#[test]
fn style_state_transition_remove_bold_keep_italic() {
    let from = Style { bold: true, italic: true, ..Default::default() };
    let to = Style { italic: true, ..Default::default() };
    // 22 kills bold+dim, then re-enable nothing extra needed since dim is off
    // But we need to check: 22 kills both. italic stays because only bold changed.
    let sgr = to.transition_from(&from);
    assert!(sgr.contains("22"), "should contain nobold: {sgr}");
    assert!(!sgr.contains("23"), "should NOT touch italic: {sgr}");
}

#[test]
fn style_state_overlay() {
    let base = Style { bg: Some(Color::Blue), ..Default::default() };
    let overlay = Style { bold: true, ..Default::default() };
    let combined = base.with_overlay(&overlay);
    assert!(combined.bold);
    assert_eq!(combined.bg, Some(Color::Blue)); // preserved from base
}

// ── StyleStack tests ─────────────────────────────────────────────

#[test]
fn stack_new_is_empty_state() {
    let ss = StyleStack::new();
    assert!(ss.current().is_empty());
    assert_eq!(ss.sgr(), "");
}

#[test]
fn stack_with_base() {
    let ss = StyleStack::with_base(Style {
        bg: Some(Color::Blue),
        ..Default::default()
    });
    assert_eq!(ss.current().bg, Some(Color::Blue));
    assert_eq!(ss.sgr(), "\x1b[44m");
}

#[test]
fn stack_push_pop() {
    let mut ss = StyleStack::new();
    let t = ss.push(Style { bold: true, ..Default::default() });
    assert_eq!(t, "\x1b[1m");
    assert!(ss.current().bold);

    let t = ss.pop().to_string();
    assert_eq!(t, "\x1b[22m"); // individual off-code, not hard reset
    assert!(!ss.current().bold);
}

#[test]
fn stack_nested_push_pop_preserves_parent() {
    let mut ss = StyleStack::with_base(Style {
        bg: Some(Color::Rgb(20, 20, 35)),
        ..Default::default()
    });
    // Push bold
    ss.push(Style { bold: true, ..Default::default() });
    assert!(ss.current().bold);
    assert_eq!(ss.current().bg, Some(Color::Rgb(20, 20, 35)));

    // Push cyan fg
    ss.push(Style { fg: Some(Color::Cyan), ..Default::default() });
    assert!(ss.current().bold);
    assert_eq!(ss.current().fg, Some(Color::Cyan));
    assert_eq!(ss.current().bg, Some(Color::Rgb(20, 20, 35)));

    // Pop cyan: should restore bold + bg, no fg
    let t = ss.pop().to_string();
    assert!(t.contains("39"), "should reset fg: {t}");
    assert!(ss.current().bold);
    assert_eq!(ss.current().fg, None);
    assert_eq!(ss.current().bg, Some(Color::Rgb(20, 20, 35)));

    // Pop bold: should restore just bg
    ss.pop();
    assert!(!ss.current().bold);
    assert_eq!(ss.current().bg, Some(Color::Rgb(20, 20, 35)));
}

#[test]
fn stack_sgr_for_continuation_lines() {
    let mut ss = StyleStack::with_base(Style {
        bg: Some(Color::Blue),
        ..Default::default()
    });
    ss.push(Style { bold: true, fg: Some(Color::Yellow), ..Default::default() });
    // sgr() should produce the full combined state
    let sgr = ss.sgr().to_string();
    assert!(sgr.contains("1"), "bold: {sgr}");
    assert!(sgr.contains("33"), "yellow: {sgr}");
    assert!(sgr.contains("44"), "blue bg: {sgr}");
}

#[test]
fn stack_set_base_recomputes_correctly() {
    // Start with blue bg, push bold overlay
    let mut ss = StyleStack::with_base(Style {
        bg: Some(Color::Blue),
        ..Default::default()
    });
    ss.push(Style { bold: true, ..Default::default() });
    assert!(ss.current().bold);
    assert_eq!(ss.current().bg, Some(Color::Blue));

    // Change base to green bg — bold overlay should survive,
    // but bg should change from blue to green.
    ss.set_base(Style { bg: Some(Color::Green), ..Default::default() });
    assert!(ss.current().bold, "bold overlay should survive set_base");
    assert_eq!(ss.current().bg, Some(Color::Green),
        "bg should be green (new base), not blue (old base)");

    // Pop should restore to green bg, no bold
    ss.pop();
    assert!(!ss.current().bold);
    assert_eq!(ss.current().bg, Some(Color::Green));
}

#[test]
fn stack_set_base_with_multiple_overlays() {
    let mut ss = StyleStack::with_base(Style {
        bg: Some(Color::Blue),
        ..Default::default()
    });
    ss.push(Style { bold: true, ..Default::default() });
    ss.push(Style { fg: Some(Color::Cyan), ..Default::default() });

    // Combined: blue bg + bold + cyan fg
    assert_eq!(ss.current().bg, Some(Color::Blue));
    assert!(ss.current().bold);
    assert_eq!(ss.current().fg, Some(Color::Cyan));

    // Change base to red bg
    ss.set_base(Style { bg: Some(Color::Red), ..Default::default() });

    // Should be: red bg + bold + cyan fg
    assert_eq!(ss.current().bg, Some(Color::Red),
        "bg should follow new base");
    assert!(ss.current().bold, "bold overlay should survive");
    assert_eq!(ss.current().fg, Some(Color::Cyan),
        "cyan overlay should survive");

    // Pop cyan → red bg + bold
    ss.pop();
    assert_eq!(ss.current().bg, Some(Color::Red));
    assert!(ss.current().bold);
    assert_eq!(ss.current().fg, None);

    // Pop bold → just red bg
    ss.pop();
    assert_eq!(ss.current().bg, Some(Color::Red));
    assert!(!ss.current().bold);
}

// ── apply_sgr tests ──────────────────────────────────────────────

#[test]
fn apply_sgr_reset() {
    let mut s = Style { bold: true, fg: Some(Color::Red), ..Default::default() };
    apply_sgr(&mut s, "\x1b[0m");
    assert!(s.is_empty());
}

#[test]
fn apply_sgr_bold() {
    let mut s = Style::NONE;
    apply_sgr(&mut s, "\x1b[1m");
    assert!(s.bold);
}

#[test]
fn apply_sgr_combined() {
    let mut s = Style::NONE;
    apply_sgr(&mut s, "\x1b[1;3;36m");
    assert!(s.bold);
    assert!(s.italic);
    assert_eq!(s.fg, Some(Color::Cyan));
}

#[test]
fn apply_sgr_nobold() {
    let mut s = Style { bold: true, dim: true, ..Default::default() };
    apply_sgr(&mut s, "\x1b[22m");
    assert!(!s.bold);
    assert!(!s.dim); // 22 kills both
}

#[test]
fn apply_sgr_rgb_fg() {
    let mut s = Style::NONE;
    apply_sgr(&mut s, "\x1b[38;2;100;200;50m");
    assert_eq!(s.fg, Some(Color::Rgb(100, 200, 50)));
}

#[test]
fn apply_sgr_rgb_bg() {
    let mut s = Style::NONE;
    apply_sgr(&mut s, "\x1b[48;2;20;20;35m");
    assert_eq!(s.bg, Some(Color::Rgb(20, 20, 35)));
}

#[test]
fn apply_sgr_ansi256() {
    let mut s = Style::NONE;
    apply_sgr(&mut s, "\x1b[38;5;42m");
    assert_eq!(s.fg, Some(Color::Ansi256(42)));
}

#[test]
fn apply_sgr_not_sgr() {
    let mut s = Style::NONE;
    assert!(!apply_sgr(&mut s, "\x1b[2J")); // not SGR
}

// ── Color::from_hex tests ────────────────────────────────────────

#[test]
fn from_hex_valid() {
    assert_eq!(Color::from_hex("#ff8000"), Some(Color::Rgb(255, 128, 0)));
}

#[test]
fn from_hex_lowercase() {
    assert_eq!(Color::from_hex("#00d7ff"), Some(Color::Rgb(0, 215, 255)));
}

#[test]
fn from_hex_no_hash() {
    assert_eq!(Color::from_hex("ff8000"), None);
}

#[test]
fn from_hex_short() {
    assert_eq!(Color::from_hex("#f80"), None);
}

// ── Theme tests ──────────────────────────────────────────────────

#[test]
fn theme_foreground() {
    let t = Theme::default();
    let s = t.foreground(ThemeColor::Accent, "hi");
    assert!(s.starts_with("\x1b["));
    assert!(s.contains("hi"));
    assert!(s.ends_with("\x1b[39m"));
}

#[test]
fn theme_background() {
    let t = Theme::default();
    let s = t.background(ThemeColor::PillBg, "x");
    assert!(s.starts_with("\x1b["));
    assert!(s.contains("x"));
    assert!(s.ends_with("\x1b[49m"));
}

#[test]
fn theme_resolve() {
    let t = Theme::default();
    assert_eq!(t.resolve(ThemeColor::Accent), Color::Cyan);
    assert_eq!(t.resolve(ThemeColor::Warning), Color::Yellow);
}

// ── from_json tests (json-theme feature) ────────────────────────

#[cfg(feature = "json-theme")]
mod json_theme {
    use super::*;
    use serde_json::json;

    #[test]
    fn from_json_empty_returns_defaults() {
        let t = Theme::from_json(&json!({}));
        assert_eq!(t, Theme::default());
    }

    #[test]
    fn from_json_hex_color() {
        let t = Theme::from_json(&json!({
            "colors": {
                "accent": "#00d7ff"
            }
        }));
        assert_eq!(t.accent, Color::Rgb(0, 215, 255));
        // Other fields keep defaults
        assert_eq!(t.border, Theme::default().border);
    }

    #[test]
    fn from_json_ansi256_integer() {
        let t = Theme::from_json(&json!({
            "colors": {
                "border": 244
            }
        }));
        assert_eq!(t.border, Color::Ansi256(244));
    }

    #[test]
    fn from_json_variable_reference() {
        let t = Theme::from_json(&json!({
            "vars": {
                "cyan": "#00d7ff"
            },
            "colors": {
                "accent": "cyan"
            }
        }));
        assert_eq!(t.accent, Color::Rgb(0, 215, 255));
    }

    #[test]
    fn from_json_recursive_variable_resolution() {
        let t = Theme::from_json(&json!({
            "vars": {
                "primary": "actualColor",
                "actualColor": "#ff0000"
            },
            "colors": {
                "accent": "primary"
            }
        }));
        assert_eq!(t.accent, Color::Rgb(255, 0, 0));
    }

    #[test]
    fn from_json_circular_reference_returns_default() {
        let t = Theme::from_json(&json!({
            "vars": {
                "a": "b",
                "b": "a"
            },
            "colors": {
                "accent": "a"
            }
        }));
        // Circular references should fall back to default
        assert_eq!(t.accent, Theme::default().accent);
    }

    #[test]
    fn from_json_empty_string_for_optional() {
        let t = Theme::from_json(&json!({
            "colors": {
                "text": "",
                "surfaceBg": ""
            }
        }));
        assert_eq!(t.text, None);
        assert_eq!(t.surface_bg, None);
    }

    #[test]
    fn from_json_selected_bg_maps_to_overlay() {
        let t = Theme::from_json(&json!({
            "colors": {
                "selectedBg": "#3a3a4a"
            }
        }));
        assert_eq!(t.overlay_highlight_bg, Color::Rgb(58, 58, 74));
    }

    #[test]
    fn from_json_all_markdown_keys() {
        let t = Theme::from_json(&json!({
            "colors": {
                "mdHeading": "#f0c674",
                "mdLink": "#81a2be",
                "mdLinkUrl": "#666666",
                "mdCode": "#8abeb7",
                "mdCodeBlock": "#b5bd68",
                "mdCodeBlockBorder": "#808080",
                "mdQuote": "#808080",
                "mdQuoteBorder": "#808080",
                "mdHr": "#808080",
                "mdListBullet": "#8abeb7"
            }
        }));
        assert_eq!(t.md_heading, Color::Rgb(240, 198, 116));
        assert_eq!(t.md_link, Color::Rgb(129, 162, 190));
        assert_eq!(t.md_link_url, Color::Rgb(102, 102, 102));
        assert_eq!(t.md_code, Color::Rgb(138, 190, 183));
        assert_eq!(t.md_code_block, Color::Rgb(181, 189, 104));
        assert_eq!(t.md_code_block_border, Color::Rgb(128, 128, 128));
        assert_eq!(t.md_quote, Color::Rgb(128, 128, 128));
        assert_eq!(t.md_quote_border, Color::Rgb(128, 128, 128));
        assert_eq!(t.md_hr, Color::Rgb(128, 128, 128));
        assert_eq!(t.md_list_bullet, Color::Rgb(138, 190, 183));
    }

    #[test]
    fn from_json_border_variants() {
        let t = Theme::from_json(&json!({
            "colors": {
                "borderAccent": "#00d7ff",
                "borderMuted": "#505050"
            }
        }));
        assert_eq!(t.border_accent, Color::Rgb(0, 215, 255));
        assert_eq!(t.border_muted, Color::Rgb(80, 80, 80));
    }

    #[test]
    fn from_json_dim_color() {
        let t = Theme::from_json(&json!({
            "colors": {
                "dim": "#666666"
            }
        }));
        assert_eq!(t.dim, Color::Rgb(102, 102, 102));
    }

    #[test]
    fn from_json_pi_mono_dark_theme() {
        // Subset of the actual pi-mono dark.json
        let t = Theme::from_json(&json!({
            "vars": {
                "cyan": "#00d7ff",
                "blue": "#5f87ff",
                "green": "#b5bd68",
                "red": "#cc6666",
                "yellow": "#ffff00",
                "gray": "#808080",
                "dimGray": "#666666",
                "darkGray": "#505050",
                "accent": "#8abeb7",
                "selectedBg": "#3a3a4a"
            },
            "colors": {
                "accent": "accent",
                "border": "blue",
                "borderAccent": "cyan",
                "borderMuted": "darkGray",
                "success": "green",
                "error": "red",
                "warning": "yellow",
                "muted": "gray",
                "dim": "dimGray",
                "text": "",
                "selectedBg": "selectedBg",
                "mdHeading": "#f0c674",
                "mdLink": "#81a2be",
                "mdLinkUrl": "dimGray",
                "mdCode": "accent",
                "mdCodeBlock": "green",
                "mdCodeBlockBorder": "gray",
                "mdQuote": "gray",
                "mdQuoteBorder": "gray",
                "mdHr": "gray",
                "mdListBullet": "accent"
            }
        }));
        assert_eq!(t.accent, Color::Rgb(138, 190, 183));
        assert_eq!(t.border, Color::Rgb(95, 135, 255));
        assert_eq!(t.border_accent, Color::Rgb(0, 215, 255));
        assert_eq!(t.border_muted, Color::Rgb(80, 80, 80));
        assert_eq!(t.success, Color::Rgb(181, 189, 104));
        assert_eq!(t.error, Color::Rgb(204, 102, 102));
        assert_eq!(t.warning, Color::Rgb(255, 255, 0));
        assert_eq!(t.muted, Color::Rgb(128, 128, 128));
        assert_eq!(t.dim, Color::Rgb(102, 102, 102));
        assert_eq!(t.text, None);
        assert_eq!(t.overlay_highlight_bg, Color::Rgb(58, 58, 74));
        assert_eq!(t.md_heading, Color::Rgb(240, 198, 116));
        assert_eq!(t.md_link, Color::Rgb(129, 162, 190));
        assert_eq!(t.md_link_url, Color::Rgb(102, 102, 102));
        assert_eq!(t.md_code, Color::Rgb(138, 190, 183)); // resolved via "accent" var
        assert_eq!(t.md_code_block, Color::Rgb(181, 189, 104)); // resolved via "green" var
        assert_eq!(t.md_code_block_border, Color::Rgb(128, 128, 128));
        assert_eq!(t.md_quote, Color::Rgb(128, 128, 128));
        assert_eq!(t.md_quote_border, Color::Rgb(128, 128, 128));
        assert_eq!(t.md_hr, Color::Rgb(128, 128, 128));
        assert_eq!(t.md_list_bullet, Color::Rgb(138, 190, 183));
    }

    #[test]
    fn from_json_unknown_keys_ignored() {
        let t = Theme::from_json(&json!({
            "colors": {
                "accent": "#ff0000",
                "syntaxComment": "#6A9955",
                "thinkingOff": "#505050",
                "bashMode": "#00ff00",
                "toolDiffAdded": "#00ff00"
            }
        }));
        assert_eq!(t.accent, Color::Rgb(255, 0, 0));
        // Unknown keys don't cause errors, just ignored
    }

    #[test]
    fn from_json_var_ref_to_integer() {
        let t = Theme::from_json(&json!({
            "vars": {
                "myborder": 244
            },
            "colors": {
                "border": "myborder"
            }
        }));
        assert_eq!(t.border, Color::Ansi256(244));
    }
}