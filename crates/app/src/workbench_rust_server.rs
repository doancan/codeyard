//! Workbench: the palette commands that control rust-analyzer itself: reload the workspace, rebuild proc macros, run,
//! cancel and clear the check, and restart the server on its own (the other language servers keep running).

use gpui::{Context, Window};
use serde_json::{Value, json};

use super::super::lsp_glue::{ServerSlot, slot_language};
use super::super::*;
use crate::i18n::t;
use crate::lsp::{self, METHOD_NOT_FOUND};

gpui::actions!(
    workbench,
    [RustReloadWorkspace, RustRebuildProcMacros, RustRunFlycheck, RustCancelFlycheck, RustClearFlycheck, RustRestartAnalyzer, RustShowStatus]
);

/// Reloading a big workspace runs `cargo metadata` and build scripts before the server answers.
const CONTROL_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(120);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Control {
    ReloadWorkspace,
    RebuildProcMacros,
    RunFlycheck,
    CancelFlycheck,
    ClearFlycheck,
}

impl Control {
    fn method(self) -> &'static str {
        match self {
            Self::ReloadWorkspace => lsp::RELOAD_WORKSPACE,
            Self::RebuildProcMacros => lsp::REBUILD_PROC_MACROS,
            Self::RunFlycheck => lsp::RUN_FLYCHECK,
            Self::CancelFlycheck => lsp::CANCEL_FLYCHECK,
            Self::ClearFlycheck => lsp::CLEAR_FLYCHECK,
        }
    }

    /// The server answers a request (and the notice waits for it); a notification has no answer.
    fn is_request(self) -> bool {
        matches!(self, Self::ReloadWorkspace | Self::RebuildProcMacros)
    }

    fn started(self) -> String {
        match self {
            Self::ReloadWorkspace => t("Reloading the Rust workspace…"),
            Self::RebuildProcMacros => t("Rebuilding proc macros…"),
            Self::RunFlycheck => t("Rust check started"),
            Self::CancelFlycheck => t("Rust check cancelled"),
            Self::ClearFlycheck => t("Rust check results cleared"),
        }
        .to_string()
    }

    fn finished(self) -> String {
        match self {
            Self::ReloadWorkspace => t("Rust workspace reloaded"),
            _ => t("Proc macros rebuilt"),
        }
        .to_string()
    }
}

impl Workbench {
    pub(in super::super) fn rust_reload_workspace(&mut self, _: &RustReloadWorkspace, _: &mut Window, cx: &mut Context<Self>) {
        self.rust_control(Control::ReloadWorkspace, cx);
    }

    pub(in super::super) fn rust_rebuild_proc_macros(&mut self, _: &RustRebuildProcMacros, _: &mut Window, cx: &mut Context<Self>) {
        self.rust_control(Control::RebuildProcMacros, cx);
    }

    pub(in super::super) fn rust_run_flycheck(&mut self, _: &RustRunFlycheck, _: &mut Window, cx: &mut Context<Self>) {
        self.rust_control(Control::RunFlycheck, cx);
    }

    pub(in super::super) fn rust_cancel_flycheck(&mut self, _: &RustCancelFlycheck, _: &mut Window, cx: &mut Context<Self>) {
        self.rust_control(Control::CancelFlycheck, cx);
    }

    pub(in super::super) fn rust_clear_flycheck(&mut self, _: &RustClearFlycheck, _: &mut Window, cx: &mut Context<Self>) {
        self.rust_control(Control::ClearFlycheck, cx);
    }

    pub(in super::super) fn rust_show_status(&mut self, _: &RustShowStatus, window: &mut Window, cx: &mut Context<Self>) {
        self.show_rust_status(window, cx);
    }

    fn rust_control(&mut self, control: Control, cx: &mut Context<Self>) {
        let Some((_, client)) = self.rust_server(cx) else {
            let path = self.active_editor().map(|ed| ed.read(cx).path.clone());
            let message = match path.filter(|p| p.ends_with(".rs")) {
                Some(path) => self.rust_unavailable_message(&path, cx),
                None => t("Open a Rust file first").to_string(),
            };
            self.notice = Some(Notice::Info(message.into()));
            cx.notify();
            return;
        };
        let params = match control {
            Control::RunFlycheck => {
                let file = self.active_editor().map(|ed| ed.read(cx).path.clone()).filter(|p| p.ends_with(".rs"));
                json!({ "textDocument": file.map(|path| json!({ "uri": lsp::path_to_uri(&path) })) })
            }
            _ => Value::Null,
        };
        self.notice = Some(Notice::Info(control.started().into()));
        cx.notify();
        if !control.is_request() {
            client.notify(control.method(), params);
            return;
        }
        let task = cx.background_executor().spawn(async move { client.request_raw(control.method(), params, CONTROL_TIMEOUT) });
        let generation = self.lsp.generation;
        cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| {
                if this.lsp.generation != generation {
                    return;
                }
                let message = match result {
                    Ok(_) => control.finished(),
                    Err(e) if e.code == Some(METHOD_NOT_FOUND) => t("The Rust language server does not support this command").to_string(),
                    Err(e) => e.message,
                };
                this.notice = Some(Notice::Info(message.into()));
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// *Rust: Restart rust-analyzer*: only the Rust servers stop and start again; other languages keep their servers.
    pub(in super::super) fn rust_restart_analyzer(&mut self, _: &RustRestartAnalyzer, _: &mut Window, cx: &mut Context<Self>) {
        let keys: Vec<String> = self
            .lsp
            .servers
            .iter()
            .filter(|(key, slot)| slot_language(key) == "rust" && !matches!(slot, ServerSlot::Starting(_)))
            .map(|(k, _)| k.clone())
            .collect();
        let mut clients = Vec::new();
        for key in &keys {
            if let Some(ServerSlot::Running(client)) = self.lsp.servers.remove(key) {
                clients.push(client);
            }
            // What the old server says from now on (its exit, a check it never ended) belongs to nobody.
            self.lsp.rust.starts.forget(key);
            self.lsp.rust.servers.remove(key);
            self.lsp.restarts.remove(key);
        }
        let mut affected = Vec::new();
        for ed in self.all_editors() {
            if ed.read(cx).lsp_client().is_some_and(|c| clients.iter().any(|old| Arc::ptr_eq(old, c))) {
                ed.update(cx, |e, cx| e.set_lsp(None, "", cx));
                affected.push(ed);
            }
        }
        self.lsp.diagnostics.retain(|path, _| !lsp::language_of(path).is_some_and(|(_, language)| language == "rust"));
        for client in clients {
            cx.background_executor().spawn(async move { client.shutdown() }).detach();
        }
        for ed in &affected {
            self.attach_lsp(ed, cx);
        }
        // A file that had no server (it failed before) gets another try as well.
        for ed in self.all_editors() {
            if ed.read(cx).lsp_client().is_none() && ed.read(cx).language() == "Rust" {
                self.attach_lsp(&ed, cx);
            }
        }
        self.notice = Some(Notice::Info(t("Restarting rust-analyzer…").into()));
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_and_notifications_are_told_apart() {
        assert!(Control::ReloadWorkspace.is_request() && Control::RebuildProcMacros.is_request());
        assert!(!Control::RunFlycheck.is_request() && !Control::CancelFlycheck.is_request() && !Control::ClearFlycheck.is_request());
        assert_eq!(Control::RunFlycheck.method(), "rust-analyzer/runFlycheck");
        assert_eq!(Control::ReloadWorkspace.method(), "rust-analyzer/reloadWorkspace");
    }
}
