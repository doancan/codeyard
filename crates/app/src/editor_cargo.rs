//! Editor: `Cargo.toml` dependency intelligence (cargo_deps.rs). The workbench binds a registry to a manifest's editor
//! (`set_cargo`) and pushes the hints it computed (`set_cargo_hints`); completion and hover ask the registry in the background.
//! Nothing here touches the network on the UI thread.

use std::time::Duration;

use ext_api::value::Tone;

use super::*;
use crate::cargo_project::deps::context::{Ctx, context_at};
use crate::cargo_project::deps::hints::{self, Hint};
use crate::cargo_project::deps::items::{self, Item};
use crate::cargo_project::deps::lock::Lock;
use crate::cargo_project::deps::manifest;
use crate::cargo_project::deps::registry::{Lookup, Registry};

/// Crate-name searches wait for the typing to pause: the API asks for no more than one request a second.
const SEARCH_DEBOUNCE: Duration = Duration::from_millis(350);
/// Manifests larger than this are not analysed on every keystroke.
const MAX_MANIFEST_BYTES: usize = 512 * 1024;

/// What the workbench gives a manifest's editor.
pub struct CargoBinding {
    pub registry: Arc<Registry>,
    pub lock: Option<Arc<Lock>>,
}

#[derive(Default)]
pub struct CargoMarks {
    binding: Option<Arc<CargoBinding>>,
    hints: Vec<Hint>,
    /// The document's line count when the hints were computed: once it changes they are stale and hidden.
    lines: usize,
}

/// Everything needed to answer a hover in the background.
pub(super) struct CargoHoverInput {
    binding: Arc<CargoBinding>,
    text: String,
    offset: usize,
}

impl CargoHoverInput {
    pub(super) fn run(self) -> Option<String> {
        let manifest = manifest::parse(&self.text).ok()?;
        let dep = manifest.dep_at(self.offset)?;
        let lock = self.binding.lock.as_deref();
        if !dep.is_registry_lookup() {
            return Some(hints::hover_text(
                dep,
                &Lookup::Failed(String::new()),
                Err(&crate::cargo_project::deps::registry::Failure::Unavailable),
                lock,
                self.offset,
            ));
        }
        let lookup = self.binding.registry.crate_info(&dep.package);
        let on_feature = dep.features.iter().any(|f| f.span.contains(&self.offset) || f.span.end == self.offset);
        let meta = match (&lookup, on_feature) {
            (Lookup::Found { .. }, false) => self.binding.registry.meta(&dep.package),
            _ => Err(crate::cargo_project::deps::registry::Failure::Unavailable),
        };
        Some(hints::hover_text(dep, &lookup, meta.as_ref(), lock, self.offset))
    }
}

impl EditorView {
    pub fn set_cargo(&mut self, binding: Option<Arc<CargoBinding>>, cx: &mut Context<Self>) {
        if binding.is_none() && !self.cargo.hints.is_empty() {
            self.cargo.hints.clear();
            cx.notify();
        }
        self.cargo.binding = binding;
    }

    pub fn has_cargo(&self) -> bool {
        self.cargo.binding.is_some()
    }

    /// The hover text at character `ix`, computed on the spot.
    #[cfg(test)]
    pub fn cargo_hover_for_test(&self, ix: usize) -> Option<String> {
        self.cargo_hover_input(ix)?.run()
    }

    pub fn cargo_registry(&self) -> Option<Arc<Registry>> {
        self.cargo.binding.as_ref().map(|b| b.registry.clone())
    }

    /// The text, its version and the cursor's byte offset: the input of an edit computed off the document.
    pub fn cargo_snapshot(&self) -> (String, u64, usize) {
        let rope = self.buffer.rope();
        (rope.to_string(), self.buffer.version(), rope.char_to_byte(self.buffer.cursor().min(rope.len_chars())))
    }

    /// Apply edits (byte ranges of the text at `version`) as one undo step; `false` when the document changed since.
    pub fn apply_text_edits(&mut self, version: u64, edits: &[(std::ops::Range<usize>, String)], cx: &mut Context<Self>) -> bool {
        if self.buffer.version() != version {
            return false;
        }
        let rope = self.buffer.rope();
        let parts: Vec<(std::ops::Range<usize>, String)> =
            edits.iter().map(|(r, t)| (rope.byte_to_char(r.start)..rope.byte_to_char(r.end), t.clone())).collect();
        self.buffer.apply_edits(parts);
        self.after_edit(cx);
        true
    }

    /// Hints computed for a document of `lines` lines.
    pub fn set_cargo_hints(&mut self, hints: Vec<Hint>, lines: usize, cx: &mut Context<Self>) {
        if self.cargo.hints != hints || self.cargo.lines != lines {
            self.cargo.hints = hints;
            self.cargo.lines = lines;
            cx.notify();
        }
    }

    /// The hints shown now (none while the text has more or fewer lines than when they were computed).
    pub fn cargo_hints(&self) -> &[Hint] {
        if self.cargo.lines == self.buffer.len_lines() { &self.cargo.hints } else { &[] }
    }

    fn cargo_hint_on(&self, line: usize) -> Option<&Hint> {
        self.cargo_hints().iter().find(|h| h.line == line)
    }

    /// Text after the end of `line`, with its tone and hover text (`editor_ext.rs::ext_after` falls back to it).
    pub(super) fn cargo_after(&self, line: usize) -> Option<(String, Tone, Option<String>)> {
        self.cargo_hint_on(line).map(|h| (h.text.clone(), h.tone, None))
    }

    /// The sentence behind a hint, for its tooltip and the accessibility tree.
    pub(crate) fn cargo_detail(&self, line: usize) -> Option<String> {
        self.cargo_hint_on(line).map(|h| h.detail.clone())
    }

