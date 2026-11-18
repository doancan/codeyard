//! A changed image, or another binary file, in Source Control's diff (#271). Both versions are read as bytes and sorted by
//! what they hold, not by the file's name alone: an image shows old and new side by side (scaled to fit, on a checkerboard for
//! transparency, with the size in pixels and bytes under each), another binary file gets one line with its old and new size,
//! and anything that is text (an LFS pointer named `.png`, say) stays a text diff.
//!
//! Decoding follows the image tab's rules (`image_size`: byte and pixel limits, the restricted SVG renderer that loads nothing
//! external) and runs on the worker that read the bytes; the view only draws decoded pictures.

use std::sync::Arc;

use gpui::{Bounds, Context, ImageFormat, ImageSource, ObjectFit, Pixels, RenderImage, SharedString, SvgRenderer, div, img, prelude::*, px};
use host::Host;

use super::DiffView;
use crate::diff_stage::DiffEvent;
use crate::git;
use crate::i18n::{t, tf};
use crate::image_size;
use crate::large::info::{self, Motion, size_label};
use crate::ui::{self, c, icon, text};

/// A decoded picture and what is known of its file.
#[derive(Clone)]
pub(crate) struct Picture {
    render: Arc<RenderImage>,
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) bytes: u64,
}

/// One side of a changed media file.
#[derive(Clone)]
pub(crate) enum Side {
    /// The file does not exist on this side: it was added or deleted. Not the same as a side that could not be read.
    Absent,
    Picture(Picture),
    /// An image that cannot be shown, with the sentence that says why.
    Failed(String),
    /// A binary file that is not an image: its size.
    Binary(u64),
}

/// What a changed image or binary file shows in the diff.
#[derive(Clone)]
pub(crate) struct Media {
    pub(crate) old: Side,
    pub(crate) new: Side,
    /// Both sides are pictures whose pixels are the same: only the file's metadata changed.
    pub(crate) same_pixels: bool,
    /// An image (two pictures) rather than another binary file (one line).
    pub(crate) image: bool,
}

/// What a Source Control diff of one file is made of.
pub(crate) enum Loaded {
    Text(Option<String>, Option<String>),
    Media(Media),
}

type Fetched = Result<Option<Vec<u8>>, String>;

#[derive(Debug, PartialEq, Eq)]
enum Kind {
    Text,
    Image(ImageFormat),
    Binary,
}

fn is_text(data: &[u8]) -> bool {
    !data.contains(&0) && std::str::from_utf8(data).is_ok()
}

/// Which view a file gets, from its name and what its sides hold. A name that says "image" is believed unless every side is text
/// (a Git LFS pointer): then the text diff is the honest one. A file that holds a NUL byte is binary whatever it is called.
fn classify(rel: &str, sides: [&Fetched; 2]) -> Kind {
    let present: Vec<&[u8]> = sides.iter().filter_map(|side| side.as_ref().ok().and_then(|bytes| bytes.as_deref())).collect();
    match crate::large::image_format(rel) {
        Some(ImageFormat::Svg) if !present.is_empty() && present.iter().all(|data| image_size::dimensions(data).is_some()) => {
            return Kind::Image(ImageFormat::Svg);
        }
        Some(ImageFormat::Svg) | None => {}
        Some(format) if present.is_empty() || present.iter().any(|data| !is_text(data)) => return Kind::Image(format),
        Some(_) => {}
    }
    if present.iter().any(|data| data.contains(&0)) { Kind::Binary } else { Kind::Text }
}

fn read_disk(host: &dyn Host, path: &str) -> Fetched {
    match host.read(path) {
        Ok((data, _)) => Ok(Some(data)),
        Err(host::HostError::Op(proto::Error::NotFound(_))) => Ok(None),
        Err(e) => Err(e.to_string()),
    }
}

/// The two sides of a Source Control diff as bytes: `HEAD` and the index, or the index and the working file. `orig` is the path in
/// `HEAD` (a renamed file's old path).
fn fetch(host: &dyn Host, root: &str, rel: &str, orig: &str, staged: bool) -> (Fetched, Fetched) {
    if staged {
        (git::show_bytes_checked(host, root, "HEAD", orig), git::show_bytes_checked(host, root, "", rel))
    } else {
        (git::show_bytes_checked(host, root, "", rel), read_disk(host, &format!("{}/{rel}", root.trim_end_matches('/'))))
    }
}

