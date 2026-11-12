//! The image viewer: an image tab (`LargeFileView` in image mode) with zoom (fit, 100 %, steps, wheel with ⌘ or Ctrl), panning by
//! dragging, a checkerboard under transparent pixels, and an info bar (pixel size, file size, format). A file that cannot be shown
//! (too big, damaged, empty, a long animation) says why in the viewport instead. The numbers are in `large_image_zoom.rs` and
//! `large_image_info.rs`; this file reads and decodes the file off the UI thread and draws.

use gpui::{
    Bounds, ImageSource, KeyBinding, MouseButton, MouseDownEvent, MouseMoveEvent, ObjectFit, Pixels, Point, RenderImage, ScrollWheelEvent, SvgRenderer, SvgSize,
};

use super::info::{self, Motion};
use super::zoom::{self, ViewState};
use super::*;

actions!(image_view, [ImageZoomIn, ImageZoomOut, ImageFit, ImageActualSize, ImagePanLeft, ImagePanRight, ImagePanUp, ImagePanDown, ImageTogglePlay]);

/// Keys of the image viewer (its view has the key context `ImageView`). `cyx` lists them as built-in keys too (its
/// `BINDING_SOURCES`).
pub fn key_bindings() -> [KeyBinding; 14] {
    let at = Some("ImageView");
    [
        KeyBinding::new("=", ImageZoomIn, at),
        KeyBinding::new("+", ImageZoomIn, at),
        KeyBinding::new("shift-=", ImageZoomIn, at),
        KeyBinding::new("-", ImageZoomOut, at),
        KeyBinding::new("0", ImageFit, at),
        KeyBinding::new("1", ImageActualSize, at),
        KeyBinding::new("left", ImagePanLeft, at),
        KeyBinding::new("right", ImagePanRight, at),
        KeyBinding::new("up", ImagePanUp, at),
        KeyBinding::new("down", ImagePanDown, at),
        KeyBinding::new("space", ImageTogglePlay, at),
        KeyBinding::new("shift-left", ImagePanLeft, at),
        KeyBinding::new("shift-right", ImagePanRight, at),
        KeyBinding::new("escape", ImageFit, at),
    ]
}

/// Pixels one arrow key moves the picture.
const PAN_STEP: f32 = 48.;
/// A wheel notch zooms by this factor per 100 pixels of travel.
const WHEEL_ZOOM: f32 = 1.35;
/// Dots per line of a wheel that reports lines.
const WHEEL_LINE: f32 = 20.;
/// Side of a checkerboard square.
const CHECKER: f32 = 10.;
/// Largest raster scale an SVG is drawn at.
const MAX_VECTOR_SCALE: u32 = 16;
/// Most pixels an SVG is drawn with at a zoom (64 MB): the picture drawn at 1x stays when a sharper one would be bigger.
const MAX_VECTOR_PIXELS: u64 = 16_000_000;

/// A picture that was read and decoded.
pub(super) struct Loaded {
    /// The file as it was read (an SVG is drawn from it again at the zoom asked for).
    source: Arc<gpui::Image>,
    picture: Arc<RenderImage>,
    /// The first frame of an animation, made while it is paused and dropped when it plays.
    still: Option<Arc<RenderImage>>,
    /// Size of the picture in pixels as the file states it.
    size: zoom::Size,
    format: gpui::ImageFormat,
    bytes: u64,
    motion: Motion,
}

#[derive(Default)]
pub(super) struct State {
    loaded: Option<Loaded>,
    /// What is known of a file that could not be shown: its format and size.
    failed: Option<(gpui::ImageFormat, u64)>,
    view: ViewState,
    viewport: Bounds<Pixels>,
    /// Where the pointer was at the last drag step.
    drag: Option<Point<Pixels>>,
    /// What the user chose with Space; until then an animation follows reduced motion.
    paused_by_user: Option<bool>,
    scale_factor: f32,
    /// The raster scale the SVG is drawn at now (1 until a sharper drawing is ready; `Loaded::picture` is that drawing).
    vector_step: u32,
    /// The scale being drawn: one drawing at a time, so a burst of wheel events cannot start many.
    vector_pending: Option<u32>,
    vector_task: Option<Task<()>>,
    /// Redraw when reduced motion changes.
    _watch: Vec<Subscription>,
    #[cfg(test)]
    vector_spawned: usize,
}

