//! A changed image or binary file in Source Control's diff (#271): what kind a file is, the numbers under each side, the limits
//! that refuse before decoding, and the whole flow in a real repository (stage, unstage, added, deleted, stale requests).

use gpui::{Entity, TestAppContext, VisualTestContext};

use super::*;
use crate::qa_integration_tests::{changes, git_in, repo, wait_ticking};
use crate::qa_workbench_tests::{open, setup};
use crate::workbench::Workbench;

fn png(width: u32, height: u32, rgba: [u8; 4]) -> Vec<u8> {
    let mut out = Vec::new();
    image::RgbaImage::from_pixel(width, height, image::Rgba(rgba)).write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png).unwrap();
    out
}

const LFS_POINTER: &str =
    "version https://git-lfs.github.com/spec/v1\noid sha256:4d7a214614ab2935c943f9e0ff69d22eadbb8f32b1258daaa5e2ca24d17e2393\nsize 12345\n";

fn side(bytes: &[u8]) -> Fetched {
    Ok(Some(bytes.to_vec()))
}

#[test]
fn a_name_that_says_image_is_believed_unless_every_side_is_text() {
    let (a, b) = (png(2, 2, [255, 0, 0, 255]), png(3, 3, [0, 0, 255, 255]));
    assert_eq!(classify("logo.png", [&side(&a), &side(&b)]), Kind::Image(ImageFormat::Png));
    assert_eq!(classify("logo.png", [&Ok(None), &side(&b)]), Kind::Image(ImageFormat::Png), "an added image");
    assert_eq!(classify("logo.png", [&side(&a), &Ok(None)]), Kind::Image(ImageFormat::Png), "a deleted image");
    assert_eq!(classify("logo.png", [&side(b"garbage \x89 not utf8"), &side(&b)]), Kind::Image(ImageFormat::Png), "a damaged image is still an image");
    assert_eq!(classify("photo.JPG", [&side(&a), &side(&a)]), Kind::Image(ImageFormat::Jpeg), "the extension's case does not matter");
    assert_eq!(
        classify("logo.png", [&side(LFS_POINTER.as_bytes()), &side(LFS_POINTER.replace("12345", "99").as_bytes())]),
        Kind::Text,
        "an LFS pointer is text"
    );
    assert_eq!(classify("logo.png", [&side(LFS_POINTER.as_bytes()), &side(&b)]), Kind::Image(ImageFormat::Png), "a pointer that became a picture");
}

#[test]
fn an_svg_is_an_image_only_when_it_is_one() {
    let svg = br#"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10"><rect width="10" height="10"/></svg>"#;
    assert_eq!(classify("icon.svg", [&side(svg), &side(svg)]), Kind::Image(ImageFormat::Svg));
    assert_eq!(classify("icon.svg", [&side(b"just some notes"), &side(b"more notes")]), Kind::Text);
}

#[test]
fn a_nul_byte_makes_a_file_binary_whatever_it_is_called() {
    assert_eq!(classify("data.bin", [&side(b"\0\x01\x02"), &side(b"\0\x01\x02\x03")]), Kind::Binary);
    assert_eq!(classify("notes.txt", [&side(b"valid utf8 but\0with a nul"), &side(b"x")]), Kind::Binary, "NUL in otherwise valid UTF-8");
    assert_eq!(classify("data.bin", [&Ok(None), &side(b"\0")]), Kind::Binary, "an added binary file");
    assert_eq!(classify("main.rs", [&side(b"fn main() {}\n"), &side(b"fn main() { }\n")]), Kind::Text);
    assert_eq!(classify("latin1.txt", [&side(b"caf\xe9\n"), &side(b"caf\xe9!\n")]), Kind::Text, "text in another encoding stays a text diff");
}

#[test]
fn the_size_line_and_the_binary_line_say_what_changed() {
    let _en = crate::i18n::LangGuard::set("en");
    let picture = |w: u32, h: u32, bytes: u64| {
        Side::Picture(Picture {
            render: Arc::new(RenderImage::new([image::Frame::new(image::RgbaImage::new(w.max(1), h.max(1)))])),
            width: w,
            height: h,
            bytes,
        })
    };
    assert_eq!(describe(&picture(1280, 800, 219_136)), "1280 × 800 · 214.0 KB");
    assert_eq!(describe(&Side::Absent), "(none)");
    assert_eq!(describe(&Side::Failed("Image exceeds 20 MB.".into())), "Image exceeds 20 MB.");
    assert_eq!(binary_line(&Side::Binary(5), &Side::Binary(2048)), "Binary file changed (5 B → 2.0 KB)");
    assert_eq!(binary_line(&Side::Absent, &Side::Binary(5)), "Binary file added (5 B)");
    assert_eq!(binary_line(&Side::Binary(5), &Side::Absent), "Binary file deleted (5 B)");
}

