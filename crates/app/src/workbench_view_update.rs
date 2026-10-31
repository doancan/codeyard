//! Workbench: rendering: the update dialog and its status bar item. What the dialog says is decided in `workbench_update.rs`
//! (`dialog_model`); this file only draws it.

use std::cell::Cell;
use std::rc::Rc;

use gpui::{Animation, AnimationExt as _, ElementId, relative};

use super::update_glue::{Body, DialogButton, DialogModel, MAX_BUTTONS, StatusItem, Tone, UpdateCmd};
use super::*;
use crate::ui::ca;

/// How the card talks to the workbench: it sends what was asked, the receiver runs it in the window the editor lives in.
/// `true`: the key was used.
pub(super) type Dispatch = Rc<dyn Fn(UpdateCmd, &mut Window, &mut App) -> bool>;

/// Width of the card: the release notes need room, the other states do not.
const NARROW_W: f32 = 440.;
const WIDE_W: f32 = 560.;
const NOTES_MAX_H: f32 = 260.;
/// The release notes in the dialog's own window: the box stops here and scrolls.
pub(super) const NOTES_BOX_MAX: f32 = 200.;
pub(super) const NOTES_PAD: f32 = 10.;
/// Space around the content of the dialog's own window, and between its rows.
pub(super) const WINDOW_INSET: f32 = 20.;
pub(super) const WINDOW_GAP: f32 = 16.;
/// The height of a row of buttons.
pub(super) const FOOTER_H: f32 = 30.;
const ICON_BOX: f32 = 36.;
/// The app icon in the dialog's own window.
pub(super) const ICON_TILE: f32 = 64.;
/// Width of the small glyph before a title.
const LEAD_W: f32 = 16.;
const BAR_H: f32 = 6.;
/// The progress bar of the dialog's own window: thin, like the system's.
const NATIVE_BAR_H: f32 = 5.;

/// Accent of the update dialog and its buttons and bars: the theme's own on light backgrounds, the brand's mint green on dark ones
/// (the same mark colour as the icon tile), with dark text on it. Returns (accent, text on accent).
fn accent_pair(p: &Palette) -> (u32, u32) {
    if ui::luminance_is_light(p.bg) { (p.accent, p.on_accent) } else { (super::welcome_glue::brand_colors(p).2, 0x0b1f1a) }
}

fn tone_color(tone: Tone, p: &Palette) -> u32 {
    match tone {
        Tone::Accent => accent_pair(p).0,
        Tone::Success => p.success,
        Tone::Danger => p.danger,
    }
}

/// Spinner for `loader`, otherwise the named icon.
fn glyph(name: &'static str, size: f32, color: u32, id: &'static str) -> AnyElement {
    if name == "loader" { ui::spinner(id, size, color).into_any_element() } else { icon(name, size, color).into_any_element() }
}

impl Workbench {
    /// The modal card with a scrim behind it, or `None` while no dialog is open (or it is open in its own window).
    pub(super) fn update_dialog(&self, p: &Palette, cx: &mut Context<Self>) -> Option<impl IntoElement + use<>> {
        if self.update.native {
            return None;
        }
        let model = self.dialog_model(cx)?;
        let weak = cx.entity().downgrade();
        let send: Dispatch = Rc::new(move |cmd, window, app| weak.update(app, |wb, cx| wb.update_cmd(cmd, window, cx)).unwrap_or(false));
        let wide = matches!(model.body, Body::Notes { .. });
        let tone = tone_color(model.tone, p);
        let header = div()
            .flex()
            .items_start()
            .gap(px(theme::SPACE[6]))
            .child(
                div()
                    .size(px(ICON_BOX))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded_full()
                    .bg(c(ui::mix(p.surface_raised, tone, 0.18)))
                    .child(glyph(model.icon, 18., tone, "update-dialog-spinner")),
            )
            .child(title_block(&model, p, None, false));
        let card = wire(
            div()
                .id("update-dialog")
                .w(px(if wide { WIDE_W } else { NARROW_W }))
                .max_w(relative(0.92))
                .max_h(relative(0.9))
                .flex()
                .flex_col()
                .gap(px(theme::SPACE[7]))
                .p(px(theme::SPACE[8]))
                .bg(c(p.surface_raised))
                .border_1()
                .border_color(c(p.border))
                .rounded(px(theme::radius::LG))
                .shadow_lg()
                .text_size(px(text::BASE.0))
                .text_color(c(p.text))
                .occlude(),
            self,
            &model,
            &send,
            None,
        )
        .child(header)
        .child(update_body(self, &model, p, &send, false))
        .child(update_footer(self, &model, p, &send, false, ui::keyboard_focus(cx)));
        Some(deferred(
            div()
                .id("update-backdrop")
                .absolute()
                .top_0()
                .left_0()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .bg(ca(0x000000, 0.45))
                .occlude()
                .child(card),
        ))
    }