struct Failure {
    message: String,
    format: gpui::ImageFormat,
    bytes: Option<u64>,
}

/// Read, check and decode the file. Everything that can be refused is refused before the decoder runs: a small file may announce
/// pixels that take gigabytes once decoded.
fn read(host: &dyn Host, path: &str, format: gpui::ImageFormat, svg: SvgRenderer) -> Result<Loaded, Failure> {
    let fail = |message: String, bytes: Option<u64>| Failure { message, format, bytes };
    let unreadable = || crate::i18n::tf("This {} file could not be read. It may be damaged or cut short.", &[&info::format_name(format)]);
    let too_big = |bytes: u64| fail(crate::i18n::t("Image exceeds 20 MB.").to_string(), Some(bytes));
    let stat = host.stat(path).map_err(|e| fail(e.to_string(), None))?;
    if stat.size > crate::image_size::MAX_BYTES {
        return Err(too_big(stat.size));
    }
    let (data, _) = host.read(path).map_err(|e| fail(e.to_string(), None))?;
    let bytes = data.len() as u64;
    if bytes > crate::image_size::MAX_BYTES {
        return Err(too_big(bytes));
    }
    if data.is_empty() {
        return Err(fail(crate::i18n::t("The file is empty.").to_string(), Some(0)));
    }
    let dimensions = crate::image_size::dimensions(&data);
    if format == gpui::ImageFormat::Svg && dimensions.is_none() {
        return Err(fail(unreadable(), Some(bytes)));
    }
    if let Some((w, h)) = dimensions
        && crate::image_size::too_large(w, h)
    {
        return Err(fail(crate::i18n::tf("This image is {0} × {1} pixels, too large to preview.", &[&w, &h]), Some(bytes)));
    }
    let motion = info::motion(&data, format);
    if let (Motion::Frames(n), Some((w, h))) = (motion, dimensions)
        && info::animated_pixels(n, w, h) > crate::image_size::MAX_PIXELS
    {
        return Err(fail(crate::i18n::tf("This animation has {0} frames of {1} × {2} pixels, too large to play.", &[&n, &w, &h]), Some(bytes)));
    }
    let source = Arc::new(gpui::Image::from_bytes(format, data));
    let picture = source.to_image_data(svg).map_err(|_| fail(unreadable(), Some(bytes)))?;
    let decoded = picture.size(0);
    let size = match (format, dimensions) {
        (gpui::ImageFormat::Svg, Some((w, h))) => (w as f32, h as f32),
        _ => (i32::from(decoded.width) as f32, i32::from(decoded.height) as f32),
    };
    Ok(Loaded { source, picture, still: None, size, format, bytes, motion })
}

/// A copy of the first frame of a (decoded, animated) picture, for showing it paused.
fn first_frame(picture: &RenderImage) -> Option<Arc<RenderImage>> {
    let size = picture.size(0);
    let buffer = image::RgbaImage::from_raw(i32::from(size.width) as u32, i32::from(size.height) as u32, picture.as_bytes(0)?.to_vec())?;
    Some(Arc::new(RenderImage::new([image::Frame::new(buffer)])))
}

