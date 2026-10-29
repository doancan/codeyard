//! The workbench's language server management: one server per language, started when the first
//! file is opened; diagnostics are distributed to editors, the Problems panel and the status bar.
//! Servers are shut down when the root or host changes (a generation counter discards stale events).

use std::sync::{Arc, mpsc};

use gpui::{AppContext as _, Context, Entity, SharedString, WeakEntity, Window};

use super::{CodeActions, FindReferences, FormatDocument, FormatSelection, GoToSymbol, InputPurpose, Notice, OverlayKind, RenameSymbol, SaveAll, Workbench};
use crate::editor::EditorView;
use crate::i18n::{t, tf, tn};
use crate::lsp::{self, Client, ServerEvent};
use crate::picker::{PickItem, Picker, Source as PickSource};
use crate::settings::Settings;
use crate::ui;
use serde_json::{Value, json};
use std::time::Duration;

/// At most this many `workspace/symbol` results are shown.
const MAX_WORKSPACE_SYMBOLS: usize = 300;

/// Is `path` inside one of the roots? `..` is resolved first, and for local paths symlinks too, so a link inside the folder that
/// points elsewhere does not count as inside. A path that does not exist yet is checked through its nearest existing parent.
fn inside_roots(path: &str, roots: &[String], resolve_links: bool) -> bool {
    use std::path::{Component, Path, PathBuf};
    let clean = |p: &str| -> PathBuf {
        let mut out = PathBuf::new();
        for c in Path::new(p).components() {
            match c {
                Component::ParentDir => {
                    out.pop();
                }
                Component::CurDir => {}
                c => out.push(c),
            }
        }
        if !resolve_links {
            return out;
        }
        let mut tail = Vec::new();
        let mut base = out.clone();
        while !base.exists() {
            let Some(name) = base.file_name().map(|n| n.to_owned()) else { return out };
            tail.push(name);
            base.pop();
        }
        let mut real = base.canonicalize().unwrap_or(base);
        real.extend(tail.iter().rev());
        real
    };
    let target = clean(path);
    roots.iter().any(|r| target.starts_with(clean(r)))
}

/// Answer to a language server's question. Whatever way the picker closes, the server gets a reply:
/// dropping it without a selection sends `null` (dismissed).
pub struct QuestionReply {
    client: Arc<Client>,
    id: Option<serde_json::Value>,
    pub actions: Vec<String>,
}

impl QuestionReply {
    pub fn answer(mut self, index: usize) {
        if let (Some(id), Some(title)) = (self.id.take(), self.actions.get(index)) {
            self.client.respond(id, serde_json::json!({ "title": title }));
        }
    }
}

impl Drop for QuestionReply {
    fn drop(&mut self) {
        if let Some(id) = self.id.take() {
            self.client.respond(id, serde_json::Value::Null);
        }
    }
}

