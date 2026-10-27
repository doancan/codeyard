//! An image's size from its header, read before the image is decoded. A small file can announce enormous dimensions (a few
//! kilobytes of PNG for a 30,000 × 30,000 image take 3.6 GB once decoded), so the preview checks the size first.

/// Most pixels the preview decodes (about 200 MB of RGBA).
pub const MAX_PIXELS: u64 = 50_000_000;

fn u16_be(d: &[u8], at: usize) -> Option<u64> {
    Some(u16::from_be_bytes(d.get(at..at + 2)?.try_into().ok()?) as u64)
}
fn u16_le(d: &[u8], at: usize) -> Option<u64> {
    Some(u16::from_le_bytes(d.get(at..at + 2)?.try_into().ok()?) as u64)
}
fn u24_le(d: &[u8], at: usize) -> Option<u64> {
    let b = d.get(at..at + 3)?;
    Some(b[0] as u64 | (b[1] as u64) << 8 | (b[2] as u64) << 16)
}
fn u32_be(d: &[u8], at: usize) -> Option<u64> {
    Some(u32::from_be_bytes(d.get(at..at + 4)?.try_into().ok()?) as u64)
}
fn i32_le(d: &[u8], at: usize) -> Option<u64> {
    Some((i32::from_le_bytes(d.get(at..at + 4)?.try_into().ok()?) as i64).unsigned_abs())
}

/// (width, height) from the header of a PNG, GIF, BMP, JPEG or WebP image; `None` if it cannot be read (another format, or
/// a header that is cut short or broken).
pub fn dimensions(data: &[u8]) -> Option<(u64, u64)> {
    if data.starts_with(b"\x89PNG\r\n\x1a\n") {
        return Some((u32_be(data, 16)?, u32_be(data, 20)?));
    }
    if data.starts_with(b"GIF8") {
        return Some((u16_le(data, 6)?, u16_le(data, 8)?));
    }
    if data.starts_with(b"BM") {
        return Some((i32_le(data, 18)?, i32_le(data, 22)?));
    }
    if data.starts_with(b"RIFF") && data.get(8..12) == Some(b"WEBP") {
        return match data.get(12..16)? {
            b"VP8 " => Some((u16_le(data, 26)? & 0x3fff, u16_le(data, 28)? & 0x3fff)),
            b"VP8L" => {
                let b = data.get(21..25)?;
                let bits = u32::from_le_bytes(b.try_into().ok()?) as u64;
                Some(((bits & 0x3fff) + 1, ((bits >> 14) & 0x3fff) + 1))
            }
            b"VP8X" => Some((u24_le(data, 24)? + 1, u24_le(data, 27)? + 1)),
            _ => None,
        };
    }
    if let Some(size) = svg_dimensions(data) {
        return Some(size);
    }
    if data.starts_with(&[0xff, 0xd8]) {
        // Walk the markers to the first start of frame (SOF0–SOF15 without DHT, JPG and DAC).
        let mut at = 2;
        while at + 4 <= data.len() {
            if data[at] != 0xff {
                return None;
            }
            let marker = data[at + 1];
            if marker == 0xff {
                at += 1;
                continue;
            }
            if matches!(marker, 0xd0..=0xd9 | 0x01) {
                at += 2;
                continue;
            }
            let len = u16_be(data, at + 2)? as usize;
            if (0xc0..=0xcf).contains(&marker) && !matches!(marker, 0xc4 | 0xc8 | 0xcc) {
                return Some((u16_be(data, at + 7)?, u16_be(data, at + 5)?));
            }
            if len < 2 {
                return None;
            }
            at += 2 + len;
        }
        return None;
    }
    None
}

