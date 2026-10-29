//! Workbench: what users and authors see of running code extensions (docs/plugin-architecture.md §6.3, §9). On a code
//! extension's page: a switch per declared permission entry, the recent denials with **Copy fix**, and the crash card
//! (**Reinstall Previous Version**, **Copy Report**, **Report…**) after three crashes. In the editor area,
//! **Developer: Extension Inspector**: per extension, when and why it activated, its calls with guest, I/O and overhead
//! time, denials, log and last trap. Everything is in memory and local; nothing is sent anywhere.
//!
//! Switches are saved per extension and publisher (`ext_core::grants`); turning one off reaches a running exthost at
//! once (`ToHost::Grants`) and the broker reads them before every call. The records come from `ext_inspector`.

use std::cell::RefCell;
use std::collections::HashMap;

use ext_api::value::{LogLevel, Why};
use ext_core::grants::{self, Entry};
use ext_manifest::{ExtId, Permissions};
use ext_proto::ToHost;
use gpui::{AnyElement, Context, FocusHandle, Window, div, px};

use super::ext_glue::{ExtEntry, permission_lines};
use super::ext_view_glue::section_title;
use super::*;
use crate::ext_inspector as insp;

gpui::actions!(workbench, [ShowExtensionInspector]);

/// Calls, log lines and denials the Inspector shows per extension (the record keeps more).
const SHOWN: usize = 20;

#[derive(Default)]
pub(super) struct InspectorState {
    /// The user's switches by extension id, read from the store on first use and after every change.
    switches: RefCell<Option<HashMap<String, Permissions>>>,
    /// The Inspector page, so the keyboard lands on it when it opens.
    focus: Option<FocusHandle>,
}

/// One line of the Inspector, as drawn and as a screen reader reads it.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct InspectorLine {
    pub kind: LineKind,
    pub text: String,
    /// The manifest line **Copy fix** copies.
    pub fix: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LineKind {
    /// An extension.
    Extension,
    /// Calls, Denials, Log, Last trap.
    Section,
    Row,
}

fn why_text(why: Why) -> &'static str {
    match why {
        Why::NotDeclared => crate::i18n::t("not declared"),
        Why::RevokedByUser => crate::i18n::t("turned off on the extension page"),
        Why::UntrustedWorkspace => crate::i18n::t("the folder is not trusted"),
        Why::RevokedExtension => crate::i18n::t("this version was revoked"),
    }
}

fn level_text(l: LogLevel) -> &'static str {
    match l {
        LogLevel::Trace => "trace",
        LogLevel::Debug => "debug",
        LogLevel::Info => "info",
        LogLevel::Warn => "warn",
        LogLevel::Error => "error",
    }
}

/// A button that takes keyboard focus, with an id per row.
fn row_button(id: impl Into<gpui::ElementId>, label: &'static str, p: &Palette) -> gpui::Stateful<gpui::Div> {
    super::container_monitor_glue::button(id, label, p)
        .tab_index(0)
        .border_1()
        .border_color(ui::ca(p.focus_ring, 0.))
        .focus(|d| d.border_color(c(p.focus_ring)))
}

/// The plain-words line of one switch ("Connect to api.example.com").
fn entry_label(e: &Entry) -> String {
    let only = grants::switch(&Permissions::default(), e, false);
    permission_lines(&only).into_iter().next().unwrap_or_else(|| e.manifest_line())
}

impl Workbench {
    pub(super) fn show_extension_inspector(&mut self, _: &ShowExtensionInspector, window: &mut Window, cx: &mut Context<Self>) {
        self.tabs.page = Some(Page::ExtensionInspector);
        self.tabs._page_sub = None;
        self.tabs.page_active = true;
        self.tabs.diff_active = false;
        self.tabs.large_active = false;
        let focus = self.cfg.exts.inspector.focus.get_or_insert_with(|| cx.focus_handle().tab_stop(true)).clone();
        window.focus(&focus);
        cx.notify();
    }

    /// What the user turned off for extension `id` (empty when nothing, or for another publisher's package).
    pub(super) fn extension_revoked(&self, id: &str) -> Permissions {
        let mut cache = self.cfg.exts.inspector.switches.borrow_mut();
        let map = cache.get_or_insert_with(|| match self.cfg.dir.as_deref().filter(|_| !crate::ext::turned_off()) {
            Some(dir) => crate::ext::store(dir).all_revoked_permissions().into_iter().collect(),
            None => HashMap::new(),
        });
        map.get(id).cloned().unwrap_or_default()
    }