pub enum ServerSlot {
    /// Starting; editors (and language ids) to attach once it is ready.
    Starting(Vec<(WeakEntity<EditorView>, &'static str)>),
    Running(Arc<Client>),
    /// Did not start (not found, etc.): not retried again in this folder.
    Failed,
}

/// A crashing server is restarted at most this many times (a lost connection does not count).
const MAX_RESTARTS: u32 = 3;

impl Workbench {
    /// Attach an editor to its language's server (starting the server if needed).
    pub(super) fn attach_lsp(&mut self, ed: &Entity<EditorView>, cx: &mut Context<Self>) {
        let settings = cx.global::<Settings>();
        if !settings.lsp_enabled {
            return;
        }
        let (path, same_host) = {
            let e = ed.read(cx);
            (e.path.clone(), e.uses_host(&self.remote.host))
        };
        let Some((lang, key)) = lsp::language_of(&path).filter(|_| same_host) else { return };
        // A scratch (a macro expansion, notes) is outside every folder: the project's server would report it full of errors.
        if !self.remote.host.is_remote() && std::path::Path::new(&path).starts_with(self.scratch_dir()) {
            return;
        }
        let Some(config) = lsp::server_for(key, &settings.lsp_servers) else { return };
        // The file's own root: a separate server for files in additional folders (rust-analyzer, gopls… work per root).
        let primary = self.root(cx);
        let Some(root) = self.root_of(&path, cx).or_else(|| primary.clone()) else { return };
        // Language servers can run project code (rust-analyzer: build.rs and proc macros, typescript: node_modules).
        // Not started in an untrusted folder (like VS Code's restricted mode); `trust_root` restarts them once trusted.
        if !self.is_trusted_root(&root, cx) {
            if self.lsp.restricted_notified.insert(root.clone()) {
                self.notice = Some(Notice::Info(
                    tf(
                        "Language servers are off in “{}” until you trust it: they can run the project's build scripts. Run “Workspace: Trust This Folder”.",
                        &[&ui::file_name(&root)],
                    )
                    .into(),
                ));
                cx.notify();
            }
            return;
        }
        ed.update(cx, |e, _| e.set_lsp_root(Some(root.clone())));
        let slot = slot_key(key, &root, primary.as_deref());
        let key = slot.as_str();
        match self.lsp.servers.get_mut(key) {
            Some(ServerSlot::Running(c)) if c.is_alive() => {
                let c = c.clone();
                ed.update(cx, |e, cx| e.set_lsp(Some(c), lang, cx));
                return;
            }
            Some(ServerSlot::Starting(waiting)) => {
                waiting.push((ed.downgrade(), lang));
                return;
            }
            Some(ServerSlot::Failed) => return,
            _ => {}
        }
        self.lsp.servers.insert(key.to_string(), ServerSlot::Starting(vec![(ed.downgrade(), lang)]));
        let generation = self.lsp.generation;
        let (tx, rx) = mpsc::channel();
        let host = self.remote.host.clone();
        let key = key.to_string();
        let k = key.clone();
        self.lsp.tasks.push(cx.spawn(async move |this, cx| {
            use futures::StreamExt;
            // A busy UI keeps one diagnostics list per file and one progress line, not one entry per server message.
            let mut rx = ui::bridge_coalescing(rx, lsp::merge_server_event);
            while let Some(ev) = rx.next().await {
                let alive = this.update(cx, |this, cx| {
                    if this.lsp.generation == generation {
                        this.on_lsp_event(&k, ev, cx);
                    }
                });
                if alive.is_err() {
                    break;
                }
            }
        }));
        let command = config.command.clone();
        let start = cx.background_executor().spawn(async move { Client::start(host, config, &root, tx) });
        cx.spawn(async move |this, cx| {
            let result = start.await;
            this.update(cx, |this, cx| this.on_lsp_started(&key, &command, generation, result, cx)).ok();
        })
        .detach();
    }

    fn on_lsp_started(&mut self, key: &str, command: &str, generation: u64, result: Result<Arc<Client>, String>, cx: &mut Context<Self>) {
        if generation != self.lsp.generation {
            // Folder/host changed meanwhile: result is stale.
            if let Ok(c) = result {
                cx.background_executor().spawn(async move { c.shutdown() }).detach();
            }
            return;
        }
        let waiting = match self.lsp.servers.remove(key) {
            Some(ServerSlot::Starting(w)) => w,
            _ => Vec::new(),
        };
        match result {
            Ok(client) => {
                // The settings may have changed while the server was starting: it gets the current ones, not a restart.
                if let Some(config) = lsp::server_for(slot_language(key), &cx.global::<Settings>().lsp_servers)
                    && config.settings != client.settings()
                {
                    client.change_settings(config.settings);
                }
                for (ed, lang) in waiting {
                    if let Some(ed) = ed.upgrade() {
                        let c = client.clone();
                        ed.update(cx, |e, cx| e.set_lsp(Some(c), lang, cx));
                    }
                }
                self.lsp.servers.insert(key.to_string(), ServerSlot::Running(client));
            }
            // No connection means no server start, which isn't the server's fault. Don't mark it permanently "failed" or
            // raise a second notification that hides the real cause (connection lost); retried when the connection returns.
            Err(_) if matches!(self.remote.conn, super::Conn::Failed { .. } | super::Conn::Reconnecting { .. }) => {}
            Err(e) => {
                self.lsp.servers.insert(key.to_string(), ServerSlot::Failed);
                let remote_label = self.remote.host.is_remote().then(|| self.remote.host.label().trim_start_matches("ssh://").to_string());
                // Never came up (no shell, or exited at once like a rustup proxy):
                // show how to install instead of just "not installed".
                let lang = slot_language(key);
                let msg = if e.contains("not found") || e.contains("exited with code") {
                    let hint = lsp::install_hint(lang).filter(|_| config_is_default(lang, command, cx)).map(lsp::install_hint_text);
                    match (&remote_label, hint) {
                        (None, None) => tf("{} isn't available. Or set “lsp.servers” in settings.", &[&command]),
                        (None, Some(h)) => tf("{0} isn't available ({1}). Or set “lsp.servers” in settings.", &[&command, &h]),
                        (Some(host), None) => tf("{0} isn't available on {1}. Or set “lsp.servers” in settings.", &[&command, host]),
                        (Some(host), Some(h)) => tf("{0} isn't available on {1} ({2}). Or set “lsp.servers” in settings.", &[&command, host, &h]),
                    }
                } else {
                    format!("{command}: {e}")
                };
                self.notice = Some(Notice::Info(msg.into()));
            }
        }
        cx.notify();
    }

    fn on_lsp_event(&mut self, key: &str, ev: ServerEvent, cx: &mut Context<Self>) {
        match ev {
            ServerEvent::Diagnostics { path, diagnostics } => {
                if diagnostics.is_empty() {
                    self.lsp.diagnostics.remove(&path);
                } else {
                    self.lsp.diagnostics.insert(path.clone(), diagnostics);
                }
                // The editor gets a list merged with task problems (the server only knows its own diagnostics).
                let merged = self.diagnostics_for(&path);
                for ed in self.all_editors() {
                    if ed.read(cx).path == path {
                        let d = merged.clone();
                        ed.update(cx, |e, cx| e.set_diagnostics(d, cx));
                    }
                }
            }
            ServerEvent::Progress(text) => {
                // Servers send hundreds of progress notifications per second while indexing; each
                // forced a full redraw (+115 MB at peak). Capped at 4 Hz,
                // and only on text change.
                self.lsp.progress_next = Some(text.map(|t| SharedString::from(format!("{key}: {t}"))));
                if self.lsp.progress_timer.is_none() {
                    self.lsp.progress_timer = Some(cx.spawn(async move |this, cx| {
                        cx.background_executor().timer(std::time::Duration::from_millis(250)).await;
                        this.update(cx, |this, cx| {
                            this.lsp.progress_timer = None;
                            if let Some(next) = this.lsp.progress_next.take()
                                && next != this.lsp.progress
                            {
                                this.lsp.progress = next;
                                cx.notify();
                            }
                        })
                        .ok();
                    }));
                }
                return;
            }
            ServerEvent::Message(m) => {
                self.log_output(format!("[{key}] {}", m.chars().take(2000).collect::<String>()));
                self.notice = Some(Notice::Info(m.into()));
            }
            ServerEvent::Log { level, text } => {
                let tag = match level {
                    1 => "error",
                    2 => "warn",
                    3 => "info",
                    _ => "log",
                };
                for line in text.lines().take(50) {
                    self.log_output(format!("[{key}] {tag}: {}", line.chars().take(2000).collect::<String>()));
                }
                cx.notify();
                return;
            }
            ServerEvent::Question { id, message, actions } => {
                let client = match self.lsp.servers.get(key) {
                    Some(ServerSlot::Running(c)) => c.clone(),
                    _ => return,
                };
                if actions.is_empty() {
                    // With no options it is an informational message: show it and reply immediately.
                    self.notice = Some(Notice::Info(message.into()));
                    client.respond(id, serde_json::Value::Null);
                } else {
                    let handle = self.window_handle;
                    cx.spawn(async move |this, cx| {
                        cx.update_window(handle, |_, window, cx| {
                            this.update(cx, |this, cx| this.ask_lsp_question(client, id, message, actions, window, cx)).ok();
                        })
                        .ok();
                    })
                    .detach();
                }
            }
            ServerEvent::ApplyEdit { id, edit } => {
                // After a code action command the server asks for an edit.
                let applied = self.apply_workspace_edit(&edit, cx);
                if let Some(ServerSlot::Running(c)) = self.lsp.servers.get(key) {
                    c.respond(id, serde_json::json!({ "applied": applied }));
                }
            }
            ServerEvent::Exited(code) => {
                let Some(ServerSlot::Running(client)) = self.lsp.servers.remove(key) else {
                    cx.notify();
                    return; // exited while starting: `on_lsp_started` handles it as an error
                };
                self.lsp.progress = None;
                // Diagnostics and links of this server's files are dropped.
                let mut affected = Vec::new();
                for ed in self.all_editors() {
                    if ed.read(cx).lsp_client().is_some_and(|c| Arc::ptr_eq(c, &client)) {
                        ed.update(cx, |e, cx| e.set_lsp(None, "", cx));
                        affected.push(ed);
                    }
                }
                // Only the diagnostics of this server's (language + root) files are dropped.
                let (lang, slot_root) = (slot_language(key), slot_root(key).map(str::to_string).or_else(|| self.root(cx)));
                let roots = self.all_roots(cx);
                self.lsp.diagnostics.retain(|p, _| {
                    let same_lang = lsp::language_of(p).is_some_and(|(_, k)| k == lang);
                    let owner = roots.iter().filter(|r| p.starts_with(&format!("{}/", r.trim_end_matches('/')))).max_by_key(|r| r.len());
                    let in_root = slot_root.as_deref().is_none_or(|r| owner.map(String::as_str) == Some(r));
                    !(same_lang && in_root)
                });
                // A lost connection (`None`) does not count; a real crash, at most MAX_RESTARTS times.
                let crashes = self.lsp.restarts.entry(key.to_string()).or_default();
                if code.is_some() {
                    *crashes += 1;
                }
                if *crashes <= MAX_RESTARTS {
                    let generation = self.lsp.generation;
                    let weak: Vec<_> = affected.iter().map(|e| e.downgrade()).collect();
                    cx.spawn(async move |this, cx| {
                        cx.background_executor().timer(std::time::Duration::from_secs(1)).await;
                        this.update(cx, |this, cx| {
                            if this.lsp.generation == generation {
                                for ed in weak.iter().filter_map(|w| w.upgrade()) {
                                    this.attach_lsp(&ed, cx);
                                }
                            }
                        })
                        .ok();
                    })
                    .detach();
                } else {
                    self.notice = Some(Notice::Error(tf("{} keeps crashing; restart the folder to try again.", &[&client.config.command]).into()));
                }
            }
        }
        cx.notify();
    }

    /// Connection is back: restart for open files that have no language server.
    pub(super) fn reattach_lsp(&mut self, cx: &mut Context<Self>) {
        for ed in self.all_editors() {
            if ed.read(cx).lsp_client().is_none() {
                self.attach_lsp(&ed, cx);
            }
        }
    }

    /// Shut down all servers (folder/host/setting changed).
    pub(super) fn shutdown_lsp(&mut self, cx: &mut Context<Self>) {
        self.lsp.generation += 1;
        self.stop_debug_quietly(cx);
        for (_, slot) in self.lsp.servers.drain() {
            if let ServerSlot::Running(c) = slot {
                cx.background_executor().spawn(async move { c.shutdown() }).detach();
            }
        }
        for ed in self.all_editors() {
            if ed.read(cx).lsp_client().is_some() {
                ed.update(cx, |e, cx| e.set_lsp(None, "", cx));
            }
        }
        self.lsp.tasks.clear();
        self.lsp.diagnostics.clear();
        self.lsp.progress = None;
        self.lsp.restarts.clear();
    }

    /// Only the `settings` of these keys changed: their running servers are told, not restarted. A server still starting
    /// picks them up in `on_lsp_started`, one in its crash back-off when `attach_lsp` starts it again.
    pub(super) fn change_lsp_settings(&mut self, keys: &[String], cx: &mut Context<Self>) {
        for key in keys {
            let Some(config) = lsp::server_for(key, &cx.global::<Settings>().lsp_servers) else { continue };
            for (slot, server) in &self.lsp.servers {
                if let ServerSlot::Running(client) = server
                    && slot_language(slot) == key
                {
                    client.change_settings(config.settings.clone());
                }
            }
        }
    }

    /// *Problems: Copy Full Diagnostic*: the most severe diagnostic under the cursor, as the compiler wrote it when the
    /// server sent that, otherwise with its code and related locations.
    pub(super) fn copy_full_diagnostic(&mut self, cx: &mut Context<Self>) {
        let found = self.active_editor().and_then(|ed| {
            let e = ed.read(cx);
            e.diagnostic_at_cursor().map(|d| (d, e.path.clone()))
        });
        let notice = match found {
            Some((d, path)) => {
                let root = self.root_of(&path, cx);
                cx.write_to_clipboard(gpui::ClipboardItem::new_string(d.full_text(root.as_deref())));
                t("Diagnostic copied")
            }
            None => t("No problem at the cursor"),
        };
        self.notice = Some(Notice::Info(notice.into()));
        cx.notify();
    }

    /// Settings changed: set the servers up from scratch.
    pub(super) fn restart_lsp(&mut self, cx: &mut Context<Self>) {
        self.shutdown_lsp(cx);
        for ed in self.all_editors() {
            self.attach_lsp(&ed, cx);
        }
    }

    /// The server's question: options in the picker, message on top.
    fn ask_lsp_question(
        &mut self,
        client: Arc<Client>,
        id: serde_json::Value,
        message: String,
        actions: Vec<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.overlay.is_some() {
            self.close_overlay(window, cx);
        }
        let items = actions.iter().map(|a| PickItem { label: a.clone().into(), detail: None, keys: None }).collect();
        let title = format!("{}: {message}", client.config.command);
        let picker =
            cx.new(|cx| Picker::new(vec![PickSource::List { prefix: "", items }], t("Choose an answer (Esc: none)"), "", None, cx).with_message(title));
        let reply = QuestionReply { client, id: Some(id), actions };
        self.show_overlay(picker, OverlayKind::LspQuestion(reply), window, cx);
    }

    /// (errors, warnings) counts.
    pub(super) fn problem_counts(&self) -> (usize, usize) {
        let task = self.jobs.problems.iter().map(|(_, d)| d);
        self.lsp.diagnostics.values().flatten().chain(task).fold((0, 0), |(e, w), d| match d.severity {
            1 => (e + 1, w),
            2 => (e, w + 1),
            _ => (e, w),
        })
    }

    /// Go to a location: open the file, show the (line, UTF-16 column) position.
    pub(super) fn open_location(&mut self, path: String, line: u32, col: u32, window: &mut Window, cx: &mut Context<Self>) {
        let path = if self.remote.host.is_remote() { path } else { ui::local_path(std::path::Path::new(&path)) };
        self.record_nav(cx);
        self.lsp.pending_reveal = Some((path.clone(), line, col));
        // Opens in the focused group (if the same document is open in another group, it shows here too).
        if let Some(ix) = self.tabs.editors.iter().position(|e| e.read(cx).path == path) {
            self.activate(ix, window, cx);
        } else {
            let host = self.remote.host.clone();
            self.open_file_on(host, path, window, cx);
        }
    }
}

/// The install hint is only meaningful for the default server.
/// Key of a server slot: for the first root just the language key (as before), for additional folders
/// `lang@root`.
fn slot_key(lang: &str, root: &str, primary: Option<&str>) -> String {
    if Some(root) == primary { lang.to_string() } else { format!("{lang}@{root}") }
}

fn slot_language(slot: &str) -> &str {
    slot.split_once('@').map_or(slot, |(l, _)| l)
}

fn slot_root(slot: &str) -> Option<&str> {
    slot.split_once('@').map(|(_, r)| r)
}

fn config_is_default(key: &str, command: &str, cx: &gpui::App) -> bool {
    lsp::server_for(key, &[]).is_some_and(|d| d.command == command)
        && !cx.global::<Settings>().lsp_servers.iter().any(|(k, spec)| k == key && spec.as_ref().is_none_or(|s| s.command.is_some()))
}

// --- Language server actions (via the active editor) ---

impl Workbench {
    fn active_lsp(&mut self, cx: &mut Context<Self>) -> Option<(Entity<EditorView>, Arc<Client>, String, (u32, u32), lsp::Range16, Option<String>)> {
        let ed = self.active_editor()?.clone();
        let path = ed.read(cx).path.clone();
        match ed.update(cx, |e, _| e.lsp_context()) {
            Some((client, pos, range, word)) => Some((ed, client, path, pos, range, word)),
            None => {
                self.notice = Some(Notice::Info(t("No language server for this file").into()));
                cx.notify();
                None
            }
        }
    }

    /// Request in the background; the result is handled in the UI with `then`, errors go to the status bar.
    fn lsp_call(
        &mut self,
        client: Arc<Client>,
        method: &'static str,
        params: Value,
        cx: &mut Context<Self>,
        then: impl FnOnce(&mut Self, Value, &mut Window, &mut Context<Self>) + 'static,
        window: &mut Window,
    ) {
        let task = cx.background_executor().spawn(async move { client.request(method, params, Duration::from_secs(15)) });
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            this.update_in(cx, |this, window, cx| match result {
                Ok(v) => then(this, v, window, cx),
                Err(e) => {
                    this.notice = Some(Notice::Error(e.into()));
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    pub(super) fn rename_symbol(&mut self, _: &RenameSymbol, window: &mut Window, cx: &mut Context<Self>) {
        let Some((_, client, path, pos, _, word)) = self.active_lsp(cx) else { return };
        let Some(word) = word else {
            self.notice = Some(Notice::Info(t("Put the cursor on a symbol to rename it").into()));
            cx.notify();
            return;
        };
        // If the server supports `prepareRename`, ask first so no input opens where renaming is impossible.
        if client.capabilities["renameProvider"]["prepareProvider"].as_bool() != Some(true) {
            return self.show_input(InputPurpose::RenameSymbol { client, path, pos }, t("Rename to"), t("New name"), &word, window, cx);
        }
        let params = json!({ "textDocument": { "uri": lsp::path_to_uri(&path) }, "position": { "line": pos.0, "character": pos.1 } });
        let c = client.clone();
        self.lsp_call(
            c,
            "textDocument/prepareRename",
            params,
            cx,
            move |this, v, window, cx| match lsp::parse_prepare_rename(&v) {
                None => {
                    this.notice = Some(Notice::Info(t("The element can't be renamed.").into()));
                    cx.notify();
                }
                Some(range) => {
                    // The server's range text becomes the suggested name (else the word at the cursor).
                    let initial = range
                        .and_then(|((l0, c0), (l1, c1))| {
                            // The tab may have changed during the query: read from the editor of the file the request was for.
                            let ed = this.all_editors().into_iter().find(|e| e.read(cx).path == path)?;
                            let e = ed.read(cx);
                            let (a, b) = (e.lsp_char_index(l0, c0), e.lsp_char_index(l1, c1));
                            e.text_in(a..b)
                        })
                        .filter(|t| !t.is_empty() && !t.contains('\n'))
                        .unwrap_or(word);
                    this.show_input(InputPurpose::RenameSymbol { client, path, pos }, t("Rename to"), t("New name"), &initial, window, cx);
                }
            },
            window,
        );
    }

    pub(super) fn rename_symbol_to(&mut self, client: Arc<Client>, path: String, pos: (u32, u32), name: String, cx: &mut Context<Self>) {
        let params = json!({ "textDocument": { "uri": lsp::path_to_uri(&path) }, "position": { "line": pos.0, "character": pos.1 }, "newName": name });
        let task = cx.background_executor().spawn(async move { client.request("textDocument/rename", params, Duration::from_secs(15)) });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| match result {
                Ok(v) if !v.is_null() => {
                    this.apply_workspace_edit(&v, cx);
                }
                Ok(_) => this.notice = Some(Notice::Info(t("Nothing to rename here").into())),
                Err(e) => this.notice = Some(Notice::Error(e.into())),
            })
            .ok();
        })
        .detach();
    }

    /// Multi-file edit: open files go into the buffer (dirty), closed ones to disk
    /// (like VS Code's `files.refactoring.autoSave` default). Files outside the open folders are never touched: a language
    /// server must not be able to write wherever the user's account can.
    /// Returns whether the edit took effect, which is what `workspace/applyEdit` answers with.
    pub(super) fn apply_workspace_edit(&mut self, edit: &Value, cx: &mut Context<Self>) -> bool {
        let (files, skipped) = lsp::parse_workspace_edit(edit);
        let total = files.len();
        let roots = self.all_roots(cx);
        let open: Vec<String> = self.all_editors().iter().map(|e| e.read(cx).path.clone()).collect();
        let resolve = !self.remote.host.is_remote();
        let files: Vec<_> = files.into_iter().filter(|(p, _)| open.contains(p) || inside_roots(p, &roots, resolve)).collect();
        let outside = total - files.len();
        let n = files.len();
        let mut closed = Vec::new();
        for (path, edits) in files {
            match self.all_editors().into_iter().find(|e| e.read(cx).path == path) {
                Some(ed) => ed.update(cx, |e, cx| e.apply_lsp_edits(&edits, cx)),
                None => closed.push((path, edits)),
            }
        }
        if !closed.is_empty() {
            let host = self.remote.host.clone();
            let written: Vec<String> = closed.iter().map(|(p, _)| p.clone()).collect();
            let task = cx.background_executor().spawn(async move {
                let mut errors = Vec::new();
                for (path, edits) in closed {
                    let result = (|| -> Result<(), String> {
                        let (data, hash) = host.read(&path).map_err(|e| e.to_string())?;
                        let edited = buffer::Buffer::edit_bytes(&data, |b| {
                            edits.iter().map(|(((l0, c0), (l1, c1)), t)| (b.from_lsp_position(*l0, *c0)..b.from_lsp_position(*l1, *c1), t.clone())).collect()
                        })
                        .map_err(|_| t("not UTF-8").to_string())?;
                        match edited {
                            Some(bytes) => host.write(&path, bytes, Some(hash)).map(|_| ()).map_err(|e| e.to_string()),
                            None => Ok(()),
                        }
                    })();
                    if let Err(e) = result {
                        errors.push(format!("{}: {e}", ui::file_name(&path)));
                    }
                }
                errors
            });
            cx.spawn(async move |this, cx| {
                let errors = task.await;
                this.update(cx, |this, cx| {
                    if !errors.is_empty() {
                        this.notice = Some(Notice::Error(tf("Could not change {}", &[&errors.join(", ")]).into()));
                        cx.notify();
                    }
                    // Closed files changed on disk: the servers must not keep their old state.
                    this.notify_files_changed(&written);
                    this.refresh_git(300, cx);
                })
                .ok();
            })
            .detach();
        }
        let msg = if skipped > 0 {
            tn("Changed {0} file ({1} file operations skipped)", "Changed {0} files ({1} file operations skipped)", n, &[&skipped])
        } else {
            tn("Changed {} file", "Changed {} files", n, &[])
        };
        self.notice = Some(if outside > 0 {
            Notice::Error(
                tn(
                    "Ignored the language server's edit to {} file outside the open folders",
                    "Ignored the language server's edit to {} files outside the open folders",
                    outside,
                    &[],
                )
                .into(),
            )
        } else {
            Notice::Info(msg.into())
        });
        cx.notify();
        // Refusing everything is a failure for the server; a partly applied edit still counts as applied.
        n > 0 || outside == 0
    }

    /// Report files changed on disk to all language servers.
    pub(super) fn notify_files_changed(&self, paths: &[String]) {
        if paths.is_empty() {
            return;
        }
        let changes: Vec<Value> = paths.iter().map(|p| json!({ "uri": lsp::path_to_uri(p), "type": 2 })).collect();
        for slot in self.lsp.servers.values() {
            if let ServerSlot::Running(c) = slot {
                c.notify("workspace/didChangeWatchedFiles", json!({ "changes": changes }));
            }
        }
    }

    pub(super) fn find_references(&mut self, _: &FindReferences, window: &mut Window, cx: &mut Context<Self>) {
        let Some((_, client, path, pos, _, _)) = self.active_lsp(cx) else { return };
        let params = json!({ "textDocument": { "uri": lsp::path_to_uri(&path) }, "position": { "line": pos.0, "character": pos.1 }, "context": { "includeDeclaration": true } });
        self.lsp_call(
            client,
            "textDocument/references",
            params,
            cx,
            |this, v, window, cx| {
                let locs = lsp::parse_locations(&v);
                if locs.is_empty() {
                    this.notice = Some(Notice::Info(t("No references found").into()));
                    cx.notify();
                    return;
                }
                this.show_locations(locs, "references", window, cx);
            },
            window,
        );
    }

    /// Location list: each line's text (from the open editor or disk) and the relative path.
    pub(super) fn show_locations(&mut self, locs: Vec<(String, (u32, u32))>, what: &'static str, window: &mut Window, cx: &mut Context<Self>) {
        let open: Vec<(String, Entity<EditorView>)> = self.all_editors().into_iter().map(|e| (e.read(cx).path.clone(), e)).collect();
        let host = self.remote.host.clone();
        let root = self.root(cx).unwrap_or_default();
        let mut texts: std::collections::HashMap<String, Vec<String>> = Default::default();
        for (p, e) in &open {
            if locs.iter().any(|(lp, _)| lp == p) {
                texts.insert(p.clone(), e.read(cx).text_lines());
            }
        }
        let need: Vec<String> = locs.iter().map(|(p, _)| p.clone()).filter(|p| !texts.contains_key(p)).collect();
        let task = cx.background_executor().spawn(async move {
            let mut more = std::collections::HashMap::new();
            for p in need {
                if more.contains_key(&p) {
                    continue;
                }
                let lines = host.read(&p).map(|(d, _)| String::from_utf8_lossy(&d).lines().map(str::to_string).collect()).unwrap_or_default();
                more.insert(p, lines);
            }
            more
        });
        cx.spawn_in(window, async move |this, cx| {
            let more = task.await;
            this.update_in(cx, |this, window, cx| {
                texts.extend(more);
                let prefix = format!("{}/", root.trim_end_matches('/'));
                let items: Vec<PickItem> = locs
                    .iter()
                    .map(|(p, (l, _))| {
                        let line = texts.get(p).and_then(|t| t.get(*l as usize)).map(|s| s.trim().to_string()).unwrap_or_default();
                        let rel = p.strip_prefix(&prefix).unwrap_or(p);
                        PickItem {
                            label: if line.is_empty() { rel.to_string().into() } else { line.into() },
                            detail: Some(format!("{rel}:{}", l + 1).into()),
                            keys: None,
                        }
                    })
                    .collect();
                let n = items.len();
                let picker = cx.new(|cx| Picker::new(vec![PickSource::List { prefix: "", items }], &lsp::locations_title(what, n), "", None, cx));
                let targets = locs.into_iter().map(|(p, (l, c))| (p, l, c)).collect();
                this.show_overlay(picker, OverlayKind::Locations(targets), window, cx);
            })
            .ok();
        })
        .detach();
    }

    pub(super) fn go_to_symbol(&mut self, _: &GoToSymbol, window: &mut Window, cx: &mut Context<Self>) {
        let Some((_, client, path, _, _, _)) = self.active_lsp(cx) else { return };
        let params = json!({ "textDocument": { "uri": lsp::path_to_uri(&path) } });
        self.lsp_call(
            client,
            "textDocument/documentSymbol",
            params,
            cx,
            |this, v, window, cx| {
                let syms = lsp::parse_symbols(&v);
                if syms.is_empty() {
                    this.notice = Some(Notice::Info(t("No symbols in this file").into()));
                    cx.notify();
                    return;
                }
                let items = syms
                    .iter()
                    .map(|s| PickItem {
                        label: format!("{}{}", "  ".repeat(s.depth), s.name).into(),
                        detail: Some(
                            if s.detail.is_empty() { lsp::symbol_kind(s.kind).to_string() } else { format!("{} · {}", lsp::symbol_kind(s.kind), s.detail) }
                                .into(),
                        ),
                        keys: None,
                    })
                    .collect();
                let picker = cx.new(|cx| Picker::new(vec![PickSource::List { prefix: "", items }], t("Go to symbol in file"), "", None, cx));
                this.show_overlay(picker, OverlayKind::Symbols(syms.iter().map(|s| (s.line, s.col)).collect()), window, cx);
            },
            window,
        );
    }

    /// "#" in the palette: ask all running language servers for `workspace/symbol` and
    /// merge the results. Fires after a 150 ms pause, not per keystroke; a new query drops the old one.
    pub(super) fn search_workspace_symbols(&mut self, query: String, cx: &mut Context<Self>) {
        let Some(picker) = self.overlay.as_ref().map(|o| o.picker.downgrade()) else { return };
        let clients: Vec<Arc<Client>> = self
            .lsp
            .servers
            .values()
            .filter_map(|s| match s {
                ServerSlot::Running(c) if c.is_alive() => Some(c.clone()),
                _ => None,
            })
            .collect();
        let status = if clients.is_empty() {
            t("No language server is running. Open a file of the language first.")
        } else if query.is_empty() {
            super::SYMBOL_HINT
        } else {
            t("Searching…")
        };
        picker.update(cx, |p, cx| p.set_remote(3, Vec::new(), status, cx)).ok();
        if clients.is_empty() || query.is_empty() {
            self.lsp.symbol_task = None;
            return;
        }
        let root = self.root(cx).map(|r| format!("{}/", r.trim_end_matches('/'))).unwrap_or_default();
        self.lsp.symbol_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(Duration::from_millis(150)).await;
            let q = query.clone();
            let found = cx
                .background_executor()
                .spawn(async move {
                    let mut all = Vec::new();
                    for c in clients {
                        if let Ok(v) = c.request("workspace/symbol", json!({ "query": q }), Duration::from_secs(10)) {
                            all.extend(lsp::parse_workspace_symbols(&v));
                        }
                    }
                    all.truncate(MAX_WORKSPACE_SYMBOLS);
                    all
                })
                .await;
            this.update(cx, |this, cx| {
                let Some(super::Overlay { kind: OverlayKind::Palette { symbols, .. }, .. }) = &mut this.overlay else { return };
                *symbols = found.iter().map(|s| (s.path.clone(), s.line, s.col)).collect();
                let items = found
                    .iter()
                    .map(|s| {
                        let rel = s.path.strip_prefix(&root).unwrap_or(&s.path);
                        let mut detail = lsp::symbol_kind(s.kind).to_string();
                        if !s.container.is_empty() {
                            detail.push_str(&format!(" · {}", s.container));
                        }
                        detail.push_str(&format!(" · {rel}:{}", s.line + 1));
                        PickItem { label: s.name.clone().into(), detail: Some(detail.into()), keys: None }
                    })
                    .collect();
                picker.update(cx, |p, cx| p.set_remote(3, items, tf("No symbols matching “{}”", &[&query]), cx)).ok();
            })
            .ok();
        }));
    }

