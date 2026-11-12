//! Workbench: the bell in the status bar and the list of notifications above it.

use super::model::{Entry, Severity, age_label};
use super::*;
use crate::a11y_controls::{Control, control};

const SURFACE: &str = "Notifications";
const PANEL_W: f32 = 420.;
const MAX_SHOWN_LINES: usize = 6;
const BADGE_CAP: usize = 99;

fn severity_icon(s: Severity, p: &Palette) -> (&'static str, u32) {
    match s {
        Severity::Info => ("info", p.info),
        Severity::Warning => ("alert", p.warning),
        Severity::Error => ("circle-x", p.danger),
    }
}

fn severity_word(s: Severity) -> String {
    match s {
        Severity::Info => crate::i18n::t("Information"),
        Severity::Warning => crate::i18n::t("Warning"),
        Severity::Error => crate::i18n::t("Error"),
    }
    .to_string()
}

/// What a screen reader says for a row: severity, source, age, count, message.
fn row_label(e: &Entry, now: u64) -> String {
    let times = if e.count > 1 { format!(", {}", crate::i18n::tf("repeated {} times", &[&e.count])) } else { String::new() };
    format!("{}, {}, {}{times}: {}", severity_word(e.severity), e.source.label(), age_label(now, e.at), e.message)
}

/// A button the keyboard can reach: Tab lands on it and the focus ring shows when the keyboard is in use.
fn focusable(d: gpui::Stateful<gpui::Div>, handle: &FocusHandle, p: &Palette, keyboard: bool) -> gpui::Stateful<gpui::Div> {
    d.track_focus(handle).border_1().border_color(ui::ca(p.focus_ring, 0.)).when(keyboard, |d| d.focus(|d| d.border_color(c(p.focus_ring))))
}

impl Workbench {
    /// The status bar's bell: how many warnings and errors were not looked at yet, and Do Not Disturb.
    pub(in crate::workbench) fn notifications_bell(&self, p: &Palette, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let (unread, errors) = self.unread_notifications();
        let dnd = self.notices.dnd;
        let label = match (unread, dnd) {
            (0, false) => crate::i18n::t("Notifications").to_string(),
            (0, true) => crate::i18n::t("Notifications, Do Not Disturb is on").to_string(),
            (n, _) => crate::i18n::tn("{} unread notification", "{} unread notifications", n, &[]),
        };
        let weak = cx.entity().downgrade();
        let color = if errors > 0 {
            p.danger
        } else if unread > 0 {
            p.text
        } else {
            p.text_muted
        };
        let badge = if unread > BADGE_CAP { format!("{BADGE_CAP}+") } else { unread.to_string() };
        let tip = label.clone();
        div()
            .id("sb-notifications")
            .debug_selector(|| "sb-notifications".into())
            .relative()
            .flex_none()
            .flex()
            .items_center()
            .gap(px(5.))
            .px(px(8.))
            .min_w(px(28.))
            .h_full()
            .cursor_pointer()
            .hover(|d| d.bg(c(p.surface_hover)))
            .text_color(c(color))
            .child(control(
                Control::button(crate::i18n::t("Status bar"), "sb-notifications", label.clone(), |_, _| {})
                    .expanded(self.notices.open)
                    .on_entity(weak, |this, w, cx| this.show_notifications(&ShowNotifications, w, cx)),
            ))
            .child(icon(if dnd { "bell-off" } else { "bell" }, 16., color))
            .when(unread > 0, |d| d.child(badge))
            .tooltip(move |_, cx| cx.new(|_| Tooltip(tip.clone().into())).into())
            .on_click(cx.listener(|this, _, w, cx| this.show_notifications(&ShowNotifications, w, cx)))
    }