    /// Read the switches again and tell every running exthost what it may call now: a permission turned off is
    /// refused from the next call, without a restart.
    pub(super) fn refresh_extension_grants(&mut self, cx: &mut Context<Self>) {
        *self.cfg.exts.inspector.switches.borrow_mut() = None;
        let reg = crate::ext::current();
        for e in reg.active.iter().filter(|e| e.manifest.code.is_some()) {
            let Some(p) = self.cfg.exts.code.process(e.id()) else { continue };
            let scope = self.extension_scope(e, cx);
            let _ = p.send(&ToHost::Grants(crate::ext_runtime::granted_interfaces(&scope.permissions)));
        }
    }

    /// A switch on the extension page: saved for this extension and publisher, then applied at once.
    pub(super) fn set_extension_permission(&mut self, id: &str, entry: &Entry, on: bool, cx: &mut Context<Self>) {
        let Some(dir) = self.cfg.dir.clone().filter(|_| !crate::ext::turned_off()) else { return };
        let store = crate::ext::store(&dir);
        let Ok(ext_id) = ExtId::parse(id) else { return };
        let source = match self.cfg.exts.list.iter().find(|e| e.manifest.id.as_str() == id) {
            Some(e) => e.source.clone(),
            None => match store.locked().ok().and_then(|l| l.get(&ext_id).map(|l| l.source.clone())) {
                Some(s) => s,
                None => return,
            },
        };
        let now = grants::switch(&self.extension_revoked(id), entry, on);
        if let Err(e) = store.set_revoked_permissions(&ext_id, &source, &now) {
            self.notice = Some(Notice::Error(e.to_string().into()));
            cx.notify();
            return;
        }
        self.refresh_extension_grants(cx);
        let label = entry_label(entry);
        let done = if on { crate::i18n::tf("{0} may again: {1}", &[&id, &label]) } else { crate::i18n::tf("{0} may no longer: {1}", &[&id, &label]) };
        self.notice = Some(Notice::Info(done.into()));
        cx.notify();
    }

    fn copy_fix(&mut self, fix: &str, cx: &mut Context<Self>) {
        cx.write_to_clipboard(gpui::ClipboardItem::new_string(fix.to_string()));
        self.notice = Some(Notice::Info(crate::i18n::tf("Copied: {}", &[&fix]).into()));
        cx.notify();
    }

    /// The crash report of an extension (version, trap, frames; nothing else).
    pub(super) fn extension_crash_report(&self, e: &ExtEntry) -> String {
        let id = e.manifest.id.as_str();
        insp::crash_report(id, &e.manifest.version.to_string(), self.cfg.exts.code.last_trap.get(id))
    }

    fn copy_crash_report(&mut self, id: &str, cx: &mut Context<Self>) {
        let Some(e) = self.cfg.exts.list.iter().find(|e| e.manifest.id.as_str() == id) else { return };
        let report = self.extension_crash_report(e);
        cx.write_to_clipboard(gpui::ClipboardItem::new_string(report));
        self.notice = Some(Notice::Info(crate::i18n::t("The crash report is on the clipboard. Nothing was sent.").into()));
        cx.notify();
    }

    /// **Report…**: a new issue in the extension's repository, filled in with the report, after the user agrees to open
    /// the link.
    fn report_crash(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(e) = self.cfg.exts.list.iter().find(|e| e.manifest.id.as_str() == id) else { return };
        let Some(repo) = e.manifest.repository.clone() else { return };
        let report = self.extension_crash_report(e);
        if let Some(url) = insp::issue_url(&repo, id, &e.manifest.version.to_string(), &report) {
            self.confirm_open_link(url, window, cx);
        }
    }

    /// **Reinstall Previous Version**: back to the version in the store, which may run again this session.
    pub(super) fn reinstall_previous_extension(&mut self, id: ExtId, window: &mut Window, cx: &mut Context<Self>) {
        self.cfg.exts.code.disabled.remove(id.as_str());
        self.roll_back_extension(id, window, cx);
        // The crash card goes now, whatever the rollback still does in the background.
        cx.notify();
    }

    /// Recent denials of one extension, newest first, with their fix.
    fn extension_denials(&self, id: &str) -> Vec<(insp::Denial, Option<String>)> {
        let revoked = self.extension_revoked(id);
        let record = self.cfg.exts.code.record();
        record.denials_of(id).rev().take(SHOWN).map(|d| (d.clone(), insp::fix_line(d, &revoked))).collect()
    }

