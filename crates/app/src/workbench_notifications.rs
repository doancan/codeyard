//! Workbench: the notification centre. Every message to the user goes through [`Workbench::notify`] /
//! [`Workbench::notify_with`]: it is recorded in the bounded history (see `notifications.rs`) and, unless Do Not
//! Disturb is on, shown as a toast (info and warnings in the status bar for a few seconds, errors as a card until closed).
//! The bell in the status bar counts what was not looked at; its list is drawn by `workbench_notifications_view.rs`.

use super::*;
use gpui::{SharedString, WeakFocusHandle};
use model::{Log, NoticeAction, NoticeSource, Notification, Severity};
use std::collections::HashMap;

#[path = "workbench_notification_actions.rs"]
mod actions;
#[path = "errors.rs"]
pub(crate) mod errors;
#[path = "persist_failures.rs"]
pub(crate) mod failures;
#[cfg(test)]
#[path = "workbench_git_failure_tests.rs"]
mod git_failure_tests;
#[cfg(all(test, unix))]
#[path = "workbench_notification_locks_tests.rs"]
mod locks;
#[path = "notifications.rs"]
pub(crate) mod model;
#[cfg(test)]
#[path = "workbench_persist_failure_tests.rs"]
mod persist_failure_tests;
#[cfg(test)]
#[path = "workbench_notifications_tests.rs"]
mod tests;
#[path = "workbench_notifications_view.rs"]
mod view;

gpui::actions!(notifications, [ShowNotifications, ClearNotifications, ToggleDoNotDisturb]);

/// How long an info or warning message stays in the status bar.
const INFO_FOR: Duration = Duration::from_secs(8);
const WARNING_FOR: Duration = Duration::from_secs(12);

/// The toast shape the call sites use: severity and text.
pub(crate) enum Notice {
    Info(SharedString),
    Warning(SharedString),
    Error(SharedString),
}

impl Notice {
    fn severity(&self) -> Severity {
        match self {
            Notice::Info(_) => Severity::Info,
            Notice::Warning(_) => Severity::Warning,
            Notice::Error(_) => Severity::Error,
        }
    }
    pub(crate) fn text(&self) -> &SharedString {
        match self {
            Notice::Info(m) | Notice::Warning(m) | Notice::Error(m) => m,
        }
    }
}

/// What is on screen as a toast. `tick` changes on every showing (a repeat included) so the timer restarts, while the
/// text stays the same, so a screen reader is not told again.
pub(crate) struct Toast {
    tick: u64,
    /// The history entry this showing belongs to (its action is drawn on the card); none for a plain flash.
    serial: Option<u64>,
    severity: Severity,
    text: SharedString,
}

#[derive(Default)]
pub(crate) struct NoticeCenter {
    log: Log,
    /// Failures reported from `&self` code, posted at the next frame.
    deferred: failures::Deferred,
    toast: Option<Toast>,
    dnd: bool,
    open: bool,
    /// Serial of the entry the keyboard is on in the open list.
    selected: Option<u64>,
    tick: u64,
    timed_tick: u64,
    timer: Option<gpui::Task<()>>,
    focus: Option<FocusHandle>,
    /// Where the focus was before the list opened; weak, so an element that is gone is noticed.
    return_focus: Option<WeakFocusHandle>,
    /// The list's buttons, reachable with Tab: (entry serial or 0 for the header, which button).
    buttons: HashMap<(u64, &'static str), FocusHandle>,
}

const HEADER_BUTTONS: [&str; 3] = ["notifications-dnd", "notifications-clear", "notifications-close"];

impl NoticeCenter {
    /// A message from before the window existed (a theme that was skipped at startup).
    pub(crate) fn with_startup(notice: Option<Notice>) -> Self {
        let mut center = Self::default();
        if let Some(n) = notice {
            center.post(Notification::new(n.severity(), NoticeSource::Settings, n.text().to_string()), n.severity());
        }
        center
    }

    /// Do Not Disturb hides information and warnings; an error still shows its card (and is still announced).
    fn post(&mut self, n: Notification, severity: Severity) {
        let text: SharedString = n.message.clone().into();
        let quiet = n.quiet;
        let posted = self.log.post(n, now_secs());
        if !quiet && (!self.dnd || severity == Severity::Error) {
            self.show(severity, text, Some(posted.serial));
        }
    }

