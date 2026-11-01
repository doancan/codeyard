//! Editor: color picker. Opens by clicking the color strip under a color literal (hex, rgb(), hsl(), CSS name) or with
//! "Editor: Pick Color...": saturation/value square, hue and alpha strips, format buttons. On mouse release the change is
//! written in place of the literal as a single edit (single undo step), in the same form.

use gpui::{linear_color_stop, linear_gradient};

use super::*;
use crate::color::{self, Color, Notation};

const W: f32 = 232.;
const PAD: f32 = 12.;
const SV_W: f32 = W - PAD * 2.;
const SV_H: f32 = 132.;
const STRIP_H: f32 = 12.;
const GAP: f32 = 10.;
const FOOT_H: f32 = 56.;
const TOTAL_H: f32 = PAD * 2. + SV_H + GAP + STRIP_H + GAP + STRIP_H + GAP + FOOT_H;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Zone {
    Sv,
    Hue,
    Alpha,
}

pub(super) struct ColorPicker {
    /// Character range of the literal in the document.
    range: Range<usize>,
    notation: Notation,
    original: Color,
    color: Color,
    /// (hue, saturation, value): kept separately so hue isn't lost at gray/black.
    hsv: (f32, f32, f32),
    drag: Option<Zone>,
    /// Document version: if another edit happened the range is stale and the picker closes.
    version: u64,
}

impl EditorView {
    /// Color literal at the cursor (or touching it): (character range, found).
    pub(super) fn color_literal_at(&self, ix: usize) -> Option<(Range<usize>, color::Found)> {
        let line = self.buffer.char_to_line(ix);
        let text = self.buffer.line(line);
        let start = self.buffer.line_to_char(line);
        let named = color::named_colors_apply(&self.path);
        color::find_colors(&text, named).into_iter().find_map(|f| {
            let a = start + text[..f.range.start].chars().count();
            let b = start + text[..f.range.end].chars().count();
            (a <= ix && ix <= b).then_some((a..b, f))
        })
    }

    fn open_picker(&mut self, range: Range<usize>, found: &color::Found, cx: &mut Context<Self>) {
        let hsv = found.color.to_hsv();
        self.color_picker =
            Some(ColorPicker { range, notation: found.notation, original: found.color, color: found.color, hsv, drag: None, version: self.buffer.version() });
        self.completion = None;
        self.hover = None;
        cx.notify();
    }

    /// Open the picker if the color strip (bottom edge of the line) was clicked.
    pub(super) fn try_open_picker_at(&mut self, ev: &MouseDownEvent, ix: usize, cx: &mut Context<Self>) -> bool {
        if ev.click_count != 1 || !cx.global::<Settings>().color_decorators {
            return false;
        }
        let near_bar = self
            .visible
            .iter()
            .find(|v| ev.position.y >= v.bounds.top() && ev.position.y < v.bounds.bottom())
            .is_some_and(|v| ev.position.y >= v.bounds.bottom() - px(9.));
        if !near_bar {
            return false;
        }
        let Some((range, found)) = self.color_literal_at(ix) else { return false };
        if range.start == ix && range.end == ix {
            return false;
        }
        self.buffer.set_cursor(range.start, false);
        self.open_picker(range, &found, cx);
        true
    }

    /// "Editor: Pick Color...": open the color literal at the cursor in the picker; if none, write a new color at the cursor and open that.
    pub(super) fn pick_color(&mut self, _: &PickColor, _: &mut Window, cx: &mut Context<Self>) {
        let cursor = self.buffer.cursor();
        if let Some((range, found)) = self.color_literal_at(cursor) {
            return self.open_picker(range, &found, cx);
        }
        let seed = "#3b82f6";
        let at = self.buffer.selection().map_or(cursor..cursor, |s| s.start.min(s.end)..s.start.max(s.end));
        let start = at.start;
        self.buffer.replace_all(vec![(at, seed.to_string())]);
        self.buffer.set_cursor(start + seed.chars().count(), false);
        self.after_edit(cx);
        if let Some((range, found)) = self.color_literal_at(start) {
            self.open_picker(range, &found, cx);
        }
    }

