//! Color literals: find, parse, write, and HSV conversions. The color swatches in the editor and the color picker use this.
//! Supported: `#rgb #rgba #rrggbb #rrggbbaa`, `rgb()/rgba()` (comma and space/`/` alpha forms), `hsl()/hsla()`,
//! CSS names (only in CSS-like files: `red` can be an ordinary word).

use std::ops::Range;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Color {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    /// 0.0 (transparent) - 1.0 (opaque).
    pub a: f32,
}

/// How the literal was written: the picker writes it back in the same form when the color changes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Notation {
    Hex { upper: bool },
    Rgb { legacy: bool },
    Hsl { legacy: bool },
    Named,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Found {
    pub range: Range<usize>,
    pub color: Color,
    pub notation: Notation,
}

impl Color {
    pub fn rgb(self) -> u32 {
        (self.r as u32) << 16 | (self.g as u32) << 8 | self.b as u32
    }

    pub fn from_rgb(rgb: u32) -> Color {
        Color { r: (rgb >> 16) as u8, g: (rgb >> 8) as u8, b: rgb as u8, a: 1. }
    }

    /// (hue 0-360, saturation 0-1, value 0-1)
    pub fn to_hsv(self) -> (f32, f32, f32) {
        let (r, g, b) = (self.r as f32 / 255., self.g as f32 / 255., self.b as f32 / 255.);
        let max = r.max(g).max(b);
        let min = r.min(g).min(b);
        let d = max - min;
        let h = if d == 0. {
            0.
        } else if max == r {
            60. * (((g - b) / d) % 6.)
        } else if max == g {
            60. * ((b - r) / d + 2.)
        } else {
            60. * ((r - g) / d + 4.)
        };
        (if h < 0. { h + 360. } else { h }, if max == 0. { 0. } else { d / max }, max)
    }

    pub fn from_hsv(h: f32, s: f32, v: f32, a: f32) -> Color {
        let (h, s, v) = (h.rem_euclid(360.), s.clamp(0., 1.), v.clamp(0., 1.));
        let c = v * s;
        let x = c * (1. - ((h / 60.) % 2. - 1.).abs());
        let m = v - c;
        let (r, g, b) = match (h / 60.) as u32 {
            0 => (c, x, 0.),
            1 => (x, c, 0.),
            2 => (0., c, x),
            3 => (0., x, c),
            4 => (x, 0., c),
            _ => (c, 0., x),
        };
        let byte = |f: f32| ((f + m) * 255.).round().clamp(0., 255.) as u8;
        Color { r: byte(r), g: byte(g), b: byte(b), a: a.clamp(0., 1.) }
    }

    /// (hue, saturation, lightness) HSL.
    pub fn to_hsl(self) -> (f32, f32, f32) {
        let (r, g, b) = (self.r as f32 / 255., self.g as f32 / 255., self.b as f32 / 255.);
        let (max, min) = (r.max(g).max(b), r.min(g).min(b));
        let l = (max + min) / 2.;
        let d = max - min;
        let s = if d == 0. { 0. } else { d / (1. - (2. * l - 1.).abs()) };
        (self.to_hsv().0, s, l)
    }

    pub fn from_hsl(h: f32, s: f32, l: f32, a: f32) -> Color {
        let (s, l) = (s.clamp(0., 1.), l.clamp(0., 1.));
        let v = l + s * l.min(1. - l);
        let sv = if v == 0. { 0. } else { 2. * (1. - l / v) };
        Color::from_hsv(h, sv, v, a)
    }
}

