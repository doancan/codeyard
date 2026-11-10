//! Workbench: the card a folder asks on when it opens untrusted. What each button does is decided here and in
//! `workbench_trust.rs` (`ask_trust_on_open` opens it, `answer_trust` closes it); this file draws it and keeps what the
//! keyboard and a screen reader need.
//!
//! The card says what Restricted Mode switches off and what stays, and offers the choices as rows that say what each one
//! trusts. "Don't Trust" is the button the keyboard starts on, so Return and Escape trust nothing: trusting takes a click or a Tab.

use gpui::{FocusHandle, relative};

use super::*;
use crate::i18n::{t, tf};
use crate::ui::ca;

gpui::actions!(workbench, [TrustDialogDismiss, TrustDialogConfirm, TrustDialogNext, TrustDialogPrev]);

/// The most characters of the folder's path the card shows.
const PATH_KEPT: usize = 44;

/// The answers, in tab order (the numbers `answer_trust` takes).
pub(super) const DONT_TRUST: usize = 0;
pub(super) const TRUST: usize = 1;
pub(super) const TRUST_PARENT: usize = 2;

/// The question that is open: what it is about and what trusting it would cover.
pub(in crate::workbench) struct TrustDialog {
    /// The folder that was opened, and the workspace file when the question is about one.
    pub(super) folder: String,
    pub(super) workspace: Option<String>,
    /// The folder above it, offered as a second place to trust with everything below it.
    pub(super) parent: Option<String>,
    /// The folders trusting this one covers (a workspace file lists several) and the name the notice uses.
    pub(super) roots: Vec<String>,
    pub(super) name: String,
    /// The state key that remembers the question was answered.
    pub(super) asked: String,
    /// One handle per answer, in `DONT_TRUST`, `TRUST`, `TRUST_PARENT` order.
    pub(super) focus: [FocusHandle; 3],
    /// What had the keyboard when the question opened.
    pub(super) back: Option<FocusHandle>,
}

impl TrustDialog {
    pub(super) fn title(&self) -> &'static str {
        if self.workspace.is_some() { t("Do you trust the authors of this workspace?") } else { t("Do you trust the authors of this folder?") }
    }

    /// The labels of the answers that are offered, in tab order.
    pub(super) fn labels(&self) -> Vec<String> {
        let mut labels = vec![t("Don't Trust").to_string(), self.trust_label().to_string()];
        labels.extend(self.parent.as_deref().map(|p| tf("Trust Parent Folder “{}” and Subfolders", &[&ui::file_name(p)])));
        labels
    }

    fn trust_label(&self) -> &'static str {
        if self.workspace.is_some() { t("Trust Workspace") } else { t("Trust Folder") }
    }

    /// Title and text in reading order, for the accessibility tree.
    pub(super) fn description(&self) -> String {
        [
            t("Only trust folders from people you know: what you trust can run programs on the computer that holds it, with your rights."),
            t("Restricted Mode keeps these off until you trust it:"),
        ]
        .iter()
        .map(|s| s.to_string())
        .chain(restricted().map(str::to_string))
        .chain([t("Editing, search, Git and themes work as usual.").to_string()])
        .collect::<Vec<_>>()
        .join("\n")
    }
}

/// Whether a keystroke is held back while the trust question is open: a shortcut (⌘, Ctrl or Alt with a key) is, except the ones
/// that leave or hide the application or its window, which stay what they are everywhere. Plain keys, Return, Space, Tab and
/// Escape are the card's own and reach it.
pub(in crate::workbench) fn trust_dialog_blocks(k: &gpui::Keystroke) -> bool {
    const LEAVES: [&str; 4] = ["q", "w", "h", "m"];
    let m = &k.modifiers;
    (m.platform || m.control || m.alt) && !(m.platform && !m.control && !m.alt && LEAVES.contains(&k.key.as_str()))
}