    pub(super) fn close_picker(&mut self, cx: &mut Context<Self>) -> bool {
        let had = self.color_picker.take().is_some();
        if had {
            cx.notify();
        }
        had
    }

    fn live_picker(&self) -> Option<&ColorPicker> {
        self.color_picker.as_ref().filter(|p| p.version == self.buffer.version())
    }

    /// Top-left corner of the picker relative to the surface.
    fn picker_origin(&self) -> Option<Point<Pixels>> {
        let pk = self.live_picker()?;
        let (v, x) = self.screen_pos(pk.range.start)?;
        let x = (x - self.surface_origin.x).max(px(8.));
        let below = v.bounds.bottom() - self.surface_origin.y + px(4.);
        let above = v.bounds.top() - self.surface_origin.y - px(TOTAL_H + 4.);
        let y = if below + px(TOTAL_H) > self.surface_h && above >= px(0.) { above } else { below };
        Some(point(x, y))
    }

    fn zone_rect(zone: Zone) -> (f32, f32, f32, f32) {
        match zone {
            Zone::Sv => (PAD, PAD, SV_W, SV_H),
            Zone::Hue => (PAD, PAD + SV_H + GAP, SV_W, STRIP_H),
            Zone::Alpha => (PAD, PAD + SV_H + GAP + STRIP_H + GAP, SV_W, STRIP_H),
        }
    }

    /// Ratio within the region from the window position (0-1, 0-1).
    fn zone_fraction(&self, zone: Zone, pos: Point<Pixels>) -> Option<(f32, f32)> {
        let o = self.picker_origin()?;
        let (x, y, w, h) = Self::zone_rect(zone);
        let rel = pos - self.surface_origin - o;
        Some((((rel.x / px(1.)) - x).clamp(0., w) / w, ((rel.y / px(1.)) - y).clamp(0., h) / h))
    }

    fn apply_zone(&mut self, zone: Zone, pos: Point<Pixels>, cx: &mut Context<Self>) {
        let Some((fx, fy)) = self.zone_fraction(zone, pos) else { return };
        let Some(pk) = &mut self.color_picker else { return };
        let (h, s, v) = pk.hsv;
        pk.hsv = match zone {
            Zone::Sv => (h, fx, 1. - fy),
            Zone::Hue => ((fx * 360.).min(359.99), s, v),
            Zone::Alpha => (h, s, v),
        };
        let alpha = if zone == Zone::Alpha { fx } else { pk.color.a };
        pk.color = Color::from_hsv(pk.hsv.0, pk.hsv.1, pk.hsv.2, alpha);
        cx.notify();
    }

    fn begin_zone(&mut self, zone: Zone, pos: Point<Pixels>, cx: &mut Context<Self>) {
        if let Some(pk) = &mut self.color_picker {
            pk.drag = Some(zone);
        }
        self.apply_zone(zone, pos, cx);
    }

    /// While dragging (outside the editor too): update the color.
    pub(super) fn picker_drag_move(&mut self, pos: Point<Pixels>, pressed: bool, cx: &mut Context<Self>) -> bool {
        let Some(zone) = self.color_picker.as_ref().and_then(|p| p.drag) else { return false };
        if pressed {
            self.apply_zone(zone, pos, cx);
        } else {
            self.finish_picker_drag(cx);
        }
        true
    }

    /// Mouse released: the color is written as a single edit.
    pub(super) fn finish_picker_drag(&mut self, cx: &mut Context<Self>) {
        if self.color_picker.as_mut().and_then(|p| p.drag.take()).is_some() {
            self.commit_color(cx);
        }
    }

