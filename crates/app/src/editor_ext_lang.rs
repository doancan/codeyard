//! Editor: hover and completion from code extensions (docs/plugin-architecture.md §5.1). The Workbench says which
//! extensions offer them for this document and asks them; the editor shows what comes back next to the language
//! server's, each item tagged with its extension.
//!
//! Nothing here waits. The document's text is taken off the UI thread, every answer arrives on its own, and a request
//! is cancelled when the user moves on: the hover task is replaced when the mouse leaves the word, and a completion
//! request when its word is left, the list is closed or a new request starts (dropping the reply tells the Workbench).

use std::rc::Rc;

use futures::StreamExt;
use futures::channel::{mpsc, oneshot};

use super::*;

/// What the Workbench asks extensions: the document as it is now and a position in it.
pub struct ExtQuery {
    pub path: String,
    pub text: String,
    /// `buffer.version()` when the query was taken.
    pub version: u64,
    /// Zero-based line and UTF-16 column.
    pub position: (u32, u32),
    pub kind: ExtQueryKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExtQueryKind {
    Hover,
    /// `trigger`: the trigger character typed, `None` when a word was typed or the user asked.
    Completion {
        trigger: Option<char>,
    },
}

/// One extension's answer, already in the editor's terms.
pub enum ExtAnswer {
    /// Plain text.
    Hover(String),
    Completion {
        items: Vec<crate::lsp::CompletionItem>,
        incomplete: bool,
    },
}

/// The answers to one query, each as it arrives: (extension id, answer). Dropping the reply cancels what is still
/// asked.
pub struct ExtReply {
    pub answers: mpsc::UnboundedReceiver<(String, ExtAnswer)>,
    /// Held while the editor still wants the answers; the Workbench cancels the requests when it is dropped.
    pub _alive: oneshot::Sender<()>,
}

/// Asks the extensions that offer `kind` for this file; `None` when none does.
pub type ExtAsker = Rc<dyn Fn(ExtQuery, &mut gpui::AsyncApp) -> Option<ExtReply>>;

/// What extensions offer for this document, set by the Workbench.
#[derive(Clone)]
pub struct ExtLanguage {
    pub hover: bool,
    /// The union of their trigger characters; `None` when no extension completes this language.
    pub completion: Option<Vec<char>>,
    pub ask: ExtAsker,
}

/// The editor's share: the offer and the completion request in flight.
#[derive(Default)]
pub struct ExtLangState {
    offer: Option<ExtLanguage>,
    /// The word start the extensions were last asked about, and what they answered so far.
    session: Option<Session>,
    /// Waits for the answers; dropping it cancels the request.
    task: Option<gpui::Task<()>>,
}

struct Session {
    start: usize,
    items: Vec<crate::lsp::CompletionItem>,
    incomplete: bool,
}

/// A query ready to send: the text is taken off the UI thread first.
pub struct ExtAsk {
    asker: ExtAsker,
    text: Box<dyn FnOnce() -> String + Send>,
    query: ExtQuery,
}

impl ExtAsk {
    pub async fn send(self, cx: &mut gpui::AsyncApp) -> Option<ExtReply> {
        let ExtAsk { asker, text, mut query } = self;
        query.text = cx.background_executor().spawn(async move { text() }).await;
        asker(query, cx)
    }
}

impl EditorView {
    /// What extensions offer for this document (`None`: nothing). The current request goes when the offer changes.
    pub fn set_ext_language(&mut self, offer: Option<ExtLanguage>) {
        let completes = offer.as_ref().is_some_and(|o| o.completion.is_some());
        self.ext.lang.offer = offer;
        if !completes {
            self.end_ext_completion();
        }
    }

    fn ext_ask(&self, asker: ExtAsker, ix: usize, kind: ExtQueryKind) -> ExtAsk {
        let rope = self.buffer.rope().clone();
        let query = ExtQuery { path: self.path.clone(), text: String::new(), version: self.buffer.version(), position: self.buffer.lsp_position(ix), kind };
        ExtAsk { asker, text: Box::new(move || rope.to_string()), query }
    }

