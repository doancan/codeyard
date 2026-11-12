//! A README is not trusted: an SVG picture in it must not make the app read other files or decode huge embedded pictures, and
//! the raster pictures next to the document keep working.

use gpui::{ImageFormat, TestAppContext};

use crate::qa_workbench_tests::{open, project, setup};

const RED_SVG: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" width="8" height="8"><rect width="8" height="8" fill="#ff0000"/></svg>"##;

/// A 1x1 transparent PNG.
const PNG_1X1: &[u8] = &[
    0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44, 0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06,
    0x00, 0x00, 0x00, 0x1F, 0x15, 0xC4, 0x89, 0x00, 0x00, 0x00, 0x0B, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9C, 0x63, 0x60, 0x00, 0x02, 0x00, 0x00, 0x05, 0x00, 0x01,
    0x7A, 0x5E, 0xAB, 0x3F, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82,
];

fn base64(data: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in data.chunks(3) {
        let n = chunk.iter().enumerate().fold(0u32, |n, (i, b)| n | u32::from(*b) << (16 - 8 * i));
        for i in 0..chunk.len() + 1 {
            out.push(T[(n >> (18 - 6 * i) & 63) as usize] as char);
        }
    }
    out
}

fn has_red(picture: &gpui::RenderImage) -> bool {
    picture.as_bytes(0).is_some_and(|b| b.chunks(4).any(|p| p[2] > 200 && p[1] < 50 && p[3] > 200))
}

