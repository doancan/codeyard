//! Workbench: what code extensions contribute besides palette commands (docs/plugin-architecture.md §5.1): menus, key
//! bindings and status items that run their commands; formatters, code actions, diagnostics, decorations and tasks
//! (hover and completion in workbench_ext_hover.rs, code lenses and inlay hints in workbench_ext_lenses.rs).
//!
//! Providers are asked with a deadline (`limits::PROVIDER_DEADLINE_MILLIS`, plus `ACTIVATE_MILLIS` while the
//! extension starts). Nothing the user does waits for them: a request about a document is cancelled when the document
//! changes, and an answer that arrives after the deadline is dropped and noted in the extension's log. Everything an
//! extension put on screen goes when it is disabled, uninstalled or crashes.

use std::collections::HashMap;
use std::rc::Rc;
use std::time::{Duration, Instant};

use ext_api::value::{self as v, LogLevel, Request, Response};
use ext_manifest::{MenuLocation, limits};
use ext_proto::ToHost;
use futures::StreamExt;
use futures::channel::oneshot;

use super::*;
use crate::editor::{FormatJob, LineDecoration, SaveFormatter};
use crate::ext::Active;
use crate::ext_broker;

/// Run a command of a code extension: what extension menus and key bindings dispatch. `args` is the command's JSON
/// argument (`null`, or the file a file-tree menu was opened on).
#[derive(Clone, PartialEq, Debug, gpui::Action)]
#[action(namespace = workbench, no_json)]
pub struct RunExtensionCommand {
    pub extension: String,
    /// The command's local id.
    pub command: String,
    pub args: String,
}

/// Hover and completion (workbench_ext_hover.rs).
#[path = "workbench_ext_hover.rs"]
mod hover;

/// Code lenses and inlay hints (workbench_ext_lenses.rs).
#[path = "workbench_ext_lenses.rs"]
mod lenses;

/// How long a late answer is still listened for (to note it in the log) before the request is forgotten.
const LATE_GRACE: Duration = Duration::from_secs(30);

/// What code extensions show, per window.
#[derive(Default)]
pub(super) struct ProviderState {
    /// Diagnostics per file, per extension.
    diagnostics: HashMap<String, Vec<(String, Vec<crate::lsp::Diagnostic>)>>,
    /// Decorations per file: (extension, set, decorations).
    decorations: HashMap<String, Vec<(String, String, Vec<v::Decoration>)>>,
    /// Status items, in the order they were first set: (extension, item).
    status: Vec<(String, v::StatusItem)>,
    /// Requests about a file, cancelled when it changes: (path, extension, request id).
    waiting: Vec<(String, String, u64)>,
    /// Hover and completion: how editors ask (workbench_ext_hover.rs).
    lang: hover::LangState,
    /// Code lenses and inlay hints asked for each editor (workbench_ext_lenses.rs).
    lenses: lenses::LensState,
}

/// One row of the quick-fix list: a language server's action or an extension's.
#[derive(Clone)]
pub(super) enum QuickFix {
    Lsp(Arc<crate::lsp::Client>, crate::lsp::CodeAction),
    /// `document`: the path the actions were asked for, the one file its edits may change without `write`.
    Extension {
        extension: String,
        document: String,
        action: v::CodeAction,
    },
}

/// The language id Codeyard gives a file (`rust`, `toml`, a language an extension adds), `plaintext` otherwise.
pub(super) fn language_id(path: &str) -> String {
    crate::lsp::language_of(path).map_or("plaintext", |(id, _)| id).to_string()
}

fn same_file(a: &str, b: &str) -> bool {
    ext_broker::normalize(a).is_some_and(|a| ext_broker::normalize(b).is_some_and(|b| a == b))
}

fn to_lsp_diagnostic(extension: &str, d: &v::Diagnostic) -> crate::lsp::Diagnostic {
    crate::lsp::Diagnostic {
        start: (d.range.start.line, d.range.start.character),
        end: (d.range.end.line, d.range.end.character),
        severity: match d.severity {
            v::DiagnosticSeverity::Error => 1,
            v::DiagnosticSeverity::Warning => 2,
            v::DiagnosticSeverity::Information => 3,
            v::DiagnosticSeverity::Hint => 4,
        },
        message: d.message.clone(),
        source: Some(extension.to_string()),
        code: d.code.clone(),
        related: Vec::new(),
        more_related: 0,
        rendered: None,
    }
}

fn from_lsp_diagnostic(d: &crate::lsp::Diagnostic) -> v::Diagnostic {
    let pos = |(line, character): (u32, u32)| v::Position { line, character };
    v::Diagnostic {
        range: v::Range { start: pos(d.start), end: pos(d.end) },
        severity: match d.severity {
            1 => v::DiagnosticSeverity::Error,
            2 => v::DiagnosticSeverity::Warning,
            3 => v::DiagnosticSeverity::Information,
            _ => v::DiagnosticSeverity::Hint,
        },
        message: d.message.clone(),
        code: d.code.clone(),
    }
}

fn to_edits(edits: &[v::TextEdit]) -> Vec<(crate::lsp::Range16, String)> {
    edits.iter().map(|e| (((e.range.start.line, e.range.start.character), (e.range.end.line, e.range.end.character)), e.new_text.clone())).collect()
}

/// Text an extension may show: trimmed to `max` characters, without control or invisible characters.
fn shown_text(s: &str, max: usize) -> Option<String> {
    let s = s.trim();
    (!s.is_empty() && s.chars().count() <= max && !s.chars().any(ext_manifest::is_deceptive)).then(|| s.to_string())
}

impl Workbench {
    // --- asking providers ---

    /// Active code extensions with a provider for `language` (`pick` chooses the kind), in id order.
    fn providers_for(&self, language: &str, pick: impl Fn(&ext_manifest::Contributions) -> &[ext_manifest::Provider]) -> Vec<Active> {
        crate::ext::current()
            .active
            .iter()
            .filter(|e| e.manifest.code.is_some() && !self.cfg.exts.code.disabled.contains(e.id()))
            .filter(|e| pick(&e.manifest.contributions).iter().any(|p| p.handles(language)))
            .cloned()
            .collect()
    }