fn text_of(side: Fetched) -> Option<String> {
    side.ok().flatten().map(|bytes| String::from_utf8_lossy(&bytes).trim_start_matches('\u{feff}').replace("\r\n", "\n"))
}

fn binary_side(side: Fetched) -> Side {
    match side {
        Ok(Some(bytes)) => Side::Binary(bytes.len() as u64),
        Ok(None) => Side::Absent,
        Err(why) => Side::Failed(why),
    }
}

/// Read both sides and decide how the diff is shown. Runs on a worker.
pub(crate) fn load(host: &dyn Host, root: &str, rel: &str, orig: &str, staged: bool, svg: &SvgRenderer) -> Loaded {
    let (old, new) = fetch(host, root, rel, orig, staged);
    match classify(rel, [&old, &new]) {
        Kind::Text => Loaded::Text(text_of(old), text_of(new)),
        Kind::Binary => Loaded::Media(Media { old: binary_side(old), new: binary_side(new), same_pixels: false, image: false }),
        Kind::Image(format) => {
            let (old, new) = (picture_side(old, format, svg), picture_side(new, format, svg));
            let same_pixels = match (&old, &new, format) {
                (Side::Picture(a), Side::Picture(b), format) if format != ImageFormat::Svg => same_pixels(&a.render, &b.render),
                _ => false,
            };
            Loaded::Media(Media { old, new, same_pixels, image: true })
        }
    }
}

fn same_pixels(a: &RenderImage, b: &RenderImage) -> bool {
    a.size(0) == b.size(0) && a.frame_count() == b.frame_count() && (0..a.frame_count()).all(|frame| a.as_bytes(frame) == b.as_bytes(frame))
}

fn picture_side(side: Fetched, format: ImageFormat, svg: &SvgRenderer) -> Side {
    match side {
        Ok(None) => Side::Absent,
        Err(why) => Side::Failed(why),
        Ok(Some(data)) => decode(data, format, svg),
    }
}

/// Check, then decode. Everything that can be refused is refused before the decoder runs: a small file can announce pixels that take
/// gigabytes once decoded.
fn decode(data: Vec<u8>, format: ImageFormat, svg: &SvgRenderer) -> Side {
    let bytes = data.len() as u64;
    let failed = Side::Failed;
    if bytes > image_size::MAX_BYTES {
        return failed(t("Image exceeds 20 MB.").to_string());
    }
    if data.is_empty() {
        return failed(t("The file is empty.").to_string());
    }
    let unreadable = || tf("This {} file could not be read. It may be damaged or cut short.", &[&info::format_name(format)]);
    let dimensions = image_size::dimensions(&data);
    if format == ImageFormat::Svg && dimensions.is_none() {
        return failed(unreadable());
    }
    if let Some((w, h)) = dimensions
        && image_size::too_large(w, h)
    {
        return failed(tf("This image is {0} × {1} pixels, too large to preview.", &[&w, &h]));
    }
    if let (Motion::Frames(n), Some((w, h))) = (info::motion(&data, format), dimensions)
        && info::animated_pixels(n, w, h) > image_size::MAX_PIXELS
    {
        return failed(tf("This animation has {0} frames of {1} × {2} pixels, too large to play.", &[&n, &w, &h]));
    }
    let Ok(render) = gpui::Image::from_bytes(format, data).to_image_data(svg.clone()) else { return failed(unreadable()) };
    let decoded = render.size(0);
    let (width, height) = match (format, dimensions) {
        (ImageFormat::Svg, Some((w, h))) => (w as u32, h as u32),
        _ => (i32::from(decoded.width) as u32, i32::from(decoded.height) as u32),
    };
    Side::Picture(Picture { render, width, height, bytes })
}

/// "1280 × 800 · 214 KB", "(none)", the reason an image cannot be shown, or a binary file's size.
pub(crate) fn describe(side: &Side) -> String {
    match side {
        Side::Absent => t("(none)").to_string(),
        Side::Picture(p) => format!("{} × {} · {}", p.width, p.height, size_label(p.bytes)),
        Side::Failed(why) => why.clone(),
        Side::Binary(bytes) => size_label(*bytes),
    }
}

/// The one line of a binary file that is not an image.
pub(crate) fn binary_line(old: &Side, new: &Side) -> String {
    match (old, new) {
        (Side::Absent, new) => tf("Binary file added ({})", &[&describe(new)]),
        (old, Side::Absent) => tf("Binary file deleted ({})", &[&describe(old)]),
        (old, new) => tf("Binary file changed ({0} → {1})", &[&describe(old), &describe(new)]),
    }
}