    /// The hover query for the character at `ix`, if an extension describes this language.
    pub(crate) fn ext_hover_ask(&self, ix: usize) -> Option<ExtAsk> {
        let asker = self.ext.lang.offer.as_ref().filter(|o| o.hover)?.ask.clone();
        Some(self.ext_ask(asker, ix, ExtQueryKind::Hover))
    }

    /// Show each extension's hover as it arrives, under the language server's (`at`, `position`: the hover's word).
    pub(crate) async fn show_ext_hovers(this: gpui::WeakEntity<Self>, mut reply: ExtReply, at: usize, position: Point<Pixels>, cx: &mut gpui::AsyncApp) {
        while let Some((ext, answer)) = reply.answers.next().await {
            let ExtAnswer::Hover(text) = answer else { continue };
            let shown = this.update(cx, |e, cx| {
                let h = e.hover.get_or_insert_with(|| Hover { at, diagnostics: Vec::new(), text: None, position, ext: Vec::new() });
                if h.at != at {
                    return false;
                }
                h.ext.push((ext, text));
                cx.notify();
                true
            });
            if !matches!(shown, Ok(true)) {
                return;
            }
        }
    }

    /// After a typed character: ask the extensions for completions when a word starts or a trigger character is typed;
    /// the list they answered is filtered as the word goes on, like the language server's.
    pub(crate) fn ext_after_typed(&mut self, typed: &str, cx: &mut Context<Self>) {
        let Some(triggers) = self.ext.lang.offer.as_ref().and_then(|o| o.completion.clone()) else { return };
        let mut chars = typed.chars();
        let (Some(ch), None) = (chars.next(), chars.next()) else { return self.end_ext_completion() };
        if self.buffer.cursor_count() > 1 || !cx.global::<Settings>().quick_suggestions {
            return self.end_ext_completion();
        }
        if Self::is_word(ch) {
            let start = self.word_start(self.buffer.cursor());
            match &self.ext.lang.session {
                Some(s) if s.start == start && !s.incomplete => {}
                _ => self.ask_ext_completion(None, cx),
            }
        } else if triggers.contains(&ch) {
            self.ask_ext_completion(Some(ch), cx);
        } else {
            self.end_ext_completion();
        }
    }

    /// Ask the extensions that complete this language (all of them, or those with `trigger` among their trigger
    /// characters). The previous request is cancelled.
    pub(crate) fn ask_ext_completion(&mut self, trigger: Option<char>, cx: &mut Context<Self>) {
        let Some(offer) = self.ext.lang.offer.clone().filter(|o| o.completion.is_some()) else { return };
        let cursor = self.buffer.cursor();
        let start = self.word_start(cursor);
        self.ext.lang.session = Some(Session { start, items: Vec::new(), incomplete: false });
        let ask = self.ext_ask(offer.ask, cursor, ExtQueryKind::Completion { trigger });
        self.ext.lang.task = Some(cx.spawn(async move |this, cx| {
            let Some(mut reply) = ask.send(cx).await else { return };
            while let Some((ext, answer)) = reply.answers.next().await {
                if let ExtAnswer::Completion { items, incomplete } = answer
                    && this.update(cx, |e, cx| e.add_ext_completion(start, ext, items, incomplete, cx)).is_err()
                {
                    return;
                }
            }
        }));
    }

    /// The user moved on: the request is cancelled and its late answers have nowhere to go.
    pub(crate) fn end_ext_completion(&mut self) {
        self.ext.lang.session = None;
        self.ext.lang.task = None;
    }