    pub(super) fn code_actions(&mut self, _: &CodeActions, window: &mut Window, cx: &mut Context<Self>) {
        // For a misspelled word, spelling suggestions first.
        if self.try_spell_fix(window, cx) {
            return;
        }
        // Extensions with code actions for this file: their actions next to the language server's.
        if self.quick_fix_with_extensions(window, cx) {
            return;
        }
        let Some((ed, client, path, _, range, _)) = self.active_lsp(cx) else { return };
        let diagnostics = ed.read(cx).diagnostics_at_cursor();
        let params = json!({
            "textDocument": { "uri": lsp::path_to_uri(&path) },
            "range": { "start": { "line": range.0.0, "character": range.0.1 }, "end": { "line": range.1.0, "character": range.1.1 } },
            "context": { "diagnostics": diagnostics },
        });
        let c = client.clone();
        self.lsp_call(
            client,
            "textDocument/codeAction",
            params,
            cx,
            move |this, v, window, cx| {
                let actions = lsp::parse_code_actions(&v);
                if actions.is_empty() {
                    this.notice = Some(Notice::Info(t("No quick fixes here").into()));
                    cx.notify();
                    return;
                }
                let items = actions
                    .iter()
                    .map(|a| PickItem { label: a.title.clone().into(), detail: (!a.kind.is_empty()).then(|| a.kind.clone().into()), keys: None })
                    .collect();
                let picker = cx.new(|cx| Picker::new(vec![PickSource::List { prefix: "", items }], t("Quick fix"), "", None, cx));
                this.show_overlay(picker, OverlayKind::CodeActions { actions, client: c }, window, cx);
            },
            window,
        );
    }

