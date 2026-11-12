//! What the image viewer says about a file, read from its bytes without decoding it: the format's name, the size for people, and
//! whether it is animated (and how many frames a decoder would have to keep in memory). Every reader is bounded by the data it
//! is given and gives up on a header that does not add up. No UI.

use gpui::ImageFormat;

/// Frames counted at most: a file claiming more is not played.
pub const MAX_FRAMES: u32 = 100_000;

/// How a picture moves.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Motion {
    Still,
    /// Plays (GIF, animated WebP) with this many frames.
    Frames(u32),
    /// An animated PNG: only its first frame is drawn.
    FirstFrameOnly,
}

pub fn format_name(format: ImageFormat) -> &'static str {
    match format {
        ImageFormat::Png => "PNG",
        ImageFormat::Jpeg => "JPEG",
        ImageFormat::Gif => "GIF",
        ImageFormat::Webp => "WebP",
        ImageFormat::Svg => "SVG",
        ImageFormat::Bmp => "BMP",
        ImageFormat::Tiff => "TIFF",
    }
}

/// "812 B", "5.4 KB", "1.2 MB".
pub fn size_label(bytes: u64) -> String {
    const KB: f64 = 1024.;
    let b = bytes as f64;
    if b < KB {
        format!("{bytes} B")
    } else if b < KB * KB {
        format!("{:.1} KB", b / KB)
    } else {
        format!("{:.1} MB", b / (KB * KB))
    }
}

pub fn motion(data: &[u8], format: ImageFormat) -> Motion {
    match format {
        ImageFormat::Gif => match gif_frames(data) {
            n if n > 1 => Motion::Frames(n),
            _ => Motion::Still,
        },
        ImageFormat::Webp => match webp_frames(data) {
            n if n > 1 => Motion::Frames(n),
            _ => Motion::Still,
        },
        ImageFormat::Png if is_animated_png(data) => Motion::FirstFrameOnly,
        _ => Motion::Still,
    }
}

/// Pixels a decoder keeps for `frames` frames of `width` × `height` (4 bytes each), as a count of pixels.
pub fn animated_pixels(frames: u32, width: u64, height: u64) -> u64 {
    (frames as u64).saturating_mul(width).saturating_mul(height)
}

/// Frames of a GIF: the image descriptors between the header and the trailer.
pub fn gif_frames(data: &[u8]) -> u32 {
    if !(data.starts_with(b"GIF87a") || data.starts_with(b"GIF89a")) {
        return 0;
    }
    let Some(&flags) = data.get(10) else { return 0 };
    let mut at = 13 + color_table_len(flags);
    let mut frames = 0;
    while let Some(&block) = data.get(at) {
        match block {
            0x3b => break,
            0x21 => match skip_sub_blocks(data, at + 2) {
                Some(next) => at = next,
                None => break,
            },
            0x2c => {
                let Some(&local) = data.get(at + 9) else { break };
                // Descriptor (10 bytes), local colour table, LZW minimum code size (1 byte), then the data.
                match skip_sub_blocks(data, at + 10 + color_table_len(local) + 1) {
                    Some(next) => at = next,
                    None => break,
                }
                frames += 1;
                if frames >= MAX_FRAMES {
                    break;
                }
            }
            _ => break,
        }
    }
    frames
}

/// Bytes of a colour table announced by a packed flags byte (0 when there is none).
fn color_table_len(flags: u8) -> usize {
    if flags & 0x80 == 0 { 0 } else { 3 << ((flags & 0x07) + 1) }
}

/// The offset after the data sub-blocks that start at `at` (each is a length byte and that many bytes; a zero ends them).
fn skip_sub_blocks(data: &[u8], mut at: usize) -> Option<usize> {
    loop {
        let len = *data.get(at)? as usize;
        at += 1 + len;
        if len == 0 {
            return (at <= data.len()).then_some(at);
        }
        if at > data.len() {
            return None;
        }
    }
}

/// Frames of an animated WebP: its `ANMF` chunks; 0 for a still image.
pub fn webp_frames(data: &[u8]) -> u32 {
    if !(data.starts_with(b"RIFF") && data.get(8..12) == Some(b"WEBP")) {
        return 0;
    }
    let mut at = 12;
    let mut frames = 0;
    while let Some(head) = data.get(at..at + 8) {
        let size = u32::from_le_bytes([head[4], head[5], head[6], head[7]]) as usize;
        if &head[..4] == b"ANMF" {
            frames += 1;
            if frames >= MAX_FRAMES {
                break;
            }
        }
        // Chunks are padded to an even length.
        let Some(next) = at.checked_add(8).and_then(|n| n.checked_add(size)).and_then(|n| n.checked_add(size & 1)) else { break };
        at = next;
    }
    frames
}

/// Whether a PNG carries an animation control chunk (`acTL`) before its image data.
pub fn is_animated_png(data: &[u8]) -> bool {
    if !data.starts_with(b"\x89PNG\r\n\x1a\n") {
        return false;
    }
    let mut at = 8;
    while let Some(head) = data.get(at..at + 8) {
        match &head[4..8] {
            b"acTL" => return true,
            b"IDAT" | b"IEND" => return false,
            _ => {}
        }
        let size = u32::from_be_bytes([head[0], head[1], head[2], head[3]]) as usize;
        // length + type + data + CRC
        let Some(next) = at.checked_add(12).and_then(|n| n.checked_add(size)) else { return false };
        at = next;
    }
    false
}

#[cfg(test)]
#[path = "large_image_info_tests.rs"]
mod tests;