/// An SVG drawn at `scale` times its size.
fn rasterize(renderer: &SvgRenderer, svg: &[u8], scale: f32) -> Option<Arc<RenderImage>> {
    let pixmap = renderer.render_pixmap(svg, SvgSize::ScaleFactor(scale)).ok()?;
    let (width, height) = (pixmap.width(), pixmap.height());
    if (width as u64).saturating_mul(height as u64) > MAX_VECTOR_PIXELS || width.max(height) as u64 > crate::image_size::MAX_SIDE {
        return None;
    }
    let mut data = pixmap.take();
    // The rasteriser's pixels are premultiplied RGBA; the picture wants straight BGRA.
    for px in data.as_chunks_mut::<4>().0 {
        let alpha = px[3] as u32;
        if alpha != 0 && alpha != 255 {
            for channel in &mut px[..3] {
                *channel = ((*channel as u32 * 255 + alpha / 2) / alpha).min(255) as u8;
            }
        }
        px.swap(0, 2);
    }
    Some(Arc::new(RenderImage::new([image::Frame::new(image::RgbaImage::from_raw(width, height, data)?)])))
}

/// Start reading the file for an image tab.
pub(super) fn spawn_load(host: Arc<dyn Host>, path: String, format: gpui::ImageFormat, cx: &mut Context<LargeFileView>) {
    let renderer = cx.svg_renderer().restricted();
    let task = cx.background_executor().spawn(async move { read(host.as_ref(), &path, format, renderer) });
    cx.spawn(async move |this, cx| {
        let result = task.await;
        this.update(cx, |v, cx| {
            match result {
                Ok(loaded) => {
                    v.size = loaded.bytes;
                    v.pic.vector_step = 1;
                    v.pic.loaded = Some(loaded);
                    v.pic._watch =
                        vec![cx.observe_global::<Settings>(|_, cx| cx.notify()), cx.observe_global::<crate::system_prefs::SystemPrefs>(|_, cx| cx.notify())];
                }
                Err(failure) => {
                    v.pic.failed = failure.bytes.map(|bytes| (failure.format, bytes));
                    v.error = Some(failure.message.into());
                }
            }
            cx.notify();
        })
        .ok();
    })
    .detach();
}

fn pixels(p: Pixels) -> f32 {
    f32::from(p)
}

impl LargeFileView {
    #[cfg(test)]
    pub(super) fn image_loaded_state(&self) -> bool {
        self.pic.loaded.is_some()
    }

    fn picture_size(&self) -> Option<zoom::Size> {
        self.pic.loaded.as_ref().map(|l| l.size)
    }

    fn viewport_size(&self) -> zoom::Size {
        (pixels(self.pic.viewport.size.width), pixels(self.pic.viewport.size.height))
    }

    /// The zoom or position changed: draw again, and draw an SVG sharper if the zoom now needs it.
    fn image_changed(&mut self, cx: &mut Context<Self>) {
        self.refresh_vector(cx);
        cx.notify();
    }

    fn with_view(&mut self, cx: &mut Context<Self>, change: impl FnOnce(&mut ViewState, zoom::Size, zoom::Size)) {
        let Some(image) = self.picture_size() else { return };
        let viewport = self.viewport_size();
        change(&mut self.pic.view, image, viewport);
        self.image_changed(cx);
    }

    fn image_zoom_in(&mut self, _: &ImageZoomIn, _: &mut Window, cx: &mut Context<Self>) {
        self.with_view(cx, |v, i, vp| v.zoom_in(i, vp));
    }

    fn image_zoom_out(&mut self, _: &ImageZoomOut, _: &mut Window, cx: &mut Context<Self>) {
        self.with_view(cx, |v, i, vp| v.zoom_out(i, vp));
    }

    fn image_fit(&mut self, _: &ImageFit, _: &mut Window, cx: &mut Context<Self>) {
        self.with_view(cx, |v, _, _| v.fit());
    }

    fn image_actual_size(&mut self, _: &ImageActualSize, _: &mut Window, cx: &mut Context<Self>) {
        self.with_view(cx, |v, i, vp| v.actual_size(i, vp));
    }

