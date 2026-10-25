//! The update dialog in a window of its own (macOS). What it says and what its buttons do are decided in `workbench_update.rs`;
//! `workbench_view_update.rs` draws the card. This file is the window around it: it opens when a dialog is on, closes when
//! there is none, and sends what is asked in it to the window the editor lives in.

use std::cell::Cell;
use std::rc::Rc;

use gpui::{Bounds, TitlebarOptions, WeakEntity, WindowBounds, WindowHandle, WindowKind, WindowOptions, size};

use super::update_glue::DialogModel;
use super::update_glue::{Body, Dialog, window_is_open};
use super::view_update_glue::{Dispatch, FOOTER_H, ICON_TILE, NOTES_BOX_MAX, NOTES_PAD, WINDOW_GAP, WINDOW_INSET, window_card};
use super::*;

const WIDTH: f32 = 560.;

/// Lines `text` takes at about `per_line` characters across (a guess: the text is not measured before the window is made).
fn lines_of(text: &str, per_line: usize) -> usize {
    text.lines().map(|l| l.chars().count().div_ceil(per_line).max(1)).sum::<usize>().max(1)
}

/// Height of the release notes box: what they need, up to `NOTES_BOX_MAX`, where the box scrolls.
fn notes_box_height(notes: &[crate::markdown::Block]) -> f32 {
    use crate::markdown::Block;
    let content: f32 = notes
        .iter()
        .map(|b| {
            let text = crate::markdown::plain_text(std::slice::from_ref(b));
            match b {
                Block::Heading(..) => 6. + 2. + 17.,
                Block::Code { .. } => lines_of(&text, 66) as f32 * 16. + 16.,
                Block::Table { .. } => lines_of(&text, 66) as f32 * 24.,
                Block::List { .. } => lines_of(&text, 60) as f32 * 18.,
                _ => lines_of(&text, 64) as f32 * 18.,
            }
        })
        .sum();
    (content + 2. * NOTES_PAD + 2.).clamp(40., NOTES_BOX_MAX)
}

/// The window's height for what it has to show. It cannot grow with the text once it is made, so it is worked out from the
/// dialog's text: the title block beside the icon, the body under it, and a row of buttons.
fn window_height(wb: &Workbench, model: &DialogModel) -> f32 {
    let title = 16. + if model.subtitle.is_some() { 2. + 14. } else { 0. };
    // The column beside the icon is 440 wide: about 68 characters of 13 px text.
    let body = match &model.body {
        Body::None => 0.,
        Body::Working(_) | Body::Progress { .. } => 12. + 5. + 6. + 14.,
        Body::Paragraphs(lines) => 12. + lines.iter().map(|l| lines_of(l, 66) as f32 * 18.).sum::<f32>() + 8. * lines.len().saturating_sub(1) as f32,
        Body::Failure { lead, detail } => 12. + lines_of(lead, 66) as f32 * 18. + 8. + lines_of(detail, 62) as f32 * 14. + 2. * NOTES_PAD,
        Body::Notes { note, .. } => 12. + note.as_ref().map_or(0., |n| lines_of(n, 64) as f32 * 17. + 20. + 8.) + notes_box_height(&wb.update.notes),
    };
    // Never lower than the icon.
    let column = (title + body).max(ICON_TILE);
    2. * WINDOW_INSET + column + WINDOW_GAP + FOOTER_H
}

pub(super) struct UpdateWindow {
    wb: WeakEntity<Workbench>,
    host: WindowHandle<Workbench>,
    /// The first frame puts the keyboard on the primary button; after that the workbench moves it as the buttons change.
    focused: bool,
    height: f32,
    closing: bool,
    /// Whether the keyboard was used last: the focus ring is drawn only then (a click is not asking where the focus is).
    keys: Rc<Cell<bool>>,
    _observe: [gpui::Subscription; 3],
}

