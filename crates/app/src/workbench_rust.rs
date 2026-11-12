//! Workbench: commands served by rust-analyzer's own protocol extensions (expand macro, parent module, `Cargo.toml`).
//! They are always in the palette and say so when the active file has no Rust language server.

use super::*;
use crate::i18n::t;

#[path = "workbench_rust_explain.rs"]
mod explain;
#[path = "workbench_rust_nextest.rs"]
mod nextest;
#[path = "workbench_rust_runnables.rs"]
mod runnables;
#[path = "workbench_rust_server.rs"]
mod server;
#[path = "workbench_rust_status.rs"]
mod status;
#[path = "workbench_rust_target.rs"]
mod target;
#[path = "workbench_rust_views.rs"]
mod views;
pub use explain::RustExplainError;
pub(super) use nextest::use_nextest;
pub use runnables::RustRunRunnable;

/// What a Rust picker is choosing between.
pub(super) enum Choice {
    Runnables(Vec<runnables::Entry>),
    /// The triples listed; `None` is the default (no `--target`).
    Targets(Vec<Option<String>>),
}
pub use server::*;
pub use target::RustSelectTarget;
pub use views::{RustOpenDocs, RustViewHir, RustViewMir, RustViewSyntaxTree};
#[cfg(test)]
#[path = "workbench_rust_explain_tests.rs"]
mod explain_tests;
#[cfg(test)]
#[path = "workbench_rust_runnables_tests.rs"]
mod runnable_tests;
#[cfg(test)]
#[path = "workbench_rust_tests.rs"]
mod tests;
#[cfg(test)]
#[path = "workbench_rust_trust_tests.rs"]
mod trust_tests;
#[cfg(test)]
#[path = "workbench_rust_views_tests.rs"]
mod view_tests;
use crate::lsp::{self, METHOD_NOT_FOUND};
use serde_json::{Value, json};

/// `Cargo.toml` dependency hints, completion, updates, adding and removing.
#[path = "workbench_cargo.rs"]
pub mod cargo;

/// How long a Rust command waits for the language server before it says the server did not answer.
pub(super) const RUST_REQUEST_TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Clone, Copy, PartialEq)]
enum RustCommand {
    ExpandMacro,
    ParentModule,
    OpenCargoToml,
}

impl Workbench {
    /// The answer of a Rust picker: the row chosen, or nothing when it was dismissed.
    pub(super) fn on_rust_choice_event(&mut self, event: &PickerEvent, window: &mut Window, cx: &mut Context<Self>) {
        match event {
            PickerEvent::Confirm(Pick::Item { index, .. }) => {
                let index = *index;
                let Some(Overlay { kind: OverlayKind::RustChoice(choice), .. }) = self.close_overlay(window, cx) else { return };
                match choice {
                    Choice::Runnables(rows) => self.chose_runnable(rows, index, window, cx),
                    Choice::Targets(triples) => self.chose_rust_target(triples, index, window, cx),
                }
            }
            PickerEvent::Dismiss => {
                self.close_overlay(window, cx);
            }
            _ => {}
        }
    }

    pub(super) fn rust_expand_macro(&mut self, _: &RustExpandMacro, window: &mut Window, cx: &mut Context<Self>) {
        self.rust_command(RustCommand::ExpandMacro, window, cx);
    }

    pub(super) fn rust_parent_module(&mut self, _: &RustParentModule, window: &mut Window, cx: &mut Context<Self>) {
        self.rust_command(RustCommand::ParentModule, window, cx);
    }

    pub(super) fn rust_open_cargo_toml(&mut self, _: &RustOpenCargoToml, window: &mut Window, cx: &mut Context<Self>) {
        self.rust_command(RustCommand::OpenCargoToml, window, cx);
    }

    /// Why there is no rust-analyzer for the file at `path`: the setting, an untrusted folder, or a server that is not up.
    pub(super) fn rust_unavailable_message(&self, path: &str, cx: &gpui::App) -> String {
        if !cx.global::<Settings>().lsp_enabled {
            t("Rust language features are disabled. Enable lsp.enabled in Settings.").to_string()
        } else if let Some(root) = self.root_of(path, cx).filter(|root| !self.is_trusted_root(root, cx)) {
            crate::i18n::tf(
                "Language servers are off in “{}” until you trust it: they can run the project's build scripts. Run “Workspace: Trust This Folder”.",
                &[&ui::file_name(&root)],
            )
        } else {
            t("Rust language server is not ready. Check Output, then run “Developer: Restart Language Servers”.").to_string()
        }
    }