    /// The percentage button and a double click: 100 %, or back to fit when it is already there.
    fn image_toggle_fit(&mut self, cx: &mut Context<Self>) {
        self.with_view(cx, |v, i, vp| if (v.scale(i, vp) - 1.).abs() < 0.001 { v.fit() } else { v.actual_size(i, vp) });
    }

    fn pan_key(&mut self, delta: (f32, f32), cx: &mut Context<Self>) {
        self.with_view(cx, |v, i, vp| v.pan_by(delta, i, vp));
    }

    /// Space: play or pause an animation.
    fn image_toggle_play(&mut self, _: &ImageTogglePlay, _: &mut Window, cx: &mut Context<Self>) {
        if self.is_animated() {
            self.pic.paused_by_user = Some(!self.is_paused(cx));
            cx.notify();
        }
    }

    fn is_animated(&self) -> bool {
        self.pic.loaded.as_ref().is_some_and(|l| matches!(l.motion, Motion::Frames(_)))
    }

    /// Whether the animation is stopped: by the user, or else by reduced motion (the setting or the system's).
    fn is_paused(&self, cx: &App) -> bool {
        self.is_animated() && self.pic.paused_by_user.unwrap_or_else(|| crate::system_prefs::reduce_motion(cx))
    }

    /// ⌘ or Ctrl with the wheel zooms around the pointer; the wheel alone moves the picture.
    fn image_wheel(&mut self, ev: &ScrollWheelEvent, cx: &mut Context<Self>) {
        let delta = ev.delta.pixel_delta(px(WHEEL_LINE));
        let (dx, dy) = (pixels(delta.x), pixels(delta.y));
        let origin = self.pic.viewport.origin;
        let viewport = self.viewport_size();
        let anchor = (pixels(ev.position.x - origin.x) - viewport.0 / 2., pixels(ev.position.y - origin.y) - viewport.1 / 2.);
        let zoom = ev.modifiers.platform || ev.modifiers.control;
        self.with_view(cx, |v, i, vp| if zoom { v.zoom_by(WHEEL_ZOOM.powf(dy / 100.), anchor, i, vp) } else { v.pan_by((dx, dy), i, vp) });
        cx.stop_propagation();
    }

    fn image_drag(&mut self, ev: &MouseMoveEvent, cx: &mut Context<Self>) {
        let Some(last) = self.pic.drag else { return };
        if ev.pressed_button != Some(MouseButton::Left) {
            self.pic.drag = None;
            return;
        }
        self.pic.drag = Some(ev.position);
        let delta = (pixels(ev.position.x - last.x), pixels(ev.position.y - last.y));
        self.with_view(cx, |v, i, vp| v.pan_by(delta, i, vp));
    }

    /// The raster scale an SVG should be drawn at for the zoom now: a power of two covering the pixels the screen shows, lowered
    /// until the drawing is within the pixel and side limits.
    fn wanted_vector_scale(&self) -> Option<u32> {
        let loaded = self.pic.loaded.as_ref().filter(|l| l.format == gpui::ImageFormat::Svg)?;
        let on_screen = self.pic.view.scale(loaded.size, self.viewport_size()) * self.pic.scale_factor.max(1.);
        let mut step = 1;
        while (step as f32) < on_screen && step < MAX_VECTOR_SCALE {
            step *= 2;
        }
        let fits = |step: u32| {
            let (w, h) = ((loaded.size.0 * step as f32) as u64, (loaded.size.1 * step as f32) as u64);
            w.saturating_mul(h) <= MAX_VECTOR_PIXELS && w.max(h) <= crate::image_size::MAX_SIDE
        };
        while step > 1 && !fits(step) {
            step /= 2;
        }
        Some(step)
    }

