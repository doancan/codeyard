//! Workbench: code lenses and inlay hints from code extensions (#143, docs/plugin-architecture.md §5.1). They show at
//! the end of their line next to the language server's, and name their extension in the tooltip.
//!
//! Providers are asked for the lines in view only: when an editor first draws, after an edit (once typing pauses for
//! `EDIT_DEBOUNCE`) and after a scroll (once the view stands still for `SCROLL_DEBOUNCE`). A new question (new text or
//! new lines) cancels the one before it, so typing never waits for a provider and an answer about old text or old
//! lines is dropped; moving the cursor cancels nothing. A lens without a command is resolved
//! when it is on screen. Clicking a lens runs its command through the broker (`Scope::may_execute`): the extension's
//! own command, a dependency's, or a `core:` command its manifest lists.

use std::collections::HashMap;
use std::time::Duration;

use ext_api::value::{self as v, LogLevel, Request, Response};
use ext_manifest::limits;
use futures::StreamExt;
use futures::channel::oneshot;
use gpui::{EntityId, Task, WeakEntity};

use super::*;
use crate::editor::{EditorEvent, ExtEvent, ExtHint, ExtLens};

/// After an edit, the providers are asked again once typing pauses this long.
const EDIT_DEBOUNCE: Duration = Duration::from_millis(300);
/// After a scroll, once the view stands still this long.
const SCROLL_DEBOUNCE: Duration = Duration::from_millis(100);
/// Lenses without a command resolved per question (each is a request of its own).
const RESOLVE_MAX: usize = 20;

/// What was asked for each editor.
#[derive(Default)]
pub(super) struct LensState {
    asks: HashMap<EntityId, Ask>,
}

struct Ask {
    /// (buffer version, lines in view): the same question is not asked twice.
    key: (u64, std::ops::Range<usize>),
    /// The debounce timer, then the wait for the answers; dropped with the question.
    _wait: Task<()>,
    _answers: Option<Task<()>>,
    /// Requests in flight: (extension, request id).
    ids: Vec<(String, u64)>,
}

/// Which question an answer is for.
#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Lenses,
    Hints,
    Resolve,
}

type Answer = oneshot::Receiver<Result<Response, v::Error>>;
/// An answer as it arrives: (extension, kind, request id, result).
type Arrived = futures::future::LocalBoxFuture<'static, (String, Kind, u64, Result<Response, v::Error>)>;

fn arrival(ext: String, kind: Kind, id: u64, answer: Answer) -> Arrived {
    Box::pin(async move { (ext, kind, id, answer.await.unwrap_or(Err(v::Error::Cancelled))) })
}

impl Workbench {
    /// A new editor: its lenses and hints follow its edits and its view.
    pub(super) fn attach_lenses(&mut self, ed: &Entity<EditorView>, window: &mut Window, cx: &mut Context<Self>) {
        self._subs.push(cx.subscribe_in(ed, window, |this, ed, ev: &EditorEvent, window, cx| match ev {
            EditorEvent::Changed => this.schedule_lenses(ed, EDIT_DEBOUNCE, window, cx),
            EditorEvent::Extension(ExtEvent::View) => this.schedule_lenses(ed, SCROLL_DEBOUNCE, window, cx),
            EditorEvent::Extension(ExtEvent::RunLens(lens)) => this.run_ext_lens(lens.clone(), window, cx),
            _ => {}
        }));
    }

