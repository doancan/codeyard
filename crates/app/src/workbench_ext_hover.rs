//! Workbench: hover and completion from code extensions (`[[hover]]`, `[[completion]]`; docs/plugin-architecture.md
//! §5.1). Each editor knows which extensions offer them for its file (`ExtLanguage`) and asks through the Workbench,
//! which sends `request.hover` or `request.completion` with the document as the editor has it (also over SSH: the
//! text comes from the app, never from the host), with the provider deadline. Answers go back one by one, as they
//! come; one that misses the deadline is dropped and noted in the extension's log, and the editor dropping the reply
//! (the user moved on) cancels what is still asked. What an extension showed goes with it.

use std::rc::Rc;

use futures::StreamExt;
use futures::channel::{mpsc, oneshot};
use futures::stream::FuturesUnordered;

use super::*;
use crate::editor::ext_lang::{ExtAnswer, ExtAsker, ExtLanguage, ExtQuery, ExtQueryKind, ExtReply};

/// Items one extension may add to one completion list; more are cut.
const MAX_ITEMS: usize = 1000;
/// Characters of hover text, and of a label or detail, an extension may show.
const MAX_HOVER: usize = 8000;
const MAX_LABEL: usize = 200;

/// How editors ask: one closure per window, made when the first editor is attached.
#[derive(Default)]
pub(super) struct LangState {
    asker: Option<ExtAsker>,
}

/// Text from an extension without characters that change how text looks, newlines kept.
fn clean(s: &str, max: usize) -> String {
    s.chars().filter(|&c| c == '\n' || !ext_manifest::is_deceptive(c)).take(max).collect()
}

fn range16(r: v::Range) -> crate::lsp::Range16 {
    ((r.start.line, r.start.character), (r.end.line, r.end.character))
}

/// Markdown reduced to the plain text the hover popup shows, as for a language server's.
fn plain(markdown: &str) -> Option<String> {
    crate::lsp::hover_text(&serde_json::json!({ "contents": markdown })).filter(|t| !t.trim().is_empty())
}

/// An extension's completion item as the editor's list holds it; `None` when its label cannot be shown.
fn to_item(i: v::CompletionItem) -> Option<crate::lsp::CompletionItem> {
    let label = shown_text(&i.label, MAX_LABEL)?;
    let raw = i.insert_text.map_or_else(|| label.clone(), |t| clean(&t, MAX_HOVER));
    let (insert, stops) = if i.snippet { crate::lsp::parse_snippet(&raw) } else { (raw, Vec::new()) };
    Some(crate::lsp::CompletionItem {
        filter: i.filter_text.and_then(|f| shown_text(&f, MAX_LABEL)).unwrap_or_else(|| label.clone()),
        sort: i.sort_text.and_then(|s| shown_text(&s, MAX_LABEL)).unwrap_or_else(|| label.clone()),
        detail: i.detail.and_then(|d| shown_text(d.lines().next().unwrap_or_default(), MAX_LABEL)),
        kind: i.kind.lsp(),
        label,
        cursor: stops.first().map(|(_, r)| r.start),
        insert,
        stops,
        range: i.range.map(range16),
        extra: i.additional_edits.into_iter().map(|e| (range16(e.range), e.new_text)).collect(),
        doc: i.documentation.and_then(|d| plain(&clean(&d, MAX_HOVER))),
        resolve: None,
        source: None,
    })
}

/// An answer as the editor shows it; `None` for nothing to show (no hover, an error, a timeout).
fn to_answer(result: Result<Response, v::Error>) -> Option<ExtAnswer> {
    match result {
        Ok(Response::Hover(Some(h))) => plain(&clean(&h.contents, MAX_HOVER)).map(ExtAnswer::Hover),
        Ok(Response::Completions(list)) => {
            let items: Vec<_> = list.items.into_iter().take(MAX_ITEMS).filter_map(to_item).collect();
            (!items.is_empty() || list.incomplete).then_some(ExtAnswer::Completion { items, incomplete: list.incomplete })
        }
        _ => None,
    }
}

impl Workbench {
    /// Active code extensions that complete `language`; with `trigger`, only those that list it.
    fn completion_providers(&self, language: &str, trigger: Option<char>) -> Vec<Active> {
        crate::ext::current()
            .active
            .iter()
            .filter(|e| e.manifest.code.is_some() && !self.cfg.exts.code.disabled.contains(e.id()))
            .filter(|e| e.manifest.contributions.completion.iter().any(|c| c.handles(language) && trigger.is_none_or(|t| c.trigger_characters.contains(&t))))
            .cloned()
            .collect()
    }

    /// The closure editors ask with: it reaches this Workbench through its window, so it works from any editor.
    fn ext_asker(&mut self, window: &Window, cx: &mut Context<Self>) -> ExtAsker {
        if let Some(a) = &self.cfg.exts.providers.lang.asker {
            return a.clone();
        }
        let wb = cx.entity().downgrade();
        let handle = window.window_handle();
        let asker: ExtAsker = Rc::new(move |query: ExtQuery, cx: &mut gpui::AsyncApp| {
            cx.update_window(handle, |_, window, cx| wb.update(cx, |wb, cx| wb.ask_language(query, window, cx)).ok().flatten()).ok().flatten()
        });
        self.cfg.exts.providers.lang.asker = Some(asker.clone());
        asker
    }