    /// Draw the SVG again at the scale it needs, off the UI thread, one drawing at a time; the picture already drawn stays until the
    /// new one is ready (and stays when the zoom goes back down: a sharper one is never thrown away for a blurrier).
    fn refresh_vector(&mut self, cx: &mut Context<Self>) {
        let Some(step) = self.wanted_vector_scale() else { return };
        if step <= self.pic.vector_step.max(1) || self.pic.vector_pending.is_some() {
            return;
        }
        let Some(loaded) = self.pic.loaded.as_ref() else { return };
        let (source, renderer) = (loaded.source.clone(), cx.svg_renderer().restricted());
        self.pic.vector_pending = Some(step);
        #[cfg(test)]
        {
            self.pic.vector_spawned += 1;
        }
        let task = cx.background_executor().spawn(async move { rasterize(&renderer, source.bytes(), step as f32) });
        self.pic.vector_task = Some(cx.spawn(async move |this, cx| {
            let drawn = task.await;
            this.update(cx, |v, cx| {
                v.pic.vector_pending = None;
                if let (Some(drawn), Some(loaded)) = (drawn, v.pic.loaded.as_mut()) {
                    // The 1x picture is released: this one is the picture now.
                    loaded.picture = drawn;
                    v.pic.vector_step = step;
                    cx.notify();
                }
                // The zoom may have moved on while this was drawn.
                v.refresh_vector(cx);
            })
            .ok();
        }));
    }

    #[cfg(test)]
    pub fn image_zoom_label_for_test(&self) -> Option<String> {
        self.picture_size().map(|i| self.pic.view.label(i, self.viewport_size()))
    }

    #[cfg(test)]
    pub fn image_view_for_test(&self) -> ViewState {
        self.pic.view
    }
}

impl LargeFileView {
    pub(super) fn render_image(&mut self, window: &mut Window, cx: &mut Context<Self>) -> gpui::AnyElement {
        let p = *palette(cx);
        self.pic.scale_factor = window.scale_factor();
        // A paused animation shows a copy of its first frame; it is made when needed and let go when it plays.
        let paused = self.is_paused(cx);
        if let Some(l) = self.pic.loaded.as_mut() {
            match (paused, l.still.is_some()) {
                (true, false) => l.still = first_frame(&l.picture),
                (false, true) => l.still = None,
                _ => {}
            }
        }
        let surface = self.path.clone();
        let me = cx.entity().downgrade();
        let button = |id: &'static str, glyph: &'static str, tip: &'static str, enabled: bool, cx: &mut Context<Self>| {
            ui::icon_button(id, glyph, tip, &p)
                .relative()
                .when(!enabled, |d| d.opacity(0.4))
                .child(crate::a11y_controls::control(crate::a11y_controls::Control::button(surface.clone(), id, tip, |_, _| {}).on_entity(
                    me.clone(),
                    move |this: &mut Self, w, cx| {
                        this.image_button(id, w, cx);
                    },
                )))
                .on_click(cx.listener(move |this, _, w, cx| this.image_button(id, w, cx)))
        };
        let loaded = self.pic.loaded.as_ref();
        let size = loaded.map(|l| l.size);
        let viewport = self.viewport_size();
        let label = size.map(|i| self.pic.view.label(i, viewport));
        let animated = loaded.is_some_and(|l| matches!(l.motion, Motion::Frames(_)));
        let pannable = size.is_some_and(|i| {
            let s = self.pic.view.scale(i, viewport);
            i.0 * s > viewport.0 + 0.5 || i.1 * s > viewport.1 + 0.5
        });