/// What Restricted Mode switches off (the table in docs/architecture.md, "Workspace Trust, in one place").
fn restricted() -> [&'static str; 3] {
    [t("Tasks, tests and debugging"), t("Language servers"), t("Project settings that start programs")]
}

impl Workbench {
    /// The modal card with a scrim behind it, or `None` while no question is open.
    pub(in crate::workbench) fn trust_dialog(&self, p: &Palette, cx: &mut Context<Self>) -> Option<impl IntoElement + use<>> {
        let d = self.cfg.trust_ask.dialog.as_ref()?;
        let muted = c(p.text_muted);
        let folder_name = d.workspace.clone().unwrap_or_else(|| ui::file_name(&d.folder).to_string());
        let above = d.folder.trim_end_matches(['/', '\\']).rsplit_once(['/', '\\']).map(|(above, _)| above.to_string()).unwrap_or_default();
        // A long path keeps its end: the folders next to the one asked about are what tells it from another.
        let above =
            if above.chars().count() > PATH_KEPT { format!("…{}", above.chars().skip(above.chars().count() - PATH_KEPT).collect::<String>()) } else { above };
        let tile = c(ui::mix(p.surface_raised, p.warning, 0.2));

        let header = div()
            .flex()
            .items_start()
            .gap(px(theme::SPACE[6]))
            .child(
                div()
                    .size(px(44.))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(theme::radius::LG))
                    .bg(tile)
                    .child(icon("lock", 22., p.warning)),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(px(theme::SPACE[2]))
                    .child(
                        div().id("trust-title").text_size(px(text::LG.0)).line_height(px(text::LG.1)).font_weight(gpui::FontWeight::SEMIBOLD).child(d.title()),
                    )
                    .child(
                        div().text_size(px(text::SM.0)).line_height(px(text::SM.1 + 2.)).text_color(muted).child(t(
                            "Only trust folders from people you know: what you trust can run programs on the computer that holds it, with your rights.",
                        )),
                    ),
            );

        let chip = div()
            .id("trust-folder")
            .flex()
            .items_center()
            .gap(px(theme::SPACE[4]))
            .px(px(theme::SPACE[5]))
            .py(px(theme::SPACE[3]))
            .rounded(px(theme::radius::MD))
            .bg(c(p.bg))
            .border_1()
            .border_color(c(p.border))
            .tooltip({
                let full = d.folder.clone();
                move |_, cx| cx.new(|_| Tooltip(full.clone().into())).into()
            })
            .child(icon("folder", 16., p.text_muted))
            .child(div().flex_none().font_weight(gpui::FontWeight::SEMIBOLD).child(folder_name))
            .child(div().flex_1().min_w_0().truncate().text_size(px(text::XS.0)).text_color(muted).child(above));