fn renderer(cx: &mut TestAppContext) -> SvgRenderer {
    cx.update(|cx| cx.svg_renderer().restricted())
}

#[gpui::test]
fn what_cannot_be_decoded_is_refused_with_a_reason_before_the_decoder_runs(cx: &mut TestAppContext) {
    let _en = crate::i18n::LangGuard::set("en");
    let svg = renderer(cx);
    let why = |side: Side| match side {
        Side::Failed(why) => why,
        other => panic!("expected a refusal, got {}", describe(&other)),
    };
    assert!(why(decode(vec![0; (image_size::MAX_BYTES + 1) as usize], ImageFormat::Png, &svg)).contains("20 MB"));
    assert_eq!(why(decode(Vec::new(), ImageFormat::Png, &svg)), "The file is empty.");
    assert!(why(decode(b"\x89PNG not really".to_vec(), ImageFormat::Png, &svg)).contains("could not be read"), "a damaged image says so");
    // A PNG header that announces 30000 × 30000 pixels: a few bytes that would need gigabytes decoded.
    let mut huge = png(1, 1, [0, 0, 0, 255]);
    huge[16..20].copy_from_slice(&30_000u32.to_be_bytes());
    huge[20..24].copy_from_slice(&30_000u32.to_be_bytes());
    assert!(why(decode(huge, ImageFormat::Png, &svg)).contains("30000 × 30000"), "refused from the header");
    match decode(png(4, 2, [1, 2, 3, 255]), ImageFormat::Png, &svg) {
        Side::Picture(p) => assert_eq!((p.width, p.height), (4, 2)),
        other => panic!("a good PNG decodes: {}", describe(&other)),
    }
}

#[gpui::test]
fn the_same_pixels_in_different_files_are_a_metadata_only_change(cx: &mut TestAppContext) {
    let svg = renderer(cx);
    let pixels = |compression| {
        let mut out = Vec::new();
        let image = image::RgbaImage::from_pixel(8, 8, image::Rgba([10, 20, 30, 255]));
        image::codecs::png::PngEncoder::new_with_quality(&mut out, compression, image::codecs::png::FilterType::NoFilter)
            .write_image(image.as_raw(), 8, 8, image::ExtendedColorType::Rgba8)
            .unwrap();
        out
    };
    use image::ImageEncoder as _;
    let (fast, best) = (pixels(image::codecs::png::CompressionType::Fast), pixels(image::codecs::png::CompressionType::Best));
    let a = decode(fast.clone(), ImageFormat::Png, &svg);
    let b = decode(best.clone(), ImageFormat::Png, &svg);
    let (Side::Picture(a), Side::Picture(b)) = (a, b) else { panic!("both decode") };
    assert_eq!(same_pixels(&a.render, &b.render), true);
    let Side::Picture(other) = decode(png(8, 8, [10, 20, 31, 255]), ImageFormat::Png, &svg) else { panic!() };
    assert!(!same_pixels(&a.render, &other.render), "one channel differs");
}

// --- in a real repository -------------------------------------------------------------------------------------------------

fn started<'a>(
    name: &str,
    cx: &'a mut TestAppContext,
    prepare: impl FnOnce(&std::path::Path),
) -> (std::path::PathBuf, Entity<Workbench>, &'a mut VisualTestContext) {
    let _en = crate::i18n::LangGuard::set("en");
    let dir = repo(name);
    std::fs::write(dir.join("logo.png"), png(4, 2, [255, 0, 0, 255])).unwrap();
    std::fs::write(dir.join("data.bin"), b"\0\x01\x02\x03\x04").unwrap();
    std::fs::write(dir.join("pointer.png"), LFS_POINTER).unwrap();
    git_in(&dir, &["add", "-A"]);
    git_in(&dir, &["commit", "-q", "-m", "media"]);
    prepare(&dir);
    let (wb, cx) = setup(cx);
    open(&wb, &dir, cx);
    wait_ticking(cx, "status", |cx| changes(&wb, cx).is_some_and(|c| !c.is_empty()));
    (dir, wb, cx)
}