    pub(super) fn cargo_hover_input(&self, ix: usize) -> Option<CargoHoverInput> {
        let binding = self.cargo.binding.clone()?;
        let rope = self.buffer.rope();
        (rope.len_bytes() <= MAX_MANIFEST_BYTES).then(|| CargoHoverInput {
            binding,
            text: rope.to_string(),
            offset: rope.char_to_byte(ix.min(rope.len_chars())),
        })
    }

    /// As you type in a manifest: open, filter or close the suggestions.
    pub(super) fn after_typed_cargo(&mut self, typed: &str, cx: &mut Context<Self>) {
        let mut chars = typed.chars();
        let (Some(ch), None) = (chars.next(), chars.next()) else {
            self.completion = None;
            return;
        };
        if self.buffer.cursor_count() > 1 || !cx.global::<Settings>().quick_suggestions {
            self.completion = None;
            return;
        }
        if !(Self::is_word(ch) || matches!(ch, '"' | '\'' | '.' | '-' | '+')) {
            self.completion = None;
            return;
        }
        let rope = self.buffer.rope();
        if rope.len_bytes() > MAX_MANIFEST_BYTES {
            return;
        }
        let cursor = self.buffer.cursor();
        let ctx = context_at(&rope.to_string(), rope.char_to_byte(cursor));
        match (&self.completion, ctx) {
            // Not a place the registry knows (a comment, [package], …): words from the document, as in any file.
            (_, None) => self.after_typed_words(typed, cx),
            (Some(open), Some(ctx)) if open.start == self.cargo_replace_start(&ctx) && !open.incomplete => self.refilter(cx),
            _ => self.request_cargo_completion(cx),
        }
    }

    fn cargo_replace_start(&self, ctx: &Ctx) -> usize {
        let (Ctx::CrateName { replace, .. } | Ctx::Version { replace, .. } | Ctx::Feature { replace, .. }) = ctx;
        self.buffer.rope().byte_to_char(replace.start)
    }

    pub(super) fn request_cargo_completion(&mut self, cx: &mut Context<Self>) {
        let Some(binding) = self.cargo.binding.clone() else { return };
        let rope = self.buffer.rope();
        if rope.len_bytes() > MAX_MANIFEST_BYTES {
            return;
        }
        let (text, version) = (rope.to_string(), self.buffer.version());
        let Some(ctx) = context_at(&text, rope.char_to_byte(self.buffer.cursor())) else {
            if cx.global::<Settings>().word_based_suggestions {
                self.request_word_completion(cx);
            }
            return;
        };
        let (Ctx::CrateName { replace, .. } | Ctx::Version { replace, .. } | Ctx::Feature { replace, .. }) = &ctx;
        let start = text[..replace.start].chars().count();
        let range = (schema::doc::position(&text, replace.start), schema::doc::position(&text, replace.end));
        let searching = matches!(ctx, Ctx::CrateName { .. });
        let executor = cx.background_executor().clone();
        self.completion_task = Some(cx.spawn(async move |this, cx| {
            if searching {
                executor.timer(SEARCH_DEBOUNCE).await;
                // Typing went on while waiting: the next key asks again.
                if this.read_with(cx, |e, _| e.buffer.version()).ok() != Some(version) {
                    return;
                }
            }
            let found = executor.spawn(async move { cargo_items(&binding, &ctx, &text) }).await;
            this.update(cx, |e, cx| {
                if e.buffer.version() != version {
                    return;
                }
                let items: Vec<crate::lsp::CompletionItem> = found
                    .into_iter()
                    .map(|i| crate::lsp::CompletionItem {
                        filter: i.label.clone(),
                        label: i.label,
                        detail: i.detail,
                        kind: i.kind,
                        sort: i.sort,
                        insert: i.insert,
                        cursor: None,
                        stops: Vec::new(),
                        range: Some(range),
                        extra: Vec::new(),
                        doc: i.doc,
                        resolve: None,
                        source: None,
                    })
                    .collect();
                if items.is_empty() {
                    e.completion = None;
                    cx.notify();
                } else {
                    e.open_completion(items, start, false, cx);
                }
            })
            .ok();
        }));
    }
}

/// The suggestions for a context; blocks on the network when the crate is not cached.
fn cargo_items(binding: &CargoBinding, ctx: &Ctx, text: &str) -> Vec<Item> {
    // A renamed dependency (`json = { package = "serde_json" }`) is looked up under its real name; the document may not parse yet.
    let parsed = manifest::parse(text).ok();
    let entry = |key: &str| parsed.as_ref().and_then(|m| m.deps.iter().find(|d| d.key == key));
    let real = |key: &str| entry(key).map_or_else(|| key.to_string(), |d| d.package.clone());
    match ctx {
        Ctx::CrateName { prefix, .. } if prefix.len() >= 2 => binding.registry.search(prefix).map(|hits| items::crate_items(&hits, prefix)).unwrap_or_default(),
        Ctx::CrateName { .. } => Vec::new(),
        Ctx::Version { package, closed, .. } => match binding.registry.crate_info(&real(package)) {
            Lookup::Found { info, .. } => items::version_items(&info, *closed),
            _ => Vec::new(),
        },
        Ctx::Feature { package, closed, .. } => match binding.registry.crate_info(&real(package)) {
            Lookup::Found { info, .. } => {
                let listed: Vec<String> = entry(package).map(|d| d.features.iter().map(|f| f.name.clone()).collect()).unwrap_or_default();
                items::feature_items(&info, None, &listed, *closed)
            }
            _ => Vec::new(),
        },
    }
}
