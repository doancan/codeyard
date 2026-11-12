//! The image viewer through a real view over real files: what it shows for each kind of file, and its zoom, pan and keys.

use super::*;
use gpui::{ImageFormat, Modifiers, ScrollDelta, TouchPhase};

const GIF_2X2_TWO_FRAMES: &[u8] = &[
    71, 73, 70, 56, 57, 97, 2, 0, 2, 0, 129, 0, 0, 255, 255, 255, 220, 30, 30, 30, 120, 220, 20, 20, 20, 33, 249, 4, 0, 40, 0, 0, 0, 44, 0, 0, 0, 0, 2, 0, 2,
    0, 0, 2, 3, 12, 24, 176, 0, 33, 249, 4, 0, 40, 0, 0, 0, 44, 0, 0, 0, 0, 2, 0, 2, 0, 0, 2, 3, 68, 136, 176, 0, 59,
];
const SVG_400X300: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" width="400" height="300" viewBox="0 0 400 300"><rect width="400" height="300" fill="#2b7de9"/><circle cx="200" cy="150" r="80" fill="#fff" fill-opacity="0.5"/></svg>"##;

/// Removes a test's folder when the test's thread ends.
struct Cleanup(std::path::PathBuf);

impl Drop for Cleanup {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

thread_local! {
    static FOLDERS: std::cell::RefCell<Vec<Cleanup>> = const { std::cell::RefCell::new(Vec::new()) };
}

fn file(tag: &str, name: &str, data: &[u8]) -> String {
    let dir = std::env::temp_dir().join(format!("codeyard-image-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    FOLDERS.with(|f| f.borrow_mut().push(Cleanup(dir.clone())));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join(name), data).unwrap();
    crate::ui::local_path(&crate::ui::canonical_for_test(dir.join(name)))
}

fn open<'a>(cx: &'a mut gpui::TestAppContext, path: &str, format: ImageFormat) -> (gpui::Entity<LargeFileView>, &'a mut gpui::VisualTestContext) {
    crate::qa_workbench_tests::globals(cx);
    cx.update(|cx| cx.bind_keys(key_bindings()));
    let path = path.to_string();
    let (view, cx) = cx.add_window_view(move |_, cx| LargeFileView::new_image(Arc::new(host::local()), path, format, cx));
    for _ in 0..300 {
        cx.run_until_parked();
        if view.read_with(cx, |v, _| v.error.is_some() || v.image_loaded()) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    cx.run_until_parked();
    (view, cx)
}

fn label(view: &gpui::Entity<LargeFileView>, cx: &gpui::VisualTestContext) -> String {
    view.read_with(cx, |v, _| v.image_zoom_label_for_test()).unwrap_or_default()
}

fn error(view: &gpui::Entity<LargeFileView>, cx: &gpui::VisualTestContext) -> String {
    view.read_with(cx, |v, _| v.error_for_test()).unwrap_or_default()
}

#[gpui::test]
fn a_damaged_or_empty_file_says_so_instead_of_showing_nothing(cx: &mut gpui::TestAppContext) {
    let (view, cx) = open(cx, &file("corrupt", "a.png", b"\x89PNG\r\n\x1a\nthis is not a png"), ImageFormat::Png);
    assert!(!view.read_with(cx, |v, _| v.image_loaded()));
    assert!(error(&view, cx).starts_with("This PNG file could not be read"), "{}", error(&view, cx));

    let (view, cx) = open(cx, &file("empty", "a.png", b""), ImageFormat::Png);
    assert_eq!(error(&view, cx), "The file is empty.");

    let (view, cx) = open(cx, &file("badsvg", "a.svg", b"<svg><not closed"), ImageFormat::Svg);
    assert!(error(&view, cx).starts_with("This SVG file could not be read"), "{}", error(&view, cx));

    let (view, cx) = open(cx, &file("badjpg", "a.jpg", &[0xff, 0xd8, 0xff, 0xe0, 1, 2, 3]), ImageFormat::Jpeg);
    assert!(error(&view, cx).contains("JPEG"), "{}", error(&view, cx));

    let (view, cx) = open(cx, &file("text", "a.gif", b"GIF89a not really"), ImageFormat::Gif);
    assert!(error(&view, cx).contains("GIF"), "{}", error(&view, cx));
}

#[gpui::test]
fn a_missing_file_gives_the_systems_reason(cx: &mut gpui::TestAppContext) {
    let (view, cx) = open(cx, "/nonexistent-codeyard-folder/a.png", ImageFormat::Png);
    assert!(!error(&view, cx).is_empty());
}

#[gpui::test]
fn a_bomb_is_refused_before_decoding_and_still_reports_the_file(cx: &mut gpui::TestAppContext) {
    let mut png = b"\x89PNG\r\n\x1a\n\0\0\0\x0dIHDR".to_vec();
    png.extend(30_000u32.to_be_bytes());
    png.extend(30_000u32.to_be_bytes());
    png.extend([8, 6, 0, 0, 0, 0, 0, 0, 0]);
    let (view, cx) = open(cx, &file("bomb", "a.png", &png), ImageFormat::Png);
    assert!(error(&view, cx).contains("30000 × 30000"));
    assert_eq!(view.read_with(cx, |v, _| v.pic.failed.map(|f| f.1)), Some(png.len() as u64), "the info bar still knows the size");
}

#[gpui::test]
fn an_animation_too_big_to_hold_in_memory_is_refused(cx: &mut gpui::TestAppContext) {
    // 4000 × 4000 pixels per frame, 5 frames: 80 million pixels.
    let mut gif = b"GIF89a".to_vec();
    gif.extend(4000u16.to_le_bytes());
    gif.extend(4000u16.to_le_bytes());
    gif.extend([0x80, 0, 0, 0, 0, 0, 255, 255, 255]);
    for _ in 0..5 {
        gif.extend(b"\x2c\0\0\0\0\xa0\x0f\xa0\x0f\x00\x02\x01\x00\x00");
    }
    gif.push(0x3b);
    let (view, cx) = open(cx, &file("longgif", "a.gif", &gif), ImageFormat::Gif);
    assert!(error(&view, cx).contains("5 frames of 4000 × 4000 pixels, too large to play"), "{}", error(&view, cx));
}

#[gpui::test]
fn an_svg_loads_and_zooms_with_buttons_keys_wheel_and_drag(cx: &mut gpui::TestAppContext) {
    let (view, cx) = open(cx, &file("svg", "a.svg", SVG_400X300.as_bytes()), ImageFormat::Svg);
    assert!(view.read_with(cx, |v, _| v.image_loaded()), "{}", error(&view, cx));
    cx.run_until_parked();
    let viewport = view.read_with(cx, |v, _| v.viewport_size());
    assert!(viewport.0 > 0. && viewport.1 > 0., "the viewport was measured: {viewport:?}");
    assert_eq!(label(&view, cx), "100%", "400 × 300 fits the window and is not enlarged");

    // Keys, as the viewer's key context binds them.
    view.update_in(cx, |v, w, _| w.focus(&v.focus_handle));
    cx.simulate_keystrokes("=");
    assert_eq!(label(&view, cx), "150%");
    cx.simulate_keystrokes("shift-= -");
    assert_eq!(label(&view, cx), "150%");
    cx.simulate_keystrokes("0");
    assert!(view.read_with(cx, |v, _| v.image_view_for_test().is_fit()));
    cx.simulate_keystrokes("1");
    assert_eq!(label(&view, cx), "100%");

    // Wheel with the platform modifier zooms; without it, it moves the picture once it is bigger than the viewport.
    let wheel = |view: &gpui::Entity<LargeFileView>, cx: &mut gpui::VisualTestContext, dy: f32, modifiers: Modifiers| {
        let at = view.read_with(cx, |v, _| v.pic.viewport.center());
        cx.simulate_event(gpui::ScrollWheelEvent {
            position: at,
            delta: ScrollDelta::Pixels(gpui::point(px(0.), px(dy))),
            modifiers,
            touch_phase: TouchPhase::Moved,
        });
    };
    let zoom_in = Modifiers { platform: true, control: true, ..Modifiers::none() };
    wheel(&view, cx, 200., zoom_in);
    let zoomed = view.read_with(cx, |v, _| v.image_view_for_test());
    assert!(matches!(zoomed.zoom, zoom::Zoom::Scale(s) if s > 1.5), "{zoomed:?}");
    for _ in 0..8 {
        wheel(&view, cx, 300., zoom_in);
    }
    let big = view.read_with(cx, |v, _| v.image_view_for_test());
    wheel(&view, cx, 40., Modifiers::none());
    let moved = view.read_with(cx, |v, _| v.image_view_for_test());
    assert!(moved.pan.1 >= big.pan.1, "{big:?} {moved:?}");

    // Dragging moves a zoomed picture; the double click is the 100 % / fit switch.
    let start = view.read_with(cx, |v, _| v.pic.viewport.center());
    cx.simulate_mouse_down(start, MouseButton::Left, Modifiers::none());
    cx.simulate_mouse_move(start + gpui::point(px(-60.), px(-30.)), MouseButton::Left, Modifiers::none());
    cx.simulate_mouse_up(start, MouseButton::Left, Modifiers::none());
    let dragged = view.read_with(cx, |v, _| v.image_view_for_test());
    assert!(dragged.pan.0 < moved.pan.0, "{moved:?} {dragged:?}");
    assert!(view.read_with(cx, |v, _| v.pic.drag.is_none()));
}

#[gpui::test]
fn a_picture_that_fits_cannot_be_dragged_and_fit_recentres(cx: &mut gpui::TestAppContext) {
    let (view, cx) = open(cx, &file("fitdrag", "a.svg", SVG_400X300.as_bytes()), ImageFormat::Svg);
    let start = view.read_with(cx, |v, _| v.pic.viewport.center());
    cx.simulate_mouse_down(start, MouseButton::Left, Modifiers::none());
    cx.simulate_mouse_move(start + gpui::point(px(50.), px(50.)), MouseButton::Left, Modifiers::none());
    cx.simulate_mouse_up(start, MouseButton::Left, Modifiers::none());
    assert_eq!(view.read_with(cx, |v, _| v.image_view_for_test().pan), (0., 0.));
}

fn settle(view: &gpui::Entity<LargeFileView>, cx: &mut gpui::VisualTestContext, mut done: impl FnMut(&LargeFileView) -> bool) {
    for _ in 0..300 {
        cx.run_until_parked();
        if view.read_with(cx, |v, _| done(v)) {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

#[gpui::test]
fn an_svg_is_drawn_again_sharper_when_zoomed_in_and_the_blurrier_one_is_released(cx: &mut gpui::TestAppContext) {
    let (view, cx) = open(cx, &file("sharp", "a.svg", SVG_400X300.as_bytes()), ImageFormat::Svg);
    settle(&view, cx, |v| v.pic.vector_pending.is_none());
    let first = view.read_with(cx, |v, _| v.pic.vector_step);
    let before = view.read_with(cx, |v, _| v.pic.loaded.as_ref().map(|l| Arc::as_ptr(&l.picture) as usize));
    for _ in 0..3 {
        view.update_in(cx, |v, w, cx| v.image_zoom_in(&ImageZoomIn, w, cx));
    }
    settle(&view, cx, |v| v.pic.vector_step > first && v.pic.vector_pending.is_none());
    let (step, after) = view.read_with(cx, |v, _| (v.pic.vector_step, v.pic.loaded.as_ref().map(|l| Arc::as_ptr(&l.picture) as usize)));
    assert!(step > first && step >= 4, "{first} -> {step}");
    assert_ne!(before, after, "the new drawing replaced the old one, which nothing holds any more");
    // Zooming back out keeps the sharper drawing rather than drawing a blurrier one.
    view.update_in(cx, |v, w, cx| v.image_fit(&ImageFit, w, cx));
    cx.run_until_parked();
    assert_eq!(view.read_with(cx, |v, _| v.pic.vector_step), step);
}

#[gpui::test]
fn a_burst_of_zoom_events_draws_an_svg_once_at_a_time(cx: &mut gpui::TestAppContext) {
    let (view, cx) = open(cx, &file("burst", "a.svg", SVG_400X300.as_bytes()), ImageFormat::Svg);
    settle(&view, cx, |v| v.pic.vector_pending.is_none());
    let spawned = view.read_with(cx, |v, _| v.pic.vector_spawned);
    view.update_in(cx, |v, w, cx| {
        for _ in 0..4 {
            v.image_zoom_in(&ImageZoomIn, w, cx);
        }
        // 60 more events at the same zoom, while the first drawing is still being made.
        for _ in 0..60 {
            v.refresh_vector(cx);
        }
    });
    assert_eq!(view.read_with(cx, |v, _| v.pic.vector_spawned), spawned + 1, "one drawing, not one per event");
    settle(&view, cx, |v| v.pic.vector_pending.is_none());
    assert!(view.read_with(cx, |v, _| v.pic.vector_spawned) <= spawned + 4, "afterwards only what the new zoom needs");
}

/// A small SVG that is plainly red: what another SVG tries to pull in.
const RED_SVG: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" width="8" height="8"><rect width="8" height="8" fill="#ff0000"/></svg>"##;

fn base64(data: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for c in data.chunks(3) {
        let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
        for i in 0..4 {
            out.push(if i <= c.len() { T[(n >> (18 - 6 * i) & 63) as usize] as char } else { '=' });
        }
    }
    out
}

/// Whether any pixel of the first frame is visibly red (BGRA).
fn has_red(picture: &RenderImage) -> bool {
    picture.as_bytes(0).is_some_and(|b| b.chunks(4).any(|p| p[2] > 200 && p[1] < 50 && p[3] > 200))
}

// The href carries a file path into XML; Windows paths (drive letters, backslashes) are not checked here.
#[cfg(unix)]
#[gpui::test]
fn an_svg_cannot_draw_other_files_or_embedded_pictures(cx: &mut gpui::TestAppContext) {
    // The renderer follows `<image href>` to another SVG, from a file or from a data: URI. (Raster pictures are not drawn by this
    // build of the renderer, but the file is still read.) The viewer must follow neither.
    let target = file("href-target", "target.svg", RED_SVG.as_bytes());
    let by_file = format!(r#"<svg xmlns="http://www.w3.org/2000/svg" width="8" height="8"><image href="{target}" width="8" height="8"/></svg>"#);
    let by_data = format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="8" height="8"><image href="data:image/svg+xml;base64,{}" width="8" height="8"/></svg>"#,
        base64(RED_SVG.as_bytes())
    );
    crate::qa_workbench_tests::globals(cx);
    // The application's own renderer is the restricted one; an unrestricted one is built here only to prove the fixture bites.
    let (unrestricted, restricted) = cx.update(|cx| (SvgRenderer::new(cx.asset_source().clone()), cx.svg_renderer()));
    for svg in [&by_file, &by_data] {
        let host = host::local();
        let path = file("href", "a.svg", svg.as_bytes());
        // A renderer that follows hrefs draws it: the fixture really pulls a picture in, so the check means something.
        let drawn = read(&host, &path, ImageFormat::Svg, unrestricted.clone()).ok().unwrap();
        assert!(has_red(&drawn.picture), "the fixture draws a red pixel with a renderer that follows hrefs");
        let safe = read(&host, &path, ImageFormat::Svg, restricted.clone()).ok().unwrap();
        assert!(!has_red(&safe.picture), "the application's renderer loads no picture an SVG points to");
        assert_eq!(safe.size, (8., 8.), "the SVG itself still draws");
    }
    // Through the whole view, including the sharper drawings made when zooming.
    let (view, cx) = open(cx, &file("href-view", "a.svg", by_file.as_bytes()), ImageFormat::Svg);
    for _ in 0..3 {
        view.update_in(cx, |v, w, cx| v.image_zoom_in(&ImageZoomIn, w, cx));
    }
    settle(&view, cx, |v| v.pic.vector_pending.is_none());
    assert!(view.read_with(cx, |v, _| v.pic.loaded.as_ref().is_some_and(|l| !has_red(&l.picture))));
}

#[gpui::test]
fn a_side_the_graphics_card_cannot_hold_is_refused_with_a_message(cx: &mut gpui::TestAppContext) {
    // 40000 x 1250 is only 50 million pixels, but no texture is 40000 wide.
    let mut png = b"\x89PNG\r\n\x1a\n\0\0\0\x0dIHDR".to_vec();
    png.extend(40_000u32.to_be_bytes());
    png.extend(1_250u32.to_be_bytes());
    png.extend([8, 2, 0, 0, 0, 0, 0, 0, 0]);
    let (view, cx) = open(cx, &file("wide-png", "a.png", &png), ImageFormat::Png);
    assert!(!view.read_with(cx, |v, _| v.image_loaded()));
    assert!(error(&view, cx).contains("40000 × 1250 pixels, too large to preview"), "{}", error(&view, cx));

    let svg = r#"<svg xmlns="http://www.w3.org/2000/svg" width="20000" height="100"><rect width="20000" height="100"/></svg>"#;
    let (view, cx) = open(cx, &file("wide-svg", "a.svg", svg.as_bytes()), ImageFormat::Svg);
    assert!(error(&view, cx).contains("20000 × 100 pixels, too large to preview"), "{}", error(&view, cx));

    let ok = r#"<svg xmlns="http://www.w3.org/2000/svg" width="16000" height="100"><rect width="16000" height="100"/></svg>"#;
    let (view, cx) = open(cx, &file("ok-svg", "a.svg", ok.as_bytes()), ImageFormat::Svg);
    assert!(view.read_with(cx, |v, _| v.image_loaded()), "{}", error(&view, cx));
    // Drawing it sharper would pass the side limit, so it stays at 1x.
    assert_eq!(view.read_with(cx, |v, _| v.wanted_vector_scale()), Some(1));
}

#[gpui::test]
fn an_animated_gif_plays_and_space_pauses_it(cx: &mut gpui::TestAppContext) {
    let (view, cx) = open(cx, &file("gif", "a.gif", GIF_2X2_TWO_FRAMES), ImageFormat::Gif);
    assert!(view.read_with(cx, |v, _| v.image_loaded()), "{}", error(&view, cx));
    assert!(view.read_with(cx, |v, _| v.pic.loaded.as_ref().is_some_and(|l| l.motion == Motion::Frames(2))));
    assert!(!view.read_with(cx, |v, cx| v.is_paused(cx)));
    assert!(view.read_with(cx, |v, _| v.pic.loaded.as_ref().is_some_and(|l| l.still.is_none())), "a playing animation keeps no copy of its first frame");
    view.update_in(cx, |v, w, _| w.focus(&v.focus_handle));
    cx.simulate_keystrokes("space");
    assert!(view.read_with(cx, |v, cx| v.is_paused(cx)));
    assert!(view.read_with(cx, |v, _| v.pic.loaded.as_ref().is_some_and(|l| l.still.is_some())), "paused: the first frame is shown");
    cx.simulate_keystrokes("space");
    assert!(!view.read_with(cx, |v, cx| v.is_paused(cx)));
    assert!(view.read_with(cx, |v, _| v.pic.loaded.as_ref().is_some_and(|l| l.still.is_none())), "playing again lets the copy go");
}

#[gpui::test]
fn reduced_motion_pauses_and_resumes_a_playing_animation_when_it_changes(cx: &mut gpui::TestAppContext) {
    let (view, cx) = open(cx, &file("gif-motion", "a.gif", GIF_2X2_TWO_FRAMES), ImageFormat::Gif);
    assert!(!view.read_with(cx, |v, cx| v.is_paused(cx)));
    cx.update(|_, cx| cx.update_global::<Settings, _>(|s, _| s.reduce_motion = Some(true)));
    cx.run_until_parked();
    assert!(view.read_with(cx, |v, cx| v.is_paused(cx)), "the setting changed while it played");
    assert!(view.read_with(cx, |v, _| v.pic.loaded.as_ref().is_some_and(|l| l.still.is_some())), "and the next frame drew the still");
    cx.update(|_, cx| cx.update_global::<Settings, _>(|s, _| s.reduce_motion = Some(false)));
    cx.run_until_parked();
    assert!(!view.read_with(cx, |v, cx| v.is_paused(cx)));
    // A choice made with Space wins over the setting.
    view.update_in(cx, |v, w, cx| v.image_toggle_play(&ImageTogglePlay, w, cx));
    cx.update(|_, cx| cx.update_global::<Settings, _>(|s, _| s.reduce_motion = Some(false)));
    assert!(view.read_with(cx, |v, cx| v.is_paused(cx)));
}

#[gpui::test]
fn a_still_picture_ignores_space(cx: &mut gpui::TestAppContext) {
    let (view, cx) = open(cx, &file("still", "a.svg", SVG_400X300.as_bytes()), ImageFormat::Svg);
    view.update_in(cx, |v, w, _| w.focus(&v.focus_handle));
    cx.simulate_keystrokes("space");
    assert!(!view.read_with(cx, |v, cx| v.is_paused(cx)));
}

#[gpui::test]
fn the_summary_names_size_format_and_bytes(cx: &mut gpui::TestAppContext) {
    let (view, cx) = open(cx, &file("info", "a.svg", SVG_400X300.as_bytes()), ImageFormat::Svg);
    let description = view.read_with(cx, |v, _| v.image_description());
    assert!(description.starts_with("400 × 300 pixels, SVG, "), "{description}");
}
