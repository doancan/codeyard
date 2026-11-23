//! Terminal colours: the ANSI palette derived from the theme's roles, and resolution of a cell's colour
//! (named, indexed 0–255 or true colour) against the program's own overrides.

use alacritty_terminal::vte::ansi::{Color, NamedColor, Rgb};

use crate::ui::Palette;

pub(super) fn mix(a: u32, b: u32, t: f32) -> u32 {
    let ch = |s: u32| {
        let (x, y) = (((a >> s) & 0xff) as f32, ((b >> s) & 0xff) as f32);
        ((x + (y - x) * t).round() as u32) << s
    };
    ch(16) | ch(8) | ch(0)
}

fn is_dark(p: &Palette) -> bool {
    let l = |s: u32| ((p.bg >> s) & 0xff) as f32;
    0.2126 * l(16) + 0.7152 * l(8) + 0.0722 * l(0) < 128.
}

fn named(p: &Palette, n: NamedColor) -> u32 {
    let dark = is_dark(p);
    let bright = |c: u32| if dark { mix(c, 0xffffff, 0.25) } else { mix(c, 0x000000, 0.15) };
    let base = |i: usize| match i {
        0 => {
            if dark {
                mix(p.bg, 0x000000, 0.4)
            } else {
                mix(p.text, 0xffffff, 0.15)
            }
        }
        1 => p.danger,
        2 => p.success,
        3 => p.warning,
        4 => p.syntax_function,
        5 => p.syntax_keyword,
        6 => p.syntax_type,
        _ => {
            if dark {
                mix(p.text, p.bg, 0.15)
            } else {
                mix(p.text, p.bg, 0.3)
            }
        }
    };
    use NamedColor::*;
    match n {
        Black => base(0),
        Red => base(1),
        Green => base(2),
        Yellow => base(3),
        Blue => base(4),
        Magenta => base(5),
        Cyan => base(6),
        White => base(7),
        BrightBlack => p.text_muted,
        BrightRed => bright(base(1)),
        BrightGreen => bright(base(2)),
        BrightYellow => bright(base(3)),
        BrightBlue => bright(base(4)),
        BrightMagenta => bright(base(5)),
        BrightCyan => bright(base(6)),
        BrightWhite => p.text,
        Foreground | BrightForeground => p.text,
        Background => p.bg,
        Cursor => p.accent,
        DimForeground => mix(p.text, p.bg, 0.4),
        DimBlack | DimRed | DimGreen | DimYellow | DimBlue | DimMagenta | DimCyan | DimWhite => {
            let i = n as usize - DimBlack as usize;
            mix(base(i), p.bg, 0.4)
        }
    }
}

fn rgb(r: Rgb) -> u32 {
    (r.r as u32) << 16 | (r.g as u32) << 8 | r.b as u32
}

/// OSC 4/10/11/12 queries use the same palette (and program overrides) as the renderer.
/// Without these replies a TUI cannot discover a light terminal and may choose pale text.
pub(super) fn queried(p: &Palette, index: usize, overrides: &alacritty_terminal::term::color::Colors) -> Option<Rgb> {
    let color = match index {
        0..=255 => Color::Indexed(index as u8),
        256 => Color::Named(NamedColor::Foreground),
        257 => Color::Named(NamedColor::Background),
        258 => Color::Named(NamedColor::Cursor),
        _ => return None,
    };
    let c = resolve(p, color, overrides);
    Some(Rgb { r: (c >> 16) as u8, g: (c >> 8) as u8, b: c as u8 })
}

/// Keep ANSI, indexed and true-colour text readable on its final cell background,
/// including selection, search matches, inverse video and SGR 2 (faint).
/// Background colours and deliberately identical foreground/background pairs are preserved.
pub(super) fn readable(fg: u32, bg: u32, dim: bool) -> u32 {
    if fg == bg {
        return fg;
    }
    let candidate = if dim { mix(fg, bg, 0.30) } else { fg };
    let toward = crate::ui::readable_on(bg);
    crate::ui::ensure_contrast(candidate, toward, &[bg], 4.5)
}

pub(super) fn resolve(p: &Palette, color: Color, overrides: &alacritty_terminal::term::color::Colors) -> u32 {
    match color {
        Color::Spec(r) => rgb(r),
        Color::Named(n) => overrides[n].map(rgb).unwrap_or_else(|| named(p, n)),
        Color::Indexed(i) => {
            if let Some(r) = overrides[i as usize] {
                return rgb(r);
            }
            match i {
                0..=15 => {
                    let names = [
                        NamedColor::Black,
                        NamedColor::Red,
                        NamedColor::Green,
                        NamedColor::Yellow,
                        NamedColor::Blue,
                        NamedColor::Magenta,
                        NamedColor::Cyan,
                        NamedColor::White,
                        NamedColor::BrightBlack,
                        NamedColor::BrightRed,
                        NamedColor::BrightGreen,
                        NamedColor::BrightYellow,
                        NamedColor::BrightBlue,
                        NamedColor::BrightMagenta,
                        NamedColor::BrightCyan,
                        NamedColor::BrightWhite,
                    ];
                    named(p, names[i as usize])
                }
                16..=231 => {
                    // 6×6×6 color cube
                    let i = i as u32 - 16;
                    let v = |x: u32| if x == 0 { 0 } else { 55 + x * 40 };
                    v(i / 36) << 16 | v((i / 6) % 6) << 8 | v(i % 6)
                }
                _ => {
                    let g = 8 + (i as u32 - 232) * 10;
                    g << 16 | g << 8 | g
                }
            }
        }
    }
}