        let marks = |glyph: &'static str, color: u32, line: &str| {
            div()
                .flex()
                .items_start()
                .gap(px(theme::SPACE[4]))
                .child(div().flex_none().h(px(text::SM.1 + 2.)).flex().items_center().child(icon(glyph, 14., color)))
                .child(div().flex_1().min_w_0().child(line.to_string()))
        };
        let limits = div()
            .flex()
            .flex_col()
            .gap(px(theme::SPACE[3]))
            .px(px(theme::SPACE[5]))
            .py(px(theme::SPACE[4]))
            .rounded(px(theme::radius::MD))
            .bg(c(ui::mix(p.surface_raised, p.warning, 0.06)))
            .border_1()
            .border_color(c(ui::mix(p.border, p.warning, 0.35)))
            .text_size(px(text::SM.0))
            .line_height(px(text::SM.1 + 2.))
            .child(div().font_weight(gpui::FontWeight::SEMIBOLD).child(t("Restricted Mode keeps these off until you trust it:")))
            .children(restricted().map(|line| marks("circle-slash-2", p.warning, line)))
            .child(marks("circle-check", p.success, t("Editing, search, Git and themes work as usual.")));

        let choice = |ix: usize, id: &'static str, glyph: &'static str, title: String, detail: String, primary: bool| {
            let accent = p.accent;
            div()
                .id(id)
                .debug_selector(move || id.into())
                .track_focus(&d.focus[ix])
                .tab_index(0)
                .flex()
                .items_center()
                .gap(px(theme::SPACE[5]))
                .px(px(theme::SPACE[5]))
                .py(px(theme::SPACE[4]))
                .rounded(px(theme::radius::MD))
                .cursor_pointer()
                .bg(c(if primary { ui::mix(p.surface_raised, accent, 0.12) } else { p.surface_raised }))
                .border_1()
                .border_color(c(if primary { ui::mix(p.border, accent, 0.6) } else { p.border }))
                .hover(|s| s.bg(c(ui::mix(p.surface_hover, accent, if primary { 0.2 } else { 0.06 }))))
                .focus(|s| s.border_color(c(p.focus_ring)))
                .child(
                    div()
                        .size(px(32.))
                        .flex_none()
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded(px(theme::radius::MD))
                        .bg(c(ui::mix(p.surface_raised, accent, 0.18)))
                        .child(icon(glyph, 16., accent)),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .child(div().font_weight(gpui::FontWeight::SEMIBOLD).child(title))
                        .child(div().text_size(px(text::XS.0)).line_height(px(text::XS.1 + 2.)).text_color(muted).child(detail)),
                )
                .child(icon("chevron-right", 14., p.text_muted))
                .on_click(cx.listener(move |this, _, window, cx| this.answer_trust(ix, window, cx)))
        };
        let this_one = choice(
            TRUST,
            "trust-folder-choice",
            "circle-check",
            d.trust_label().to_string(),
            if d.workspace.is_some() {
                t("Its tasks, language servers and settings run. Nothing outside the workspace's folders is trusted.").to_string()
            } else {
                t("Its tasks, language servers and settings run. Other folders keep asking.").to_string()
            },
            true,
        );
        let with_parent = d.parent.as_deref().map(|parent| {
            let name = ui::file_name(parent).to_string();
            choice(
                TRUST_PARENT,
                "trust-parent-choice",
                "folder-tree",
                tf("Trust Parent Folder “{}” and Subfolders", &[&name]),
                tf("Also every folder inside “{}”, now and later.", &[&name]),
                false,
            )
        });

        let footer = div()
            .flex()
            .items_center()
            .gap(px(theme::SPACE[5]))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_size(px(text::XS.0))
                    .line_height(px(text::XS.1 + 2.))
                    .text_color(muted)
                    .child(t("Esc keeps Restricted Mode. Change it later with Workspace: Manage Workspace Trust…")),
            )
            .child(
                div()
                    .id("trust-dont")
                    .debug_selector(|| "trust-dont".into())
                    .track_focus(&d.focus[DONT_TRUST])
                    .tab_index(0)
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .h(px(30.))
                    .px(px(theme::SPACE[7]))
                    .rounded(px(theme::radius::SM))
                    .cursor_pointer()
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .bg(c(p.surface_hover))
                    .border_1()
                    .border_color(ca(p.focus_ring, 0.))
                    .hover(|s| s.bg(c(p.surface_active)))
                    .focus(|s| s.border_color(c(p.focus_ring)))
                    .child(t("Don't Trust"))
                    .on_click(cx.listener(|this, _, window, cx| this.answer_trust(DONT_TRUST, window, cx))),
            );

        let card = div()
            .id("trust-dialog")
            .key_context("TrustDialog")
            .debug_selector(|| "trust-dialog".into())
            .w(px(540.))
            .max_w(relative(0.92))
            .max_h(relative(0.92))
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .gap(px(theme::SPACE[6]))
            .p(px(theme::SPACE[8]))
            .bg(c(p.surface_raised))
            .border_1()
            .border_color(c(p.border))
            .rounded(px(theme::radius::LG))
            .shadow_lg()
            .text_size(px(text::BASE.0))
            .text_color(c(p.text))
            .occlude()
            // Escape answers "Don't Trust", as in every other dialog.
            .on_action(cx.listener(|this, _: &TrustDialogDismiss, window, cx| this.answer_trust(DONT_TRUST, window, cx)))
            .on_action(cx.listener(|this, _: &TrustDialogNext, window, cx| this.move_trust_focus(true, window, cx)))
            .on_action(cx.listener(|this, _: &TrustDialogPrev, window, cx| this.move_trust_focus(false, window, cx)))
            // Return and Space press the button the keyboard is on.
            .on_action(cx.listener(|this, _: &TrustDialogConfirm, window, cx| {
                let focused = this.cfg.trust_ask.dialog.as_ref().and_then(|d| d.focus.iter().position(|h| h.is_focused(window)));
                this.answer_trust(focused.unwrap_or(DONT_TRUST), window, cx);
            }))
            .child(header)
            .child(chip)
            .child(limits)
            .child(div().flex().flex_col().gap(px(theme::SPACE[3])).child(this_one).children(with_parent))
            .child(footer);
        Some(deferred(
            div()
                .id("trust-backdrop")
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

    /// What a screen reader gets while the question is open: the same snapshot as the update dialog's.
    pub(in crate::workbench) fn trust_dialog_snap(&self, window: &Window) -> Option<super::a11y::DialogSnap> {
        let d = self.cfg.trust_ask.dialog.as_ref()?;
        let labels = d.labels();
        Some(super::a11y::DialogSnap {
            title: d.title().to_string(),
            description: d.description(),
            progress: None,
            focused: d.focus.iter().take(labels.len()).position(|h| h.is_focused(window)),
            buttons: labels,
        })
    }

    /// Tab and Shift+Tab walk the buttons as they are drawn (the choices from the top, "Don't Trust" last) and stay in the card.
    fn move_trust_focus(&mut self, forward: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(d) = &self.cfg.trust_ask.dialog else { return };
        let order: Vec<usize> = [TRUST].into_iter().chain(d.parent.as_ref().map(|_| TRUST_PARENT)).chain([DONT_TRUST]).collect();
        let at = order.iter().position(|i| d.focus[*i].is_focused(window)).unwrap_or(order.len() - 1);
        let next = if forward { (at + 1) % order.len() } else { (at + order.len() - 1) % order.len() };
        window.focus(&d.focus[order[next]]);
        cx.notify();
    }

    /// A screen reader presses or focuses a button of the question. The tree may be old: it acts only when that button is still
    /// there under the same name.
    pub(in crate::workbench) fn press_trust_button(&mut self, index: usize, label: Option<&str>, press: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(d) = &self.cfg.trust_ask.dialog else { return };
        if d.labels().get(index).map(String::as_str) != label {
            return;
        }
        if press {
            self.answer_trust(index, window, cx);
        } else {
            window.focus(&d.focus[index]);
        }
    }

    /// The buttons of the open question, in tab order (`None`: no question is open).
    #[cfg(test)]
    pub fn trust_dialog_for_test(&self) -> Option<Vec<String>> {
        self.cfg.trust_ask.dialog.as_ref().map(TrustDialog::labels)
    }

    /// Presses the button of the open question that is labelled `label`.
    #[cfg(test)]
    pub fn press_trust_for_test(&mut self, label: &str, window: &mut Window, cx: &mut Context<Self>) {
        let index = self.trust_dialog_for_test().and_then(|labels| labels.iter().position(|l| l == label));
        self.answer_trust(index.expect("a button with that label"), window, cx);
    }

    /// The accessibility snapshot of the open question.
    #[cfg(test)]
    pub fn trust_dialog_snap_for_test(&self, window: &Window) -> Option<super::a11y::DialogSnap> {
        self.trust_dialog_snap(window)
    }
}