    /// What extensions offer for `path`: `None` when none describes or completes its language.
    pub(super) fn ext_language_offer(&mut self, path: &str, window: &Window, cx: &mut Context<Self>) -> Option<ExtLanguage> {
        let ask = self.ext_asker(window, cx);
        self.offer_with(path, ask)
    }

    fn offer_with(&self, path: &str, ask: ExtAsker) -> Option<ExtLanguage> {
        let language = language_id(path);
        let hover = !self.providers_for(&language, |c| &c.hover).is_empty();
        let completers = self.completion_providers(&language, None);
        let completion = (!completers.is_empty()).then(|| {
            let mut all: Vec<char> = Vec::new();
            for c in completers.iter().flat_map(|e| e.manifest.contributions.completion.iter().filter(|c| c.handles(&language))) {
                for &t in &c.trigger_characters {
                    if !all.contains(&t) {
                        all.push(t);
                    }
                }
            }
            all
        });
        (hover || completion.is_some()).then_some(ExtLanguage { hover, completion, ask })
    }

    /// After extensions changed (installed, turned off, uninstalled): every editor learns what is offered now, and
    /// what extensions that are gone showed goes at once.
    pub(super) fn sync_ext_language(&mut self, cx: &mut Context<Self>) {
        let ask = self.cfg.exts.providers.lang.asker.clone();
        for ed in self.all_editors() {
            let offer = ask.clone().and_then(|ask| self.offer_with(&ed.read(cx).path, ask));
            ed.update(cx, |e, _| e.set_ext_language(offer));
        }
        let reg = crate::ext::current();
        let disabled = self.cfg.exts.code.disabled.clone();
        let keep = move |id: &str| reg.active.iter().any(|e| e.id() == id) && !disabled.contains(id);
        self.drop_ext_language_items(&keep, cx);
    }

    /// Remove the hover text and completion items of extensions `keep` refuses, in every editor.
    pub(super) fn drop_ext_language_items(&mut self, keep: &dyn Fn(&str) -> bool, cx: &mut Context<Self>) {
        for ed in self.all_editors() {
            ed.update(cx, |e, cx| e.drop_ext_items(keep, cx));
        }
    }

    /// An editor's query: ask every extension that offers it, and pass on each answer as it comes.
    fn ask_language(&mut self, query: ExtQuery, window: &mut Window, cx: &mut Context<Self>) -> Option<ExtReply> {
        let language = language_id(&query.path);
        let providers = match query.kind {
            ExtQueryKind::Hover => self.providers_for(&language, |c| &c.hover),
            ExtQueryKind::Completion { trigger } => self.completion_providers(&language, trigger),
        };
        if providers.is_empty() {
            return None;
        }
        let document = v::Document { uri: ext_broker::uri(&query.path), language: language.clone(), version: query.version as u32, text: query.text };
        let position = v::Position { line: query.position.0, character: query.position.1 };
        let (request, about) = match query.kind {
            // A hover is about the text as it is: an edit cancels it.
            ExtQueryKind::Hover => (Request::Hover { document, position }, Some(query.path)),
            // Completion goes on while the word is typed; the editor cancels it when the word is left.
            ExtQueryKind::Completion { trigger } => (Request::Completion { document, position, trigger: trigger.map(String::from) }, None),
        };
        let reason = format!("language:{language}");
        let asked: Vec<(String, u64, oneshot::Receiver<Result<Response, v::Error>>)> = providers
            .iter()
            .map(|a| {
                let (id, answer) = self.ask_provider_with_id(a, &reason, request.clone(), about.clone(), window, cx);
                (a.id().to_string(), id, answer)
            })
            .collect();
        let (tx, answers) = mpsc::unbounded();
        let (alive, mut gone) = oneshot::channel::<()>();
        let mut open: Vec<(String, u64)> = asked.iter().map(|(e, id, _)| (e.clone(), *id)).collect();
        let mut waiting: FuturesUnordered<_> =
            asked.into_iter().map(|(ext, id, answer)| async move { (ext, id, answer.await.unwrap_or(Err(v::Error::Cancelled))) }).collect();
        cx.spawn(async move |this, cx| {
            loop {
                match futures::future::select(waiting.next(), &mut gone).await {
                    futures::future::Either::Left((Some((ext, id, result)), _)) => {
                        open.retain(|(e, i)| !(*e == ext && *i == id));
                        if let Some(answer) = to_answer(result) {
                            let _ = tx.unbounded_send((ext, answer));
                        }
                    }
                    futures::future::Either::Left((None, _)) => return,
                    // The editor no longer wants them: what is still asked is cancelled.
                    futures::future::Either::Right(_) => {
                        this.update(cx, |this, _| open.iter().for_each(|(ext, id)| this.cancel_provider_request(ext, *id))).ok();
                        return;
                    }
                }
            }
        })
        .detach();
        Some(ExtReply { answers, _alive: alive })
    }

    // --- performance scenarios (crate::perf) ---

    /// Extensions that complete the active file's language (`CODEYARD_PERF_COMPLETION`).
    pub fn completion_providers_for_perf(&self, cx: &App) -> usize {
        let Some(path) = self.active_editor().map(|e| e.read(cx).path.clone()) else { return 0 };
        self.completion_providers(&language_id(&path), None).len()
    }
}