        let toolbar = div()
            .h(px(30.))
            .flex_none()
            .flex()
            .items_center()
            .gap(px(theme::SPACE[1]))
            .px(px(theme::SPACE[4]))
            .border_b_1()
            .border_color(c(p.border))
            .child(icon("file-image", 14., p.text_muted))
            .child(div().flex_1())
            .when(animated, |d| {
                let (glyph, tip) =
                    if paused { ("play", crate::i18n::t("Play animation (Space)")) } else { ("pause", crate::i18n::t("Pause animation (Space)")) };
                d.child(button("image-play", glyph, tip, true, cx))
            })
            .child(button("image-zoom-out", "minus", crate::i18n::t("Zoom out (-)"), loaded.is_some(), cx))
            .child(
                div()
                    .id("image-zoom-level")
                    .relative()
                    .w(px(56.))
                    .h(px(22.))
                    .flex()
                    .flex_none()
                    .items_center()
                    .justify_center()
                    .rounded(px(theme::radius::SM))
                    .cursor_pointer()
                    .text_size(px(text::XS.0))
                    .text_color(c(p.text))
                    .hover(|d| d.bg(c(p.surface_hover)))
                    .tooltip(|_, cx| cx.new(|_| crate::workbench::Tooltip(crate::i18n::t("Switch between 100% and fit").into())).into())
                    .child(crate::a11y_controls::control(
                        crate::a11y_controls::Control::button(self.path.clone(), "image-zoom-level", crate::i18n::t("Zoom level"), |_, _| {})
                            .value(label.clone().unwrap_or_default())
                            .on_entity(cx.entity().downgrade(), |this: &mut Self, _, cx| this.image_toggle_fit(cx)),
                    ))
                    .child(label.clone().unwrap_or_default())
                    .on_click(cx.listener(|this, _, _, cx| this.image_toggle_fit(cx))),
            )
            .child(button("image-zoom-in", "plus", crate::i18n::t("Zoom in (+)"), loaded.is_some(), cx))
            .child(button("image-fit", "maximize-2", crate::i18n::t("Fit to window (0)"), loaded.is_some(), cx));