    fn denial_text(d: &insp::Denial) -> String {
        format!("{} {}: {} ({})", insp::clock(d.at), d.call, d.permission, why_text(d.why))
    }

    /// The code extension's part of its page: the crash card, the switches and the recent denials.
    pub(super) fn extension_code_cards(&self, e: &ExtEntry, p: &Palette, cx: &mut Context<Self>) -> Option<gpui::Div> {
        let m = e.manifest.clone();
        m.code.as_ref()?;
        let id = m.id.to_string();
        let mut out = div().flex().flex_col().gap(px(theme::SPACE[1]));
        if self.cfg.exts.code.disabled.contains(&id) {
            out = out.child(self.crash_card(e, p, cx));
        }
        let revoked = self.extension_revoked(&id);
        out = out.child(section_title(crate::i18n::t("Permission switches"), p));
        let entries = grants::entries(&m.permissions);
        if entries.is_empty() {
            out = out.child(div().text_color(c(p.text_muted)).child(crate::i18n::t("It declares no permissions.")));
        }
        for (i, entry) in entries.into_iter().enumerate() {
            let on = !grants::is_revoked(&revoked, &entry);
            let (ext, label) = (id.clone(), entry_label(&entry));
            out = out.child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(theme::SPACE[2]))
                    .debug_selector(move || format!("ext-switch-{i}"))
                    .child(div().flex_1().min_w_0().text_color(c(if on { p.text } else { p.text_muted })).child(label))
                    .child(
                        row_button(("ext-switch", i), if on { crate::i18n::t("On") } else { crate::i18n::t("Off") }, p)
                            .on_click(cx.listener(move |this, _, _, cx| this.set_extension_permission(&ext, &entry, !on, cx))),
                    ),
            );
        }
        out = out.child(section_title(crate::i18n::t("Recent denials"), p));
        let denials = self.extension_denials(&id);
        if denials.is_empty() {
            out = out.child(div().text_color(c(p.text_muted)).child(crate::i18n::t("None in this session.")));
        }
        for (i, (d, fix)) in denials.into_iter().enumerate() {
            out = out.child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(theme::SPACE[2]))
                    .debug_selector(move || format!("ext-denial-{i}"))
                    .child(div().flex_1().min_w_0().child(Self::denial_text(&d)))
                    .children(fix.map(|fix| {
                        row_button(("ext-denial-fix", i), crate::i18n::t("Copy fix"), p).on_click(cx.listener(move |this, _, _, cx| this.copy_fix(&fix, cx)))
                    })),
            );
        }
        Some(out)
    }

    fn crash_card(&self, e: &ExtEntry, p: &Palette, cx: &mut Context<Self>) -> gpui::Div {
        let id = e.manifest.id.clone();
        let trap = self.cfg.exts.code.last_trap.get(id.as_str()).map(|t| insp::scrub(&t.message));
        let (back, copy, report) = (id.clone(), id.to_string(), id.to_string());
        let reportable = e.manifest.repository.as_deref().is_some_and(|r| insp::issue_url(r, "", "", "").is_some());
        div()
            .debug_selector(|| "ext-crash-card".into())
            .mt(px(theme::SPACE[2]))
            .p(px(theme::SPACE[3]))
            .flex()
            .flex_col()
            .gap(px(theme::SPACE[1]))
            .rounded(px(theme::radius::SM))
            .border_1()
            .border_color(c(p.danger))
            .child(div().text_color(c(p.danger)).child(crate::i18n::tf(
                "{} crashed three times within a minute and is turned off until Codeyard restarts. Nothing was sent anywhere.",
                &[&e.manifest.name],
            )))
            .children(trap.map(|t| div().text_color(c(p.text_muted)).child(crate::i18n::tf("Last trap: {}", &[&t]))))
            .when(e.previous.is_none(), |d| d.child(div().text_color(c(p.text_muted)).child(crate::i18n::t("No previous version is in the store."))))
            .child(
                div()
                    .pt(px(theme::SPACE[1]))
                    .flex()
                    .flex_wrap()
                    .gap(px(theme::SPACE[2]))
                    .when(e.previous.is_some(), |d| {
                        d.child(
                            row_button("ext-crash-reinstall", crate::i18n::t("Reinstall Previous Version"), p)
                                .on_click(cx.listener(move |this, _, w, cx| this.reinstall_previous_extension(back.clone(), w, cx))),
                        )
                    })
                    .child(
                        row_button("ext-crash-copy", crate::i18n::t("Copy Report"), p)
                            .on_click(cx.listener(move |this, _, _, cx| this.copy_crash_report(&copy, cx))),
                    )
                    .when(reportable, |d| {
                        d.child(
                            row_button("ext-crash-report", crate::i18n::t("Report…"), p)
                                .on_click(cx.listener(move |this, _, w, cx| this.report_crash(&report, w, cx))),
                        )
                    }),
            )
    }

    /// The Inspector's lines: per active code extension, its activation, calls, denials, log and last trap.
    pub(crate) fn inspector_lines(&self) -> Vec<InspectorLine> {
        let reg = crate::ext::current();
        let code = &self.cfg.exts.code;
        let record = code.record();
        let mut out = Vec::new();
        let mut line = |kind: LineKind, text: String, fix: Option<String>| out.push(InspectorLine { kind, text, fix });
        let mut any = false;
        for e in reg.active.iter().filter(|e| e.manifest.code.is_some()) {
            any = true;
            let id = e.id();
            let state = if code.disabled.contains(id) {
                crate::i18n::t("turned off after three crashes")
            } else if code.process(id).is_some() {
                crate::i18n::t("running")
            } else {
                crate::i18n::t("not running")
            };
            line(LineKind::Extension, format!("{id} {} ({state})", e.manifest.version), None);
            line(
                LineKind::Row,
                match record.activations.get(id) {
                    Some(a) => crate::i18n::tf("Activated {0} times, last at {1} by {2}", &[&a.count, &insp::clock(a.at), &a.reason]),
                    None => crate::i18n::t("Not activated in this window yet").to_string(),
                },
                None,
            );
            line(LineKind::Section, crate::i18n::t("Calls").to_string(), None);
            let calls: Vec<_> = record.calls.iter().rev().filter(|c| c.extension == id).take(SHOWN).collect();
            if calls.is_empty() {
                line(LineKind::Row, crate::i18n::t("None yet").to_string(), None);
            }
            for c in calls {
                let mut text = match c.guest {
                    Some(g) => crate::i18n::tf("{0} {1}: guest {2}", &[&insp::clock(c.at), &c.name, &insp::duration(g)]),
                    None => {
                        crate::i18n::tf("{0} {1}: I/O {2}, overhead {3}", &[&insp::clock(c.at), &c.name, &insp::duration(c.io), &insp::duration(c.overhead)])
                    }
                };
                if let Some(err) = &c.error {
                    text.push_str(&format!(" ({err})"));
                }
                line(LineKind::Row, text, None);
            }
            line(LineKind::Section, crate::i18n::t("Denials").to_string(), None);
            let revoked = self.extension_revoked(id);
            let denials: Vec<_> = record.denials_of(id).rev().take(SHOWN).collect();
            if denials.is_empty() {
                line(LineKind::Row, crate::i18n::t("None yet").to_string(), None);
            }
            for d in denials {
                line(LineKind::Row, Self::denial_text(d), insp::fix_line(d, &revoked));
            }
            line(LineKind::Section, crate::i18n::t("Log").to_string(), None);
            let log: Vec<_> = code.logs.get(id).map(|l| l.iter().rev().take(SHOWN).collect()).unwrap_or_default();
            if log.is_empty() {
                line(LineKind::Row, crate::i18n::t("None yet").to_string(), None);
            }
            for (level, text) in log.into_iter().rev() {
                line(LineKind::Row, format!("[{}] {text}", level_text(*level)), None);
            }
            if let Some(t) = code.last_trap.get(id) {
                line(LineKind::Section, crate::i18n::t("Last trap").to_string(), None);
                line(LineKind::Row, t.message.clone(), None);
                for f in &t.frames {
                    line(LineKind::Row, crate::i18n::tf("at {}", &[f]), None);
                }
            }
        }
        if !any {
            line(LineKind::Row, crate::i18n::t("No code extension is installed and enabled.").to_string(), None);
        }
        out
    }

    /// **Developer: Extension Inspector** in the editor area. Every row is a tab stop; **Copy fix** answers Enter and
    /// Space like a click.
    pub(super) fn extension_inspector(&self, p: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let mut column = div()
            .max_w(px(900.))
            .flex()
            .flex_col()
            .gap(px(theme::SPACE[1]))
            .text_color(c(p.text))
            .child(div().text_size(px(text::LG.0)).font_weight(gpui::FontWeight::SEMIBOLD).child(crate::i18n::t("Extension Inspector")));
        column = column.child(div().text_color(c(p.text_muted)).child(crate::i18n::t(
            "Running code extensions in this window: why they started, how long their calls took, what was denied. Nothing here leaves this computer.",
        )));
        for (i, l) in self.inspector_lines().into_iter().enumerate() {
            column = column.child(match l.kind {
                LineKind::Extension => div().pt(px(theme::SPACE[4])).font_weight(gpui::FontWeight::SEMIBOLD).child(l.text).into_any_element(),
                LineKind::Section => section_title(&l.text, p).into_any_element(),
                LineKind::Row => div()
                    .id(("insp-line", i))
                    .debug_selector(move || format!("insp-line-{i}"))
                    .flex()
                    .items_center()
                    .gap(px(theme::SPACE[2]))
                    .px(px(theme::SPACE[1]))
                    .tab_index(0)
                    .border_1()
                    .border_color(ui::ca(p.focus_ring, 0.))
                    .focus(|d| d.border_color(c(p.focus_ring)))
                    .text_size(px(text::XS.0))
                    .child(div().flex_1().min_w_0().child(l.text))
                    .children(l.fix.map(|fix| {
                        row_button(("insp-fix", i), crate::i18n::t("Copy fix"), p).on_click(cx.listener(move |this, _, _, cx| this.copy_fix(&fix, cx)))
                    }))
                    .into_any_element(),
            });
        }
        let mut page =
            div().id("extension-inspector").size_full().overflow_y_scroll().px(px(theme::SPACE[8])).py(px(theme::SPACE[6])).text_size(px(text::SM.0));
        if let Some(f) = &self.cfg.exts.inspector.focus {
            page = page.track_focus(f);
        }
        page.child(column).into_any_element()
    }

    /// A screen reader's **Copy fix** on Inspector line `ix`, if the line still says what it said.
    pub(super) fn inspector_copy_fix(&mut self, ix: usize, expected: &InspectorLine, cx: &mut Context<Self>) {
        let now = self.inspector_lines();
        if let Some(l) = now.get(ix).filter(|l| *l == expected)
            && let Some(fix) = l.fix.clone()
        {
            self.copy_fix(&fix, cx);
        }
    }
}

