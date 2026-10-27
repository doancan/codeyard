//! An image's size from its header, read before the image is decoded. A small file can announce enormous dimensions (a few
//! kilobytes of PNG for a 30,000 × 30,000 image take 3.6 GB once decoded), so the preview checks the size first.

use std::sync::Arc;

/// Encoded byte limit shared by image tabs and Markdown resources.
pub const MAX_BYTES: u64 = 20 * 1024 * 1024;

/// Most pixels the preview decodes (about 200 MB of RGBA).
pub const MAX_PIXELS: u64 = 50_000_000;

/// Read and validate on a worker before handing bytes to GPUI's decoder. Checking both metadata and the
/// returned bytes handles a file that grows between stat and read. Host reads themselves are bounded by MAX_FILE.
pub fn load(host: &dyn host::Host, path: &str, format: gpui::ImageFormat) -> Result<(Arc<gpui::Image>, u64), String> {
    let too_many_bytes = || crate::i18n::t("Image exceeds 20 MB.").to_string();
    if host.stat(path).map_err(|e| e.to_string())?.size > MAX_BYTES {
        return Err(too_many_bytes());
    }
    let (data, _) = host.read(path).map_err(|e| e.to_string())?;
    let size = data.len() as u64;
    if size > MAX_BYTES {
        return Err(too_many_bytes());
    }
    let dimensions = dimensions(&data);
    if format == gpui::ImageFormat::Svg && dimensions.is_none() {
        return Err(crate::i18n::t("Image unavailable").to_string());
    }
    if let Some((w, h)) = dimensions
        && too_large(w, h)
    {
        return Err(crate::i18n::tf("This image is {0} × {1} pixels, too large to preview.", &[&w, &h]));
    }
    Ok((Arc::new(gpui::Image::from_bytes(format, data)), size))
}

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

/// Use the renderer's parser: substring/attribute scans can be fooled by comments, XML whitespace,
/// units and an incomplete header. No linked resources are read while calculating the canvas size.
fn svg_dimensions(data: &[u8]) -> Option<(u64, u64)> {
    let text = std::str::from_utf8(data).ok()?;
    let options = usvg::Options {
        image_href_resolver: usvg::ImageHrefResolver { resolve_data: Box::new(|_, _, _| None), resolve_string: Box::new(|_, _| None) },
        ..Default::default()
    };
    let size = usvg::Tree::from_str(text, &options).ok()?.size();
    Some((size.width().ceil() as u64, size.height().ceil() as u64))
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
        assert_eq!(dimensions(b"<svg/>"), Some((100, 100)), "the renderer's default canvas");
        assert_eq!(dimensions(br#"<?xml version="1.0"?><svg xmlns="http://www.w3.org/2000/svg" width="100000" height="100000px"/>"#), Some((100_000, 100_000)));
        assert_eq!(dimensions(br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 64000,64000"><g/></svg>"#), Some((64_000, 64_000)));
        assert_eq!(dimensions(br#"<svg xmlns="http://www.w3.org/2000/svg" width='24' height='24'/>"#), Some((24, 24)));
        assert_eq!(
            dimensions(b"<svg xmlns=\"http://www.w3.org/2000/svg\"\n  width=\"9000\"\n\theight=\"9000\"/>"),
            Some((9_000, 9_000)),
            "attributes on their own lines"
        );
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

    #[test]
    fn svg_sizing_matches_the_renderer_even_with_xml_whitespace_comments_and_units() {
        let data = br#"<!-- <svg width="1" height="1"> -->
            <svg xmlns="http://www.w3.org/2000/svg" width = "100000" height = "100000"/>"#;
        assert_eq!(dimensions(data), Some((100_000, 100_000)), "a fake root in a comment must not bypass the limit");
        assert_eq!(dimensions(br#"<svg xmlns="http://www.w3.org/2000/svg" width="100in" height="100in"/>"#), Some((9600, 9600)));
        assert!(too_large(9600, 9600));
        assert_eq!(dimensions(br#"<svg xmlns="http://www.w3.org/2000/svg" width="1" height="1">"#), None, "truncated XML is refused");
    }

    #[test]
    fn image_loading_rejects_encoded_files_before_reading_them() {
        let path = std::env::temp_dir().join(format!("codeyard-image-byte-limit-{}.png", std::process::id()));
        let file = std::fs::File::create(&path).unwrap();
        file.set_len(MAX_BYTES + 1).unwrap();
        let result = load(&host::local(), &path.to_string_lossy(), gpui::ImageFormat::Png);
        std::fs::remove_file(path).unwrap();
        assert_eq!(result.unwrap_err(), "Image exceeds 20 MB.");
    }
}
