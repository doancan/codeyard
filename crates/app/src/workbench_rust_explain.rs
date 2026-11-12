//! Workbench: *Explain error E0xxx*. A diagnostic that has a rustc error code opens the page its server linked; without
//! a link `rustc --explain` runs in the background and its text opens as a read-only page.

use gpui::{AnyElement, Context, Window, div, px};

use super::super::*;
use crate::i18n::{t, tf};
use crate::rust_toolchain::explain::{self, Explain};

gpui::actions!(workbench, [RustExplainError]);

impl Workbench {
    /// *Rust: Explain Error at Cursor*: the most severe problem under the cursor that has an error code.
    pub(in super::super) fn rust_explain_error(&mut self, _: &RustExplainError, window: &mut Window, cx: &mut Context<Self>) {
        let found = self.active_editor().and_then(|ed| ed.read(cx).diagnostic_at_cursor());
        let plan = found.as_ref().and_then(|d| explain::code_of(d.code.as_deref(), &d.message).and_then(|code| explain::plan(code, d.code_href.as_deref())));
        match plan {
            Some(plan) => self.explain(plan, window, cx),
            None => {
                self.notify(NoticeSource::Language, Notice::Info(t("No rustc error code at the cursor").into()));
                cx.notify();
            }
        }
    }

    /// The small button on a Problems row that has an error code.
    pub(in super::super) fn explain_button(&self, row: usize, code: String, href: Option<String>, p: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let label = tf("Explain error {}", &[&code]);
        let tip: gpui::SharedString = label.clone().into();
        let plan = explain::plan(&code, href.as_deref());
        let weak = cx.entity().downgrade();
        let press_plan = plan.clone();
        div()
            .id(("explain", row))
            .relative()
            .flex_none()
            .size(px(20.))
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(theme::radius::SM))
            .cursor_pointer()
            .hover(|d| d.bg(c(p.surface_active)))
            .child(icon("book-open", 14., p.text_muted))
            .child(crate::a11y_controls::control(crate::a11y_controls::Control::button(t("Problems"), format!("explain{row}"), label, |_, _| {}).on_entity(
                weak,
                move |this: &mut Workbench, window, cx| {
                    if let Some(plan) = press_plan.clone() {
                        this.explain(plan, window, cx);
                    }
                },
            )))
            .tooltip(move |_, cx| cx.new(|_| Tooltip(tip.clone())).into())
            .on_click(cx.listener(move |this, _, window, cx| {
                cx.stop_propagation();
                if let Some(plan) = plan.clone() {
                    this.explain(plan, window, cx);
                }
            }))
            .into_any_element()
    }

    fn explain(&mut self, plan: Explain, window: &mut Window, cx: &mut Context<Self>) {
        match plan {
            Explain::Browse(url) => cx.open_url(&url),
            Explain::Rustc(code) => self.explain_with_rustc(code, window, cx),
        }
    }

    /// `rustc --explain` on the machine that builds, without waiting for it on the UI thread. It runs in no project
    /// folder, so a project's pinned toolchain is not started for it.
    fn explain_with_rustc(&mut self, code: String, window: &mut Window, cx: &mut Context<Self>) {
        self.notify(NoticeSource::Language, Notice::Info(tf("Looking up {}…", &[&code]).into()));
        cx.notify();
        let host = self.remote.host.clone();
        let asked = code.clone();
        let output = cx.background_executor().spawn(async move { host.exec("rustc", &["--explain", &asked], None, None) });
        cx.spawn_in(window, async move |this, cx| {
            let output = output.await;
            this.update_in(cx, |this, window, cx| match output {
                Ok(out) if out.ok() && !out.stdout_str().trim().is_empty() => {
                    this.dismiss_notice();
                    let name = format!("{}.md", tf("Error code {}", &[&code]));
                    this.show_pull_page(name, &explain::explanation_page(&code, &out.stdout_str()), window, cx);
                }
                Ok(out) => {
                    let why = format!("{}{}", out.stdout_str(), out.stderr_str());
                    this.notify(
                        NoticeSource::Language,
                        Notice::Error(tf("Could not explain {0}: {1}", &[&code, &crate::cargo_project::explain_failure(&why)]).into()),
                    );
                    cx.notify();
                }
                Err(e) => {
                    this.notify(
                        NoticeSource::Language,
                        Notice::Error(tf("Could not explain {0}: {1}", &[&code, &crate::cargo_project::explain_failure(&e.to_string())]).into()),
                    );
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }
}