#[cfg(test)]
impl Workbench {
    /// (call, permission, why, fix) of each recent denial of `id`, newest first.
    pub(crate) fn denials_for_test(&self, id: &str) -> Vec<(String, String, Why, Option<String>)> {
        self.extension_denials(id).into_iter().map(|(d, fix)| (d.call, d.permission, d.why, fix)).collect()
    }

    pub(crate) fn set_permission_for_test(&mut self, id: &str, kind: &'static str, value: &str, on: bool, cx: &mut Context<Self>) {
        self.set_extension_permission(id, &Entry { kind, value: value.into() }, on, cx);
    }

    pub(crate) fn crash_report_for_test(&mut self, id: &str) -> Option<String> {
        if self.cfg.exts.list.is_empty() {
            self.refresh_extension_list();
        }
        let e = self.cfg.exts.list.iter().find(|e| e.manifest.id.as_str() == id)?;
        Some(self.extension_crash_report(e))
    }

    pub(crate) fn extension_disabled_for_test(&self, id: &str) -> bool {
        self.cfg.exts.code.disabled.contains(id)
    }

    /// The extension's page, as the Extensions view opens it (the list read first).
    pub(crate) fn open_extension_page_for_test(&mut self, id: &str, cx: &mut Context<Self>) {
        self.refresh_extension_list();
        self.select_extension(id, cx);
    }

    pub(crate) fn reinstall_previous_for_test(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.reinstall_previous_extension(ExtId::parse(id).expect("an id"), window, cx);
    }

    pub(crate) fn record_denial_for_test(&mut self, id: &str, call: &str, permission: &str, why: Why) {
        self.cfg.exts.code.record().denied(id, call, &ext_api::value::Denied { permission: permission.into(), why });
    }

    pub(crate) fn extension_calls_for_test(&self, id: &str) -> Vec<insp::Call> {
        self.cfg.exts.code.record().calls.iter().filter(|c| c.extension == id).cloned().collect()
    }
}
