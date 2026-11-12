use super::*;

/// A GIF with `n` frames of 1 × 1 pixel (no colour tables beyond the global one).
fn gif(n: usize) -> Vec<u8> {
    let mut g = b"GIF89a".to_vec();
    g.extend([1, 0, 1, 0, 0x80, 0, 0]); // screen 1 x 1, a global table of 2 colours
    g.extend([0, 0, 0, 255, 255, 255]);
    g.extend(b"\x21\xff\x0bNETSCAPE2.0\x03\x01\x00\x00\x00");
    for _ in 0..n {
        g.extend(b"\x21\xf9\x04\x00\x0a\x00\x00\x00");
        g.extend(b"\x2c\x00\x00\x00\x00\x01\x00\x01\x00\x00\x02\x02\x44\x01\x00");
    }
    g.push(0x3b);
    g
}

fn png_with(chunks: &[(&[u8; 4], usize)]) -> Vec<u8> {
    let mut p = b"\x89PNG\r\n\x1a\n".to_vec();
    for (kind, len) in chunks {
        p.extend((*len as u32).to_be_bytes());
        p.extend(*kind);
        p.extend(vec![0; *len]);
        p.extend([0; 4]);
    }
    p
}

fn riff_webp(chunks: &[(&[u8; 4], usize)]) -> Vec<u8> {
    let mut w = b"RIFF\0\0\0\0WEBP".to_vec();
    for (kind, len) in chunks {
        w.extend(*kind);
        w.extend((*len as u32).to_le_bytes());
        w.extend(vec![0; *len + len % 2]);
    }
    w
}

#[test]
fn a_gif_with_one_frame_is_still_and_with_several_is_animated() {
    assert_eq!(gif_frames(&gif(1)), 1);
    assert_eq!(gif_frames(&gif(5)), 5);
    assert_eq!(motion(&gif(1), ImageFormat::Gif), Motion::Still);
    assert_eq!(motion(&gif(3), ImageFormat::Gif), Motion::Frames(3));
}

#[test]
fn a_cut_gif_counts_the_frames_it_has() {
    let g = gif(4);
    for cut in [0, 5, 12, 13, 20, g.len() / 2, g.len() - 1] {
        assert!(gif_frames(&g[..cut]) <= 4, "cut at {cut}");
    }
    assert_eq!(gif_frames(&g[..g.len() - 1]), 4, "a missing trailer does not hide the frames");
}

#[test]
fn a_gif_whose_blocks_run_past_the_end_stops() {
    let mut g = gif(1);
    let at = g.len() - 6;
    g[at] = 200; // a data block longer than the file
    assert!(gif_frames(&g) <= 1);
    assert_eq!(gif_frames(b"GIF89a"), 0);
    assert_eq!(gif_frames(b"not a gif at all"), 0);
    assert_eq!(gif_frames(&[]), 0);
}

#[test]
fn a_gif_of_a_hundred_thousand_frames_is_counted_up_to_a_limit() {
    let g = gif(MAX_FRAMES as usize + 50);
    assert_eq!(gif_frames(&g), MAX_FRAMES);
}

#[test]
fn an_animated_png_has_an_animation_chunk_before_its_data() {
    assert!(is_animated_png(&png_with(&[(b"IHDR", 13), (b"acTL", 8), (b"IDAT", 4), (b"IEND", 0)])));
    assert!(!is_animated_png(&png_with(&[(b"IHDR", 13), (b"IDAT", 4), (b"acTL", 8), (b"IEND", 0)])), "after the data it is not an animation header");
    assert!(!is_animated_png(&png_with(&[(b"IHDR", 13), (b"IDAT", 4), (b"IEND", 0)])));
    assert_eq!(motion(&png_with(&[(b"IHDR", 13), (b"acTL", 8), (b"IDAT", 4)]), ImageFormat::Png), Motion::FirstFrameOnly);
}

#[test]
fn a_png_with_a_wild_chunk_length_is_not_followed() {
    let mut p = b"\x89PNG\r\n\x1a\n".to_vec();
    p.extend(u32::MAX.to_be_bytes());
    p.extend(b"abcd");
    assert!(!is_animated_png(&p));
    assert!(!is_animated_png(b"\x89PNG\r\n\x1a\n"));
    assert!(!is_animated_png(b"plain text"));
}

#[test]
fn a_webp_counts_its_frame_chunks() {
    let still = riff_webp(&[(b"VP8X", 10), (b"VP8 ", 5)]);
    assert_eq!(webp_frames(&still), 0);
    assert_eq!(motion(&still, ImageFormat::Webp), Motion::Still);
    let moving = riff_webp(&[(b"VP8X", 10), (b"ANIM", 6), (b"ANMF", 17), (b"ANMF", 16), (b"ANMF", 3)]);
    assert_eq!(webp_frames(&moving), 3, "odd lengths are padded");
    assert_eq!(motion(&moving, ImageFormat::Webp), Motion::Frames(3));
    let mut hostile = riff_webp(&[(b"VP8X", 10)]);
    hostile.extend(b"ANMF");
    hostile.extend(u32::MAX.to_le_bytes());
    assert_eq!(webp_frames(&hostile), 1);
    assert_eq!(webp_frames(b"RIFF"), 0);
}

#[test]
fn other_formats_do_not_move() {
    assert_eq!(motion(b"\xff\xd8\xff", ImageFormat::Jpeg), Motion::Still);
    assert_eq!(motion(b"<svg/>", ImageFormat::Svg), Motion::Still);
    assert_eq!(motion(b"BM", ImageFormat::Bmp), Motion::Still);
}

#[test]
fn sizes_are_written_for_people() {
    assert_eq!(size_label(0), "0 B");
    assert_eq!(size_label(812), "812 B");
    assert_eq!(size_label(5530), "5.4 KB");
    assert_eq!(size_label(1_258_291), "1.2 MB");
    assert_eq!(size_label(u64::MAX), format!("{:.1} MB", u64::MAX as f64 / 1048576.));
}

#[test]
fn the_pixels_of_an_animation_cannot_overflow() {
    assert_eq!(animated_pixels(10, 100, 100), 100_000);
    assert_eq!(animated_pixels(u32::MAX, u64::MAX, u64::MAX), u64::MAX);
}

#[test]
fn every_format_has_a_name() {
    assert_eq!(format_name(ImageFormat::Webp), "WebP");
    assert_eq!(format_name(ImageFormat::Jpeg), "JPEG");
}