    /// Write the chosen color in place of the literal, in the original form (if the field changed). The picker stays open.
    pub(super) fn commit_color(&mut self, cx: &mut Context<Self>) {
        let Some(pk) = &self.color_picker else { return };
        if pk.version != self.buffer.version() {
            self.color_picker = None;
            return;
        }
        let text = color::format(pk.color, pk.notation);
        let current = self.buffer.text_range(pk.range.clone());
        if text == current {
            return;
        }
        let start = pk.range.start;
        self.buffer.replace_all(vec![(pk.range.clone(), text.clone())]);
        self.after_edit(cx);
        let version = self.buffer.version();
        if let Some(pk) = &mut self.color_picker {
            pk.range = start..start + text.chars().count();
            pk.version = version;
            pk.original = pk.color;
        }
        cx.notify();
    }

    /// One channel set from the tree (0 hue in degrees, 1 saturation, 2 brightness, 3 opacity, in percent), written
    /// as a release of the mouse writes it.
    fn set_channel(&mut self, channel: u8, value: f64, cx: &mut Context<Self>) {
        let Some(pk) = &mut self.color_picker else { return };
        let (mut h, mut s, mut v) = pk.hsv;
        let mut a = pk.color.a;
        let x = value as f32;
        match channel {
            0 => h = x.clamp(0., 359.99),
            1 => s = (x / 100.).clamp(0., 1.),
            2 => v = (x / 100.).clamp(0., 1.),
            _ => a = (x / 100.).clamp(0., 1.),
        }
        pk.hsv = (h, s, v);
        pk.color = Color::from_hsv(h, s, v, a);
        self.commit_color(cx);
        cx.notify();
    }

    /// The picker's planes for the tree: a slider per channel over the region a drag sets it in, and the value.
    fn picker_sliders(&self, h: f32, s: f32, v: f32, a: f32, value: &str, cx: &Context<Self>) -> impl IntoElement + use<> {
        let weak = cx.entity().downgrade();
        let slider = |channel: u8, key: &'static str, label: &'static str, now: f32, max: f64, step: f64| {
            let weak = weak.clone();
            let zone = match channel {
                0 => Zone::Hue,
                3 => Zone::Alpha,
                _ => Zone::Sv,
            };
            let (x, y, w, hh) = Self::zone_rect(zone);
            div().absolute().left(px(x)).top(px(y)).w(px(w)).h(px(hh)).child(crate::a11y_controls::control(
                crate::a11y_controls::Control::new(crate::i18n::t("Color picker"), key, accesskit::Role::Slider, label).slider(
                    crate::a11y_controls::Range { value: now as f64, min: 0., max, step },
                    move |to, _, cx| {
                        weak.update(cx, |e, cx| e.set_channel(channel, to, cx)).ok();
                    },
                ),
            ))
        };
        div()
            .absolute()
            .size_full()
            .child(slider(0, "hue", crate::i18n::t("Hue"), h.round(), 359., 5.))
            .child(slider(1, "saturation", crate::i18n::t("Saturation"), (s * 100.).round(), 100., 5.))
            .child(slider(2, "brightness", crate::i18n::t("Brightness"), (v * 100.).round(), 100., 5.))
            .child(slider(3, "opacity", crate::i18n::t("Opacity"), (a * 100.).round(), 100., 5.))
            .child(
                div()
                    .absolute()
                    .left(px(PAD))
                    .top(px(PAD + SV_H + GAP + STRIP_H + GAP + STRIP_H + GAP))
                    .w(px(SV_W))
                    .h(px(FOOT_H / 2.))
                    .child(crate::a11y_controls::said(crate::i18n::t("Color picker"), "value", accesskit::Role::Label, value.to_string())),
            )
    }

    fn set_notation(&mut self, notation: Notation, cx: &mut Context<Self>) {
        if let Some(pk) = &mut self.color_picker {
            pk.notation = notation;
        }
        self.commit_color(cx);
        // Refresh the view when the format changes even if the text doesn't (e.g. clicking the same format).
        cx.notify();
    }