    /// Send `request` to an extension's provider (starting it if needed). The answer comes within the deadline, or as
    /// `timeout`; a request `about` a file is cancelled when that file changes.
    fn ask_provider(
        &mut self,
        active: &Active,
        reason: &str,
        request: Request,
        about: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> oneshot::Receiver<Result<Response, v::Error>> {
        self.ask_provider_with_id(active, reason, request, about, window, cx).1
    }

    /// `ask_provider`, with the request's id (to cancel it: `cancel_provider_request`).
    fn ask_provider_with_id(
        &mut self,
        active: &Active,
        reason: &str,
        request: Request,
        about: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> (u64, oneshot::Receiver<Result<Response, v::Error>>) {
        let ext = active.id().to_string();
        let running = self.cfg.exts.code.procs.contains_key(&ext);
        let deadline = Duration::from_millis(limits::PROVIDER_DEADLINE_MILLIS + if running { 0 } else { limits::ACTIVATE_MILLIS });
        let (id, answer) = self.cfg.exts.code.pending.lock().unwrap_or_else(|p| p.into_inner()).add(&ext);
        let msg = ToHost::Handle { id, deadline_ms: deadline.as_millis() as u32, request };
        self.send_to_extension(active, reason, Some(msg), window, cx);
        if let Some(path) = about {
            self.cfg.exts.providers.waiting.push((path, ext.clone(), id));
        }
        let (tx, out) = oneshot::channel();
        let mut answer = crate::ui::bridge(answer);
        let sent = Instant::now();
        cx.spawn(async move |this, cx| {
            let timer = cx.background_executor().timer(deadline);
            match futures::future::select(answer.next(), timer).await {
                futures::future::Either::Left((a, _)) => {
                    // The platform's share: a request to a running extension (starting one is measured on its own).
                    if running {
                        crate::perf::sample("extensions.provider_overhead_ms", sent.elapsed().as_secs_f64() * 1000.0);
                    }
                    let _ = tx.send(a.unwrap_or(Err(v::Error::Cancelled)));
                    this.update(cx, |this, _| this.cfg.exts.providers.waiting.retain(|w| !(w.1 == ext && w.2 == id))).ok();
                }
                futures::future::Either::Right((_, late)) => {
                    let _ = tx.send(Err(v::Error::Timeout));
                    // Tell the extension to stop, and keep listening a while so a late answer is noted, not lost.
                    this.update(cx, |this, _| {
                        this.cfg.exts.providers.waiting.retain(|w| !(w.1 == ext && w.2 == id));
                        if let Some(p) = this.cfg.exts.code.procs.get(&ext) {
                            let _ = p.send(&ToHost::Cancel { id });
                        }
                    })
                    .ok();
                    let grace = cx.background_executor().timer(LATE_GRACE);
                    let late = futures::future::select(late, grace).await;
                    this.update(cx, |this, _| match late {
                        // An answer from the extension; `cancelled` and `other` come from the app (the exthost ended).
                        futures::future::Either::Left((Some(r), _)) if !matches!(r, Err(v::Error::Cancelled | v::Error::Other(_))) => this.log_extension(
                            &ext,
                            LogLevel::Warn,
                            format!("request {id} answered after its {} ms deadline; the answer was dropped", deadline.as_millis()),
                        ),
                        futures::future::Either::Left(_) => {}
                        futures::future::Either::Right(_) => {
                            this.cfg.exts.code.pending.lock().unwrap_or_else(|p| p.into_inner()).answer(&ext, id, Err(v::Error::Cancelled));
                        }
                    })
                    .ok();
                }
            }
        })
        .detach();
        (id, out)
    }

    /// The user moved on (`path` changed): requests about it are cancelled, and their answers will be dropped.
    pub(super) fn cancel_providers_about(&mut self, path: &str) {
        let (gone, keep) = std::mem::take(&mut self.cfg.exts.providers.waiting).into_iter().partition(|w| w.0 == path);
        self.cfg.exts.providers.waiting = keep;
        let gone: Vec<(String, String, u64)> = gone;
        for (_, ext, id) in gone {
            self.cancel_provider_request(&ext, id);
        }
    }

    /// Stop waiting for request `id` of `ext`: the extension is told, and its answer will be dropped.
    fn cancel_provider_request(&mut self, ext: &str, id: u64) {
        if let Some(p) = self.cfg.exts.code.procs.get(ext) {
            let _ = p.send(&ToHost::Cancel { id });
        }
        self.cfg.exts.code.pending.lock().unwrap_or_else(|p| p.into_inner()).answer(ext, id, Err(v::Error::Cancelled));
    }

    // --- formatters ---

    /// The extension that formats `language`: `editor.formatters` decides; without an entry the language server formats
    /// (`has_lsp`), or else the first extension that formats the language.
    fn formatter_for(&self, language: &str, has_lsp: bool, cx: &App) -> Option<Active> {
        let all = self.providers_for(language, |c| &c.formatters);
        let chosen = cx.global::<Settings>().formatters.iter().rev().find(|(l, _)| l == language).map(|(_, who)| who.clone());
        match chosen.as_deref() {
            Some("lsp") => None,
            Some(id) => all.into_iter().find(|e| e.id() == id),
            None if has_lsp => None,
            None => all.into_iter().next(),
        }
    }

    /// Ask the chosen extension to format `job`: `None` when no extension formats this file.
    fn ask_formatter(
        &mut self,
        job: FormatJob,
        has_lsp: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<(String, oneshot::Receiver<Result<Response, v::Error>>)> {
        let language = language_id(&job.path);
        let active = self.formatter_for(&language, has_lsp, cx)?;
        let document = v::Document { uri: ext_broker::uri(&job.path), language: language.clone(), version: job.version as u32, text: job.text };
        let options = v::FormatOptions { tab_size: job.tab_size, insert_spaces: job.insert_spaces };
        let request = Request::Format { document, range: None, options };
        let answer = self.ask_provider(&active, &format!("language:{language}"), request, Some(job.path), window, cx);
        Some((active.id().to_string(), answer))
    }

    /// Format Document with an extension; `false` when none formats this file (the language server path runs).
    pub(super) fn format_with_extension(&mut self, ed: &Entity<EditorView>, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let (job, has_lsp) = {
            let e = ed.read(cx);
            (e.format_job(), e.lsp_client().is_some())
        };
        let version = job.version;
        let Some((ext, answer)) = self.ask_formatter(job, has_lsp, window, cx) else { return false };
        let ed = ed.downgrade();
        cx.spawn(async move |this, cx| {
            let result = answer.await.unwrap_or(Err(v::Error::Cancelled));
            this.update(cx, |this, cx| {
                this.notice = match result {
                    Ok(Response::Edits(edits)) => {
                        let applied = ed.update(cx, |e, cx| e.apply_lsp_edits_if_unchanged(version, &to_edits(&edits), cx)).unwrap_or(false);
                        applied.then(|| Notice::Info(crate::i18n::tf("Formatted by {}", &[&ext]).into()))
                    }
                    // The user moved on: nothing to say.
                    Err(v::Error::Cancelled) => None,
                    Ok(_) => Some(Notice::Error(crate::i18n::tf("{}: the formatter returned no edits", &[&ext]).into())),
                    Err(e) => Some(Notice::Error(format!("{ext}: {}", super::code_glue::explain(&e)).into())),
                };
                cx.notify();
            })
            .ok();
        })
        .detach();
        true
    }

    /// The hook an editor calls when it saves with `editor.formatOnSave`: it asks the chosen extension, if any.
    fn save_formatter(&self, window: &Window, cx: &mut Context<Self>) -> SaveFormatter {
        let wb = cx.entity().downgrade();
        let handle = window.window_handle();
        Rc::new(move |job: FormatJob, cx: &mut gpui::AsyncApp| {
            cx.update_window(handle, |_, window, cx| wb.update(cx, |wb, cx| wb.format_for_save(job, window, cx)).ok().flatten()).ok().flatten()
        })
    }

    fn format_for_save(
        &mut self,
        job: FormatJob,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<oneshot::Receiver<Vec<(crate::lsp::Range16, String)>>> {
        let has_lsp = self.all_editors().iter().find(|e| e.read(cx).path == job.path).is_some_and(|e| e.read(cx).lsp_client().is_some());
        let (ext, answer) = self.ask_formatter(job, has_lsp, window, cx)?;
        let (tx, out) = oneshot::channel();
        cx.spawn(async move |this, cx| {
            match answer.await.unwrap_or(Err(v::Error::Cancelled)) {
                Ok(Response::Edits(edits)) => {
                    let _ = tx.send(to_edits(&edits));
                }
                // Saved as it is: the file is written in every case.
                Ok(_) | Err(v::Error::Cancelled) => {}
                Err(e) => {
                    this.update(cx, |this, cx| {
                        this.notice =
                            Some(Notice::Error(crate::i18n::tf("Saved without formatting: {0}: {1}", &[&ext, &super::code_glue::explain(&e)]).into()));
                        cx.notify();
                    })
                    .ok();
                }
            }
        })
        .detach();
        Some(out)
    }

    // --- code actions ---

    /// Quick Fix with extension actions next to the language server's; `false` when no extension offers actions for
    /// this file (the language server path runs).
    pub(super) fn quick_fix_with_extensions(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let Some(ed) = self.active_editor().cloned() else { return false };
        let path = ed.read(cx).path.clone();
        let language = language_id(&path);
        let providers = self.providers_for(&language, |c| &c.code_actions);
        if providers.is_empty() {
            return false;
        }
        let (job, range, diagnostic, lsp_diagnostics) = {
            let e = ed.read(cx);
            (e.format_job(), e.selection_range16(), e.diagnostic_at_cursor(), e.diagnostics_at_cursor())
        };
        // The language server's actions, asked in the background like Quick Fix does.
        let lsp = ed.update(cx, |e, _| e.lsp_context()).map(|(client, _, r, _)| {
            let params = serde_json::json!({
                "textDocument": { "uri": crate::lsp::path_to_uri(&path) },
                "range": { "start": { "line": r.0.0, "character": r.0.1 }, "end": { "line": r.1.0, "character": r.1.1 } },
                "context": { "diagnostics": lsp_diagnostics },
            });
            let c = client.clone();
            (client, cx.background_executor().spawn(async move { c.request("textDocument/codeAction", params, Duration::from_secs(15)) }))
        });
        let document = v::Document { uri: ext_broker::uri(&path), language: language.clone(), version: job.version as u32, text: job.text };
        let pos = |(line, character): (u32, u32)| v::Position { line, character };
        let request = Request::CodeActions {
            document,
            range: v::Range { start: pos(range.0), end: pos(range.1) },
            diagnostics: diagnostic.iter().map(from_lsp_diagnostic).collect(),
        };
        let asked: Vec<(String, oneshot::Receiver<Result<Response, v::Error>>)> = providers
            .iter()
            .map(|a| (a.id().to_string(), self.ask_provider(a, &format!("language:{language}"), request.clone(), Some(path.clone()), window, cx)))
            .collect();
        cx.spawn_in(window, async move |this, cx| {
            let mut fixes = Vec::new();
            if let Some((client, task)) = lsp
                && let Ok(v) = task.await
            {
                fixes.extend(crate::lsp::parse_code_actions(&v).into_iter().map(|a| QuickFix::Lsp(client.clone(), a)));
            }
            for (extension, answer) in asked {
                if let Ok(Ok(Response::CodeActions(list))) = answer.await {
                    fixes.extend(list.into_iter().map(|action| QuickFix::Extension { extension: extension.clone(), document: path.clone(), action }));
                }
            }
            // Preferred first, the rest in the order they came.
            fixes.sort_by_key(|f| match f {
                QuickFix::Lsp(_, a) => !a.preferred,
                QuickFix::Extension { action, .. } => !action.preferred,
            });
            this.update_in(cx, |this, window, cx| this.show_quick_fixes(fixes, window, cx)).ok();
        })
        .detach();
        true
    }

    fn show_quick_fixes(&mut self, fixes: Vec<QuickFix>, window: &mut Window, cx: &mut Context<Self>) {
        if fixes.is_empty() {
            self.notice = Some(Notice::Info(crate::i18n::t("No quick fixes here").into()));
            cx.notify();
            return;
        }
        let items = fixes
            .iter()
            .map(|f| match f {
                QuickFix::Lsp(_, a) => PickItem { label: a.title.clone().into(), detail: (!a.kind.is_empty()).then(|| a.kind.clone().into()), keys: None },
                // Every extension surface names the extension.
                QuickFix::Extension { extension, action, .. } => {
                    PickItem { label: action.title.clone().into(), detail: Some(extension.clone().into()), keys: None }
                }
            })
            .collect();
        let picker = cx.new(|cx| Picker::new(vec![PickSource::List { prefix: "", items }], crate::i18n::t("Quick fix"), "", None, cx));
        self.show_overlay(picker, OverlayKind::QuickFixes(fixes), window, cx);
    }

    /// Run the chosen fix. An extension's: its edits, then its command; with neither, `run-code-action` asks the
    /// extension to do it.
    pub(super) fn run_quick_fix(&mut self, fix: QuickFix, window: &mut Window, cx: &mut Context<Self>) {
        let (extension, document, action) = match fix {
            QuickFix::Lsp(client, action) => return self.run_code_action(client, action, cx),
            QuickFix::Extension { extension, document, action } => (extension, document, action),
        };
        if !action.edits.is_empty() {
            // The extension chose the files; each must be the document or in its `write`, checked as it is now. One
            // refused file refuses the whole fix: half of an edit is worse than none.
            let reg = crate::ext::current();
            let scope = reg.active.iter().find(|a| a.id() == extension).map(|a| self.extension_scope(a, cx));
            let mut changes = serde_json::Map::new();
            for (uri, edits) in &action.edits {
                let allowed = match &scope {
                    Some(scope) => scope.may_edit(&document, uri),
                    None => Err(v::Error::Invalid(format!("{extension} is no longer active"))),
                };
                let path = match allowed {
                    Ok(path) => path,
                    Err(e) => {
                        let why = super::code_glue::explain(&e);
                        self.log_extension(&extension, LogLevel::Warn, format!("code action \"{}\": edit to {uri} refused: {why}", action.title));
                        self.notice = Some(Notice::Error(format!("{extension}: {why}").into()));
                        cx.notify();
                        return;
                    }
                };
                let list: Vec<serde_json::Value> = edits
                    .iter()
                    .map(|e| {
                        serde_json::json!({
                            "range": { "start": { "line": e.range.start.line, "character": e.range.start.character },
                                       "end": { "line": e.range.end.line, "character": e.range.end.character } },
                            "newText": e.new_text,
                        })
                    })
                    .collect();
                changes.insert(crate::lsp::path_to_uri(&path), serde_json::Value::Array(list));
            }
            self.apply_workspace_edit(&serde_json::json!({ "changes": changes }), cx);
        }
        match action.command {
            Some(command) => self.run_extension_command_id(&extension, &command, "null".into(), window, cx),
            None if action.edits.is_empty() => {
                self.ask_extension_as_user(&extension, Request::RunCodeAction { id: action.id }, "code-action", window, cx);
            }
            None => {}
        }
    }

    // --- commands from menus, key bindings, status items and code actions ---

    pub(super) fn on_run_extension_command(&mut self, a: &RunExtensionCommand, window: &mut Window, cx: &mut Context<Self>) {
        if a.args == "null" {
            self.run_extension_command(&a.extension, &a.command, window, cx);
        } else {
            let request = crate::ext_runtime::command(&a.command, &a.args);
            self.ask_extension_as_user(&a.extension, request, &format!("command:{}", a.command), window, cx);
        }
    }

    /// Run `command` named by `extension` (its own local or full id, or a dependency's full id).
    fn run_extension_command_id(&mut self, extension: &str, command: &str, args: String, window: &mut Window, cx: &mut Context<Self>) {
        let Some((owner, local)) = resolve_command(extension, command) else {
            self.log_extension(extension, LogLevel::Warn, format!("{command} is not one of its commands or a dependency's"));
            return;
        };
        self.on_run_extension_command(&RunExtensionCommand { extension: owner, command: local, args }, window, cx);
    }

    /// A request the user started (a command with arguments, a code action, a task): it may use the active document and
    /// open links while it runs, and an error is shown.
    fn ask_extension_as_user(&mut self, extension: &str, request: Request, reason: &str, window: &mut Window, cx: &mut Context<Self>) {
        let reg = crate::ext::current();
        let Some(active) = reg.active.iter().find(|e| e.id() == extension).cloned() else { return };
        if self.cfg.exts.code.disabled.contains(extension) {
            self.notice = Some(Notice::Error(crate::i18n::tf("{} crashed three times and is turned off until Codeyard restarts.", &[&extension]).into()));
            cx.notify();
            return;
        }
        let (id, answer) = self.cfg.exts.code.pending.lock().unwrap_or_else(|p| p.into_inner()).add(extension);
        self.cfg.exts.code.user_action.insert(extension.to_string(), id);
        self.send_to_extension(&active, reason, Some(ToHost::Handle { id, deadline_ms: 0, request }), window, cx);
        let ext = extension.to_string();
        let mut answer = crate::ui::bridge(answer);
        cx.spawn(async move |this, cx| {
            let result = answer.next().await;
            this.update(cx, |this, cx| {
                if this.cfg.exts.code.user_action.get(&ext) == Some(&id) {
                    this.cfg.exts.code.user_action.remove(&ext);
                }
                if let Some(Ok(Response::Json(j))) = &result {
                    this.cfg.exts.code.last_answer.insert(ext.clone(), j.clone());
                }
                if let Some(Err(e)) = result.filter(|_| !this.cfg.exts.code.disabled.contains(&ext)) {
                    this.notice = Some(Notice::Error(format!("{ext}: {}", super::code_glue::explain(&e)).into()));
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    // --- menus ---

    /// Extension commands for a menu: (title, action, enabled). `when` hides an entry, `enablement` greys it out. An
    /// entry runs the extension's command with `target` (a file-tree menu's file) or the view (a view-title menu) as its
    /// argument, or a core command (`core:…`) as the palette runs it.
    pub(super) fn extension_menu(
        &self,
        location: MenuLocation,
        view: Option<&str>,
        target: Option<&str>,
        cx: &App,
    ) -> Vec<(String, Box<dyn gpui::Action>, bool)> {
        let reg = crate::ext::current();
        let language = self.active_editor().map(|e| language_id(&e.read(cx).path));
        let trusted = self.all_roots(cx).iter().all(|r| self.is_trusted_root(r, cx));
        let lookup = |key: &str| -> Option<ext_manifest::when::Value> {
            use ext_manifest::when::Value;
            Some(match key {
                "editor" => Value::Bool(location == MenuLocation::EditorContext),
                "file-tree" => Value::Bool(location == MenuLocation::FileTree),
                "terminal" | "scm" | "search" => Value::Bool(false),
                "panel" => Value::Bool(self.chrome.panel_open),
                "workspace.trusted" => Value::Bool(trusted),
                "editor.language" => Value::Str(language.clone().unwrap_or_default()),
                _ => return None,
            })
        };
        let holds = |expr: &Option<String>| expr.as_deref().is_none_or(|w| ext_manifest::when::parse(w).is_ok_and(|e| e.eval(&lookup)));
        let args = match (target, view) {
            (Some(path), _) => serde_json::Value::String(ext_broker::uri(path)).to_string(),
            (None, Some(view)) if location == MenuLocation::ViewTitle => serde_json::Value::String(view.to_string()).to_string(),
            _ => "null".to_string(),
        };
        let mut out: Vec<(String, Box<dyn gpui::Action>, bool)> = Vec::new();
        for e in reg.active.iter().filter(|e| e.manifest.code.is_some()) {
            for m in e
                .manifest
                .contributions
                .menus
                .iter()
                .filter(|m| m.location == location && m.view.as_deref() == view.filter(|_| location == MenuLocation::ViewTitle))
            {
                if !holds(&m.when) {
                    continue;
                }
                // A core command the manifest check allowed: listed for extensions, with no argument needed.
                if let Some(core) = m.command.strip_prefix("core:").and_then(ext_manifest::core_command) {
                    if let Some(action) = crate::commands::action_by_id(core.id).filter(|_| !core.title.is_empty()) {
                        out.push((crate::i18n::t(core.title).to_string(), action, true));
                    }
                    continue;
                }
                let Some((owner, local)) = resolve_command(e.id(), &m.command) else { continue };
                let Some(cmd) = reg.active.iter().find(|x| x.id() == owner).and_then(|x| x.manifest.contributions.commands.iter().find(|c| c.id == m.command))
                else {
                    continue;
                };
                let enabled = holds(&cmd.enablement) && !self.cfg.exts.code.disabled.contains(&owner);
                out.push((cmd.title.clone(), Box::new(RunExtensionCommand { extension: owner, command: local, args: args.clone() }), enabled));
            }
        }
        out
    }

    /// The view-title menu button of the extension view `view`, when extensions put entries there.
    pub(super) fn ext_view_title_menu(&self, view: &str, p: &Palette, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !crate::ext::current().active.iter().any(|e| e.manifest.contributions.menus.iter().any(|m| m.view.as_deref() == Some(view))) {
            return None;
        }
        if self.extension_menu(MenuLocation::ViewTitle, Some(view), None, cx).is_empty() {
            return None;
        }
        let id = view.to_string();
        let button = ui::icon_button(SharedString::from(format!("ext-view-menu-{view}")), "ellipsis", crate::i18n::t("More Actions…"), p).on_mouse_down(
            gpui::MouseButton::Left,
            cx.listener(move |this, ev: &gpui::MouseDownEvent, window, cx| {
                cx.stop_propagation();
                this.show_ext_view_title_menu(&id, ev.position, window, cx);
            }),
        );
        Some(div().ml_auto().flex_none().child(button).into_any_element())
    }

    /// The view-title menu of the extension view `view`, at `position`.
    pub(super) fn show_ext_view_title_menu(&mut self, view: &str, position: gpui::Point<gpui::Pixels>, window: &mut Window, cx: &mut Context<Self>) {
        let (mut items, mut commands, mut actions) = (Vec::new(), Vec::new(), Vec::new());
        let key = |a: &dyn gpui::Action| Command { label: String::new(), run: Run::Action(a.boxed_clone()) }.shortcut(cx);
        for (title, action, enabled) in self.extension_menu(MenuLocation::ViewTitle, Some(view), None, cx) {
            items.push(MenuItem::new(&title).shortcut(key(action.as_ref())).enabled(enabled));
            commands.push(MenuCmd::Dispatch(actions.len()));
            actions.push(action);
        }
        if items.is_empty() {
            return;
        }
        let target = MenuTarget { path: String::new(), is_dir: false, tab: None };
        self.show_menu(items, commands, actions, target, position, window, cx);
    }

    /// Like `show_action_menu`, with the extension entries for `location` after the built-in ones.
    pub(super) fn show_action_menu_with_extensions(
        &mut self,
        entries: Vec<(&str, Box<dyn gpui::Action>, bool)>,
        location: MenuLocation,
        position: gpui::Point<gpui::Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let key = |a: &dyn gpui::Action| Command { label: String::new(), run: Run::Action(a.boxed_clone()) }.shortcut(cx);
        let (mut items, mut commands, mut actions) = (Vec::new(), Vec::new(), Vec::new());
        for (i, (label, action, sep)) in entries.into_iter().enumerate() {
            let item = MenuItem::new(label).shortcut(key(action.as_ref()));
            items.push(if sep { item.separator() } else { item });
            commands.push(MenuCmd::Dispatch(i));
            actions.push(action);
        }
        self.add_extension_items(location, None, &mut items, &mut commands, &mut actions, cx);
        let target = MenuTarget { path: String::new(), is_dir: false, tab: None };
        self.show_menu(items, commands, actions, target, position, window, cx);
    }

    /// Append the extension entries for `location` to a menu being built.
    pub(super) fn add_extension_items(
        &self,
        location: MenuLocation,
        target: Option<&str>,
        items: &mut Vec<MenuItem>,
        commands: &mut Vec<MenuCmd>,
        actions: &mut Vec<Box<dyn gpui::Action>>,
        cx: &App,
    ) {
        let key = |a: &dyn gpui::Action| Command { label: String::new(), run: Run::Action(a.boxed_clone()) }.shortcut(cx);
        for (i, (title, action, enabled)) in self.extension_menu(location, None, target, cx).into_iter().enumerate() {
            let item = MenuItem::new(&title).shortcut(key(action.as_ref())).enabled(enabled);
            items.push(if i == 0 { item.separator() } else { item });
            commands.push(MenuCmd::Dispatch(actions.len()));
            actions.push(action);
        }
    }

    // --- status items ---

    fn on_set_status_item(&mut self, id: &str, mut item: v::StatusItem, cx: &mut Context<Self>) {
        let reg = crate::ext::current();
        let Some(active) = reg.active.iter().find(|e| e.id() == id) else { return };
        let full = format!("{id}.{}", item.id);
        if !active.manifest.contributions.status_items.iter().any(|s| s.id == full) {
            self.log_extension(id, LogLevel::Warn, format!("status item \"{}\" is not declared in [[status-bar]]; it is not shown", item.id));
            return;
        }
        let Some(text) = shown_text(&item.text, limits::STATUS_TEXT_LEN) else {
            self.log_extension(
                id,
                LogLevel::Warn,
                format!("status item \"{}\": its text must be 1 to {} visible characters", item.id, limits::STATUS_TEXT_LEN),
            );
            return;
        };
        item.text = text;
        item.tooltip = item.tooltip.as_deref().and_then(|t| shown_text(t, limits::ITEM_LEN));
        let list = &mut self.cfg.exts.providers.status;
        match list.iter_mut().find(|(e, i)| e == id && i.id == item.id) {
            Some(slot) => slot.1 = item,
            None => list.push((id.to_string(), item)),
        }
        cx.notify();
    }

    /// Status items of code extensions, for the status bar.
    pub(super) fn extension_status_items(&self, p: &Palette, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let reg = crate::ext::current();
        self.cfg
            .exts
            .providers
            .status
            .iter()
            .enumerate()
            .map(|(n, (ext, item))| {
                let declared = reg.active.iter().find(|e| e.id() == ext).and_then(|e| {
                    let full = format!("{ext}.{}", item.id);
                    e.manifest.contributions.status_items.iter().find(|s| s.id == full).and_then(|s| s.command.clone())
                });
                let command = item.command.clone().or(declared);
                // Every extension surface names the extension.
                let tip: SharedString = match &item.tooltip {
                    Some(t) => format!("{t}\n{ext}").into(),
                    None => ext.clone().into(),
                };
                let color = crate::editor::tone_color(p, item.tone);
                let (ext, label) = (ext.clone(), item.text.clone());
                div()
                    .id(("extension-status", n))
                    .debug_selector(move || format!("extension-status-{n}"))
                    .flex()
                    .flex_none()
                    .items_center()
                    .px(px(8.))
                    .h_full()
                    .text_color(c(color))
                    .max_w(px(200.))
                    .child(div().min_w_0().truncate().child(label))
                    .tooltip(move |_, cx| cx.new(|_| Tooltip(tip.clone())).into())
                    .when_some(command, |d, command| {
                        d.cursor_pointer()
                            .hover(|d| d.bg(c(p.surface_hover)))
                            .on_click(cx.listener(move |this, _, window, cx| this.run_extension_command_id(&ext, &command, "null".into(), window, cx)))
                    })
                    .into_any_element()
            })
            .collect()
    }

    // --- diagnostics and decorations ---

    /// Diagnostics extensions set on `path`, tagged with the extension.
    pub(super) fn extension_diagnostics_for(&self, path: &str) -> Vec<crate::lsp::Diagnostic> {
        self.cfg.exts.providers.diagnostics.get(path).map(|l| l.iter().flat_map(|(_, d)| d.iter().cloned()).collect()).unwrap_or_default()
    }

    /// Every file's extension diagnostics.
    pub(super) fn extension_problems(&self) -> impl Iterator<Item = (&String, Vec<crate::lsp::Diagnostic>)> {
        self.cfg.exts.providers.diagnostics.iter().map(|(p, l)| (p, l.iter().flat_map(|(_, d)| d.iter().cloned()).collect()))
    }

    /// Push the merged diagnostics of `paths` to their open editors.
    fn refresh_diagnostics(&mut self, paths: &[String], cx: &mut Context<Self>) {
        for ed in self.all_editors() {
            let path = ed.read(cx).path.clone();
            if paths.contains(&path) {
                let d = self.diagnostics_for(&path);
                ed.update(cx, |e, cx| e.set_diagnostics(d, cx));
            }
        }
        cx.notify();
    }

    fn on_set_diagnostics(&mut self, id: &str, uri: &str, list: Vec<v::Diagnostic>, cx: &mut Context<Self>) {
        let Some(path) = ext_broker::normalize(uri) else {
            self.log_extension(id, LogLevel::Warn, format!("set-diagnostics: '{uri}' is not a file URI"));
            return;
        };
        let path = self.open_path_matching(&path, cx).unwrap_or(path);
        let mut list: Vec<crate::lsp::Diagnostic> = list.iter().map(|d| to_lsp_diagnostic(id, d)).collect();
        list.truncate(limits::MARKS_PER_FILE);
        let entry = self.cfg.exts.providers.diagnostics.entry(path.clone()).or_default();
        entry.retain(|(e, _)| e != id);
        if !list.is_empty() {
            entry.push((id.to_string(), list));
        }
        if entry.is_empty() {
            self.cfg.exts.providers.diagnostics.remove(&path);
        }
        self.refresh_diagnostics(&[path], cx);
    }

    fn on_clear_diagnostics(&mut self, id: &str, uri: Option<&str>, cx: &mut Context<Self>) {
        let only = uri.and_then(ext_broker::normalize);
        let mut touched = Vec::new();
        self.cfg.exts.providers.diagnostics.retain(|path, list| {
            if only.as_deref().is_none_or(|o| same_file(o, path)) && list.iter().any(|(e, _)| e == id) {
                list.retain(|(e, _)| e != id);
                touched.push(path.clone());
            }
            !list.is_empty()
        });
        self.refresh_diagnostics(&touched, cx);
    }

    /// An open editor's own spelling of `path` (paths from URIs lose nothing, but compare as normalized).
    fn open_path_matching(&self, path: &str, cx: &App) -> Option<String> {
        self.all_editors().iter().map(|e| e.read(cx).path.clone()).find(|p| same_file(p, path))
    }

    fn on_set_decorations(&mut self, id: &str, uri: &str, set: String, mut list: Vec<v::Decoration>, cx: &mut Context<Self>) {
        let Some(path) = ext_broker::normalize(uri) else {
            self.log_extension(id, LogLevel::Warn, format!("set-decorations: '{uri}' is not a file URI"));
            return;
        };
        let path = self.open_path_matching(&path, cx).unwrap_or(path);
        list.truncate(limits::MARKS_PER_FILE);
        for d in &mut list {
            d.text = d.text.as_deref().and_then(|t| shown_text(t, limits::ITEM_LEN));
            d.hover = d.hover.as_deref().and_then(|t| shown_text(t, limits::ITEM_LEN));
        }
        let entry = self.cfg.exts.providers.decorations.entry(path.clone()).or_default();
        entry.retain(|(e, s, _)| !(e == id && *s == set));
        if !list.is_empty() {
            entry.push((id.to_string(), set, list));
        }
        if entry.is_empty() {
            self.cfg.exts.providers.decorations.remove(&path);
        }
        self.refresh_decorations(&[path], cx);
    }

    fn line_decorations(&self, path: &str) -> Vec<LineDecoration> {
        let Some(sets) = self.cfg.exts.providers.decorations.get(path) else { return Vec::new() };
        sets.iter()
            .flat_map(|(_, _, list)| list.iter())
            .map(|d| LineDecoration {
                first: d.range.start.line as usize,
                last: d.range.end.line.max(d.range.start.line) as usize,
                place: d.place,
                tone: d.tone,
                text: d.text.clone(),
                hover: d.hover.clone(),
            })
            .collect()
    }

    fn refresh_decorations(&mut self, paths: &[String], cx: &mut Context<Self>) {
        for ed in self.all_editors() {
            let path = ed.read(cx).path.clone();
            if paths.contains(&path) {
                let d = self.line_decorations(&path);
                ed.update(cx, |e, cx| e.set_ext_decorations(d, cx));
            }
        }
    }

    /// Notifications from an exthost that land here (the arms of `on_extension_notify`).
    pub(super) fn on_provider_notify(&mut self, id: &str, n: ext_api::value::Notify, cx: &mut Context<Self>) {
        use ext_api::value::Notify;
        match n {
            Notify::SetDiagnostics { uri, diagnostics } => self.on_set_diagnostics(id, &uri, diagnostics, cx),
            Notify::ClearDiagnostics { uri } => self.on_clear_diagnostics(id, uri.as_deref(), cx),
            Notify::SetDecorations { uri, set, decorations } => self.on_set_decorations(id, &uri, set, decorations, cx),
            Notify::SetStatusItem { item } => self.on_set_status_item(id, item, cx),
            Notify::RemoveStatusItem { id: item } => {
                self.cfg.exts.providers.status.retain(|(e, i)| !(e == id && i.id == item));
                cx.notify();
            }
            _ => {}
        }
    }

    /// Remove everything `id` put on screen: its status items, diagnostics and decorations.
    pub(super) fn forget_extension_marks(&mut self, id: &str, cx: &mut Context<Self>) {
        let p = &mut self.cfg.exts.providers;
        p.status.retain(|(e, _)| e != id);
        let mut touched = Vec::new();
        p.diagnostics.retain(|path, list| {
            if list.iter().any(|(e, _)| e == id) {
                touched.push(path.clone());
                list.retain(|(e, _)| e != id);
            }
            !list.is_empty()
        });
        let mut decorated = Vec::new();
        p.decorations.retain(|path, list| {
            if list.iter().any(|(e, _, _)| e == id) {
                decorated.push(path.clone());
                list.retain(|(e, _, _)| e != id);
            }
            !list.is_empty()
        });
        p.waiting.retain(|(_, e, _)| e != id);
        self.forget_extension_lenses(id, cx);
        self.refresh_diagnostics(&touched, cx);
        self.refresh_decorations(&decorated, cx);
        self.drop_ext_language_items(&|e| e != id, cx);
    }

    /// After a settings reload: what extensions that are no longer active (disabled, uninstalled) showed goes.
    pub(super) fn forget_removed_extension_marks(&mut self, cx: &mut Context<Self>) {
        let reg = crate::ext::current();
        let p = &self.cfg.exts.providers;
        let mut owners: Vec<String> = p.status.iter().map(|(e, _)| e.clone()).collect();
        owners.extend(p.diagnostics.values().flatten().map(|(e, _)| e.clone()));
        owners.extend(p.decorations.values().flatten().map(|(e, _, _)| e.clone()));
        owners.sort();
        owners.dedup();
        for id in owners.into_iter().filter(|id| !reg.active.iter().any(|e| e.id() == id.as_str())) {
            self.forget_extension_marks(&id, cx);
        }
        self.sync_ext_language(cx);
        self.forget_inactive_lenses(cx);
    }

    /// An exthost ended. A crash takes what the extension showed with it (it would be stale); an idle stop keeps it.
    pub(super) fn on_extension_exited(&mut self, id: &str, crashed: bool, cx: &mut Context<Self>) {
        if crashed {
            self.forget_extension_marks(id, cx);
            // Its views in windows of their own go back to the side bar, which shows what happened.
            self.return_ext_view_windows(id, cx);
        }
    }

    // --- editors ---

    /// A new editor: the formatter hook, what extensions already put on its file, and the extensions that start for its
    /// language.
    pub(super) fn attach_extensions(&mut self, ed: &Entity<EditorView>, window: &mut Window, cx: &mut Context<Self>) {
        let formatter = self.save_formatter(window, cx);
        let path = ed.read(cx).path.clone();
        let decorations = self.line_decorations(&path);
        let diagnostics = self.extension_diagnostics_for(&path);
        let offer = self.ext_language_offer(&path, window, cx);
        ed.update(cx, |e, cx| {
            e.set_save_formatter(Some(formatter));
            e.set_ext_decorations(decorations, cx);
            e.set_ext_language(offer);
        });
        self.attach_lenses(ed, window, cx);
        if !diagnostics.is_empty() {
            let d = self.diagnostics_for(&path);
            ed.update(cx, |e, cx| e.set_diagnostics(d, cx));
        }
        // Diagnostics and decorations providers start with a file of their language (`language:<id>`); nothing is asked,
        // the extension reads what it needs.
        let language = language_id(&path);
        let mut wake = self.providers_for(&language, |c| &c.diagnostics);
        wake.extend(self.providers_for(&language, |c| &c.decorations));
        wake.dedup_by(|a, b| a.id() == b.id());
        for active in wake {
            if !self.cfg.exts.code.running(active.id()) {
                self.send_to_extension(&active, &format!("language:{language}"), None, window, cx);
            }
        }
    }

    // --- tasks ---

    /// Ask every extension with `[[tasks]]` for its tasks, then continue with them added to `tasks`. Extensions that do
    /// not answer within the deadline are left out.
    pub(super) fn with_extension_tasks(
        &mut self,
        mut tasks: Vec<tasks::Task>,
        window: &mut Window,
        cx: &mut Context<Self>,
        then: impl FnOnce(&mut Self, Vec<tasks::Task>, &mut Window, &mut Context<Self>) + 'static,
    ) {
        let providers: Vec<Active> = crate::ext::current()
            .active
            .iter()
            .filter(|e| e.manifest.code.is_some() && !e.manifest.contributions.tasks.is_empty() && !self.cfg.exts.code.disabled.contains(e.id()))
            .cloned()
            .collect();
        if providers.is_empty() {
            return then(self, tasks, window, cx);
        }
        let asked: Vec<(String, oneshot::Receiver<Result<Response, v::Error>>)> =
            providers.iter().map(|a| (a.id().to_string(), self.ask_provider(a, "tasks", Request::ProvideTasks, None, window, cx))).collect();
        cx.spawn_in(window, async move |this, cx| {
            for (ext, answer) in asked {
                if let Ok(Ok(Response::Tasks(list))) = answer.await {
                    tasks.extend(list.into_iter().map(|t| tasks::Task::from_extension(&ext, &t.id, &t.label, t.detail.as_deref())));
                }
            }
            this.update_in(cx, |this, window, cx| then(this, tasks, window, cx)).ok();
        })
        .detach();
    }

    /// Run a task an extension provided: `run-task` asks the extension, which does the work (and shows progress).
    pub(super) fn run_extension_task(&mut self, extension: &str, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.ask_extension_as_user(extension, Request::RunTask { id: id.to_string() }, "tasks", window, cx);
    }

    // --- performance scenarios (crate::perf) ---

    /// `CODEYARD_PERF_DECORATIONS`: run the `decorate` command of every active extension with `[[decorations]]` for the
    /// active file's language (scripts/perf/run.sh installs two); the number asked.
    pub fn start_decorations_for_perf(&mut self, window: &mut Window, cx: &mut Context<Self>) -> usize {
        let Some(path) = self.active_editor().map(|e| e.read(cx).path.clone()) else { return 0 };
        let ids: Vec<String> = self.providers_for(&language_id(&path), |c| &c.decorations).iter().map(|a| a.id().to_string()).collect();
        for id in &ids {
            self.run_extension_command(id, "decorate", window, cx);
        }
        ids.len()
    }

    /// Decoration sets extensions have on the active file.
    pub fn decoration_sets_for_perf(&self, cx: &App) -> usize {
        let path = self.active_editor().map(|e| e.read(cx).path.clone()).unwrap_or_default();
        self.cfg.exts.providers.decorations.get(&path).map_or(0, Vec::len)
    }

    /// `CODEYARD_PERF_PROVIDERS`: ask every task provider for its tasks once; the flag turns true when all answered.
    pub fn ask_tasks_for_perf(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Rc<std::cell::Cell<bool>> {
        let done = Rc::new(std::cell::Cell::new(false));
        let flag = done.clone();
        self.with_extension_tasks(Vec::new(), window, cx, move |_, _, _, _| flag.set(true));
        done
    }
}

/// `command` as named by `extension` → (the extension that owns it, its local id): a local id or full id of its own,
/// or a dependency's full id.
fn resolve_command(extension: &str, command: &str) -> Option<(String, String)> {
    let reg = crate::ext::current();
    let me = reg.active.iter().find(|e| e.id() == extension)?;
    let full = if command.contains('.') { command.to_string() } else { format!("{extension}.{command}") };
    let owner = if full.strip_prefix(extension).is_some_and(|r| r.starts_with('.')) {
        me
    } else {
        let dep = me.manifest.dependencies.iter().find(|(d, _)| full.strip_prefix(d.as_str()).is_some_and(|r| r.starts_with('.')))?;
        reg.active.iter().find(|e| e.id() == dep.0.as_str())?
    };
    owner.manifest.contributions.commands.iter().any(|c| c.id == full).then(|| (owner.id().to_string(), full[owner.id().len() + 1..].to_string()))
}

#[cfg(test)]
impl Workbench {
    pub fn extension_status_for_test(&self) -> Vec<(String, String)> {
        self.cfg.exts.providers.status.iter().map(|(e, i)| (e.clone(), i.text.clone())).collect()
    }

    pub fn extension_menu_for_test(&self, location: MenuLocation, cx: &App) -> Vec<(String, bool)> {
        self.extension_menu(location, None, None, cx).into_iter().map(|(t, _, e)| (t, e)).collect()
    }

    pub fn run_extension_menu_for_test(&mut self, location: MenuLocation, title: &str, target: Option<&str>, window: &mut Window, cx: &mut Context<Self>) {
        let (_, action, _) = self.extension_menu(location, None, target, cx).into_iter().find(|(t, ..)| t == title).expect("menu entry");
        self.run_menu_action_for_test(action, window, cx);
    }

    /// An extension command runs here; a core command is dispatched as the menu dispatches it.
    fn run_menu_action_for_test(&mut self, action: Box<dyn gpui::Action>, window: &mut Window, cx: &mut Context<Self>) {
        match action.as_any().downcast_ref::<RunExtensionCommand>() {
            Some(a) => self.on_run_extension_command(a, window, cx),
            None => window.dispatch_action(action, cx),
        }
    }

    /// The view-title menu of `view`: (title, enabled).
    pub fn ext_view_title_menu_for_test(&self, view: &str, cx: &App) -> Vec<(String, bool)> {
        self.extension_menu(MenuLocation::ViewTitle, Some(view), None, cx).into_iter().map(|(t, _, e)| (t, e)).collect()
    }

    pub fn run_ext_view_title_menu_for_test(&mut self, view: &str, title: &str, window: &mut Window, cx: &mut Context<Self>) {
        let (_, action, _) = self.extension_menu(MenuLocation::ViewTitle, Some(view), None, cx).into_iter().find(|(t, ..)| t == title).expect("menu entry");
        self.run_menu_action_for_test(action, window, cx);
    }

    /// The most decoration sets any file has.
    pub fn decoration_sets_any_file_for_test(&self) -> usize {
        self.cfg.exts.providers.decorations.values().map(Vec::len).max().unwrap_or(0)
    }

    pub fn forget_extension_answer_for_test(&mut self, id: &str) {
        self.cfg.exts.code.last_answer.remove(id);
    }

    /// Run `command` of `extension` with a JSON argument, as a menu or key binding does.
    pub fn run_extension_command_with_for_test(&mut self, extension: &str, command: &str, args: &str, window: &mut Window, cx: &mut Context<Self>) {
        let a = RunExtensionCommand { extension: extension.into(), command: command.into(), args: args.into() };
        self.on_run_extension_command(&a, window, cx);
    }

    pub fn click_extension_status_for_test(&mut self, text: &str, window: &mut Window, cx: &mut Context<Self>) {
        let (ext, item) = self.cfg.exts.providers.status.iter().find(|(_, i)| i.text == text).cloned().expect("status item");
        let command = item.command.expect("a command");
        self.run_extension_command_id(&ext, &command, "null".into(), window, cx);
    }

    pub fn extension_log_for_test(&self, id: &str) -> Vec<String> {
        self.cfg.exts.code.logs.get(id).map(|l| l.iter().map(|(_, s)| s.clone()).collect()).unwrap_or_default()
    }

    pub fn quick_fix_labels_for_test(&self, cx: &App) -> Option<Vec<String>> {
        match &self.overlay {
            Some(Overlay { kind: OverlayKind::QuickFixes(fixes), .. }) => Some(
                fixes
                    .iter()
                    .map(|f| match f {
                        QuickFix::Lsp(_, a) => a.title.clone(),
                        QuickFix::Extension { action, .. } => action.title.clone(),
                    })
                    .collect(),
            ),
            _ => {
                let _ = cx;
                None
            }
        }
    }

    pub fn choose_quick_fix_for_test(&mut self, title: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(Overlay { kind: OverlayKind::QuickFixes(fixes), .. }) = &self.overlay else { panic!("no quick fixes open") };
        let fix = fixes
            .iter()
            .find(|f| match f {
                QuickFix::Lsp(_, a) => a.title == title,
                QuickFix::Extension { action, .. } => action.title == title,
            })
            .cloned()
            .expect("quick fix");
        self.close_overlay(window, cx);
        self.run_quick_fix(fix, window, cx);
    }

    /// Run an extension's code action as if the user chose it from the list asked for `document`.
    pub fn run_extension_code_action_for_test(&mut self, extension: &str, document: &str, action: v::CodeAction, window: &mut Window, cx: &mut Context<Self>) {
        self.run_quick_fix(QuickFix::Extension { extension: extension.into(), document: document.into(), action }, window, cx);
    }

    pub fn task_labels_for_test(&mut self, window: &mut Window, cx: &mut Context<Self>) -> std::rc::Rc<std::cell::RefCell<Option<Vec<String>>>> {
        let out = std::rc::Rc::new(std::cell::RefCell::new(None));
        let slot = out.clone();
        self.with_extension_tasks(Vec::new(), window, cx, move |_, tasks, _, _| *slot.borrow_mut() = Some(tasks.iter().map(|t| t.label.clone()).collect()));
        out
    }

    pub fn run_task_for_test(&mut self, label: &str, window: &mut Window, cx: &mut Context<Self>) {
        let label = label.to_string();
        self.with_extension_tasks(Vec::new(), window, cx, move |this, tasks, window, cx| {
            let t = tasks.into_iter().find(|t| t.label == label).expect("task");
            this.run_task(t, window, cx);
        });
    }
}