    /// Focus handles for every button of the header and of the rows now listed; handles of rows that are gone are dropped.
    pub(super) fn sync_handles(&mut self, cx: &mut App) {
        let mut wanted: Vec<(u64, &'static str)> = HEADER_BUTTONS.iter().map(|k| (0, *k)).collect();
        for e in self.log.newest_first() {
            wanted.extend([(e.serial, "copy"), (e.serial, "dismiss")]);
            if e.action.is_some() {
                wanted.push((e.serial, "action"));
            }
        }
        self.buttons.retain(|k, _| wanted.contains(k));
        for key in wanted {
            self.buttons.entry(key).or_insert_with(|| cx.focus_handle().tab_stop(true));
        }
    }

    pub(super) fn handle(&self, serial: u64, kind: &'static str) -> FocusHandle {
        self.buttons.get(&(serial, kind)).cloned().expect("sync_handles ran before the list was drawn")
    }

    /// The card of an entry that was removed from the history goes away with it.
    fn drop_toast_of_missing_entry(&mut self) {
        if self.toast.as_ref().and_then(|t| t.serial).is_some_and(|s| self.log.get(s).is_none()) {
            self.toast = None;
        }
    }

    fn show(&mut self, severity: Severity, text: SharedString, serial: Option<u64>) {
        self.tick += 1;
        self.toast = Some(Toast { tick: self.tick, serial, severity, text });
    }
}

pub(crate) fn now_secs() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

impl Workbench {
    /// Tell the user something: the one place that sets what the status bar and the error card show.
    pub(crate) fn notify(&mut self, source: NoticeSource, notice: Notice) {
        let severity = notice.severity();
        self.notify_with(Notification::new(severity, source, notice.text().to_string()));
    }

    /// A notice with a long text behind it (a command's output), shown in the list and copied with it.
    pub(crate) fn notify_detailed(&mut self, source: NoticeSource, notice: Notice, detail: String) {
        self.notify_with(Notification::new(notice.severity(), source, notice.text().to_string()).detail(detail));
    }

    /// The full form: a detail text (stderr, a log), an action button, a stable id for collapsing repeats.
    pub(crate) fn notify_with(&mut self, n: Notification) {
        let severity = n.severity;
        self.notices.post(n, severity);
    }

    /// Remove the entry with this id (a failure that has been put right), and its card when it is on screen.
    pub(in crate::workbench) fn dismiss_notice_id(&mut self, id: &str) -> bool {
        let removed = self.notices.log.dismiss_id(id);
        if removed {
            self.notices.drop_toast_of_missing_entry();
        }
        removed
    }

    /// Take the toast away (a progress message that is over, the error card closed). The history keeps the entry.
    pub(crate) fn dismiss_notice(&mut self) {
        self.notices.toast = None;
    }

    /// The text of the toast on screen, if any.
    pub fn notice_text(&self) -> Option<String> {
        self.notices.toast.as_ref().map(|t| t.text.to_string())
    }

    /// The toast as the call sites' type.
    pub(crate) fn notice(&self) -> Option<Notice> {
        self.notices.toast.as_ref().map(|t| match t.severity {
            Severity::Info => Notice::Info(t.text.clone()),
            Severity::Warning => Notice::Warning(t.text.clone()),
            Severity::Error => Notice::Error(t.text.clone()),
        })
    }

    pub(crate) fn toast_is_none(&self) -> bool {
        self.notices.toast.is_none()
    }

    /// Hold the toast aside while something else may replace it (see `restore_toast`).
    pub(crate) fn take_toast(&mut self) -> Option<Toast> {
        self.notices.toast.take()
    }

    pub(crate) fn restore_toast(&mut self, toast: Option<Toast>) {
        self.notices.toast = toast;
    }

    /// A confirmation that is not worth a place in the history ("Copied" from the list itself).
    pub(crate) fn flash(&mut self, text: String) {
        if !self.notices.dnd {
            self.notices.show(Severity::Info, text.into(), None);
        }
    }