    /// The status bar item: click opens the dialog.
    pub(super) fn update_status_button(&self, item: StatusItem, show_label: bool, p: &Palette, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let color = tone_color(item.tone, p);
        let tip = item.tip.clone();
        div()
            .id("sb-update")
            .debug_selector(|| "sb-update".into())
            .relative()
            .child(super::a11y::part_frame(super::a11y::Part::Status(super::a11y::StatusPart::Update), cx))
            .flex_none()
            .min_w(px(32.))
            .h_full()
            .px(px(theme::SPACE[4]))
            .flex()
            .items_center()
            .gap(px(theme::SPACE[2]))
            .cursor_pointer()
            .hover(|d| d.bg(c(p.surface_hover)))
            .text_color(c(color))
            .child(glyph(item.icon, 14., color, "sb-update-spinner"))
            .when(show_label, |d| d.child(div().max_w(px(100.)).truncate().child(item.label)))
            .tooltip(move |_, cx| cx.new(|_| Tooltip(tip.clone())).into())
            .on_click(cx.listener(|this, _, window, cx| this.open_update_dialog(window, cx)))
    }
}

/// Title and subtitle, the part of the header both layouts share. `lead` is a small glyph before the title; the subtitle lines up
/// with the title's text, not with the glyph. `native`: the sizes of a Mac alert (13 semibold and 11), not the card's.
fn title_block(model: &DialogModel, p: &Palette, lead: Option<AnyElement>, native: bool) -> impl IntoElement + use<> {
    let (size, line, sub, sub_line) = if native { (13., 16., 11., 14.) } else { (text::LG.0, text::LG.1, text::SM.0, text::SM.1) };
    let indent = if lead.is_some() { LEAD_W + theme::SPACE[4] } else { 0. };
    div()
        .flex_1()
        .min_w_0()
        .flex()
        .flex_col()
        .gap(px(if native { 2. } else { theme::SPACE[1] }))
        .child(
            div()
                .flex()
                .items_start()
                .gap(px(theme::SPACE[4]))
                .children(lead.map(|l| div().flex_none().w(px(LEAD_W)).h(px(line)).flex().items_center().child(l)))
                .child(
                    div()
                        .id("update-title")
                        .flex_1()
                        .min_w_0()
                        .text_size(px(size))
                        .line_height(px(line))
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .child(model.title.clone()),
                ),
        )
        .when_some(model.subtitle.clone(), |d, s| {
            d.child(div().pl(px(indent)).text_size(px(sub)).line_height(px(sub_line)).text_color(c(p.text_muted)).child(s))
        })
}

/// What both layouts do with keys and clicks: the key context, Esc, Enter/Space, Tab, and a click on the text keeping the
/// keyboard inside the dialog instead of leaving it in whatever had it before. `keys`: the dialog's own window, which draws the
/// focus ring only after the keyboard was used (Tab turns it on, a click turns it off).
fn wire(card: gpui::Stateful<gpui::Div>, wb: &Workbench, model: &DialogModel, send: &Dispatch, keys: Option<&Rc<Cell<bool>>>) -> gpui::Stateful<gpui::Div> {
    let handles = wb.update.focus.clone();
    let notes = wb.update.notes_focus.clone();
    let primary = model.buttons.iter().position(|b| b.primary).unwrap_or(0);
    let (dismiss, confirm, quit) = (send.clone(), send.clone(), send.clone());
    let confirm_handles = handles.clone();
    let (next_keys, prev_keys, click_keys) = (keys.cloned(), keys.cloned(), keys.cloned());
    let by_keyboard = |keys: &Option<Rc<Cell<bool>>>, window: &mut Window, on: bool| {
        if let Some(k) = keys
            && k.replace(on) != on
        {
            window.refresh();
        }
    };
    card.key_context("UpdateDialog")
        .on_action(move |_: &super::UpdateDismiss, window, app| {
            dismiss(UpdateCmd::Dismiss, window, app);
        })
        .on_action(move |_: &super::UpdateConfirm, window, app| {
            let focused = confirm_handles.iter().position(|h| h.is_focused(window));
            confirm(UpdateCmd::Confirm(focused), window, app);
        })
        .on_action(move |_: &super::Quit, window, app| {
            quit(UpdateCmd::Quit, window, app);
        })
        .on_action(move |_: &super::UpdateFocusNext, window, _| {
            by_keyboard(&next_keys, window, true);
            window.focus_next();
        })
        .on_action(move |_: &super::UpdateFocusPrev, window, _| {
            by_keyboard(&prev_keys, window, true);
            window.focus_prev();
        })
        .on_mouse_down(MouseButton::Left, move |_, window, app| {
            by_keyboard(&click_keys, window, false);
            if !handles.iter().chain([&notes]).any(|h| h.contains_focused(window, app))
                && let Some(h) = handles.get(primary)
            {
                window.focus(h);
            }
        })
}