    /// Ask the lens and hint providers of `ed`'s language about the lines in view, after `delay`, unless that was
    /// asked already. What was asked before is cancelled.
    fn schedule_lenses(&mut self, ed: &Entity<EditorView>, delay: Duration, window: &mut Window, cx: &mut Context<Self>) {
        let (path, version, view) = {
            let e = ed.read(cx);
            (e.path.clone(), e.version(), e.ext_view())
        };
        let language = language_id(&path);
        let (lenses_on, hints_on) = {
            let s = cx.global::<Settings>();
            (s.code_lens, s.inlay_hints)
        };
        let lens_providers = if lenses_on { self.providers_for(&language, |c| &c.code_lenses) } else { Vec::new() };
        let hint_providers = if hints_on { self.providers_for(&language, |c| &c.inlay_hints) } else { Vec::new() };
        // What extensions that no longer provide for this file showed goes.
        let (lens_ids, hint_ids): (Vec<String>, Vec<String>) =
            (lens_providers.iter().map(|a| a.id().to_string()).collect(), hint_providers.iter().map(|a| a.id().to_string()).collect());
        ed.update(cx, |e, cx| e.keep_ext_lenses_of(&lens_ids, &hint_ids, cx));
        let id = ed.entity_id();
        if lens_providers.is_empty() && hint_providers.is_empty() {
            if let Some(old) = self.cfg.exts.providers.lenses.asks.remove(&id) {
                self.cancel_lens_requests(old.ids);
            }
            return;
        }
        // Not drawn yet: the first frame reports its lines.
        if view.is_empty() {
            return;
        }
        let key = (version, view);
        if self.cfg.exts.providers.lenses.asks.get(&id).is_some_and(|a| a.key == key) {
            return;
        }
        // The user moved on: the old question is cancelled, and its answers are dropped.
        if let Some(old) = self.cfg.exts.providers.lenses.asks.remove(&id) {
            self.cancel_lens_requests(old.ids);
        }
        let weak = ed.downgrade();
        let wait = cx.spawn_in(window, async move |this, cx| {
            cx.background_executor().timer(delay).await;
            this.update_in(cx, |this, window, cx| this.ask_lenses(weak, lens_providers, hint_providers, window, cx)).ok();
        });
        self.cfg.exts.providers.lenses.asks.insert(id, Ask { key, _wait: wait, _answers: None, ids: Vec::new() });
    }

    /// Send the questions for the lines in view, and show each answer as it comes.
    fn ask_lenses(
        &mut self,
        ed: WeakEntity<EditorView>,
        lens_providers: Vec<crate::ext::Active>,
        hint_providers: Vec<crate::ext::Active>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(editor) = ed.upgrade() else { return };
        let id = editor.entity_id();
        let (job, view) = {
            let e = editor.read(cx);
            (e.format_job(), e.ext_view())
        };
        let language = language_id(&job.path);
        let document = v::Document { uri: ext_broker::uri(&job.path), language: language.clone(), version: job.version as u32, text: job.text };
        let pos = |line: usize| v::Position { line: line as u32, character: 0 };
        let range = v::Range { start: pos(view.start), end: pos(view.end) };
        let reason = format!("language:{language}");
        let mut pending: futures::stream::FuturesUnordered<Arrived> = Default::default();
        let mut ids = Vec::new();
        for (providers, kind) in [(&lens_providers, Kind::Lenses), (&hint_providers, Kind::Hints)] {
            for a in providers {
                let request = match kind {
                    Kind::Hints => Request::InlayHints { document: document.clone(), range },
                    _ => Request::CodeLens { document: document.clone(), range },
                };
                // Not `about` the file: an edit cancels them here (a new question), a cursor move does not.
                let (rid, answer) = self.ask_provider_with_id(a, &reason, request, None, window, cx);
                ids.push((a.id().to_string(), rid));
                pending.push(arrival(a.id().to_string(), kind, rid, answer));
            }
        }
        let version = job.version;
        let answers = cx.spawn_in(window, async move |this, cx| {
            // Each answer shows as it comes: a slow extension does not hold back the others.
            while let Some((ext, kind, rid, result)) = pending.next().await {
                let resolving = this
                    .update_in(cx, |this, window, cx| {
                        if let Some(ask) = this.cfg.exts.providers.lenses.asks.get_mut(&id) {
                            ask.ids.retain(|(e, r)| !(*e == ext && *r == rid));
                        }
                        this.show_lens_answer(&ed, &ext, kind, result, version, &view, &document, window, cx)
                    })
                    .unwrap_or_default();
                // Lenses without a command, resolved now that they are on screen.
                for (rid, answer) in resolving {
                    pending.push(arrival(ext.clone(), Kind::Resolve, rid, answer));
                }
            }
        });
        if let Some(ask) = self.cfg.exts.providers.lenses.asks.get_mut(&id) {
            ask.ids = ids;
            ask._answers = Some(answers);
        }
    }