    /// If it has no edit, `codeAction/resolve` first; then the edit and/or command.
    pub(super) fn run_code_action(&mut self, client: Arc<Client>, action: lsp::CodeAction, cx: &mut Context<Self>) {
        let task = cx.background_executor().spawn(async move {
            let mut raw = action.raw;
            if raw["edit"].is_null() && raw["command"].is_null() && !raw["data"].is_null() {
                raw = client.request("codeAction/resolve", raw.clone(), Duration::from_secs(15)).unwrap_or(raw);
            }
            (client, raw)
        });
        cx.spawn(async move |this, cx| {
            let (client, raw) = task.await;
            this.update(cx, |this, cx| {
                if !raw["edit"].is_null() {
                    this.apply_workspace_edit(&raw["edit"], cx);
                }
                // Command: either the action itself (a `command` string) or a `command` object inside it.
                let cmd = if raw["command"].is_string() { raw.clone() } else { raw["command"].clone() };
                if let Some(name) = cmd["command"].as_str() {
                    let params = json!({ "command": name, "arguments": cmd["arguments"].clone() });
                    cx.background_executor().spawn(async move { client.request("workspace/executeCommand", params, Duration::from_secs(30)) }).detach();
                }
            })
            .ok();
        })
        .detach();
    }

    pub(super) fn format_document(&mut self, _: &FormatDocument, window: &mut Window, cx: &mut Context<Self>) {
        self.format(false, window, cx);
    }