/// `native`: the layout of the dialog's own window, which is small and tight: the notes are set small, sit on a fill instead of
/// in a frame, and stop at `NOTES_BOX_MAX` (the window is as high as they need, up to that). Otherwise the card's larger layout.
fn update_body(wb: &Workbench, model: &DialogModel, p: &Palette, send: &Dispatch, native: bool) -> AnyElement {
    let muted = |d: gpui::Div| {
        d.text_size(px(if native { text::XXS.0 } else { text::SM.0 })).line_height(px(if native { 14. } else { text::SM.1 })) // scale-keep: native alert caption line.text_color(c(p.text_muted))
    };
    let (body_size, body_line) = if native { (13., 18.) } else { (text::SM.0, text::SM.1 + 2.) };
    match &model.body {
        Body::None => div().into_any_element(),
        Body::Paragraphs(lines) => div()
            .flex()
            .flex_col()
            .gap(px(if native { 8. } else { theme::SPACE[4] }))
            .text_size(px(body_size))
            .line_height(px(body_line))
            .children(lines.iter().map(|l| div().child(l.clone())))
            .into_any_element(),
        Body::Notes { note, .. } => {
            let keys = send.clone();
            let notes_focus = wb.update.notes_focus.clone();
            div()
                .min_h_0()
                .flex()
                .flex_col()
                .gap(px(if native { theme::SPACE[4] } else { theme::SPACE[5] }))
                .when_some(note.clone(), |d, n| {
                    d.child(
                        div()
                            .flex()
                            .items_start()
                            .gap(px(theme::SPACE[4]))
                            .p(px(theme::SPACE[5]))
                            .rounded(px(if native { 6. } else { theme::radius::MD }))
                            .bg(c(ui::mix(p.surface_raised, p.warning, 0.12)))
                            .text_size(px(if native { text::XS.0 } else { text::SM.0 }))
                            .child(div().pt(px(2.)).child(icon("info", 14., p.warning)))
                            .child(div().flex_1().min_w_0().child(n)),
                    )
                })
                .child(
                    // Focusable so that the keyboard can read what the mouse wheel can: Tab reaches it before the buttons.
                    div()
                        .id("update-notes")
                        .track_focus(&wb.update.notes_focus)
                        .track_scroll(&wb.update.notes_scroll)
                        .border_1()
                        .border_color(if native { gpui::transparent_black() } else { c(p.border).into() })
                        .focus(|s| s.border_color(c(p.focus_ring)))
                        .on_key_down(move |event: &gpui::KeyDownEvent, window, app| {
                            if keys(UpdateCmd::NotesKey(event.keystroke.key.clone()), window, app) {
                                app.stop_propagation();
                            }
                        })
                        .on_mouse_down(MouseButton::Left, move |_, window, _| window.focus(&notes_focus))
                        .min_h_0()
                        .max_h(px(if native { NOTES_BOX_MAX } else { NOTES_MAX_H }))
                        .overflow_y_scroll()
                        .p(px(if native { NOTES_PAD } else { theme::SPACE[6] }))
                        .rounded(px(if native { 6. } else { theme::radius::MD }))
                        .bg(c(if native { p.surface } else { p.bg }))
                        .flex()
                        .flex_col()
                        .gap(px(if native { 0. } else { theme::SPACE[3] }))
                        .text_size(px(text::SM.0))
                        .children(if native {
                            crate::markdown::compact_document_view(&wb.update.notes, "update-notes", p)
                        } else {
                            crate::markdown::document_view(&wb.update.notes, "update-notes", p)
                        }),
                )
                .into_any_element()
        }
        Body::Progress { fraction, caption } => div()
            .flex()
            .flex_col()
            .gap(px(if native { 6. } else { theme::SPACE[3] }))
            .child(progress_bar(*fraction, p, native))
            .child(muted(div()).child(caption.clone()))
            .into_any_element(),
        Body::Working(text) => div()
            .flex()
            .flex_col()
            .gap(px(if native { 6. } else { theme::SPACE[3] }))
            .child(progress_bar(None, p, native))
            .child(muted(div()).child(text.clone()))
            .into_any_element(),
        Body::Failure { lead, detail } => div()
            .flex()
            .flex_col()
            .gap(px(if native { theme::SPACE[4] } else { theme::SPACE[5] }))
            .child(div().text_size(px(body_size)).line_height(px(body_line)).child(lead.clone()))
            .child(
                div()
                    .p(px(if native { NOTES_PAD } else { theme::SPACE[4] }))
                    .rounded(px(if native { 6. } else { theme::radius::SM }))
                    .when(!native, |d| d.border_1().border_color(c(p.border)))
                    .bg(c(if native { p.surface } else { p.bg }))
                    .font_family(ui::MONO_FONT)
                    .text_size(px(if native { text::XXS.0 } else { text::XS.0 }))
                    .line_height(px(if native { 14. } else { 16. }))
                    .text_color(c(p.text_muted))
                    .child(detail.clone()),
            )
            .into_any_element(),
    }
}

