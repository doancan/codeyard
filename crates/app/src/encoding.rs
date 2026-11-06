//! Non-UTF-8 text files: UTF-16 with BOM (LE/BE), Windows-1254 (Turkish),
//! ISO-8859-9 and ISO-8859-1. Decoded on open, saved back in the same encoding.
//! Small tables: `encoding_rs` would add ~1 MB to the app.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Encoding {
    Utf8,
    Utf16Le,
    Utf16Be,
    Windows1254,
    Iso8859_9,
    Iso8859_1,
}

impl Encoding {
    /// Every encoding, in the order the pickers list them.
    pub const ALL: [Encoding; 6] = [Encoding::Utf8, Encoding::Utf16Le, Encoding::Utf16Be, Encoding::Windows1254, Encoding::Iso8859_9, Encoding::Iso8859_1];

    pub fn label(self) -> &'static str {
        match self {
            Encoding::Utf8 => "UTF-8",
            Encoding::Utf16Le => "UTF-16 LE",
            Encoding::Utf16Be => "UTF-16 BE",
            Encoding::Windows1254 => "Windows 1254",
            Encoding::Iso8859_9 => "ISO 8859-9",
            Encoding::Iso8859_1 => "ISO 8859-1",
        }
    }

    /// The name `from_setting` accepts (for writing to the settings file).
    pub fn setting_name(self) -> &'static str {
        match self {
            Encoding::Utf8 | Encoding::Utf16Le | Encoding::Utf16Be => "utf8",
            Encoding::Windows1254 => "windows1254",
            Encoding::Iso8859_9 => "iso88599",
            Encoding::Iso8859_1 => "iso88591",
        }
    }

    /// Name in the setting ("windows1254", "iso88599", "iso88591"; VS Code names).
    pub fn from_setting(name: &str) -> Option<Encoding> {
        Some(match name.to_ascii_lowercase().replace(['-', '_', ' '], "").as_str() {
            "utf8" => Encoding::Utf8,
            "windows1254" | "cp1254" => Encoding::Windows1254,
            "iso88599" | "latin5" => Encoding::Iso8859_9,
            "iso88591" | "latin1" => Encoding::Iso8859_1,
            _ => return None,
        })
    }
}

/// Windows-1254, 0x80-0x9F: the part that differs from ISO-8859-9 (undefined ones are U+FFFD).
const CP1254_80: [char; 32] = [
    '€', '\u{FFFD}', '‚', 'ƒ', '„', '…', '†', '‡', 'ˆ', '‰', 'Š', '‹', 'Œ', '\u{FFFD}', '\u{FFFD}', '\u{FFFD}', '\u{FFFD}', '‘', '’', '“', '”', '•', '–', '—',
    '˜', '™', 'š', '›', 'œ', '\u{FFFD}', '\u{FFFD}', 'Ÿ',
];

/// ISO-8859-9: differs from Latin-1 in only 6 positions (Turkish letters).
fn latin5(b: u8) -> char {
    match b {
        0xD0 => 'Ğ',
        0xDD => 'İ',
        0xDE => 'Ş',
        0xF0 => 'ğ',
        0xFD => 'ı',
        0xFE => 'ş',
        _ => b as char,
    }
}

fn decode_byte(enc: Encoding, b: u8) -> char {
    match (enc, b) {
        (_, 0..=0x7F) => b as char,
        (Encoding::Windows1254, 0x80..=0x9F) => CP1254_80[(b - 0x80) as usize],
        (Encoding::Windows1254 | Encoding::Iso8859_9, _) => latin5(b),
        _ => b as char, // ISO-8859-1: byte = code point
    }
}

fn encode_char(enc: Encoding, ch: char) -> Option<u8> {
    let cp = ch as u32;
    if cp < 0x80 {
        return Some(cp as u8);
    }
    match enc {
        Encoding::Iso8859_1 => (cp <= 0xFF).then_some(cp as u8),
        Encoding::Windows1254 | Encoding::Iso8859_9 => {
            if enc == Encoding::Windows1254
                && let Some(i) = CP1254_80.iter().position(|&c| c == ch && c != '\u{FFFD}')
            {
                return Some(0x80 + i as u8);
            }
            (0xA0..=0xFF).find(|&b| latin5(b) == ch).or(if enc == Encoding::Iso8859_9 && (0x80..0xA0).contains(&cp) { Some(cp as u8) } else { None })
        }
        _ => None,
    }
}

/// Text from bytes: BOM first, then UTF-8; if invalid, `fallback` (single-byte).
/// `None`: looks like a binary file (NUL byte, no BOM).
pub fn decode(data: &[u8], fallback: Encoding) -> Option<(String, Encoding)> {
    match data {
        // Lossy decoding (a lone trailing byte or a lone surrogate pair gets dropped and becomes "�") would write different bytes on save: invalid UTF-16
        // counts as "binary" (`None`), so the file opens in the hex view without being corrupted.
        [0xFF, 0xFE, rest @ ..] => return utf16(rest, u16::from_le_bytes).map(|t| (t, Encoding::Utf16Le)),
        [0xFE, 0xFF, rest @ ..] => return utf16(rest, u16::from_be_bytes).map(|t| (t, Encoding::Utf16Be)),
        _ => {}
    }
    if data[..data.len().min(8192)].contains(&0) {
        return None;
    }
    if let Ok(s) = std::str::from_utf8(data) {
        return Some((s.to_string(), Encoding::Utf8));
    }
    let enc = if fallback == Encoding::Utf8 { Encoding::Windows1254 } else { fallback };
    Some((data.iter().map(|&b| decode_byte(enc, b)).collect(), enc))
}