/// The size an SVG is drawn at: the root element's `width` and `height` (plain or `px` numbers), else its `viewBox`.
fn svg_dimensions(data: &[u8]) -> Option<(u64, u64)> {
    let head = String::from_utf8_lossy(&data[..data.len().min(64 * 1024)]);
    let start = head.find("<svg")?;
    // Attributes may be on their own lines.
    let tag = head[start..start + head[start..].find('>')?].replace(['\n', '\r', '\t'], " ");
    let attr = |name: &str| {
        let at = tag.find(&format!(" {name}="))? + name.len() + 2;
        let quote = tag[at..].chars().next()?;
        let rest = &tag[at + quote.len_utf8()..];
        Some(&rest[..rest.find(quote)?])
    };
    let number = |v: &str| v.trim().trim_end_matches("px").parse::<f64>().ok().filter(|n| n.is_finite() && *n >= 0.0);
    if let (Some(w), Some(h)) = (attr("width").and_then(number), attr("height").and_then(number)) {
        return Some((w.ceil() as u64, h.ceil() as u64));
    }
    let view: Vec<f64> = attr("viewBox")?.split([' ', ',']).filter(|p| !p.is_empty()).filter_map(|p| p.parse().ok()).collect();
    match view.as_slice() {
        [_, _, w, h] if w.is_finite() && h.is_finite() => Some((w.abs().ceil() as u64, h.abs().ceil() as u64)),
        _ => None,
    }
}

/// Whether an image this size is too large to decode for the preview.
pub fn too_large(width: u64, height: u64) -> bool {
    width.saturating_mul(height) > MAX_PIXELS
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png(w: u32, h: u32) -> Vec<u8> {
        let mut d = b"\x89PNG\r\n\x1a\n\0\0\0\x0dIHDR".to_vec();
        d.extend(w.to_be_bytes());
        d.extend(h.to_be_bytes());
        d.extend([8, 6, 0, 0, 0]);
        d
    }

    #[test]
    fn sizes_come_from_each_header() {
        assert_eq!(dimensions(&png(30_000, 30_000)), Some((30_000, 30_000)));
        assert!(too_large(30_000, 30_000) && !too_large(4_000, 3_000));
        assert_eq!(dimensions(b"GIF89a\x10\x00\x20\x00"), Some((16, 32)));
        let mut bmp = b"BM".to_vec();
        bmp.resize(18, 0);
        bmp.extend(640i32.to_le_bytes());
        bmp.extend((-480i32).to_le_bytes());
        assert_eq!(dimensions(&bmp), Some((640, 480)), "a top-down BMP has a negative height");
        let jpeg = [0xff, 0xd8, 0xff, 0xe0, 0x00, 0x04, 0, 0, 0xff, 0xc0, 0x00, 0x11, 0x08, 0x01, 0xe0, 0x02, 0x80, 0x03];
        assert_eq!(dimensions(&jpeg), Some((640, 480)));
        let mut webp = b"RIFF\0\0\0\0WEBPVP8X".to_vec();
        webp.resize(24, 0);
        webp.extend([0x3f, 0x1f, 0x00, 0xdf, 0x0e, 0x00]);
        assert_eq!(dimensions(&webp), Some((8000, 3808)));
    }

    #[test]
    fn short_and_broken_headers_are_not_read() {
        assert_eq!(dimensions(&png(1, 1)[..20]), None);
        assert_eq!(dimensions(b"GIF8"), None);
        assert_eq!(dimensions(&[0xff, 0xd8, 0xff, 0xe0, 0x00, 0x00]), None, "a zero-length segment");
        assert_eq!(dimensions(&[0xff, 0xd8, 0x00]), None);
        assert_eq!(dimensions(b"<svg/>"), None);
        assert_eq!(dimensions(br#"<?xml version="1.0"?><svg xmlns="x" width="100000" height="100000px">"#), Some((100_000, 100_000)));
        assert_eq!(dimensions(br#"<svg viewBox="0 0 64000,64000"><g/></svg>"#), Some((64_000, 64_000)));
        assert_eq!(dimensions(br#"<svg width='24' height='24'>"#), Some((24, 24)));
        assert_eq!(dimensions(b"<svg\n  width=\"9000\"\n\theight=\"9000\">"), Some((9_000, 9_000)), "attributes on their own lines");
        assert_eq!(dimensions(br#"<svg width="100%" height="1e999">"#), None, "relative and infinite sizes are not trusted");
        let mut seed = 3u64;
        for _ in 0..20_000 {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            let mut d: Vec<u8> = [&png(1, 1)[..], b"GIF89a", b"BM", b"RIFF\0\0\0\0WEBPVP8L", &[0xff, 0xd8, 0xff]][(seed % 5) as usize].to_vec();
            d.extend((0..(seed % 40)).map(|i| (seed >> (i % 50)) as u8));
            let _ = dimensions(&d);
        }
    }
}