    fn rust_command(&mut self, cmd: RustCommand, window: &mut Window, cx: &mut Context<Self>) {
        let Some(editor) = self.active_editor().cloned().filter(|ed| ed.read(cx).language() == "Rust") else {
            show_info(self, t("Open a Rust file first").into(), cx);
            return;
        };
        // The text sent with `didChange` is what the server answers about (`lsp_context` sends pending edits first).
        let context = editor.update(cx, |e, _| e.lsp_context().map(|(client, pos, _, _)| (client, pos, e.path.clone())));
        let Some((client, (line, character), path)) = context.filter(|(client, _, _)| client.config.language == "rust") else {
            let path = editor.read(cx).path.clone();
            if cmd == RustCommand::OpenCargoToml {
                let root = editor.read(cx).uses_host(&self.remote.host).then(|| self.root_of(&path, cx)).flatten();
                let host = editor.read(cx).host_arc();
                let lookup_host = host.clone();
                let generation = self.lsp.generation;
                let task = cx.background_executor().spawn(async move { crate::cargo_project::nearest_manifest(lookup_host.as_ref(), &path, root.as_deref()) });
                cx.spawn_in(window, async move |this, cx| {
                    let manifest = task.await;
                    this.update_in(cx, |this, window, cx| {
                        if this.lsp.generation != generation || this.active_editor().is_none_or(|ed| ed != &editor) {
                            return;
                        }
                        match manifest {
                            Some(path) => this.open_file_as(host, path, false, window, cx),
                            None => show_info(this, t("No Cargo.toml found for this file").into(), cx),
                        }
                    })
                    .ok();
                })
                .detach();
            } else {
                let message = self.rust_unavailable_message(&path, cx);
                show_info(self, message, cx);
            }
            return;
        };
        let document = json!({ "uri": lsp::path_to_uri(&path) });
        let at = json!({ "textDocument": document, "position": { "line": line, "character": character } });
        let (method, params) = match cmd {
            RustCommand::ExpandMacro => ("rust-analyzer/expandMacro", at),
            RustCommand::ParentModule => ("experimental/parentModule", at),
            RustCommand::OpenCargoToml => ("experimental/openCargoToml", json!({ "textDocument": document })),
        };
        #[cfg(test)]
        let timeout = self.rust_request_timeout;
        #[cfg(not(test))]
        let timeout = RUST_REQUEST_TIMEOUT;
        let task = cx.background_executor().spawn(async move { client.request_raw(method, params, timeout) });
        let generation = self.lsp.generation;
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            this.update_in(cx, |this, window, cx| {
                if this.lsp.generation != generation {
                    return;
                }
                match result {
                    Ok(v) => this.on_rust_reply(cmd, &v, window, cx),
                    Err(e) => {
                        show_info(
                            this,
                            if e.code == Some(METHOD_NOT_FOUND) { t("The Rust language server does not support this command").into() } else { e.message },
                            cx,
                        );
                    }
                }
            })
            .ok();
        })
        .detach();
    }

    fn on_rust_reply(&mut self, cmd: RustCommand, v: &Value, window: &mut Window, cx: &mut Context<Self>) {
        match cmd {
            RustCommand::ExpandMacro => match lsp::parse_expansion(v) {
                // An ordinary editor tab outside every folder: Rust highlighting, no language server (see `attach_lsp`).
                Some((name, text)) => {
                    self.create_scratch_with(lsp::expansion_file_name(&name), &text, window, cx);
                }
                None => show_info(self, t("No macro at the cursor").into(), cx),
            },
            RustCommand::ParentModule => {
                let mut locs = lsp::parse_locations(v);
                match locs.len() {
                    0 => show_info(self, t("No parent module found").into(), cx),
                    1 => {
                        let (path, (line, col)) = locs.remove(0);
                        self.open_location(path, line, col, window, cx);
                    }
                    _ => self.show_locations(locs, "parent modules", window, cx),
                }
            }
            RustCommand::OpenCargoToml => match lsp::parse_locations(v).into_iter().next() {
                Some((path, (line, col))) => self.open_location(path, line, col, window, cx),
                None => show_info(self, t("No Cargo.toml found for this file").into(), cx),
            },
        }
    }
}

fn show_info(wb: &mut Workbench, text: String, cx: &mut Context<Workbench>) {
    wb.notify(NoticeSource::Language, Notice::Info(text.into()));
    cx.notify();
}