/// Side of a checkerboard square.
const CHECKER: f32 = 10.;

/// The checkerboard under a picture drawn at `scale` of its size in a pane `bounds`, as big as the picture is drawn.
fn paint_checkerboard(window: &mut gpui::Window, pane: Bounds<Pixels>, (width, height): (u32, u32), colors: (u32, u32)) {
    let (pw, ph) = (f32::from(pane.size.width), f32::from(pane.size.height));
    let scale = (pw / width as f32).min(ph / height as f32).min(1.);
    let (w, h) = (width as f32 * scale, height as f32 * scale);
    if w < 1. || h < 1. {
        return;
    }
    let (left, top) = ((pw - w) / 2., (ph - h) / 2.);
    let rect = |x: f32, y: f32, w: f32, h: f32| Bounds::new(pane.origin + gpui::point(px(x), px(y)), gpui::size(px(w), px(h)));
    window.paint_quad(gpui::fill(rect(left, top, w, h), c(colors.0)));
    let (mut row, mut y) = (0, 0.);
    while y < h {
        let mut x = if row % 2 == 0 { CHECKER } else { 0. };
        while x < w {
            window.paint_quad(gpui::fill(rect(left + x, top + y, CHECKER.min(w - x), CHECKER.min(h - y)), c(colors.1)));
            x += CHECKER * 2.;
        }
        row += 1;
        y += CHECKER;
    }
}

impl DiffView {
    /// A diff of an image or another binary file.
    pub fn new_media(path: String, staged: bool, media: Media, cx: &mut Context<Self>) -> Self {
        let mut view = Self::new(path, staged, "", "", cx);
        view.media = Some(media);
        view
    }