    /// Picker window (if any): added to the `popups` list.
    pub(super) fn color_picker_popup(&self, p: &Palette, font: SharedString, font_size: f32, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        let pk = self.live_picker()?;
        let origin = self.picker_origin()?;
        let (h, s, v) = pk.hsv;
        let col = pk.color;
        let hue_rgb = Color::from_hsv(h, 1., 1., 1.).rgb();
        let on = |zone: Zone| {
            cx.on(move |e, ev: &MouseDownEvent, _, cx| {
                cx.stop_propagation();
                e.begin_zone(zone, ev.position, cx);
            })
        };
        let place = |zone: Zone| {
            let (x, y, w, hh) = Self::zone_rect(zone);
            div().absolute().left(px(x)).top(px(y)).w(px(w)).h(px(hh))
        };
        let outline = ui::ca(p.text, 0.25);
        // Thumbs sit outside the clipped container (so they aren't cut off at the edges): positioned relative to the root.
        let thumb = |zone: Zone, fx: f32, fy: f32| {
            let (zx, zy, zw, zh) = Self::zone_rect(zone);
            div()
                .absolute()
                .left(px(zx + fx * zw - 6.))
                .top(px(zy + fy * zh - 6.))
                .size(px(12.))
                .rounded_full()
                .border_2()
                .border_color(gpui::white())
                .shadow_sm()
        };
        let sv = place(Zone::Sv)
            .rounded(px(4.))
            .overflow_hidden()
            .bg(ui::c(hue_rgb))
            .child(div().absolute().size_full().bg(linear_gradient(
                90.,
                linear_color_stop(ui::ca(0xffffff, 1.), 0.),
                linear_color_stop(ui::ca(0xffffff, 0.), 1.),
            )))
            .child(div().absolute().size_full().bg(linear_gradient(
                180.,
                linear_color_stop(ui::ca(0x000000, 0.), 0.),
                linear_color_stop(ui::ca(0x000000, 1.), 1.),
            )))
            .on_mouse_down(MouseButton::Left, on(Zone::Sv));
        let hue_strip = {
            let segs = (0..6).map(|k| {
                let (a, b) = (Color::from_hsv(k as f32 * 60., 1., 1., 1.).rgb(), Color::from_hsv((k + 1) as f32 * 60., 1., 1., 1.).rgb());
                div().flex_1().h_full().bg(linear_gradient(90., linear_color_stop(ui::c(a), 0.), linear_color_stop(ui::c(b), 1.)))
            });
            place(Zone::Hue)
                .rounded(px(STRIP_H / 2.))
                .border_1()
                .border_color(outline)
                .overflow_hidden()
                .flex()
                .children(segs)
                .on_mouse_down(MouseButton::Left, on(Zone::Hue))
        };
        let rgb = col.rgb();
        let alpha_strip = place(Zone::Alpha)
            .rounded(px(STRIP_H / 2.))
            .border_1()
            .border_color(outline)
            .overflow_hidden()
            .bg(ui::c(p.bg))
            .child(div().absolute().size_full().bg(linear_gradient(90., linear_color_stop(ui::ca(rgb, 0.), 0.), linear_color_stop(ui::ca(rgb, 1.), 1.))))
            .on_mouse_down(MouseButton::Left, on(Zone::Alpha));
        let value = color::format(col, pk.notation);
        let sliders = self.picker_sliders(h, s, v, col.a, &value, cx);
        let active = |n: &Notation| {
            std::mem::discriminant(&pk.notation) == std::mem::discriminant(n) || (matches!(pk.notation, Notation::Named) && matches!(n, Notation::Hex { .. }))
        };
        let seg = |id: &'static str, label: &'static str, n: Notation| {
            let is = active(&n);
            div()
                .id(id)
                .relative()
                .child(crate::a11y_controls::control(
                    crate::a11y_controls::Control::new(crate::i18n::t("Color picker"), id, accesskit::Role::RadioButton, label)
                        .selected(is)
                        .on_entity(cx.entity().downgrade(), move |e: &mut Self, _, cx| e.set_notation(n, cx)),
                ))
                .px(px(theme::SPACE[3]))
                .h(px(20.))
                .flex()
                .items_center()
                .rounded(px(4.))
                .cursor_pointer()
                .text_size(px(text::XS.0))
                .when(is, |d| d.bg(ui::ca(p.accent, 0.22)).text_color(ui::c(p.accent)))
                .when(!is, |d| d.text_color(ui::c(p.text_muted)).hover(|d| d.bg(ui::c(p.surface_hover))))
                .child(label)
                .on_mouse_down(
                    MouseButton::Left,
                    cx.on(move |e, _: &MouseDownEvent, _, cx| {
                        cx.stop_propagation();
                        e.set_notation(n, cx);
                    }),
                )
        };
        let (orig, now) = (pk.original, col);
        let preview = div()
            .flex_none()
            .w(px(28.))
            .h(px(20.))
            .rounded(px(4.))
            .border_1()
            .border_color(outline)
            .overflow_hidden()
            .flex()
            .child(div().flex_1().h_full().bg(ui::ca(orig.rgb(), orig.a)))
            .child(div().flex_1().h_full().bg(ui::ca(now.rgb(), now.a)));
        let footer = div()
            .absolute()
            .left(px(PAD))
            .top(px(PAD + SV_H + GAP + STRIP_H + GAP + STRIP_H + GAP))
            .w(px(SV_W))
            .h(px(FOOT_H))
            .flex()
            .flex_col()
            .justify_between()
            .child(
                div().flex().items_center().gap(px(theme::SPACE[3])).child(preview).child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .font_family(font)
                        .text_size(px(font_size - 1.))
                        .text_color(ui::c(p.text))
                        .child(value),
                ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(theme::SPACE[2]))
                    .child(seg("color-hex", "HEX", Notation::Hex { upper: false }))
                    .child(seg("color-rgb", "RGB", Notation::Rgb { legacy: true }))
                    .child(seg("color-hsl", "HSL", Notation::Hsl { legacy: true })),
            );
        Some(
            div()
                .absolute()
                .left(origin.x)
                .top(origin.y)
                .w(px(W))
                .h(px(TOTAL_H))
                .bg(ui::c(p.surface_raised))
                .border_1()
                .border_color(ui::c(p.border))
                .rounded(px(theme::radius::MD))
                .shadow_md()
                .on_mouse_down(MouseButton::Left, cx.on(|_, _: &MouseDownEvent, _, cx| cx.stop_propagation()))
                .child(sv)
                .child(hue_strip)
                .child(alpha_strip)
                .child(sliders)
                .child(thumb(Zone::Sv, s, 1. - v).on_mouse_down(MouseButton::Left, on(Zone::Sv)))
                .child(thumb(Zone::Hue, h / 360., 0.5).on_mouse_down(MouseButton::Left, on(Zone::Hue)))
                .child(thumb(Zone::Alpha, col.a, 0.5).on_mouse_down(MouseButton::Left, on(Zone::Alpha)))
                .child(footer)
                .into_any_element(),
        )
    }

    #[cfg(test)]
    pub fn picker_state_for_test(&self) -> Option<(String, String)> {
        self.live_picker().map(|p| (color::format(p.color, p.notation), self.buffer.text_range(p.range.clone())))
    }

    /// Test: set the color in the picker (written as if the mouse was released).
    #[cfg(test)]
    pub fn picker_set_for_test(&mut self, h: f32, s: f32, v: f32, a: f32, cx: &mut Context<Self>) {
        if let Some(p) = &mut self.color_picker {
            p.hsv = (h, s, v);
            p.color = Color::from_hsv(h, s, v, a);
        }
        self.commit_color(cx);
    }

    #[cfg(test)]
    pub fn picker_notation_for_test(&mut self, n: Notation, cx: &mut Context<Self>) {
        self.set_notation(n, cx);
    }
}
