//! The questions Codeyard asks with the system's dialog (a sheet on macOS, a task dialog on Windows): what is asked,
//! the buttons in order, which one Return answers and which one Escape answers. Building a question is plain data, so
//! every question is tested without the window, the network, Docker or an SSH configuration; asking it is one call.
//!
//! The Cancel button is marked as such for the platform whatever its label's language (GPUI recognises only the English
//! word "Cancel" by itself), so Escape answers it on macOS and Windows. Return answers the first button, unless that is
//! the Cancel button. A question answered with Cancel (or closed) changes nothing and gives the keyboard back to what had it
//! when the question was asked.

use gpui::{App, PromptButton, PromptLevel, Window};

/// A question for the system dialog.
#[derive(Clone, Debug, PartialEq)]
pub struct Question {
    pub level: PromptLevel,
    pub title: String,
    pub detail: Option<String>,
    /// Button labels in the order given to the platform (the answer is an index into it).
    pub buttons: Vec<String>,
    /// The button that changes nothing: Escape answers it.
    pub cancel: Option<usize>,
}

impl Question {
    pub fn new(level: PromptLevel, title: impl Into<String>) -> Self {
        Self { level, title: title.into(), detail: None, buttons: Vec::new(), cancel: None }
    }

    pub fn detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    /// A button that acts.
    pub fn button(mut self, label: impl Into<String>) -> Self {
        self.buttons.push(label.into());
        self
    }

    /// The button that changes nothing ("Cancel", "Not Now", "Keep Waiting"…): Escape answers it.
    pub fn cancel(mut self, label: impl Into<String>) -> Self {
        self.cancel = Some(self.buttons.len());
        self.buttons.push(label.into());
        self
    }

    /// The button Return answers (the platform's own rule, kept here so the tests say which button it is): the first one, unless that is the Cancel button (a question that puts Cancel first
    /// so that Return never confirms by accident; Return then changes nothing).
    #[cfg(test)]
    pub fn default_button(&self) -> Option<usize> {
        (!self.buttons.is_empty() && self.cancel != Some(0)).then_some(0)
    }

    /// The buttons as GPUI gives them to the platform.
    pub fn answers(&self) -> Vec<PromptButton> {
        self.buttons
            .iter()
            .enumerate()
            .map(|(i, label)| if Some(i) == self.cancel { PromptButton::cancel(label.clone()) } else { PromptButton::new(label.clone()) })
            .collect()
    }

    /// Whether `answer` (from `ask`) is a button that acts: not Cancel, not a closed dialog.
    #[cfg(test)]
    pub fn acts(&self, answer: Option<usize>) -> bool {
        answer.is_some_and(|a| a < self.buttons.len() && Some(a) != self.cancel)
    }

    /// Asks in `window`. The answer is the index of the button pressed, `None` when the dialog went away unanswered.
    /// On Cancel (or nothing) the keyboard goes back to what had it when the question was asked, if that moved.
    pub fn ask(&self, window: &mut Window, cx: &mut App) -> gpui::Task<Option<usize>> {
        let before = window.focused(cx);
        let receiver = window.prompt(self.level, &self.title, self.detail.as_deref(), &self.answers(), cx);
        let cancel = self.cancel;
        window.spawn(cx, async move |cx| {
            let answer = receiver.await.ok();
            if answer.is_none() || answer == cancel {
                cx.update(|window, _| restore(window, before)).ok();
            }
            answer
        })
    }
}

/// Gives the keyboard back to `before` if it is no longer focused.
fn restore(window: &mut Window, before: Option<gpui::FocusHandle>) {
    if let Some(handle) = before
        && !handle.is_focused(window)
    {
        window.focus(&handle);
    }
}

/// Test support: answer the open question with `label` (its Cancel button) and check that the keyboard and the
/// accessibility tree's focus are where they were while it was open.
#[cfg(test)]
pub(crate) fn cancel_keeps_focus(wb: &gpui::Entity<crate::workbench::Workbench>, cx: &mut gpui::VisualTestContext, label: &str) {
    let tree = |cx: &mut gpui::VisualTestContext| wb.update_in(cx, |wb, window, cx| wb.a11y_snapshot(window, cx)).to_tree().focus;
    let keys = |cx: &mut gpui::VisualTestContext| cx.update(|window, cx| window.focused(cx));
    assert!(cx.has_pending_prompt(), "a question is open");
    let (tree_before, keys_before) = (tree(cx), keys(cx));
    cx.simulate_prompt_answer(label);
    cx.run_until_parked();
    assert_eq!(keys(cx), keys_before, "{label}: the keyboard is where it was");
    assert_eq!(tree(cx), tree_before, "{label}: the tree's focus is where it was");
}