    /// Show one provider's answer on `ed`, if its text did not change since. Lenses without a command are asked to be
    /// resolved: those requests are returned.
    #[allow(clippy::too_many_arguments)]
    fn show_lens_answer(
        &mut self,
        ed: &WeakEntity<EditorView>,
        ext: &str,
        kind: Kind,
        result: Result<Response, v::Error>,
        version: u64,
        view: &std::ops::Range<usize>,
        document: &v::Document,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Vec<(u64, Answer)> {
        let Some(editor) = ed.upgrade().filter(|e| e.read(cx).version() == version) else { return Vec::new() };
        let what = match kind {
            Kind::Lenses => "code lenses",
            Kind::Hints => "inlay hints",
            Kind::Resolve => "resolving a code lens",
        };
        let in_view = |line: u32| view.contains(&(line as usize));
        match (kind, result) {
            (Kind::Lenses, Ok(Response::CodeLenses(list))) => {
                let mut shown = Vec::new();
                let mut unresolved = Vec::new();
                for lens in list.into_iter().filter(|l| in_view(l.range.start.line)).take(limits::MARKS_PER_FILE) {
                    match &lens.command {
                        Some(_) => shown.extend(self.ext_lens(ext, lens)),
                        None => unresolved.push(lens),
                    }
                }
                editor.update(cx, |e, cx| e.set_ext_lenses(ext, shown, cx));
                let Some(active) = crate::ext::current().active.iter().find(|a| a.id() == ext).cloned() else { return Vec::new() };
                let mut answers = Vec::new();
                for lens in unresolved.into_iter().take(RESOLVE_MAX) {
                    let request = Request::ResolveCodeLens { document: document.clone(), lens };
                    let (rid, answer) = self.ask_provider_with_id(&active, &format!("language:{}", document.language), request, None, window, cx);
                    if let Some(ask) = self.cfg.exts.providers.lenses.asks.get_mut(&editor.entity_id()) {
                        ask.ids.push((ext.to_string(), rid));
                    }
                    answers.push((rid, answer));
                }
                return answers;
            }
            (Kind::Resolve, Ok(Response::CodeLenses(mut list))) if list.len() == 1 && list[0].command.is_some() => {
                if let Some(lens) = self.ext_lens(ext, list.remove(0)) {
                    editor.update(cx, |e, cx| e.add_ext_lens(lens, cx));
                }
            }
            (Kind::Resolve, Ok(_)) => self.log_extension(ext, LogLevel::Warn, "resolving a code lens: answer with the one lens, with its command".into()),
            (Kind::Hints, Ok(Response::InlayHints(list))) => {
                let hints: Vec<ExtHint> = list
                    .into_iter()
                    .filter(|h| in_view(h.position.line))
                    .take(limits::MARKS_PER_FILE)
                    .filter_map(|h| {
                        let label = super::shown_text(&h.label, limits::ITEM_LEN)?;
                        let tooltip = h.tooltip.as_deref().and_then(|t| super::shown_text(t, limits::ITEM_LEN));
                        Some(ExtHint { extension: ext.to_string(), line: h.position.line as usize, character: h.position.character, label, tooltip })
                    })
                    .collect();
                editor.update(cx, |e, cx| e.set_ext_hints(ext, hints, cx));
            }
            // The user moved on, or the provider was too slow (`ask_provider` notes a late answer in the log).
            (_, Err(v::Error::Cancelled | v::Error::Timeout)) => {}
            (_, Err(e)) => self.log_extension(ext, LogLevel::Warn, format!("{what}: {}", super::super::code_glue::explain(&e))),
            (_, Ok(_)) => self.log_extension(ext, LogLevel::Warn, format!("{what}: the answer is not {what}")),
        }
        Vec::new()
    }

    /// A lens with a command, as the editor shows it: `None` when its title cannot be shown.
    fn ext_lens(&mut self, ext: &str, lens: v::CodeLens) -> Option<ExtLens> {
        let command = lens.command?;
        let Some(title) = super::shown_text(&command.title, limits::ITEM_LEN) else {
            self.log_extension(ext, LogLevel::Warn, format!("code lens \"{}\": its title must be 1 to {} visible characters", lens.id, limits::ITEM_LEN));
            return None;
        };
        Some(ExtLens { extension: ext.to_string(), line: lens.range.start.line as usize, title, command: command.command, args: command.args_json })
    }

    /// Cancel requests still in flight: the extension is told, and their answers will be dropped.
    fn cancel_lens_requests(&mut self, ids: Vec<(String, u64)>) {
        for (ext, id) in ids {
            self.cancel_provider_request(&ext, id);
        }
    }

    /// Clicking a lens: its command runs with its arguments, if the extension may run it.
    fn run_ext_lens(&mut self, lens: ExtLens, window: &mut Window, cx: &mut Context<Self>) {
        let reg = crate::ext::current();
        let Some(active) = reg.active.iter().find(|a| a.id() == lens.extension).cloned() else { return };
        let ext = lens.extension.clone();
        let full = if lens.command.starts_with("core:") || lens.command.contains('.') { lens.command.clone() } else { format!("{ext}.{}", lens.command) };
        let scope = self.extension_scope(&active, cx);
        if let Err(e) = scope.may_execute(&full) {
            let why = super::super::code_glue::explain(&e);
            self.log_extension(&ext, LogLevel::Warn, format!("code lens \"{}\": {full} refused: {why}", lens.title));
            self.notice = Some(Notice::Error(format!("{ext}: {why}").into()));
            cx.notify();
            return;
        }
        if full.starts_with("core:") {
            let done: super::super::core_commands_glue::CoreDone = Box::new(move |this, c, cx| {
                if let v::Completion::Failed(e) = c {
                    this.notice = Some(Notice::Error(format!("{ext}: {}", super::super::code_glue::explain(&e)).into()));
                    cx.notify();
                }
            });
            self.run_core_for_extension(&lens.extension, scope, &full, &lens.args, done, window, cx);
        } else {
            self.run_extension_command_id(&ext, &full, lens.args, window, cx);
        }
    }

    /// `id` went (disabled, uninstalled, crashed): its lenses and hints go from every editor.
    pub(super) fn forget_extension_lenses(&mut self, id: &str, cx: &mut Context<Self>) {
        for ed in self.all_editors() {
            ed.update(cx, |e, cx| e.clear_ext_lenses(Some(id), cx));
        }
        let mut mine: Vec<(String, u64)> = Vec::new();
        for a in self.cfg.exts.providers.lenses.asks.values_mut() {
            mine.extend(a.ids.extract_if(.., |(e, _)| e == id));
            // Asked again on the next edit, scroll or cursor move (after a crash, the restarted extension answers).
            a.key = (u64::MAX, 0..0);
        }
        self.cancel_lens_requests(mine);
    }

    /// After a settings reload: lenses and hints of extensions that are no longer active go.
    pub(super) fn forget_inactive_lenses(&mut self, cx: &mut Context<Self>) {
        let active: Vec<String> =
            crate::ext::current().active.iter().filter(|e| !self.cfg.exts.code.disabled.contains(e.id())).map(|e| e.id().to_string()).collect();
        for ed in self.all_editors() {
            ed.update(cx, |e, cx| e.keep_ext_lenses_of(&active, &active, cx));
        }
    }

    /// Lenses and hints extensions show on the active editor (performance scenarios).
    pub fn lens_marks_for_perf(&self, cx: &App) -> usize {
        self.active_editor().map_or(0, |e| e.read(cx).ext_marks_count())
    }
}

#[cfg(test)]
impl Workbench {
    /// Requests for lenses and hints in flight, for every editor.
    pub fn lens_requests_in_flight_for_test(&self) -> usize {
        self.cfg.exts.providers.lenses.asks.values().map(|a| a.ids.len()).sum()
    }
}