/// Text from bytes read as `enc`, whatever they look like ("Reopen with Encoding"). `None` when the bytes are not valid in `enc`.
pub fn decode_as(data: &[u8], enc: Encoding) -> Option<String> {
    match enc {
        Encoding::Utf8 => std::str::from_utf8(data).ok().map(str::to_string),
        Encoding::Utf16Le => utf16(data.strip_prefix(&[0xFF, 0xFE]).unwrap_or(data), u16::from_le_bytes),
        Encoding::Utf16Be => utf16(data.strip_prefix(&[0xFE, 0xFF]).unwrap_or(data), u16::from_be_bytes),
        _ => Some(data.iter().map(|&b| decode_byte(enc, b)).collect()),
    }
}

fn utf16(rest: &[u8], f: fn([u8; 2]) -> u16) -> Option<String> {
    let (pairs, odd) = rest.as_chunks::<2>();
    if !odd.is_empty() {
        return None;
    }
    let units: Vec<u16> = pairs.iter().map(|c| f(*c)).collect();
    String::from_utf16(&units).ok()
}

/// Convert text to the file's encoding. The first character with no mapping is an `Err`.
pub fn encode(text: &str, enc: Encoding) -> Result<Vec<u8>, char> {
    match enc {
        Encoding::Utf8 => Ok(text.as_bytes().to_vec()),
        Encoding::Utf16Le | Encoding::Utf16Be => {
            let mut out = if enc == Encoding::Utf16Le { vec![0xFF, 0xFE] } else { vec![0xFE, 0xFF] };
            for u in text.encode_utf16() {
                out.extend_from_slice(&if enc == Encoding::Utf16Le { u.to_le_bytes() } else { u.to_be_bytes() });
            }
            Ok(out)
        }
        _ => text.chars().map(|c| encode_char(enc, c).ok_or(c)).collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn turkish_single_byte_round_trip() {
        // "Işık ğüşöç İĞÜŞÖÇ €" in Windows-1254.
        let text = "Işık ğüşöç İĞÜŞÖÇ €";
        let bytes = encode(text, Encoding::Windows1254).unwrap();
        assert_eq!(bytes.len(), text.chars().count(), "single-byte");
        assert_eq!(bytes[1], 0xFE, "ş");
        let (back, enc) = decode(&bytes, Encoding::Windows1254).unwrap();
        assert_eq!((back.as_str(), enc), (text, Encoding::Windows1254));
        // ISO-8859-9 has the same letters, no €.
        assert_eq!(encode("€", Encoding::Iso8859_9), Err('€'));
        assert_eq!(encode("ğ", Encoding::Iso8859_1), Err('ğ'), "no ğ in Latin-1");
        assert_eq!(encode("😀", Encoding::Windows1254), Err('😀'));
    }

    #[test]
    fn decode_as_reads_the_bytes_in_the_chosen_encoding() {
        // "ş" is 0xFE in Windows-1254 and an invalid byte alone in UTF-8; as Latin-1 it is "þ".
        assert_eq!(decode_as(&[b'a', 0xFE], Encoding::Windows1254).as_deref(), Some("aş"));
        assert_eq!(decode_as(&[b'a', 0xFE], Encoding::Iso8859_1).as_deref(), Some("aþ"));
        assert_eq!(decode_as(&[b'a', 0xFE], Encoding::Utf8), None, "invalid UTF-8 is refused, not mangled");
        assert_eq!(decode_as("ğ".as_bytes(), Encoding::Utf8).as_deref(), Some("ğ"));
        // UTF-16 with or without its BOM; an odd byte count is refused.
        assert_eq!(decode_as(&[b'a', 0, b'b', 0], Encoding::Utf16Le).as_deref(), Some("ab"));
        assert_eq!(decode_as(&[0xFE, 0xFF, 0, b'a'], Encoding::Utf16Be).as_deref(), Some("a"));
        assert_eq!(decode_as(&[b'a', 0, b'b'], Encoding::Utf16Le), None);
        for enc in Encoding::ALL {
            let bytes = encode("abc", enc).unwrap();
            assert_eq!(decode_as(&bytes, enc).as_deref(), Some("abc"), "{}", enc.label());
        }
    }

    #[test]
    fn bom_utf16_and_detection() {
        let text = "satır\r\nğüş 😀";
        for enc in [Encoding::Utf16Le, Encoding::Utf16Be] {
            let bytes = encode(text, enc).unwrap();
            assert_eq!(decode(&bytes, Encoding::Windows1254), Some((text.to_string(), enc)));
        }
        assert_eq!(decode("ğ".as_bytes(), Encoding::Windows1254).unwrap().1, Encoding::Utf8, "valid UTF-8 comes first");
        assert_eq!(decode(b"ab\0cd", Encoding::Windows1254), None, "ikili");
        // Invalid UTF-16 (lone trailing byte, lone surrogate) can't be opened losslessly: it isn't shown as text.
        assert_eq!(decode(&[0xFF, 0xFE, b'a', 0, b'b'], Encoding::Windows1254), None, "odd number of bytes");
        assert_eq!(decode(&[0xFF, 0xFE, 0x00, 0xD8, b'a', 0], Encoding::Windows1254), None, "lone high surrogate");
        assert_eq!(decode(&[0xFE, 0xFF, 0xDC, 0x00], Encoding::Windows1254), None, "lone low surrogate (BE)");
        assert_eq!(decode(&[0xFF, 0xFE], Encoding::Windows1254), Some((String::new(), Encoding::Utf16Le)), "a lone BOM is an empty file");
        assert_eq!(Encoding::from_setting("windows-1254"), Some(Encoding::Windows1254));
    }
}