    /// "Format Selection": only the selection (or the cursor's line if none). With no server there is
    /// nothing to do outside a JSON document.
    pub(super) fn format_selection(&mut self, _: &FormatSelection, window: &mut Window, cx: &mut Context<Self>) {
        self.format(true, window, cx);
    }

    fn format(&mut self, selection: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(ed) = self.active_editor().cloned() else { return };
        // The extension chosen to format this file (`editor.formatters`) formats the whole document.
        if !selection && self.format_with_extension(&ed, window, cx) {
            return;
        }
        let request = ed.update(cx, |e, _| if selection { e.format_selection_request() } else { e.format_request() });
        let Some((client, params)) = request else {
            if selection {
                self.notice = Some(Notice::Info(t("No language server for this file").into()));
                cx.notify();
                return;
            }
            // No server: we format JSON/JSONC ourselves.
            let is_json =
                ed.read(cx).path.rsplit_once('.').is_some_and(|(_, e)| matches!(e.to_ascii_lowercase().as_str(), "json" | "jsonc" | "geojson" | "webmanifest"));
            self.notice = Some(if is_json {
                match ed.update(cx, |e, cx| e.format_json(false, cx)) {
                    Ok(true) => Notice::Info(t("Formatted").into()),
                    Ok(false) => Notice::Info(t("Already formatted").into()),
                    Err(e) => Notice::Error(tf("Could not format: {}", &[&e]).into()),
                }
            } else {
                Notice::Info(t("No language server for this file").into())
            });
            cx.notify();
            return;
        };
        let method = if selection { "textDocument/rangeFormatting" } else { "textDocument/formatting" };
        self.lsp_call(
            client,
            method,
            params,
            cx,
            move |this, v, _, cx| {
                let edits: Vec<(lsp::Range16, String)> = v
                    .as_array()
                    .map(|a| a.iter().map(|e| (lsp::parse_range(&e["range"]), e["newText"].as_str().unwrap_or_default().to_string())).collect())
                    .unwrap_or_default();
                if edits.is_empty() {
                    this.notice = Some(Notice::Info(t("Already formatted").into()));
                    cx.notify();
                } else {
                    ed.update(cx, |e, cx| e.apply_lsp_edits(&edits, cx));
                }
            },
            window,
        );
    }