impl UpdateWindow {
    fn new(wb: Entity<Workbench>, host: WindowHandle<Workbench>, height: f32, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let weak = wb.downgrade();
        let me = window.window_handle();
        let gone = cx.observe_release(&wb, move |_, _, cx| {
            me.update(cx, |_, window, _| window.remove_window()).ok();
        });
        let changed = cx.observe(&wb, |_, _, cx| cx.notify());
        // The editor window can close while the dialog is up (the entity may outlive it by a moment, or be kept by a running task).
        let host_closed = cx.on_window_closed(move |cx| {
            if !window_is_open(host, cx) {
                me.update(cx, |_, window, _| window.remove_window()).ok();
            }
        });
        // The red button answers like Esc does: closing is not a way around a download or an install in progress.
        let asked = weak.clone();
        window.on_window_should_close(cx, move |_, app| {
            let Some(wb) = asked.upgrade() else { return true };
            host.update(app, |_, hw, app| {
                wb.update(app, |wb, cx| {
                    wb.dismiss_update_dialog(hw, cx);
                    wb.dialog_model(cx).is_none()
                })
            })
            .unwrap_or(true)
        });
        UpdateWindow { wb: weak, host, focused: false, height, closing: false, keys: Rc::new(Cell::new(false)), _observe: [gone, changed, host_closed] }
    }

    fn close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.closing {
            return;
        }
        self.closing = true;
        cx.defer_in(window, |_, window, _| window.remove_window());
    }
}

impl Render for UpdateWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(wb) = self.wb.upgrade() else {
            self.close(window, cx);
            return div().into_any_element();
        };
        let Some(model) = wb.read(cx).dialog_model(cx) else {
            self.close(window, cx);
            return div().into_any_element();
        };
        // The active theme's colours: the window is the app's, so it wears what the user chose (accent, surfaces, text).
        let p = *palette(cx);
        if !self.focused {
            self.focused = true;
            let ix = model.buttons.iter().position(|b| b.primary).unwrap_or(0);
            if let Some(h) = wb.read(cx).update.focus.get(ix) {
                window.focus(h);
            }
        }
        let height = window_height(wb.read(cx), &model);
        if (height - self.height).abs() > 0.5 {
            self.height = height;
            cx.defer_in(window, move |_, window, _| window.resize(size(px(WIDTH), px(height))));
        }
        let host = self.host;
        let weak = self.wb.clone();
        let send: Dispatch = Rc::new(move |cmd, _, app| {
            let Some(wb) = weak.upgrade() else { return false };
            host.update(app, |_, hw, app| wb.update(app, |wb, cx| wb.update_cmd(cmd, hw, cx))).unwrap_or(false)
        });
        window_card(wb.read(cx), &model, &p, &send, &self.keys)
    }
}

impl Workbench {
    /// Opens the dialog's window if a dialog is on and none is open yet. Called from `render`, the one place that sees every change
    /// of the dialog, so a dialog opened by the menu, the status bar or the first-run question all end up here.
    pub(super) fn sync_update_window(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.update.native || self.update.opening || self.update.dialog == Dialog::Closed {
            return;
        }
        if self.update.window.is_some_and(|w| window_is_open(w, cx)) {
            return;
        }
        let Some(model) = self.dialog_model(cx) else { return };
        let Some(host) = window.window_handle().downcast::<Workbench>() else { return };
        let (w, h) = (px(WIDTH), px(window_height(self, &model)));
        // Over the middle of the editor window, where the eye already is.
        let at = window.bounds();
        let bounds = Bounds::new(at.origin + gpui::point((at.size.width - w) / 2., ((at.size.height - h) / 2.).max(px(40.))), size(w, h));
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            titlebar: Some(TitlebarOptions { title: Some(crate::i18n::t("Software Update").into()), appears_transparent: false, traffic_light_position: None }),
            kind: WindowKind::Normal,
            is_resizable: false,
            is_minimizable: false,
            ..Default::default()
        };
        self.update.opening = true;
        let wb = cx.entity();
        let height = f32::from(h);
        // The new window draws its first frame while it is being made, and that frame reads this workbench: it must not be in the
        // middle of an update then.
        cx.defer(move |cx| {
            let handle = cx.open_window(options, |window, cx| cx.new(|cx| UpdateWindow::new(wb.clone(), host, height, window, cx))).ok();
            wb.update(cx, |wb, _| {
                wb.update.opening = false;
                wb.update.window = handle;
            });
        });
    }
}
