//! `Cargo.toml` lookups are off until the user turns them on (`rust.cargo.dependencyLookups`): nothing contacts crates.io by
//! itself. This file holds the way in: a one-time, non-modal question above a manifest that has dependencies (Enable, Not now,
//! Don't ask again), and *Cargo: Check Dependency Updates*, which looks the file up once on request.

use super::*;
use crate::cargo_project::deps::hints;
use crate::editor::CargoChoice;

/// The stored answer "Don't ask again" (kept in the state database, like other one-time choices).
const CONSENT_KEY: &str = "cargo.lookups.consent";
const NEVER: &str = "never";
/// Manifests larger than this are not read to decide about the question.
const MAX_MANIFEST_BYTES: usize = 512 * 1024;

impl Workbench {
    fn cargo_never_ask(&self) -> bool {
        self.persist.state.as_ref().and_then(|s| s.get(CONSENT_KEY)).as_deref() == Some(NEVER)
    }

    /// Show or hide the question above `ed`: shown while lookups are off, the user did not say no, and the file names a crate
    /// that would be looked up.
    pub(in crate::workbench) fn cargo_consider_consent(&mut self, ed: &Entity<EditorView>, cx: &mut Context<Self>) {
        let text = {
            let rope = ed.read(cx).rope_snapshot();
            (rope.len_bytes() <= MAX_MANIFEST_BYTES).then(|| rope.to_string())
        };
        let wanted = text.is_some_and(|t| manifest::parse(&t).is_ok_and(|m| !hints::wanted(&m).is_empty()));
        let show = wanted && !self.lsp.cargo.declined && !self.cargo_never_ask();
        ed.update(cx, |e, cx| e.set_cargo_consent(show, cx));
    }

    fn hide_cargo_consent(&mut self, cx: &mut Context<Self>) {
        for ed in self.all_editors() {
            ed.update(cx, |e, cx| e.set_cargo_consent(false, cx));
        }
    }

    /// A button of the question.
    pub(in crate::workbench) fn cargo_consent_chosen(&mut self, choice: CargoChoice, window: &mut Window, cx: &mut Context<Self>) {
        match choice {
            CargoChoice::Enable => {
                self.write_setting("rust.cargo.dependencyLookups", serde_json::json!(true), window, cx);
                for ed in self.all_editors() {
                    self.schedule_cargo(&ed, Duration::ZERO, cx);
                }
            }
            CargoChoice::NotNow => self.lsp.cargo.declined = true,
            CargoChoice::Never => {
                if let Some(Err(e)) = self.persist.state.as_ref().map(|s| s.set(CONSENT_KEY, NEVER)) {
                    eprintln!("could not remember {CONSENT_KEY}: {e}");
                }
                self.lsp.cargo.declined = true;
            }
        }
        self.hide_cargo_consent(cx);
        cx.notify();
    }

    /// Look `ed`'s manifest up now, whatever the setting says; it stays looked up (and fresh) until its tab closes.
    pub(in crate::workbench) fn cargo_look_up_now(&mut self, ed: &Entity<EditorView>, cx: &mut Context<Self>) {
        let path = ed.read(cx).path.clone();
        self.lsp.cargo.explicit.insert(path);
        ed.update(cx, |e, cx| e.set_cargo_consent(false, cx));
        self.schedule_cargo(ed, Duration::ZERO, cx);
    }

    /// *Cargo: Check Dependency Updates*: one explicit lookup for this file, then the hints.
    pub(in crate::workbench) fn cargo_check_updates(&mut self, _: &CheckDependencyUpdates, _: &mut Window, cx: &mut Context<Self>) {
        let Some(ed) = self.cargo_editor(cx) else { return };
        self.cargo_look_up_now(&ed, cx);
        notify(self, t("Looking up the latest versions on crates.io…"), cx);
    }
}
