//! Workbench: the card that edits the whole message of a squashed commit (#324). The messages of the squashed commits
//! come in order, ready to be edited as one text; ⌘↵ (Ctrl+↵) squashes, Return is a line break, Esc closes the card and
//! nothing happens. The rewrite itself is `workbench_rewrite.rs`.

use gpui::{FocusHandle, Subscription, deferred, relative};

use super::*;
use crate::i18n::t;
use crate::input::{FieldEvent, TextField};
use crate::ui::ca;

/// What the card is editing the message for: the run it squashes and the branch tip the run was read at.
pub(in crate::workbench) struct MessageDialog {
    field: Entity<TextField>,
    run: Vec<String>,
    tip: String,
    repo: Option<crate::scm::Target>,
    /// What had the keyboard when the card opened.
    back: Option<FocusHandle>,
    _sub: Subscription,
}

impl Workbench {
    /// Open the card with `message` in it, to squash `run` (oldest first) of the branch at `tip` in `repo`.
    pub(super) fn show_squash_message(
        &mut self,
        message: &str,
        run: Vec<String>,
        tip: String,
        repo: Option<crate::scm::Target>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.vcs.message_dialog.is_some() {
            return;
        }
        let field = cx.new(|cx| TextField::new(crate::i18n::tr("Message of the squashed commit"), cx).modifier_submits());
        field.update(cx, |f, cx| f.set_text(message, cx));
        let _sub = cx.subscribe_in(&field, window, |this, _, event: &FieldEvent, window, cx| match event {
            FieldEvent::Submit => this.submit_squash_message(window, cx),
            FieldEvent::Escape => this.close_message_dialog(window, cx),
            FieldEvent::Changed => {}
        });
        let back = window.focused(cx);
        window.focus(&field.focus_handle(cx));
        self.vcs.message_dialog = Some(MessageDialog { field, run, tip, repo, back, _sub });
        cx.notify();
    }

    fn submit_squash_message(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(d) = self.vcs.message_dialog.take() else { return };
        let message = d.field.read(cx).text().to_string();
        if message.trim().is_empty() {
            // Kept open: what was typed is not lost to an empty message.
            self.vcs.message_dialog = Some(d);
            self.notify_rewrite_failed(t("The message of the squashed commit cannot be empty.").to_string(), cx);
            return;
        }
        self.restore_focus_after_message(d.back.clone(), window);
        self.finish_squash(d.run, d.tip, d.repo, message, cx);
    }

    pub(super) fn close_message_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(d) = self.vcs.message_dialog.take() {
            self.restore_focus_after_message(d.back, window);
            cx.notify();
        }
    }

    fn restore_focus_after_message(&self, back: Option<FocusHandle>, window: &mut Window) {
        window.focus(&back.unwrap_or_else(|| self.focus_handle.clone()));
    }

    /// The modal card with a scrim behind it, or `None` while no message is being edited.
    pub(in crate::workbench) fn message_dialog(&self, p: &Palette, cx: &mut Context<Self>) -> Option<impl IntoElement + use<>> {
        let d = self.vcs.message_dialog.as_ref()?;
        let muted = c(p.text_muted);
        let count = d.run.len();
        let submit = cx.listener(|this, _, window, cx| this.submit_squash_message(window, cx));
        let cancel = cx.listener(|this, _, window, cx| this.close_message_dialog(window, cx));
        let buttons = div()
            .flex()
            .items_center()
            .justify_end()
            .gap(px(theme::SPACE[4]))
            .child(div().flex_1().min_w_0().text_size(px(text::XS.0)).text_color(muted).child(t("↵ for a new line, ⌘↵ to squash, Esc to cancel")))
            .child(crate::editor::button("squash-message-cancel", t("Cancel"), false, p, move |e, window, cx| cancel(e, window, cx)))
            .child(crate::editor::button("squash-message-submit", t("Squash"), true, p, move |e, window, cx| submit(e, window, cx)));
        let card = div()
            .id("squash-message-dialog")
            .debug_selector(|| "squash-message-dialog".into())
            .w(px(560.))
            .max_w(relative(0.92))
            .max_h(relative(0.92))
            .flex()
            .flex_col()
            .gap(px(theme::SPACE[5]))
            .p(px(theme::SPACE[7]))
            .bg(c(p.surface_raised))
            .border_1()
            .border_color(c(p.border))
            .rounded(px(theme::radius::LG))
            .shadow_lg()
            .text_size(px(text::BASE.0))
            .text_color(c(p.text))
            .occlude()
            .child(div().font_weight(gpui::FontWeight::SEMIBOLD).child(crate::i18n::tf("Squash {} commits into one", &[&count])))
            .child(
                div()
                    .id("squash-message-field")
                    .min_h(px(140.))
                    .max_h(px(320.))
                    .overflow_y_scroll()
                    .px(px(theme::SPACE[4]))
                    .py(px(theme::SPACE[3]))
                    .rounded(px(theme::radius::MD))
                    .bg(c(p.bg))
                    .border_1()
                    .border_color(c(p.border))
                    .child(d.field.clone()),
            )
            .child(buttons);
        Some(deferred(
            div()
                .id("squash-message-backdrop")
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

    #[cfg(test)]
    pub fn message_dialog_text_for_test(&self, cx: &App) -> Option<String> {
        self.vcs.message_dialog.as_ref().map(|d| d.field.read(cx).text().to_string())
    }

    #[cfg(test)]
    pub fn message_dialog_field_for_test(&self) -> Option<Entity<TextField>> {
        self.vcs.message_dialog.as_ref().map(|d| d.field.clone())
    }
}