fn alpha_text(a: f32) -> String {
    let s = format!("{:.2}", a.clamp(0., 1.));
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

/// Write the color in the given form. `Named` is written as hex (a name may not exist).
pub fn format(color: Color, notation: Notation) -> String {
    let opaque = color.a >= 0.995;
    match notation {
        Notation::Hex { upper } => {
            let s = if opaque {
                format!("#{:02x}{:02x}{:02x}", color.r, color.g, color.b)
            } else {
                format!("#{:02x}{:02x}{:02x}{:02x}", color.r, color.g, color.b, (color.a * 255.).round() as u8)
            };
            if upper { s.to_ascii_uppercase() } else { s }
        }
        // Hex is written instead of the name: not every color has a name.
        Notation::Named => format(color, Notation::Hex { upper: false }),
        Notation::Rgb { legacy } => match (legacy, opaque) {
            (true, true) => format!("rgb({}, {}, {})", color.r, color.g, color.b),
            (true, false) => format!("rgba({}, {}, {}, {})", color.r, color.g, color.b, alpha_text(color.a)),
            (false, true) => format!("rgb({} {} {})", color.r, color.g, color.b),
            (false, false) => format!("rgb({} {} {} / {})", color.r, color.g, color.b, alpha_text(color.a)),
        },
        Notation::Hsl { legacy } => {
            let (h, s, l) = color.to_hsl();
            let (h, s, l) = (h.round() as i32 % 360, (s * 100.).round() as i32, (l * 100.).round() as i32);
            match (legacy, opaque) {
                (true, true) => format!("hsl({h}, {s}%, {l}%)"),
                (true, false) => format!("hsla({h}, {s}%, {l}%, {})", alpha_text(color.a)),
                (false, true) => format!("hsl({h} {s}% {l}%)"),
                (false, false) => format!("hsl({h} {s}% {l}% / {})", alpha_text(color.a)),
            }
        }
    }
}

fn hex_digit(c: u8) -> Option<u32> {
    (c as char).to_digit(16)
}

fn parse_hex(s: &str) -> Option<Color> {
    let b = s.as_bytes();
    let d = |i: usize| hex_digit(b[i]);
    match b.len() {
        3 | 4 => {
            let (r, g, bl) = (d(0)? * 17, d(1)? * 17, d(2)? * 17);
            let a = if b.len() == 4 { d(3)? * 17 } else { 255 };
            Some(Color { r: r as u8, g: g as u8, b: bl as u8, a: a as f32 / 255. })
        }
        6 | 8 => {
            let byte = |i: usize| Some((d(i)? * 16 + d(i + 1)?) as u8);
            let a = if b.len() == 8 { byte(6)? } else { 255 };
            Some(Color { r: byte(0)?, g: byte(2)?, b: byte(4)?, a: a as f32 / 255. })
        }
        _ => None,
    }
}

/// Numbers inside `rgb(...)`/`hsl(...)`: comma, space or `/` separator; `%` and units (deg, turn, rad).
fn parse_args(inner: &str) -> Option<(Vec<f32>, Vec<bool>, bool)> {
    let legacy = inner.contains(',');
    let cleaned = inner.replace(['/', ','], " ");
    let mut vals = Vec::new();
    let mut pct = Vec::new();
    for tok in cleaned.split_whitespace() {
        let (num, unit) = tok.find(|c: char| c.is_ascii_alphabetic() || c == '%').map_or((tok, ""), |i| (&tok[..i], &tok[i..]));
        let mut v: f32 = num.parse().ok()?;
        match unit {
            "" | "%" | "deg" => {}
            "turn" => v *= 360.,
            "rad" => v = v.to_degrees(),
            "grad" => v *= 0.9,
            _ => return None,
        }
        vals.push(v);
        pct.push(unit == "%");
    }
    (vals.len() == 3 || vals.len() == 4).then_some((vals, pct, legacy))
}

fn parse_func(name: &str, inner: &str) -> Option<(Color, Notation)> {
    let (v, pct, legacy) = parse_args(inner)?;
    let alpha = match v.get(3) {
        Some(a) => (if pct[3] { a / 100. } else { *a }).clamp(0., 1.),
        None => 1.,
    };
    match name {
        "rgb" | "rgba" => {
            let ch = |i: usize| (if pct[i] { v[i] * 2.55 } else { v[i] }).round().clamp(0., 255.) as u8;
            Some((Color { r: ch(0), g: ch(1), b: ch(2), a: alpha }, Notation::Rgb { legacy }))
        }
        "hsl" | "hsla" => Some((Color::from_hsl(v[0], v[1] / 100., v[2] / 100., alpha), Notation::Hsl { legacy })),
        _ => None,
    }
}

/// Longest `rgb(…)` / `hsl(…)` that is read as a colour, in bytes.
const MAX_FUNCTION: usize = 120;

/// Color literals in the text (byte range, color, written form). `named`: CSS names count too.
pub fn find_colors(text: &str, named: bool) -> Vec<Found> {
    let b = text.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    // Whether the word (the text since the last whitespace) before `scanned` holds `://`: moved forward only, so asking at every `#`
    // costs one pass over the line, not one pass per `#`.
    let (mut scanned, mut url_word) = (0, false);
    while i < b.len() {
        let c = b[i];
        let boundary_before = i == 0 || !(b[i - 1].is_ascii_alphanumeric() || b[i - 1] == b'_' || b[i - 1] == b'-');
        // `https://site/page#fff`: the URL fragment isn't a color.
        let in_url = c == b'#' && {
            for (at, ch) in text[scanned..i].char_indices() {
                let end = scanned + at + ch.len_utf8();
                if ch.is_whitespace() {
                    url_word = false;
                } else if b[..end].ends_with(b"://") {
                    url_word = true;
                }
            }
            scanned = i;
            url_word
        };
        if c == b'#' && !in_url && (i == 0 || !b[i - 1].is_ascii_alphanumeric()) {
            let n = b[i + 1..].iter().take_while(|c| c.is_ascii_hexdigit()).count();
            let after_ok = b.get(i + 1 + n).is_none_or(|c| !c.is_ascii_alphanumeric() && *c != b'_');
            if matches!(n, 3 | 4 | 6 | 8)
                && after_ok
                && let Some(color) = parse_hex(&text[i + 1..i + 1 + n])
            {
                let upper = text[i + 1..i + 1 + n].bytes().any(|c| c.is_ascii_uppercase());
                out.push(Found { range: i..i + 1 + n, color, notation: Notation::Hex { upper } });
                i += 1 + n;
                continue;
            }
        } else if matches!(c, b'r' | b'h' | b'R' | b'H') && boundary_before {
            let name_len = b[i..].iter().take_while(|c| c.is_ascii_alphabetic()).count();
            let lowered = text[i..i + name_len].to_ascii_lowercase();
            let name = lowered.as_str();
            // A colour function is short; looking for its `)` to the end of the line made `rgb(rgb(rgb(…` quadratic.
            let close = b[i + name_len..].iter().take(MAX_FUNCTION).position(|&c| c == b')');
            if matches!(name, "rgb" | "rgba" | "hsl" | "hsla")
                && b.get(i + name_len) == Some(&b'(')
                && let Some(close) = close
            {
                let inner = &text[i + name_len + 1..i + name_len + close];
                if let Some((color, notation)) = parse_func(name, inner) {
                    let end = i + name_len + close + 1;
                    out.push(Found { range: i..end, color, notation });
                    i = end;
                    continue;
                }
            }
        }
        if named && c.is_ascii_alphabetic() && boundary_before {
            let len = b[i..].iter().take_while(|c| c.is_ascii_alphabetic()).count();
            let after_ok = b.get(i + len).is_none_or(|c| !c.is_ascii_alphanumeric() && *c != b'_' && *c != b'-');
            // CSS names are case-insensitive (`Red`, `BLUE`); only first-letter-capitalized or all lower/upper case:
            // mixed case like `rEd` doesn't look like a color.
            let word = &text[i..i + len];
            let plausible = word.chars().skip(1).all(|ch| ch.is_ascii_lowercase()) || word.chars().all(|ch| ch.is_ascii_uppercase());
            if after_ok
                && plausible
                && let Some(rgb) = named_color(&word.to_ascii_lowercase())
            {
                out.push(Found { range: i..i + len, color: Color::from_rgb(rgb), notation: Notation::Named });
                i += len;
                continue;
            }
        }
        i += 1;
    }
    out
}

/// CSS names (`transparent` and `currentcolor` aren't colors, skipped).
pub fn named_color(name: &str) -> Option<u32> {
    NAMED.binary_search_by(|(n, _)| n.cmp(&name)).ok().map(|i| NAMED[i].1)
}

const NAMED: &[(&str, u32)] = &[
    ("aliceblue", 0xf0f8ff),
    ("antiquewhite", 0xfaebd7),
    ("aqua", 0x00ffff),
    ("aquamarine", 0x7fffd4),
    ("azure", 0xf0ffff),
    ("beige", 0xf5f5dc),
    ("bisque", 0xffe4c4),
    ("black", 0x000000),
    ("blanchedalmond", 0xffebcd),
    ("blue", 0x0000ff),
    ("blueviolet", 0x8a2be2),
    ("brown", 0xa52a2a),
    ("burlywood", 0xdeb887),
    ("cadetblue", 0x5f9ea0),
    ("chartreuse", 0x7fff00),
    ("chocolate", 0xd2691e),
    ("coral", 0xff7f50),
    ("cornflowerblue", 0x6495ed),
    ("cornsilk", 0xfff8dc),
    ("crimson", 0xdc143c),
    ("cyan", 0x00ffff),
    ("darkblue", 0x00008b),
    ("darkcyan", 0x008b8b),
    ("darkgoldenrod", 0xb8860b),
    ("darkgray", 0xa9a9a9),
    ("darkgreen", 0x006400),
    ("darkgrey", 0xa9a9a9),
    ("darkkhaki", 0xbdb76b),
    ("darkmagenta", 0x8b008b),
    ("darkolivegreen", 0x556b2f),
    ("darkorange", 0xff8c00),
    ("darkorchid", 0x9932cc),
    ("darkred", 0x8b0000),
    ("darksalmon", 0xe9967a),
    ("darkseagreen", 0x8fbc8f),
    ("darkslateblue", 0x483d8b),
    ("darkslategray", 0x2f4f4f),
    ("darkslategrey", 0x2f4f4f),
    ("darkturquoise", 0x00ced1),
    ("darkviolet", 0x9400d3),
    ("deeppink", 0xff1493),
    ("deepskyblue", 0x00bfff),
    ("dimgray", 0x696969),
    ("dimgrey", 0x696969),
    ("dodgerblue", 0x1e90ff),
    ("firebrick", 0xb22222),
    ("floralwhite", 0xfffaf0),
    ("forestgreen", 0x228b22),
    ("fuchsia", 0xff00ff),
    ("gainsboro", 0xdcdcdc),
    ("ghostwhite", 0xf8f8ff),
    ("gold", 0xffd700),
    ("goldenrod", 0xdaa520),
    ("gray", 0x808080),
    ("green", 0x008000),
    ("greenyellow", 0xadff2f),
    ("grey", 0x808080),
    ("honeydew", 0xf0fff0),
    ("hotpink", 0xff69b4),
    ("indianred", 0xcd5c5c),
    ("indigo", 0x4b0082),
    ("ivory", 0xfffff0),
    ("khaki", 0xf0e68c),
    ("lavender", 0xe6e6fa),
    ("lavenderblush", 0xfff0f5),
    ("lawngreen", 0x7cfc00),
    ("lemonchiffon", 0xfffacd),
    ("lightblue", 0xadd8e6),
    ("lightcoral", 0xf08080),
    ("lightcyan", 0xe0ffff),
    ("lightgoldenrodyellow", 0xfafad2),
    ("lightgray", 0xd3d3d3),
    ("lightgreen", 0x90ee90),
    ("lightgrey", 0xd3d3d3),
    ("lightpink", 0xffb6c1),
    ("lightsalmon", 0xffa07a),
    ("lightseagreen", 0x20b2aa),
    ("lightskyblue", 0x87cefa),
    ("lightslategray", 0x778899),
    ("lightslategrey", 0x778899),
    ("lightsteelblue", 0xb0c4de),
    ("lightyellow", 0xffffe0),
    ("lime", 0x00ff00),
    ("limegreen", 0x32cd32),
    ("linen", 0xfaf0e6),
    ("magenta", 0xff00ff),
    ("maroon", 0x800000),
    ("mediumaquamarine", 0x66cdaa),
    ("mediumblue", 0x0000cd),
    ("mediumorchid", 0xba55d3),
    ("mediumpurple", 0x9370db),
    ("mediumseagreen", 0x3cb371),
    ("mediumslateblue", 0x7b68ee),
    ("mediumspringgreen", 0x00fa9a),
    ("mediumturquoise", 0x48d1cc),
    ("mediumvioletred", 0xc71585),
    ("midnightblue", 0x191970),
    ("mintcream", 0xf5fffa),
    ("mistyrose", 0xffe4e1),
    ("moccasin", 0xffe4b5),
    ("navajowhite", 0xffdead),
    ("navy", 0x000080),
    ("oldlace", 0xfdf5e6),
    ("olive", 0x808000),
    ("olivedrab", 0x6b8e23),
    ("orange", 0xffa500),
    ("orangered", 0xff4500),
    ("orchid", 0xda70d6),
    ("palegoldenrod", 0xeee8aa),
    ("palegreen", 0x98fb98),
    ("paleturquoise", 0xafeeee),
    ("palevioletred", 0xdb7093),
    ("papayawhip", 0xffefd5),
    ("peachpuff", 0xffdab9),
    ("peru", 0xcd853f),
    ("pink", 0xffc0cb),
    ("plum", 0xdda0dd),
    ("powderblue", 0xb0e0e6),
    ("purple", 0x800080),
    ("rebeccapurple", 0x663399),
    ("red", 0xff0000),
    ("rosybrown", 0xbc8f8f),
    ("royalblue", 0x4169e1),
    ("saddlebrown", 0x8b4513),
    ("salmon", 0xfa8072),
    ("sandybrown", 0xf4a460),
    ("seagreen", 0x2e8b57),
    ("seashell", 0xfff5ee),
    ("sienna", 0xa0522d),
    ("silver", 0xc0c0c0),
    ("skyblue", 0x87ceeb),
    ("slateblue", 0x6a5acd),
    ("slategray", 0x708090),
    ("slategrey", 0x708090),
    ("snow", 0xfffafa),
    ("springgreen", 0x00ff7f),
    ("steelblue", 0x4682b4),
    ("tan", 0xd2b48c),
    ("teal", 0x008080),
    ("thistle", 0xd8bfd8),
    ("tomato", 0xff6347),
    ("turquoise", 0x40e0d0),
    ("violet", 0xee82ee),
    ("wheat", 0xf5deb3),
    ("white", 0xffffff),
    ("whitesmoke", 0xf5f5f5),
    ("yellow", 0xffff00),
    ("yellowgreen", 0x9acd32),
];

/// Files where named colors apply: CSS and its relatives, markup languages.
pub fn named_colors_apply(path: &str) -> bool {
    let ext = path.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).unwrap_or_default();
    matches!(ext.as_str(), "css" | "scss" | "sass" | "less" | "styl" | "html" | "htm" | "vue" | "svelte" | "svg" | "xml" | "pcss")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn one(text: &str, named: bool) -> Found {
        let f = find_colors(text, named);
        assert_eq!(f.len(), 1, "{text}: {f:?}");
        f.into_iter().next().unwrap()
    }

    #[test]
    fn hex_forms_and_boundaries() {
        assert_eq!(one("color: #fff;", false).color, Color { r: 255, g: 255, b: 255, a: 1. });
        assert_eq!(one("#1e90ff", false).color.rgb(), 0x1e90ff);
        let f = one("#0f08", false);
        assert_eq!((f.color.rgb(), (f.color.a * 255.).round() as u8), (0x00ff00, 136));
        assert_eq!(one("#AABBCCDD", false).notation, Notation::Hex { upper: true });
        // Not a color: part of an identifier, wrong length, issue references.
        assert!(find_colors("a#fff abc#123456 #12345 #gggggg #12345678a", false).is_empty());
        assert_eq!(find_colors("#fff #000", false).len(), 2);
        // A mixed line, as it appears in real files.
        let f = find_colors("color: #1e90ff; bg: #FFF; x: #12345; y: abc#fff; z: #aabbccdd", false);
        assert_eq!(f.iter().map(|f| (f.range.clone(), f.color.rgb())).collect::<Vec<_>>(), [(7..14, 0x1e90ff), (20..24, 0xffffff), (52..61, 0xaabbcc)]);
    }

    #[test]
    fn url_fragments_are_not_colours_and_function_names_ignore_case() {
        assert!(find_colors("see https://example.com/docs#fff and http://x/#abc123 end", false).is_empty());
        assert_eq!(find_colors("a#fff", false).len(), 0);
        assert_eq!(one("background: RGB(1, 2, 3)", false).color.rgb(), 0x010203);
        assert_eq!(one("color: HSL(120 100% 50%)", false).color.rgb(), 0x00ff00);
        // A color next to a URL on the same line is still found.
        assert_eq!(find_colors("url(https://x/y#z) color: #fff", false).len(), 1);
    }

    #[test]
    fn rgb_and_hsl_functions_in_both_syntaxes() {
        let f = one("rgb(255, 128, 0)", false);
        assert_eq!((f.color.rgb(), f.notation), (0xff8000, Notation::Rgb { legacy: true }));
        let f = one("rgba(0 0 0 / 50%)", false);
        assert_eq!((f.color.rgb(), f.color.a, f.notation), (0, 0.5, Notation::Rgb { legacy: false }));
        assert_eq!(one("rgb(100% 0% 0%)", false).color.rgb(), 0xff0000);
        let f = one("hsl(120, 100%, 50%)", false);
        assert_eq!(f.color.rgb(), 0x00ff00);
        let f = one("hsla(240deg 100% 50% / 0.25)", false);
        assert_eq!((f.color.rgb(), f.color.a), (0x0000ff, 0.25));
        assert_eq!(one("hsl(0.5turn 100% 50%)", false).color.rgb(), 0x00ffff);
        // Not colors: variables named like functions, malformed arguments, unclosed.
        assert!(find_colors("myrgb(1,2,3) rgb(1,2) rgb(a,b,c) rgb(1,2,3", false).is_empty());
        assert_eq!(find_colors("a { color: rgb(1,2,3); background: hsl(1 2% 3%) }", false).len(), 2);
    }

    #[test]
    fn named_colors_only_where_asked_and_only_whole_words() {
        assert!(find_colors("let red = 1; blue", false).is_empty());
        assert_eq!(one("a { color: Red }", true).color.rgb(), 0xff0000, "CSS names ignore case");
        assert_eq!(one("color: BLUE;", true).color.rgb(), 0x0000ff);
        assert!(find_colors("rEd bLuE", true).is_empty(), "mixed case is not a color name");
        assert_eq!(one("a { color: red }", true).color.rgb(), 0xff0000);
        assert_eq!(one("color: rebeccapurple;", true).color.rgb(), 0x663399);
        assert!(find_colors("reddish bluegreen my-red red_dot", true).is_empty());
        assert_eq!(find_colors("red, green and blue", true).len(), 3);
        assert!(NAMED.windows(2).all(|w| w[0].0 < w[1].0), "the table must stay sorted for binary search");
        assert!(named_colors_apply("/x/a.css") && named_colors_apply("a.SCSS") && !named_colors_apply("a.rs"));
    }

    #[test]
    fn formatting_keeps_the_notation_and_alpha() {
        let c = Color { r: 255, g: 128, b: 0, a: 1. };
        assert_eq!(format(c, Notation::Hex { upper: false }), "#ff8000");
        assert_eq!(format(c, Notation::Hex { upper: true }), "#FF8000");
        assert_eq!(format(c, Notation::Named), "#ff8000", "names are written as hex");
        assert_eq!(format(c, Notation::Rgb { legacy: true }), "rgb(255, 128, 0)");
        assert_eq!(format(c, Notation::Rgb { legacy: false }), "rgb(255 128 0)");
        assert_eq!(format(c, Notation::Hsl { legacy: true }), "hsl(30, 100%, 50%)");
        let half = Color { a: 0.5, ..c };
        assert_eq!(format(half, Notation::Hex { upper: false }), "#ff800080");
        assert_eq!(format(half, Notation::Rgb { legacy: true }), "rgba(255, 128, 0, 0.5)");
        assert_eq!(format(half, Notation::Rgb { legacy: false }), "rgb(255 128 0 / 0.5)");
        assert_eq!(format(half, Notation::Hsl { legacy: false }), "hsl(30 100% 50% / 0.5)");
    }

    #[test]
    fn round_trips_through_text_and_hsv_hsl() {
        for text in ["#1e90ff", "rgb(12, 200, 99)", "hsl(200, 50%, 40%)", "rgba(1, 2, 3, 0.4)", "rgb(9 8 7 / 0.7)"] {
            let f = one(text, false);
            let back = format(f.color, f.notation);
            let g = one(&back, false);
            assert_eq!(f.color.rgb(), g.color.rgb(), "{text} -> {back}");
        }
        for rgb in [0x000000, 0xffffff, 0xff0000, 0x12ab34, 0x808080, 0x7f7fff] {
            let c = Color::from_rgb(rgb);
            let (h, s, v) = c.to_hsv();
            assert_eq!(Color::from_hsv(h, s, v, 1.).rgb(), rgb, "hsv {rgb:06x}");
            let (h, s, l) = c.to_hsl();
            let back = Color::from_hsl(h, s, l, 1.).rgb();
            assert!((0..3).all(|i| ((back >> (8 * i)) & 255).abs_diff((rgb >> (8 * i)) & 255) <= 1), "hsl {rgb:06x} -> {back:06x}");
        }
        assert_eq!(Color::from_hsv(360., 1., 1., 1.).rgb(), 0xff0000, "hue wraps");
    }

    #[test]
    fn random_text_never_panics_and_ranges_are_valid() {
        let tokens = [
            "#", "fff", "12", "rgb(", "rgba(", "hsl(", ")", ",", " ", "/", "%", "deg", "turn", "red", "Red", "é", "😀", "(", "0.5", "-", "\n", "http://", ".",
            "e", "+", "#abcdef", "255",
        ];
        let mut seed = 0x9E37_79B9_7F4A_7C15u64;
        let mut next = || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        for _ in 0..4000 {
            let n = (next() % 14) as usize;
            let text: String = (0..n).map(|_| tokens[(next() % tokens.len() as u64) as usize]).collect();
            for named in [false, true] {
                for f in find_colors(&text, named) {
                    assert!(f.range.start < f.range.end && f.range.end <= text.len(), "{text:?} {f:?}");
                    assert!(text.is_char_boundary(f.range.start) && text.is_char_boundary(f.range.end), "{text:?} {f:?}");
                    // What was found can be written back and read again as the same color.
                    let back = format(f.color, f.notation);
                    let again = find_colors(&back, named);
                    assert_eq!(again.len(), 1, "{text:?} -> {back:?}");
                    assert_eq!(again[0].color.rgb(), f.color.rgb(), "{text:?} -> {back:?}");
                }
            }
        }
    }

    /// A minified file is one line of hundreds of kilobytes. The colour scan runs on every line that is drawn, so its cost must grow with
    /// the line, not with the square of it (these three took minutes at 300 000 characters).
    #[test]
    fn scanning_a_huge_line_for_colours_is_linear() {
        let cases = [
            ("hashes", "#".repeat(60_000)),
            ("hex colours with no space between them", "-#fff".repeat(20_000)),
            ("unclosed rgb calls", "rgb(".repeat(30_000)),
            ("unclosed hsl calls after a url", format!("https://x/{}", "hsl(1,".repeat(20_000))),
        ];
        for (name, text) in cases {
            let started = std::time::Instant::now();
            let found = find_colors(&text, true);
            assert!(started.elapsed() < std::time::Duration::from_secs(1), "{name}: {} bytes took {:?}", text.len(), started.elapsed());
            if name.starts_with("hex colours") {
                assert_eq!(found.len(), 20_000, "every `-#fff` holds a colour");
            }
        }
        // The meaning of the cheap checks did not change.
        assert_eq!(find_colors("a https://x.y/p#fff b #fff", false).len(), 1, "a URL fragment is not a colour, the next word is");
        assert_eq!(find_colors("color: rgb(1, 2, 3);", false).len(), 1);
    }
}