    /// Info and warnings leave the status bar after a few seconds; errors stay until closed.
    pub(crate) fn track_notice(&mut self, cx: &mut Context<Self>) {
        self.post_deferred_notices();
        let Some(toast) = &self.notices.toast else {
            self.notices.timer = None;
            return;
        };
        if toast.tick == self.notices.timed_tick {
            return;
        }
        let tick = toast.tick;
        let after = match toast.severity {
            Severity::Info => INFO_FOR,
            Severity::Warning => WARNING_FOR,
            Severity::Error => {
                self.notices.timed_tick = tick;
                self.notices.timer = None;
                return;
            }
        };
        self.notices.timed_tick = tick;
        self.notices.timer = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(after).await;
            this.update(cx, |this, cx| {
                // A newer message has its own timer.
                if this.notices.toast.as_ref().is_some_and(|t| t.tick == tick) {
                    this.notices.toast = None;
                    cx.notify();
                }
            })
            .ok();
        }));
    }

    /// Press the button of the notification `serial`, as a click on it would.
    #[cfg(test)]
    pub(crate) fn run_notification_action_for_test(&mut self, serial: u64, window: &mut Window, cx: &mut Context<Self>) {
        self.run_notification_action(serial, window, cx);
    }

    /// What the history holds, newest first.
    #[cfg(test)]
    pub(crate) fn history_for_test(&self) -> Vec<model::Entry> {
        self.notices.log.newest_first().cloned().collect()
    }

    pub(super) fn unread_notifications(&self) -> (usize, usize) {
        (self.notices.log.unread(), self.notices.log.unread_errors())
    }

    pub(in crate::workbench) fn show_notifications(&mut self, _: &ShowNotifications, window: &mut Window, cx: &mut Context<Self>) {
        if self.notices.open {
            return self.close_notifications(window, cx);
        }
        self.notices.open = true;
        self.notices.log.mark_all_read();
        self.notices.selected = self.notices.log.newest_first().next().map(|e| e.serial);
        self.notices.return_focus = window.focused(cx).map(|h| h.downgrade());
        let focus = self.notices.focus.get_or_insert_with(|| cx.focus_handle()).clone();
        window.focus(&focus);
        cx.notify();
    }

    /// Close the list and give the focus back to what had it, or to the editor when that is gone.
    pub(super) fn close_notifications(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.notices.open {
            return;
        }
        self.notices.open = false;
        self.notices.selected = None;
        let before = self.notices.return_focus.take().and_then(|weak| weak.upgrade());
        let restored = before.is_some_and(|handle| {
            window.focus(&handle);
            handle.is_focused(window)
        });
        if !restored {
            self.focus_active_editor(window, cx);
        }
        cx.notify();
    }

    /// Called as the window is drawn: while the list has the keyboard, what it shows counts as seen; once the focus
    /// is somewhere else the list closes (a failure arriving then stays unread in the bell).
    pub(in crate::workbench) fn settle_notifications(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.notices.open {
            return;
        }
        let inside = self.notices.focus.as_ref().is_some_and(|f| f.contains_focused(window, cx));
        if inside {
            self.notices.log.mark_all_read();
        } else {
            self.notices.open = false;
            self.notices.selected = None;
            self.notices.return_focus = None;
        }
    }

    pub(in crate::workbench) fn clear_notifications(&mut self, _: &ClearNotifications, _: &mut Window, cx: &mut Context<Self>) {
        self.notices.log.clear();
        self.notices.selected = None;
        cx.notify();
    }

    /// Hide information and warning toasts (errors still show); everything is still recorded.
    pub(in crate::workbench) fn toggle_do_not_disturb(&mut self, _: &ToggleDoNotDisturb, _: &mut Window, cx: &mut Context<Self>) {
        self.notices.dnd = !self.notices.dnd;
        if self.notices.dnd && self.notices.toast.as_ref().is_some_and(|t| t.severity != Severity::Error) {
            self.notices.toast = None;
        }
        cx.notify();
    }

    pub fn do_not_disturb(&self) -> bool {
        self.notices.dnd
    }

    pub(super) fn dismiss_notification(&mut self, serial: u64) {
        let order: Vec<u64> = self.notices.log.newest_first().map(|e| e.serial).collect();
        self.notices.log.dismiss(serial);
        if self.notices.selected == Some(serial) {
            let at = order.iter().position(|s| *s == serial).unwrap_or(0);
            let remaining: Vec<u64> = order.into_iter().filter(|s| *s != serial).collect();
            self.notices.selected = remaining.get(at).or_else(|| remaining.last()).copied();
        }
    }

    pub(super) fn copy_notification(&mut self, serial: u64, cx: &mut Context<Self>) {
        let Some(entry) = self.notices.log.get(serial) else { return };
        cx.write_to_clipboard(gpui::ClipboardItem::new_string(model::copy_text(entry)));
        self.flash(crate::i18n::t("Copied the notification").to_string());
    }

    /// The button of the notification on the card, if it has one.
    pub(crate) fn toast_action(&self) -> Option<(u64, NoticeAction)> {
        let serial = self.notices.toast.as_ref()?.serial?;
        Some((serial, self.notices.log.get(serial)?.action.clone()?))
    }

    /// The button of the list that has the keyboard, if any.
    fn focused_button(&self, window: &Window) -> Option<(u64, &'static str)> {
        self.notices.buttons.iter().find(|(_, h)| h.is_focused(window)).map(|(k, _)| *k)
    }

    fn press_button(&mut self, (serial, kind): (u64, &'static str), window: &mut Window, cx: &mut Context<Self>) {
        match (serial, kind) {
            (_, "notifications-dnd") => self.toggle_do_not_disturb(&ToggleDoNotDisturb, window, cx),
            (_, "notifications-clear") => self.clear_notifications(&ClearNotifications, window, cx),
            (_, "notifications-close") => self.close_notifications(window, cx),
            (s, "action") => self.run_notification_action(s, window, cx),
            (s, "copy") => self.copy_notification(s, cx),
            (s, "dismiss") => {
                self.dismiss_notification(s);
                // The button is gone: the keyboard goes back to the list.
                if let Some(focus) = self.notices.focus.clone() {
                    window.focus(&focus);
                }
            }
            _ => {}
        }
    }

    /// Keys in the open list. Up, Down, Home and End move; Enter runs the row's button (or presses the button that has the
    /// keyboard); Tab walks the buttons; Delete dismisses; Cmd/Ctrl+C copies; Escape closes. Returns whether the key was used.
    pub(super) fn notifications_key(&mut self, key: &str, command: bool, shift: bool, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let order: Vec<u64> = self.notices.log.newest_first().map(|e| e.serial).collect();
        let at = self.notices.selected.and_then(|s| order.iter().position(|o| *o == s));
        let go = |i: usize| order.get(i).copied();
        match (key, command) {
            ("escape", _) => {
                self.close_notifications(window, cx);
                return true;
            }
            ("tab", false) => {
                if shift {
                    window.focus_prev();
                } else {
                    window.focus_next();
                }
                // The walk stays in the list: past its last button it starts again at the list itself.
                if let Some(focus) = self.notices.focus.clone().filter(|f| !f.contains_focused(window, cx)) {
                    window.focus(&focus);
                }
                cx.notify();
                return true;
            }
            ("down", _) => self.notices.selected = go(at.map_or(0, |i| (i + 1).min(order.len().saturating_sub(1)))),
            ("up", _) => self.notices.selected = go(at.map_or(0, |i| i.saturating_sub(1))),
            ("home", _) => self.notices.selected = go(0),
            ("end", _) => self.notices.selected = order.last().copied(),
            ("enter" | "space", false) => {
                if let Some(button) = self.focused_button(window) {
                    self.press_button(button, window, cx);
                } else if key == "enter"
                    && let Some(s) = self.notices.selected
                {
                    self.run_notification_action(s, window, cx);
                }
            }
            ("delete" | "backspace", false) => {
                let Some(s) = self.notices.selected else { return false };
                self.dismiss_notification(s);
            }
            ("c", true) => {
                let Some(s) = self.notices.selected else { return false };
                self.copy_notification(s, cx);
            }
            _ => return false,
        }
        cx.notify();
        true
    }
}
