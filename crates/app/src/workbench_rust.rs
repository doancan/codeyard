//! Workbench: commands served by rust-analyzer's own protocol extensions (expand macro, parent module, `Cargo.toml`).
//! They are always in the palette and say so when the active file has no Rust language server.

use super::*;
use crate::i18n::t;
use crate::lsp::{self, METHOD_NOT_FOUND};
use serde_json::{Value, json};

#[derive(Clone, Copy, PartialEq)]
enum RustCommand {
    ExpandMacro,
    ParentModule,
    OpenCargoToml,
}

impl Workbench {
    pub(super) fn rust_expand_macro(&mut self, _: &RustExpandMacro, window: &mut Window, cx: &mut Context<Self>) {
        self.rust_command(RustCommand::ExpandMacro, window, cx);
    }

    pub(super) fn rust_parent_module(&mut self, _: &RustParentModule, window: &mut Window, cx: &mut Context<Self>) {
        self.rust_command(RustCommand::ParentModule, window, cx);
    }

    pub(super) fn rust_open_cargo_toml(&mut self, _: &RustOpenCargoToml, window: &mut Window, cx: &mut Context<Self>) {
        self.rust_command(RustCommand::OpenCargoToml, window, cx);
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
                let message = if !cx.global::<Settings>().lsp_enabled {
                    t("Rust language features are disabled. Enable lsp.enabled in Settings.").to_string()
                } else if let Some(root) = self.root_of(&path, cx).filter(|root| !self.is_trusted_root(root, cx)) {
                    crate::i18n::tf(
                        "Language servers are off in “{}” until you trust it: they can run the project's build scripts. Run “Workspace: Trust This Folder”.",
                        &[&ui::file_name(&root)],
                    )
                } else {
                    t("Rust language server is not ready. Check Output, then run “Developer: Restart Language Servers”.").to_string()
                };
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
        let task = cx.background_executor().spawn(async move { client.request_raw(method, params, Duration::from_secs(15)) });
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
    wb.notice = Some(Notice::Info(text.into()));
    cx.notify();
}