/// `ring`: draw the keyboard focus ring (shown only after the keyboard was used, in the dialog's own window and in the workbench).
fn update_footer(wb: &Workbench, model: &DialogModel, p: &Palette, send: &Dispatch, native: bool, ring: bool) -> impl IntoElement + use<> {
    let draw = |ix: usize, b: &DialogButton| {
        let (action, send) = (b.action, send.clone());
        update_button(ix, b, wb.update.focus.get(ix), p, native, ring).on_click(move |_, window, app| {
            send(UpdateCmd::Action(action), window, app);
        })
    };
    let start = model.buttons.iter().enumerate().filter(|(_, b)| b.start).map(|(i, b)| draw(i, b)).collect::<Vec<_>>();
    let end = model.buttons.iter().enumerate().filter(|(_, b)| !b.start).map(|(i, b)| draw(i, b)).collect::<Vec<_>>();
    // A native button sits in a 2 px ring slot on every side (so the ring does not move anything), which makes the visible gap
    // twice that; the outer groups reach back over the slot so the buttons line up with the content.
    let (gap, pull) = if native { (0., -(RING_SLOT)) } else { (theme::SPACE[2], 0.) };
    div()
        .flex()
        .items_center()
        .gap(px(gap))
        .child(div().flex().items_center().gap(px(gap)).ml(px(pull)).children(start))
        .child(div().flex_1())
        .child(div().flex().items_center().gap(px(gap)).mr(px(pull)).children(end))
}

/// The dialog as the content of a window of its own, laid out like the update windows of other Mac apps: the app's icon on the
/// left, the title and what there is to say beside it, the buttons below. Nothing is framed: the content sits on the window.
pub(super) fn window_card(wb: &Workbench, model: &DialogModel, p: &Palette, send: &Dispatch, keys: &Rc<Cell<bool>>) -> AnyElement {
    let (tile, _, mark) = super::welcome_glue::brand_colors(p);
    let tone = tone_color(model.tone, p);
    // A spinner while something is going on, a mark for the end of a check; the plain "update available" needs neither.
    let status = (model.icon == "loader" || model.tone != Tone::Accent).then(|| glyph(model.icon, 14., tone, "update-window-spinner"));
    wire(
        div()
            .id("update-window")
            .size_full()
            .flex()
            .flex_col()
            .gap(px(WINDOW_GAP))
            .p(px(WINDOW_INSET))
            .bg(c(p.bg))
            .font_family(ui::UI_FONT)
            .text_size(px(text::SM.0))
            .text_color(c(p.text)),
        wb,
        model,
        send,
        Some(keys),
    )
    .child(
        div()
            .flex_1()
            .min_h_0()
            .flex()
            .gap(px(WINDOW_GAP))
            .child(div().flex_none().size(px(ICON_TILE)).flex().items_center().justify_center().rounded(px(ICON_TILE * 0.23)).bg(c(tile)).child(icon(
                "codeyard-mark",
                ICON_TILE * 0.56,
                mark,
            )))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .min_h_0()
                    .flex()
                    .flex_col()
                    .gap(px(theme::SPACE[5]))
                    .child(title_block(model, p, status, true))
                    .child(update_body(wb, model, p, send, true)),
            ),
    )
    .child(update_footer(wb, model, p, send, true, keys.get()))
    .into_any_element()
}