fn hostile_svgs(dir: &std::path::Path) -> Vec<(&'static str, String)> {
    std::fs::write(dir.join("secret.svg"), RED_SVG).unwrap();
    let secret = crate::ui::local_path(&dir.join("secret.svg"));
    let wrap = |inner: String| format!(r#"<svg xmlns="http://www.w3.org/2000/svg" width="8" height="8">{inner}</svg>"#);
    vec![
        ("by-file.svg", wrap(format!(r#"<image href="{secret}" width="8" height="8"/>"#))),
        ("by-data.svg", wrap(format!(r#"<image href="data:image/svg+xml;base64,{}" width="8" height="8"/>"#, base64(RED_SVG.as_bytes())))),
    ]
}

#[cfg(unix)]
#[gpui::test]
fn a_markdown_svg_cannot_pull_in_a_local_file_or_a_data_picture(cx: &mut TestAppContext) {
    let dir = project("md-hostile-svg");
    let mut doc = String::new();
    for (name, svg) in hostile_svgs(&dir) {
        std::fs::write(dir.join(name), svg).unwrap();
        doc.push_str(&format!("![{name}]({name})\n\n"));
    }
    std::fs::write(dir.join("pixel.png"), PNG_1X1).unwrap();
    doc.push_str("![raster](pixel.png)\n");
    std::fs::write(dir.join("doc.md"), doc).unwrap();
    let (wb, cx) = setup(cx);
    open(&wb, &dir.join("doc.md"), cx);
    wb.update_in(cx, |wb, window, cx| wb.run_command_for_test("Markdown: Open Preview", window, cx));
    cx.run_until_parked();
    let images = wb.read_with(cx, |wb, cx| wb.markdown_images_for_test(cx)).unwrap();
    for url in ["by-file.svg", "by-data.svg", "pixel.png"] {
        assert_eq!(images[url], Ok(()), "{url}: the picture itself still loads: {images:?}");
    }
    // The renderer the preview draws with (the application's shared one) follows no href inside an SVG...
    cx.update(|_, cx| {
        for url in ["by-file.svg", "by-data.svg"] {
            let bytes = std::fs::read(dir.join(url)).unwrap();
            let drawn = gpui::Image::from_bytes(ImageFormat::Svg, bytes).to_image_data(cx.svg_renderer()).expect("the SVG itself draws");
            assert!(!has_red(&drawn), "{url}: nothing was pulled in");
            // ...while a renderer that does follow them would have: the fixture bites.
            let bytes = std::fs::read(dir.join(url)).unwrap();
            let open = gpui::Image::from_bytes(ImageFormat::Svg, bytes).to_image_data(gpui::SvgRenderer::new(cx.asset_source().clone())).unwrap();
            assert!(has_red(&open), "{url}: the fixture pulls a picture in when hrefs are followed");
        }
        // Raster pictures next to the document are not affected.
        let raster = gpui::Image::from_bytes(ImageFormat::Png, PNG_1X1.to_vec()).to_image_data(cx.svg_renderer()).expect("a PNG draws");
        assert_eq!(raster.size(0).width.0, 1);
    });
}

/// A 4x4 solid red PNG.
const RED_PNG: &[u8] = &[
    0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44, 0x52, 0x00, 0x00, 0x00, 0x04, 0x00, 0x00, 0x00, 0x04, 0x08, 0x06,
    0x00, 0x00, 0x00, 0xA9, 0xF1, 0x9E, 0x7E, 0x00, 0x00, 0x00, 0x12, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9C, 0x63, 0xF8, 0xCF, 0xC0, 0xF0, 0x1F, 0x19, 0x33, 0x90,
    0x2E, 0x00, 0x00, 0x3C, 0x40, 0x1F, 0xE1, 0xE0, 0x81, 0x77, 0xB4, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82,
];

fn svg_with_image(href: &str) -> Vec<u8> {
    format!(r#"<svg xmlns="http://www.w3.org/2000/svg" width="8" height="8"><image href="{href}" width="8" height="8"/></svg>"#).into_bytes()
}

/// Draw `svg` with the application's renderer; the time it took is bounded.
fn draw(cx: &mut TestAppContext, svg: Vec<u8>) -> std::sync::Arc<gpui::RenderImage> {
    let started = std::time::Instant::now();
    let drawn = cx.update(|cx| gpui::Image::from_bytes(ImageFormat::Svg, svg).to_image_data(cx.svg_renderer()).expect("the SVG itself draws"));
    assert!(started.elapsed() < std::time::Duration::from_secs(5), "drawing took {:?}", started.elapsed());
    drawn
}

#[gpui::test]
fn a_markdown_svg_with_a_huge_embedded_picture_is_never_decoded(cx: &mut TestAppContext) {
    // A PNG header that announces 30000 x 30000 pixels inside a data: URI: decoding it would need gigabytes. The renderer's
    // loaders are switched off, so nothing is decoded and the SVG itself is drawn at its own size, in a fraction of a second.
    let mut png = b"\x89PNG\r\n\x1a\n\0\0\0\x0dIHDR".to_vec();
    png.extend(30_000u32.to_be_bytes());
    png.extend(30_000u32.to_be_bytes());
    png.extend([8, 6, 0, 0, 0, 0, 0, 0, 0]);
    for mime in ["image/png", "image/jpeg", "image/svg+xml"] {
        let drawn = draw(cx, svg_with_image(&format!("data:{mime};base64,{}", base64(&png))));
        assert_eq!((drawn.size(0).width.0, drawn.size(0).height.0), (8, 8), "{mime}");
        assert!(!has_red(&drawn));
    }
}

/// Raster pictures inside an SVG are not drawn by this build of the renderer at all, and the restricted renderer does not even
/// read them: an `<image>` in an SVG renders blank, from a file, a `file://` address or a `data:` address alike.
#[cfg(unix)]
#[gpui::test]
fn a_markdown_svg_draws_no_red_png_from_a_file_address_or_a_data_address(cx: &mut TestAppContext) {
    let dir = project("md-red-png");
    std::fs::write(dir.join("red.png"), RED_PNG).unwrap();
    let path = crate::ui::local_path(&dir.join("red.png"));
    for href in [format!("file://{path}"), path.clone(), "red.png".to_string(), format!("data:image/png;base64,{}", base64(RED_PNG))] {
        let drawn = draw(cx, svg_with_image(&href));
        assert!(!has_red(&drawn), "{href}: no red pixel");
    }
}