    /// An extension answered for the word at `start`: its items join the list if the user is still on that word.
    fn add_ext_completion(&mut self, start: usize, ext: String, items: Vec<crate::lsp::CompletionItem>, incomplete: bool, cx: &mut Context<Self>) {
        let Some(session) = self.ext.lang.session.as_mut().filter(|s| s.start == start) else { return };
        session.incomplete |= incomplete;
        let items: Vec<_> = items.into_iter().map(|mut i| (i.source = Some(ext.clone()), i).1).collect();
        if items.is_empty() {
            return;
        }
        session.items.extend(items.iter().cloned());
        let cursor = self.buffer.cursor();
        if cursor < start || self.word_start(cursor) != start {
            return;
        }
        match &mut self.completion {
            Some(c) if c.start == start => c.items.extend(items),
            Some(_) => return,
            None => self.completion = Some(Completion { items, shown: Vec::new(), selected: 0, start, incomplete: false, prefix: String::new() }),
        }
        self.refilter_keeping_selection(cx);
    }

    /// The list opens with `items` (from the language server, the schema or the document's words) at `start`, with what
    /// the extensions already answered for that word.
    pub(crate) fn open_completion(&mut self, mut items: Vec<crate::lsp::CompletionItem>, start: usize, incomplete: bool, cx: &mut Context<Self>) {
        if let Some(s) = self.ext.lang.session.as_ref().filter(|s| s.start == start) {
            items.extend(s.items.iter().cloned());
        }
        self.completion = (!items.is_empty()).then(|| Completion { items, shown: Vec::new(), selected: 0, start, incomplete, prefix: String::new() });
        self.refilter(cx);
    }

    /// Filter again after items were added, keeping the selected item selected (an answer may arrive while the user
    /// moves through the list).
    fn refilter_keeping_selection(&mut self, cx: &mut Context<Self>) {
        let before = self.completion.as_ref().and_then(|c| c.shown.get(c.selected).copied());
        self.refilter(cx);
        if let (Some(before), Some(c)) = (before, self.completion.as_mut())
            && let Some(pos) = c.shown.iter().position(|&i| i == before)
        {
            c.selected = pos;
        }
    }

    /// Remove what extensions that are gone showed (`keep` says which stay): their completion items and hover text.
    pub fn drop_ext_items(&mut self, keep: &dyn Fn(&str) -> bool, cx: &mut Context<Self>) {
        let gone = |i: &crate::lsp::CompletionItem| i.source.as_deref().is_some_and(|s| !keep(s));
        if let Some(s) = &mut self.ext.lang.session {
            s.items.retain(|i| !gone(i));
        }
        let mut changed = false;
        if let Some(c) = &mut self.completion
            && c.items.iter().any(gone)
        {
            c.items.retain(|i| !gone(i));
            changed = true;
        }
        if changed {
            if self.completion.as_ref().is_some_and(|c| c.items.is_empty()) {
                self.completion = None;
            } else {
                self.refilter(cx);
            }
        }
        if let Some(h) = &mut self.hover
            && h.ext.iter().any(|(e, _)| !keep(e))
        {
            h.ext.retain(|(e, _)| keep(e));
            if h.ext.is_empty() && h.text.is_none() && h.diagnostics.is_empty() {
                self.hover = None;
            }
            changed = true;
        }
        if changed {
            cx.notify();
        }
    }

    /// The extension hovers shown: (extension, text).
    #[cfg(test)]
    pub fn ext_hovers_for_test(&self) -> Vec<(String, String)> {
        self.hover.as_ref().map(|h| h.ext.clone()).unwrap_or_default()
    }

    /// The completion list's items with the extension that offered each (`None`: the language server or the document).
    #[cfg(test)]
    pub fn completion_sources_for_test(&self) -> Option<Vec<(String, Option<String>)>> {
        self.completion.as_ref().map(|c| c.shown.iter().map(|&i| (c.items[i].label.clone(), c.items[i].source.clone())).collect())
    }

    #[cfg(test)]
    pub fn set_cursor_for_test(&mut self, ix: usize, cx: &mut Context<Self>) {
        self.buffer.set_cursor(ix, false);
        cx.notify();
    }

    /// Show the hover for the character at `ix` as Show Hover does, without needing the text laid out.
    #[cfg(test)]
    pub fn hover_at_for_test(&mut self, ix: usize, cx: &mut Context<Self>) {
        self.hover = None;
        self.schedule_hover_at(ix, Point::default(), cx);
    }
}