fn show(wb: &Entity<Workbench>, rel: &str, staged: bool, cx: &mut VisualTestContext) -> Entity<crate::diff::DiffView> {
    let rel = rel.to_string();
    wb.update_in(cx, |wb, window, cx| wb.open_diff(rel.clone(), staged, window, cx));
    wait_ticking(cx, "the diff", |cx| wb.read_with(cx, |wb, app| wb.diff_view().is_some_and(|v| v.read(app).path == rel)));
    cx.run_until_parked();
    wb.read_with(cx, |wb, _| wb.diff_view()).unwrap()
}

fn picture_size(side: &Side) -> Option<(u32, u32)> {
    match side {
        Side::Picture(p) => Some((p.width, p.height)),
        _ => None,
    }
}

#[gpui::test]
fn ac1_a_modified_png_shows_both_versions_with_their_sizes(cx: &mut TestAppContext) {
    let (_dir, wb, cx) = started("media-modified", cx, |dir| std::fs::write(dir.join("logo.png"), png(6, 3, [0, 0, 255, 255])).unwrap());
    let view = show(&wb, "logo.png", false, cx);
    let media = view.read_with(cx, |v, _| v.media.clone()).expect("a changed image is not a text diff");
    assert!(media.image && !media.same_pixels);
    assert_eq!((picture_size(&media.old), picture_size(&media.new)), (Some((4, 2)), Some((6, 3))));
    assert!(cx.debug_bounds("diff-image-old").is_some() && cx.debug_bounds("diff-image-new").is_some(), "both versions are drawn");
    // The accessibility tree names both, with their sizes.
    let tree = wb.update_in(cx, |wb, window, cx| wb.a11y_snapshot(window, cx)).to_tree();
    let labels: Vec<String> =
        tree.nodes.iter().filter(|(_, n)| n.role() == accesskit::Role::Image).filter_map(|(_, n)| n.label().map(str::to_string)).collect();
    assert!(labels.iter().any(|l| l.starts_with("Index: 4 × 2 · ")), "{labels:?}");
    assert!(labels.iter().any(|l| l.starts_with("Working Tree: 6 × 3 · ")), "{labels:?}");
}

#[gpui::test]
fn the_staged_comparison_reads_head_and_the_index(cx: &mut TestAppContext) {
    let (_dir, wb, cx) = started("media-staged", cx, |dir| {
        std::fs::write(dir.join("logo.png"), png(6, 3, [0, 0, 255, 255])).unwrap();
        git_in(dir, &["add", "logo.png"]);
        std::fs::write(dir.join("logo.png"), png(9, 9, [0, 255, 0, 255])).unwrap();
    });
    let staged = show(&wb, "logo.png", true, cx).read_with(cx, |v, _| v.media.clone()).unwrap();
    assert_eq!((picture_size(&staged.old), picture_size(&staged.new)), (Some((4, 2)), Some((6, 3))), "HEAD ↔ Staged");
    let unstaged = show(&wb, "logo.png", false, cx).read_with(cx, |v, _| v.media.clone()).unwrap();
    assert_eq!((picture_size(&unstaged.old), picture_size(&unstaged.new)), (Some((6, 3)), Some((9, 9))), "Index ↔ Working Tree");
}

#[gpui::test]
fn ac2_an_added_image_says_none_on_the_old_side_and_a_deleted_one_on_the_new(cx: &mut TestAppContext) {
    let (_dir, wb, cx) = started("media-added", cx, |dir| {
        std::fs::write(dir.join("new.png"), png(5, 5, [9, 9, 9, 255])).unwrap();
        std::fs::remove_file(dir.join("logo.png")).unwrap();
    });
    let added = show(&wb, "new.png", false, cx).read_with(cx, |v, _| v.media.clone()).unwrap();
    assert!(matches!(added.old, Side::Absent));
    assert_eq!(picture_size(&added.new), Some((5, 5)));
    assert_eq!(describe(&added.old), "(none)");
    let deleted = show(&wb, "logo.png", false, cx).read_with(cx, |v, _| v.media.clone()).unwrap();
    assert_eq!(picture_size(&deleted.old), Some((4, 2)));
    assert!(matches!(deleted.new, Side::Absent), "a missing file is not an unreadable one");
}