    /// The list: bottom right above the status bar, like the error card.
    pub(in crate::workbench) fn notifications_panel(&mut self, p: &Palette, cx: &mut Context<Self>) -> Option<impl IntoElement + use<>> {
        if !self.notices.open {
            return None;
        }
        self.notices.sync_handles(cx);
        let keyboard = ui::keyboard_focus(cx);
        let now = super::now_secs();
        let weak = cx.entity().downgrade();
        let focus = self.notices.focus.clone()?;
        let header_button = |cx: &mut Context<Workbench>,
                             key: &'static str,
                             glyph: &'static str,
                             label: String,
                             on: bool,
                             f: fn(&mut Workbench, &mut Window, &mut Context<Workbench>)| {
            focusable(div().id(key), &self.notices.handle(0, key), p, keyboard)
                .relative()
                .flex_none()
                .size(px(24.))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(theme::radius::SM))
                .cursor_pointer()
                .when(on, |d| d.bg(c(p.surface_active)))
                .hover(|d| d.bg(c(p.surface_hover)))
                .child(control(
                    Control::button(SURFACE, key, label.clone(), |_, _| {}).toggled(on).focus(self.notices.handle(0, key)).on_entity(weak.clone(), f),
                ))
                .child(icon(glyph, 14., p.text_muted))
                .tooltip({
                    let label = label.clone();
                    move |_, cx| cx.new(|_| Tooltip(label.clone().into())).into()
                })
                .on_click(cx.listener(move |this, _, w, cx| f(this, w, cx)))
        };
        let dnd = self.notices.dnd;
        let rows: Vec<AnyElement> = self.notices.log.newest_first().map(|e| self.notification_row(e, now, keyboard, p, cx)).collect();
        let empty = rows.is_empty();
        Some(
            div()
                .id("notifications-panel")
                .debug_selector(|| "notifications-panel".into())
                .key_context("Notifications")
                .track_focus(&focus)
                .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, window, cx| {
                    let k = &event.keystroke;
                    // Keys the list does not use go on to whoever else wants them.
                    if this.notifications_key(&k.key, k.modifiers.secondary(), k.modifiers.shift, window, cx) {
                        cx.stop_propagation();
                    }
                }))
                .absolute()
                .right(px(theme::SPACE[6]))
                .bottom(px(layout::STATUSBAR_H + theme::SPACE[4]))
                .w(px(PANEL_W))
                .max_w_full()
                .max_h(px(480.))
                .flex()
                .flex_col()
                .bg(c(p.surface_raised))
                .border_1()
                .border_color(c(p.border))
                .rounded(px(theme::radius::MD))
                .shadow_lg()
                .text_size(px(text::SM.0))
                .text_color(c(p.text))
                .occlude()
                .child(
                    div()
                        .flex_none()
                        .flex()
                        .items_center()
                        .gap(px(theme::SPACE[2]))
                        .px(px(theme::SPACE[4]))
                        .h(px(34.))
                        .border_b_1()
                        .border_color(c(p.border))
                        .child(control(Control::text(SURFACE, "title", accesskit::Role::Heading, crate::i18n::t("Notifications"))))
                        .child(div().flex_1().font_weight(gpui::FontWeight::SEMIBOLD).child(crate::i18n::t("Notifications")))
                        .child(header_button(
                            cx,
                            "notifications-dnd",
                            if dnd { "bell-off" } else { "bell" },
                            if dnd {
                                crate::i18n::t("Do Not Disturb is on: information and warning toasts are hidden. Click to turn it off").to_string()
                            } else {
                                crate::i18n::t("Turn on Do Not Disturb").to_string()
                            },
                            dnd,
                            |this, w, cx| this.toggle_do_not_disturb(&ToggleDoNotDisturb, w, cx),
                        ))
                        .child(header_button(
                            cx,
                            "notifications-clear",
                            "trash",
                            crate::i18n::t("Clear all notifications").to_string(),
                            false,
                            |this, w, cx| this.clear_notifications(&ClearNotifications, w, cx),
                        ))
                        .child(header_button(cx, "notifications-close", "x", crate::i18n::t("Close").to_string(), false, |this, w, cx| {
                            this.close_notifications(w, cx)
                        })),
                )
                .child(
                    div()
                        .id("notifications-list")
                        .flex_1()
                        .min_h_0()
                        .overflow_y_scroll()
                        .flex()
                        .flex_col()
                        .when(empty, |d| {
                            d.child(
                                div()
                                    .p(px(theme::SPACE[5]))
                                    .text_color(c(p.text_muted))
                                    .child(control(Control::text(SURFACE, "empty", accesskit::Role::Label, crate::i18n::t("No notifications"))))
                                    .child(crate::i18n::t("No notifications")),
                            )
                        })
                        .children(rows),
                ),
        )
    }

    fn notification_row(&self, e: &Entry, now: u64, keyboard: bool, p: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let serial = e.serial;
        let weak = cx.entity().downgrade();
        let selected = self.notices.selected == Some(serial);
        let (glyph, tone) = severity_icon(e.severity, p);
        let small_button = |kind: &'static str, glyph: &'static str, label: String, f: fn(&mut Workbench, u64, &mut Window, &mut Context<Workbench>)| {
            let key = format!("{kind}-{serial}");
            let handle = self.notices.handle(serial, kind);
            focusable(div().id(SharedString::from(key.clone())), &handle, p, keyboard)
                .relative()
                .flex_none()
                .size(px(22.))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(theme::radius::SM))
                .cursor_pointer()
                .hover(|d| d.bg(c(p.surface_hover)))
                .child(control(
                    Control::button(SURFACE, key, label.clone(), |_, _| {}).focus(handle).on_entity(weak.clone(), move |this, w, cx| f(this, serial, w, cx)),
                ))
                .child(icon(glyph, 13., p.text_muted))
                .tooltip(move |_, cx| cx.new(|_| Tooltip(label.clone().into())).into())
                .on_click(cx.listener(move |this, _, w, cx| f(this, serial, w, cx)))
        };
        let action = e.action.map(|a| {
            let label = crate::i18n::t(a.label).to_string();
            let handle = self.notices.handle(serial, "action");
            focusable(div().id(SharedString::from(format!("notification-action-{serial}"))), &handle, p, keyboard)
                .relative()
                .mt(px(theme::SPACE[2]))
                .px(px(theme::SPACE[4]))
                .h(px(22.))
                .flex()
                .items_center()
                .rounded(px(theme::radius::SM))
                .bg(c(p.accent))
                .text_color(c(p.on_accent))
                .text_size(px(text::XS.0))
                .cursor_pointer()
                .hover(|d| d.opacity(0.9))
                .child(control(
                    Control::button(SURFACE, format!("action-{serial}"), label.clone(), |_, _| {})
                        .focus(handle.clone())
                        .on_entity(weak.clone(), move |this, w, cx| this.run_notification_action(serial, w, cx)),
                ))
                .child(label)
                .on_click(cx.listener(move |this, _, w, cx| this.run_notification_action(serial, w, cx)))
        });
        let meta = {
            let mut m = format!("{} · {}", e.source.label(), age_label(now, e.at));
            if e.count > 1 {
                m.push_str(&format!(" · ×{}", e.count));
            }
            m
        };
        let detail = e.detail.as_ref().map(|d| d.lines().take(MAX_SHOWN_LINES).collect::<Vec<_>>().join("\n"));
        div()
            .id(SharedString::from(format!("notification-row-{serial}")))
            .debug_selector(move || format!("notification-row-{serial}"))
            .relative()
            .flex_none()
            .flex()
            .items_start()
            .gap(px(theme::SPACE[3]))
            .px(px(theme::SPACE[4]))
            .py(px(theme::SPACE[3]))
            .border_b_1()
            .border_color(c(p.border))
            .when(selected, |d| d.bg(c(p.surface_hover)))
            .child(control(Control::text(SURFACE, format!("row-{serial}"), accesskit::Role::ListItem, row_label(e, now)).selected(selected)))
            .child(div().flex_none().pt(px(1.)).child(icon(glyph, 16., tone)))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(div().text_size(px(text::XS.0)).text_color(c(p.text_muted)).child(meta))
                    .child(div().line_clamp(MAX_SHOWN_LINES).child(e.message.clone()))
                    .when_some(detail, |d, detail| {
                        d.child(div().mt(px(theme::SPACE[2])).line_clamp(MAX_SHOWN_LINES).text_size(px(text::XS.0)).text_color(c(p.text_muted)).child(detail))
                    })
                    .children(action),
            )
            .child(small_button("copy", "copy", crate::i18n::t("Copy notification").to_string(), |this, s, _, cx| {
                this.copy_notification(s, cx);
                cx.notify();
            }))
            .child(small_button("dismiss", "x", crate::i18n::t("Dismiss notification").to_string(), |this, s, w, cx| {
                this.dismiss_notification(s);
                // The button is gone: the keyboard goes back to the list instead of leaving it.
                if let Some(focus) = this.notices.focus.clone() {
                    w.focus(&focus);
                }
                cx.notify();
            }))
            .into_any_element()
    }
}