    /// Stage (Index ↔ Working Tree) or unstage (HEAD ↔ Staged) the whole file: lines of a picture cannot be picked.
    pub(crate) fn stage_file(&mut self, cx: &mut Context<Self>) {
        let Some(stager) = self.stage.clone() else { return };
        if self.busy {
            return;
        }
        self.busy = true;
        self.outcome = None;
        let rel = self.path.clone();
        let name = ui::file_name(&rel).to_string();
        let task = cx.background_executor().spawn(async move {
            let paths = [rel];
            if stager.staged { git::unstage(stager.host.as_ref(), &stager.root, &paths) } else { git::stage(stager.host.as_ref(), &stager.root, &paths) }
                .map(|()| stager.staged)
        });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| {
                this.busy = false;
                let wrote = result.is_ok();
                this.outcome = Some(match result {
                    Ok(true) => (tf("Unstaged “{}”", &[&name]).into(), false),
                    Ok(false) => (tf("Staged “{}”", &[&name]).into(), false),
                    Err(e) => (e.into(), true),
                });
                if wrote {
                    cx.emit(DiffEvent::Changed);
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    /// The tab's content for a media diff: a header, then the two pictures or the binary line.
    pub(crate) fn render_media(&self, media: &Media, p: ui::Palette, cx: &mut Context<Self>) -> gpui::Div {
        let (left, right) = self.titles.clone().unwrap_or_else(|| (t("Before").into(), t("After").into()));
        let unstages = self.unstages();
        let stage_label = if unstages { t("Unstage File") } else { t("Stage File") };
        let button = self.stage.is_some().then(|| {
            let label = stage_label.to_string();
            crate::a11y_controls::press_on(
                div()
                    .id("diff-stage-file")
                    .relative()
                    .flex_none()
                    .px(px(theme::SPACE[4]))
                    .h(px(22.))
                    .flex()
                    .items_center()
                    .rounded(px(theme::radius::MD))
                    .bg(c(p.accent))
                    .text_color(c(p.on_accent))
                    .when(!self.busy, |d| d.cursor_pointer().hover(|s| s.opacity(0.9)))
                    .when(self.busy, |d| d.opacity(0.5))
                    .child(label.clone()),
                crate::a11y_controls::Control::button(self.path.clone(), "stage-file", label, |_, _| {}),
                cx,
                |this, _, cx| this.stage_file(cx),
            )
        });
        let header = div()
            .h(px(30.))
            .flex_none()
            .flex()
            .items_center()
            .gap(px(theme::SPACE[3]))
            .px(px(theme::SPACE[6]))
            .border_b_1()
            .border_color(c(p.border))
            .text_size(px(text::XS.0))
            .text_color(c(p.text_muted))
            .child(icon("git-compare", 14., p.text_muted))
            .child(div().min_w_0().truncate().text_color(c(p.text)).child(self.path.clone()))
            .child(div().min_w_0().truncate().child(self.label.clone()))
            .when_some(self.outcome.clone(), |d, (message, failed)| {
                d.child(div().min_w_0().truncate().text_color(c(if failed { p.danger } else { p.success })).child(message))
            })
            .child(div().flex_1())
            .children(button);
        let note = (media.image && media.same_pixels).then(|| {
            let message = t("Only the file's metadata changed: the pixels are the same.");
            div()
                .h(px(24.))
                .flex_none()
                .flex()
                .items_center()
                .px(px(theme::SPACE[6]))
                .bg(c(p.surface))
                .border_b_1()
                .border_color(c(p.border))
                .font_family(ui::UI_FONT)
                .text_size(px(text::XS.0))
                .text_color(c(p.text))
                .child(div().relative().child(message).child(crate::a11y_controls::said(self.path.clone(), "metadata-only", accesskit::Role::Status, message)))
        });
        let body = if media.image {
            div()
                .flex_1()
                .min_h_0()
                .flex()
                .child(self.picture_pane("old", left, p.danger, &media.old, p))
                .child(div().w(px(1.)).h_full().flex_none().bg(c(p.border)))
                .child(self.picture_pane("new", right, p.success, &media.new, p))
        } else {
            let line = binary_line(&media.old, &media.new);
            div()
                .flex_1()
                .min_h_0()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap(px(theme::SPACE[4]))
                .font_family(ui::UI_FONT)
                .text_size(px(text::SM.0))
                .text_color(c(p.text_muted))
                .child(icon("file", 28., p.text_muted))
                .child(div().relative().child(line.clone()).child(crate::a11y_controls::said(self.path.clone(), "binary", accesskit::Role::Label, line)))
        };
        div().key_context("DiffView").track_focus(&self.focus_handle).size_full().flex().flex_col().bg(c(p.bg)).child(header).children(note).child(body)
    }

    /// One version of an image: its name, the picture on a checkerboard (or why there is none) and its size line.
    fn picture_pane(&self, key: &'static str, title: SharedString, dot: u32, side: &Side, p: ui::Palette) -> gpui::Div {
        let what = describe(side);
        let spoken = format!("{title}: {what}");
        let area =
            div().relative().flex_1().min_h_0().flex().items_center().justify_center().overflow_hidden().debug_selector(move || format!("diff-image-{key}"));
        let area = match side {
            Side::Picture(picture) => {
                let size = (picture.width.max(1), picture.height.max(1));
                area.child(
                    gpui::canvas(|_, _, _| {}, move |bounds, _, window, _| paint_checkerboard(window, bounds, size, (p.surface, p.surface_hover)))
                        .absolute()
                        .size_full(),
                )
                .child(
                    img(ImageSource::Render(picture.render.clone()))
                        .w_full()
                        .h_full()
                        .max_w(px(size.0 as f32))
                        .max_h(px(size.1 as f32))
                        .object_fit(ObjectFit::Contain),
                )
            }
            other => area.text_size(px(text::SM.0)).text_color(c(p.text_muted)).child(match other {
                Side::Absent => t("(none)").to_string(),
                _ => describe(other),
            }),
        };
        div()
            .flex_1()
            .min_w_0()
            .h_full()
            .flex()
            .flex_col()
            .font_family(ui::UI_FONT)
            .child(
                div()
                    .h(px(22.))
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap(px(theme::SPACE[3]))
                    .px(px(theme::SPACE[6]))
                    .bg(c(p.surface))
                    .border_b_1()
                    .border_color(c(p.border))
                    .text_size(px(text::XS.0))
                    .text_color(c(p.text_muted))
                    .child(div().size(px(6.)).flex_none().rounded_full().bg(c(dot)))
                    .child(div().truncate().child(title)),
            )
            .child(area)
            .child(
                div()
                    .h(px(24.))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .border_t_1()
                    .border_color(c(p.border))
                    .text_size(px(text::XS.0))
                    .text_color(c(p.text_muted))
                    .child(div().relative().child(what).child(crate::a11y_controls::said(
                        self.path.clone(),
                        format!("image-{key}"),
                        accesskit::Role::Image,
                        spoken,
                    ))),
            )
    }
}

#[cfg(test)]
#[path = "diff_media_tests.rs"]
mod tests;