        let viewport_el = self.image_viewport(&p, pannable, cx);
        let bar = self.image_info_bar(&p);
        div()
            .key_context("LargeFile ImageView")
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(Self::image_zoom_in))
            .on_action(cx.listener(Self::image_zoom_out))
            .on_action(cx.listener(Self::image_fit))
            .on_action(cx.listener(Self::image_actual_size))
            .on_action(cx.listener(Self::image_toggle_play))
            .on_action(cx.listener(|this, _: &ImagePanLeft, _, cx| this.pan_key((PAN_STEP, 0.), cx)))
            .on_action(cx.listener(|this, _: &ImagePanRight, _, cx| this.pan_key((-PAN_STEP, 0.), cx)))
            .on_action(cx.listener(|this, _: &ImagePanUp, _, cx| this.pan_key((0., PAN_STEP), cx)))
            .on_action(cx.listener(|this, _: &ImagePanDown, _, cx| this.pan_key((0., -PAN_STEP), cx)))
            .size_full()
            .flex()
            .flex_col()
            .bg(c(p.bg))
            .child(toolbar)
            .child(viewport_el)
            .child(bar)
            .into_any_element()
    }

    fn image_button(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        match id {
            "image-zoom-in" => self.image_zoom_in(&ImageZoomIn, window, cx),
            "image-zoom-out" => self.image_zoom_out(&ImageZoomOut, window, cx),
            "image-fit" => self.image_fit(&ImageFit, window, cx),
            "image-play" => self.image_toggle_play(&ImageTogglePlay, window, cx),
            _ => {}
        }
    }

    /// The area the picture is drawn in: checkerboard, picture, or the reason there is none.
    fn image_viewport(&mut self, p: &ui::Palette, pannable: bool, cx: &mut Context<Self>) -> gpui::AnyElement {
        let area = div()
            .id("image-viewport")
            .relative()
            .flex_1()
            .min_h_0()
            .overflow_hidden()
            .on_scroll_wheel(cx.listener(|this, ev: &ScrollWheelEvent, _, cx| this.image_wheel(ev, cx)))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, ev: &MouseDownEvent, window, cx| {
                    window.focus(&this.focus_handle);
                    if ev.click_count >= 2 {
                        this.image_toggle_fit(cx);
                    } else {
                        this.pic.drag = Some(ev.position);
                    }
                }),
            )
            .on_mouse_move(cx.listener(|this, ev: &MouseMoveEvent, _, cx| this.image_drag(ev, cx)))
            .on_mouse_up(MouseButton::Left, cx.listener(|this, _, _, _| this.pic.drag = None))
            .on_mouse_up_out(MouseButton::Left, cx.listener(|this, _, _, _| this.pic.drag = None));
        let Some(loaded) = &self.pic.loaded else {
            let (glyph, text, color) = match &self.error {
                Some(e) => ("file-image", e.to_string(), p.text),
                None => ("file-image", crate::i18n::t("Loading…").to_string(), p.text_muted),
            };
            return area
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap(px(theme::SPACE[4]))
                .px(px(theme::SPACE[8]))
                .child(icon(glyph, 32., p.text_muted))
                .child(div().max_w(px(520.)).text_center().text_size(px(text::SM.0)).text_color(c(color)).child(text.clone()))
                .child(crate::a11y_controls::said(self.path.clone(), "message", accesskit::Role::Label, text))
                .into_any_element();
        };
        let viewport = self.viewport_size();
        let scale = self.pic.view.scale(loaded.size, viewport);
        let (width, height) = (loaded.size.0 * scale, loaded.size.1 * scale);
        let (pan_x, pan_y) = if self.pic.view.is_fit() { (0., 0.) } else { self.pic.view.pan };
        let paused = loaded.motion != Motion::Still && self.is_paused(cx);
        let shown = loaded.still.clone().filter(|_| paused).unwrap_or_else(|| loaded.picture.clone());
        let ready = viewport.0 > 0. && viewport.1 > 0.;
        let (low, high) = (ui::mix(p.bg, p.text, 0.05), ui::mix(p.bg, p.text, 0.13));
        let me = cx.entity().downgrade();
        let description = self.image_description();
        let name = self.name.to_string();
        let surface = self.path.clone();
        area.when(pannable, |d| if self.pic.drag.is_some() { d.cursor_grabbing() } else { d.cursor_grab() })
            .child(
                gpui::canvas(
                    |bounds, _, _| bounds,
                    move |_, bounds, window, cx| {
                        if width > 0. && height > 0. {
                            paint_checkerboard(window, bounds, (width, height), (pan_x, pan_y), (low, high));
                        }
                        // The view needs the viewport's size for fit and for keeping the picture inside: tell it, after this frame.
                        let me = me.clone();
                        cx.defer(move |cx| {
                            me.update(cx, |v, cx| {
                                if v.pic.viewport != bounds {
                                    v.pic.viewport = bounds;
                                    v.image_changed(cx);
                                }
                            })
                            .ok();
                        });
                    },
                )
                .absolute()
                .top_0()
                .left_0()
                .size_full(),
            )
            .when(ready, |d| {
                d.child(
                    gpui::img(ImageSource::Render(shown))
                        .id("image-picture")
                        .absolute()
                        .left(gpui::relative(0.5))
                        .top(gpui::relative(0.5))
                        .ml(px(pan_x - width / 2.))
                        .mt(px(pan_y - height / 2.))
                        .w(px(width))
                        .h(px(height))
                        .object_fit(ObjectFit::Fill),
                )
            })
            .child(crate::a11y_controls::control(crate::a11y_controls::Control::new(surface, "image", accesskit::Role::Image, name).description(description)))
            .into_any_element()
    }

    /// "800 × 600 pixels, PNG, 5.4 KB" for the accessibility tree.
    fn image_description(&self) -> String {
        let Some(l) = &self.pic.loaded else { return String::new() };
        crate::i18n::tf("{0} × {1} pixels, {2}, {3}", &[&l.size.0, &l.size.1, &info::format_name(l.format), &info::size_label(l.bytes)])
    }

    /// Pixel size, format and file size, and a word about animation.
    fn image_info_bar(&self, p: &ui::Palette) -> gpui::AnyElement {
        const SEPARATOR: &str = "·";
        let mut parts: Vec<String> = Vec::new();
        match (&self.pic.loaded, self.pic.failed) {
            (Some(l), _) => {
                parts.push(format!("{} × {}", l.size.0 as u64, l.size.1 as u64));
                parts.push(info::format_name(l.format).to_string());
                parts.push(info::size_label(l.bytes));
                match l.motion {
                    Motion::Frames(n) => parts.push(crate::i18n::tn("animated, {} frame", "animated, {} frames", n as usize, &[])),
                    Motion::FirstFrameOnly => parts.push(crate::i18n::t("animated PNG, first frame shown").to_string()),
                    Motion::Still => {}
                }
            }
            (None, Some((format, bytes))) => {
                parts.push(info::format_name(format).to_string());
                parts.push(info::size_label(bytes));
            }
            (None, None) => {}
        }
        let summary = parts.join(" · ");
        div()
            .h(px(24.))
            .flex_none()
            .flex()
            .items_center()
            .gap(px(theme::SPACE[3]))
            .px(px(theme::SPACE[6]))
            .border_t_1()
            .border_color(c(p.border))
            .overflow_hidden()
            .whitespace_nowrap()
            .text_size(px(text::XS.0))
            .text_color(c(p.text_muted))
            .relative()
            .child(crate::a11y_controls::said(self.path.clone(), "info", accesskit::Role::Label, summary))
            .children(parts.into_iter().enumerate().flat_map(|(i, part)| {
                let separator = (i > 0).then(|| div().flex_none().child(SEPARATOR).into_any_element());
                separator.into_iter().chain([div().flex_none().child(part).into_any_element()])
            }))
            .into_any_element()
    }
}