    /// "JSON: Minify Document".
    pub(super) fn minify_json(&mut self, _: &super::MinifyJson, _: &mut Window, cx: &mut Context<Self>) {
        let Some(ed) = self.active_editor().cloned() else { return };
        self.notice = Some(match ed.update(cx, |e, cx| e.format_json(true, cx)) {
            Ok(true) => Notice::Info(t("Minified").into()),
            Ok(false) => Notice::Info(t("Already minified").into()),
            Err(e) => Notice::Error(tf("Could not minify: {}", &[&e]).into()),
        });
        cx.notify();
    }

    pub(super) fn save_all(&mut self, _: &SaveAll, _: &mut Window, cx: &mut Context<Self>) {
        for ed in self.all_editors() {
            if ed.read(cx).is_dirty() {
                ed.update(cx, |e, cx| e.save_now(cx));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::inside_roots;

    #[test]
    fn only_paths_under_a_root_are_inside() {
        let roots = vec!["/p".to_string(), "/q/".to_string()];
        assert!(inside_roots("/p/src/a.rs", &roots, false));
        assert!(inside_roots("/q/x", &roots, false));
        assert!(!inside_roots("/etc/hosts", &roots, false));
        assert!(!inside_roots("/p/../etc/hosts", &roots, false), "dot-dot must not climb out");
        assert!(!inside_roots("/pp/a.rs", &roots, false), "a sibling sharing the prefix is not inside");
        assert!(!inside_roots("/etc/hosts", &[], false), "no folder open: nothing is inside");
    }

    #[cfg(unix)]
    #[test]
    fn a_symlink_out_of_the_root_does_not_count() {
        let base = std::env::temp_dir().join(format!("codeyard-inside-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let (root, elsewhere) = (base.join("root"), base.join("elsewhere"));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(&elsewhere).unwrap();
        std::os::unix::fs::symlink(&elsewhere, root.join("link")).unwrap();
        let roots = vec![root.to_string_lossy().into_owned()];
        let through_link = root.join("link/new.txt").to_string_lossy().into_owned();
        assert!(!inside_roots(&through_link, &roots, true));
        assert!(inside_roots(&through_link, &roots, false), "remote paths cannot be resolved here");
        assert!(inside_roots(&root.join("fresh/new.txt").to_string_lossy(), &roots, true), "a not yet existing file is judged by its parent");
        let _ = std::fs::remove_dir_all(&base);
    }
}