/// Room around a native button for its focus ring.
const RING_SLOT: f32 = 4.;

/// A button the keyboard can reach: the outer element holds the focus and draws the ring (a ring in the accent colour would vanish
/// on the primary button's own background, so it sits outside it). `native`: a Mac push button: 22 px high, 13 px text, a soft fill
/// and no frame (the primary one is the accent colour and the default button of the window).
fn update_button(ix: usize, b: &DialogButton, handle: Option<&FocusHandle>, p: &Palette, native: bool, ring: bool) -> gpui::Stateful<gpui::Div> {
    debug_assert!(ix < MAX_BUTTONS);
    let slot = div()
        .id(ElementId::Integer(ix as u64 + 1000))
        .p(px(2.))
        .rounded(px(if native { 8. } else { theme::radius::MD }))
        .border_2()
        .border_color(gpui::transparent_black());
    let slot = match handle {
        Some(h) if ring => slot.track_focus(h).focus(|s| s.border_color(c(p.focus_ring))),
        Some(h) => slot.track_focus(h),
        None => slot,
    };
    if native {
        let light = ui::luminance_is_light(p.bg);
        let (fill, hover, pressed, fg) = if b.primary {
            {
                let (accent, on) = accent_pair(p);
                (accent, ui::mix(accent, 0xffffff, 0.12), ui::mix(accent, 0x000000, 0.18), on)
            }
        } else {
            (p.surface_hover, p.surface_active, ui::mix(p.surface_hover, 0x000000, 0.14), p.text)
        };
        return slot.cursor_pointer().child(
            div()
                .id(ElementId::Integer(ix as u64 + 2000))
                .h(px(22.))
                .min_w(px(70.))
                .px(px(13.))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(6.))
                .text_size(px(text::SM.0))
                .line_height(px(22.))
                .text_color(c(fg))
                .when(b.primary, |d| d.font_weight(gpui::FontWeight::MEDIUM))
                .bg(c(fill))
                .when(light && !b.primary, |d| d.shadow_sm())
                .hover(move |d| d.bg(c(hover)))
                .active(move |d| d.bg(c(pressed)))
                .child(b.label.clone()),
        );
    }
    let (bg, fg) = if b.primary {
        (Some(accent_pair(p).0), accent_pair(p).1)
    } else if b.start {
        (None, p.text_muted)
    } else {
        (Some(p.surface_hover), p.text)
    };
    slot.cursor_pointer().child(
        div()
            .h(px(26.))
            .px(px(theme::SPACE[5]))
            .flex()
            .items_center()
            .rounded(px(theme::radius::SM))
            .text_size(px(text::SM.0))
            .text_color(c(fg))
            .when(b.primary, |d| d.font_weight(gpui::FontWeight::MEDIUM))
            .when_some(bg, |d, bg| d.bg(c(bg)))
            .when(bg.is_none(), |d| d.hover(|d| d.bg(c(p.surface_hover))))
            .when(bg.is_some(), |d| d.hover(|d| d.opacity(0.9)))
            .child(b.label.clone()),
    )
}

/// A bar that fills as `fraction` grows; without a fraction a short piece slides back and forth (with reduced motion
/// it stands still in the middle).
fn progress_bar(fraction: Option<f32>, p: &Palette, native: bool) -> impl IntoElement + use<> {
    let (height, track) = if native { (NATIVE_BAR_H, ui::mix(p.bg, p.text, 0.16)) } else { (BAR_H, ui::mix(p.surface_raised, p.text_muted, 0.25)) };
    let track = div().h(px(height)).w_full().rounded_full().overflow_hidden().bg(c(track));
    match fraction {
        Some(f) => track.child(div().h_full().w(relative(f.clamp(0., 1.))).rounded_full().bg(c(accent_pair(p).0))).into_any_element(),
        None if crate::system_prefs::motion_reduced() => {
            track.child(div().h_full().w(relative(0.3)).ml(relative(0.35)).rounded_full().bg(c(accent_pair(p).0))).into_any_element()
        }
        None => track
            .child(div().h_full().w(relative(0.3)).rounded_full().bg(c(accent_pair(p).0)).with_animation(
                "update-indeterminate",
                Animation::new(std::time::Duration::from_millis(1400)).repeat(),
                |bar, t| {
                    // 0 -> 1 -> 0 over one period, so the piece turns around at the ends.
                    let x = 1. - (2. * t - 1.).abs();
                    bar.ml(relative(0.7 * x))
                },
            ))
            .into_any_element(),
    }
}