/// Alternating squares under the picture, where it is (the part inside `viewport`).
fn paint_checkerboard(window: &mut Window, viewport: Bounds<Pixels>, size: zoom::Size, pan: (f32, f32), colors: (u32, u32)) {
    let (vw, vh) = (pixels(viewport.size.width), pixels(viewport.size.height));
    let (left, top) = ((vw - size.0) / 2. + pan.0, (vh - size.1) / 2. + pan.1);
    // The part of the picture that is on screen, a pixel in from its own edges: the two are placed separately and may differ by a
    // fraction of a pixel, which would show the squares as a dashed line along an opaque picture.
    let edge = if size.0 >= 8. && size.1 >= 8. { 1. } else { 0. };
    let (x0, y0, x1, y1) = ((left + edge).max(0.), (top + edge).max(0.), (left + size.0 - edge).min(vw), (top + size.1 - edge).min(vh));
    if x1 <= x0 || y1 <= y0 {
        return;
    }
    let rect = |x: f32, y: f32, w: f32, h: f32| Bounds::new(viewport.origin + gpui::point(px(x), px(y)), gpui::size(px(w), px(h)));
    window.paint_quad(gpui::fill(rect(x0, y0, x1 - x0, y1 - y0), c(colors.0)));
    let (first_col, first_row) = (((x0 - left) / CHECKER).floor() as i32, ((y0 - top) / CHECKER).floor() as i32);
    let mut row = first_row;
    loop {
        let y = top + row as f32 * CHECKER;
        if y >= y1 {
            break;
        }
        let mut col = first_col;
        loop {
            let x = left + col as f32 * CHECKER;
            if x >= x1 {
                break;
            }
            if (row + col).rem_euclid(2) == 1 {
                let (sx, sy) = (x.max(x0), y.max(y0));
                let (ex, ey) = ((x + CHECKER).min(x1), (y + CHECKER).min(y1));
                window.paint_quad(gpui::fill(rect(sx, sy, ex - sx, ey - sy), c(colors.1)));
            }
            col += 1;
        }
        row += 1;
    }
}

#[cfg(test)]
#[path = "large_image_tests.rs"]
mod tests;