#[gpui::test]
fn ac3_a_changed_binary_file_gets_one_line_instead_of_a_text_diff(cx: &mut TestAppContext) {
    let (_dir, wb, cx) = started("media-binary", cx, |dir| std::fs::write(dir.join("data.bin"), b"\0\x01\x02\x03\x04\x05\x06\x07\x08").unwrap());
    let view = show(&wb, "data.bin", false, cx);
    let media = view.read_with(cx, |v, _| v.media.clone()).expect("binary view");
    assert!(!media.image);
    assert_eq!(binary_line(&media.old, &media.new), "Binary file changed (5 B → 9 B)");
    assert!(view.read_with(cx, |v, _| v.rows.is_empty() && v.unsupported.is_none()), "no lines of replacement characters");
}

#[gpui::test]
fn an_lfs_pointer_named_png_stays_a_text_diff(cx: &mut TestAppContext) {
    let (_dir, wb, cx) = started("media-lfs", cx, |dir| std::fs::write(dir.join("pointer.png"), LFS_POINTER.replace("12345", "54321")).unwrap());
    let view = show(&wb, "pointer.png", false, cx);
    assert!(view.read_with(cx, |v, _| v.media.is_none() && !v.rows.is_empty()), "text, with lines");
}

#[gpui::test]
fn a_damaged_image_says_it_cannot_be_shown_and_the_other_side_still_shows(cx: &mut TestAppContext) {
    let (_dir, wb, cx) = started("media-damaged", cx, |dir| std::fs::write(dir.join("logo.png"), b"\x89PNG\r\n\x1a\n broken").unwrap());
    let media = show(&wb, "logo.png", false, cx).read_with(cx, |v, _| v.media.clone()).unwrap();
    assert_eq!(picture_size(&media.old), Some((4, 2)));
    assert!(matches!(&media.new, Side::Failed(why) if why.contains("could not be read")), "{}", describe(&media.new));
}

#[gpui::test]
fn the_whole_file_is_staged_and_unstaged_from_the_header(cx: &mut TestAppContext) {
    let (dir, wb, cx) = started("media-stage", cx, |dir| {
        std::fs::write(dir.join("logo.png"), png(6, 3, [0, 0, 255, 255])).unwrap();
        std::fs::write(dir.join("README.md"), "# changed\n").unwrap();
    });
    let view = show(&wb, "logo.png", false, cx);
    view.update(cx, |v, cx| v.stage_file(cx));
    wait_ticking(cx, "staged", |_| git_in(&dir, &["diff", "--cached", "--name-only"]).trim() == "logo.png");
    assert_eq!(git_in(&dir, &["diff", "--name-only"]).trim(), "README.md", "the other file was not staged");
    assert!(view.read_with(cx, |v, _| v.outcome.as_ref().is_some_and(|(m, failed)| !failed && m.contains("Staged"))));
    wait_ticking(cx, "Source Control reads the index again", |cx| changes(&wb, cx).is_some_and(|c| c.iter().any(|f| f.0 == "logo.png" && f.1.is_some())));
    let staged = show(&wb, "logo.png", true, cx);
    staged.update(cx, |v, cx| v.stage_file(cx));
    wait_ticking(cx, "unstaged", |_| git_in(&dir, &["diff", "--cached", "--name-only"]).trim().is_empty());
}

#[gpui::test]
fn a_diff_that_finishes_after_another_was_asked_for_is_dropped(cx: &mut TestAppContext) {
    let (_dir, wb, cx) = started("media-stale", cx, |dir| {
        std::fs::write(dir.join("logo.png"), png(6, 3, [0, 0, 255, 255])).unwrap();
        std::fs::write(dir.join("data.bin"), b"\0\x01").unwrap();
    });
    wb.update_in(cx, |wb, window, cx| {
        wb.open_diff("logo.png".into(), false, window, cx);
        wb.open_diff("data.bin".into(), false, window, cx);
    });
    wait_ticking(cx, "the diff", |cx| wb.read_with(cx, |wb, _| wb.diff_view().is_some()));
    cx.run_until_parked();
    std::thread::sleep(std::time::Duration::from_millis(300));
    cx.run_until_parked();
    assert_eq!(wb.read_with(cx, |wb, app| wb.diff_view().unwrap().read(app).path.clone()), "data.bin", "the last request wins");
}
