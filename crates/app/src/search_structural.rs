//! Search panel: structural mode. The pattern is code in a chosen language (`$X` one node, `$$$X` several); results list
//! like any other search and Replace goes through the workbench's confirmation, like Replace in Files. The matching itself
//! is in `structural.rs`; this file is the panel's state and its glue.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};

use syntax::structural::{self as st, Family, PatternError, PatternErrorKind};

use super::*;
use crate::structural as engine;

pub(super) struct State {
    pub on: bool,
    pub family: &'static Family,
    /// Multi-line: Python and Go patterns need line breaks (⇧↵).
    pub pattern: Entity<TextField>,
    /// Why the pattern does not run; shown under the field.
    pub error: Option<String>,
    /// Bumped by every run; the workbench's answer with the unsaved buffers carries the number it was asked for.
    pub generation: u32,
    pub cancel: Option<Arc<AtomicBool>>,
    pub totals: engine::Totals,
}

impl State {
    pub fn new(cx: &mut Context<SearchPanel>) -> Self {
        State {
            on: false,
            family: st::family("Rust").expect("Rust is a family"),
            pattern: cx.new(|cx| TextField::new(crate::i18n::tr("Pattern, e.g. foo($X, $$$REST)"), cx).multiline()),
            error: None,
            generation: 0,
            cancel: None,
            totals: engine::Totals::default(),
        }
    }
}

/// Line and column (1-based, in characters) of a byte offset of the pattern.
fn line_col(text: &str, at: usize) -> (usize, usize) {
    let before = &text[..at.min(text.len())];
    (before.matches('\n').count() + 1, before.rsplit('\n').next().map_or(0, |l| l.chars().count()) + 1)
}

/// What is wrong with the pattern, in words; the position is part of the message so the user can find it.
pub(crate) fn describe(pattern: &str, e: &PatternError) -> String {
    let (line, col) = line_col(pattern, e.span.start);
    let near = pattern.get(e.span.clone()).unwrap_or_default();
    let what = match &e.kind {
        PatternErrorKind::Empty => return String::new(),
        PatternErrorKind::Syntax => crate::i18n::tf("syntax error at “{}”", &[&near]),
        PatternErrorKind::Missing(tok) => crate::i18n::tf("“{}” is missing here", &[tok]),
        PatternErrorKind::NotOneNode => return crate::i18n::t("A pattern is one expression or statement, not several.").to_string(),
        PatternErrorKind::OnlyMetavariable => return crate::i18n::t("A pattern that is only a variable would match everything.").to_string(),
    };
    crate::i18n::tf("The pattern does not parse: {0} (line {1}, column {2})", &[&what, &line, &col])
}

impl SearchPanel {
    /// The text field the user types the search into.
    pub(super) fn active_query(&self) -> &Entity<TextField> {
        if self.st.on { &self.st.pattern } else { &self.query }
    }

    /// "Search: Structural Search…" / "Structural Replace…": switch the panel to structural mode.
    pub fn open_structural(&mut self, family: Option<&'static Family>, prefill: Option<String>, replace: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.st.on = true;
        if let Some(f) = family {
            self.st.family = f;
        }
        if let Some(t) = prefill.filter(|t| !t.trim().is_empty() && t.len() < 2000) {
            self.st.pattern.update(cx, |p, cx| p.set_text(&t, cx));
        }
        let target = if replace { self.replace.focus_handle(cx) } else { self.st.pattern.focus_handle(cx) };
        window.focus(&target);
        self.start(cx);
    }

    /// The language of the pattern; a click goes to the next one.
    pub(super) fn language_chip(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let p = *palette(cx);
        div()
            .id("s-language")
            .flex_none()
            .h(px(20.))
            .px(px(theme::SPACE[2]))
            .flex()
            .items_center()
            .rounded(px(theme::radius::SM))
            .bg(c(p.surface_hover))
            .text_size(px(text::XXS.0))
            .text_color(c(p.text))
            .cursor_pointer()
            .hover(|d| d.bg(c(p.surface_active)))
            .tooltip(|_, cx| cx.new(|_| crate::workbench::Tooltip(crate::i18n::t("Language of the pattern (click for the next one)").into())).into())
            .relative()
            .child(self.st.family.id)
            .child(crate::a11y_controls::control(
                crate::a11y_controls::Control::button(crate::i18n::t("Search"), "s-language", self.st.family.id, |_, _| {})
                    .description(crate::i18n::t("Language of the pattern (click for the next one)"))
                    .on_entity(cx.entity().downgrade(), |this: &mut Self, _, cx| this.cycle_family(cx)),
            ))
            .on_click(cx.listener(|this, _, _, cx| this.cycle_family(cx)))
    }

    /// The header's Clear: the field, the results and the status.
    pub(super) fn clear_search(&mut self, cx: &mut Context<Self>) {
        self.clear_query(cx);
        self.results.clear();
        self.rebuild_rows();
        self.status = Status::Idle;
    }

    /// Next language in the list (the chip next to the pattern).
    pub(super) fn cycle_family(&mut self, cx: &mut Context<Self>) {
        let at = st::FAMILIES.iter().position(|f| f.id == self.st.family.id).unwrap_or(0);
        self.st.family = &st::FAMILIES[(at + 1) % st::FAMILIES.len()];
        self.start(cx);
    }

    pub(super) fn cancel_structural(&mut self) {
        if let Some(c) = self.st.cancel.take() {
            c.store(true, Ordering::Relaxed);
        }
    }

    pub(super) fn compile_pattern(&self, cx: &App) -> Result<Arc<st::Pattern>, PatternError> {
        st::compile(self.st.family.language(), self.st.pattern.read(cx).text()).map(Arc::new)
    }

    /// Check the pattern, then ask the workbench for the unsaved buffers; the run starts when they arrive (`run_structural`).
    pub(super) fn start_structural(&mut self, cx: &mut Context<Self>) {
        self.stop();
        self.results.clear();
        self.current = None;
        self.rebuild_rows();
        self.st.error = None;
        self.st.totals = engine::Totals::default();
        let text = self.st.pattern.read(cx).text().to_string();
        if text.trim().is_empty() || self.root.is_none() {
            self.status = Status::Idle;
            cx.notify();
            return;
        }
        if let Err(e) = self.compile_pattern(cx) {
            self.st.error = Some(describe(&text, &e));
            self.status = Status::Idle;
            cx.notify();
            return;
        }
        self.status = Status::Searching;
        self.st.generation = self.st.generation.wrapping_add(1);
        cx.emit(SearchEvent::StructuralBuffers { generation: self.st.generation, family: self.st.family.id });
        cx.notify();
    }

    /// The workbench's answer: unsaved text of the open files of the language. Starts the search itself.
    pub fn run_structural(&mut self, generation: u32, open: HashMap<String, String>, cx: &mut Context<Self>) {
        use futures::{FutureExt as _, StreamExt as _};
        if generation != self.st.generation || !self.st.on {
            return;
        }
        let Ok(pattern) = self.compile_pattern(cx) else { return };
        let settings = cx.global::<crate::settings::Settings>();
        let max_files = settings.search_structural_max_files;
        let globs = |f: &Entity<TextField>| -> Vec<String> { f.read(cx).text().split(',').map(|g| g.trim().to_string()).filter(|g| !g.is_empty()).collect() };
        let include = if self.show_globs { globs(&self.include) } else { Vec::new() };
        let mut exclude = if self.show_globs { globs(&self.exclude) } else { Vec::new() };
        exclude.extend(settings.files_exclude.iter().cloned());
        exclude.extend(settings.search_exclude.iter().cloned());

        let mut dirs: Vec<(Option<String>, String)> = vec![(None, self.root.clone().unwrap_or_default())];
        dirs.extend(self.extra_roots.iter().map(|e| (Some(ui::file_name(e).to_string()), e.clone())));
        let (mut roots, mut ids) = (Vec::new(), Vec::new());
        for (label, dir) in dirs {
            let q = engine::narrowing_query(&dir, &pattern, self.st.family, include.clone(), exclude.clone());
            match self.host.search(q) {
                Ok(stream) => {
                    ids.push(stream.id);
                    roots.push(engine::Root { label, dir, stream });
                }
                Err(e) => {
                    for id in ids {
                        self.host.cancel(id);
                    }
                    self.status = Status::Failed(e.to_string().into());
                    cx.notify();
                    return;
                }
            }
        }
        let cancel = Arc::new(AtomicBool::new(false));
        self.st.cancel = Some(cancel.clone());
        let job = engine::Job { family: self.st.family, pattern, max_files, open };
        let (tx, mut rx) = futures::channel::mpsc::unbounded();
        let host = self.host.clone();
        let work = cx.background_executor().spawn(async move {
            engine::scan(host, job, roots, &cancel, |ev| {
                tx.unbounded_send(ev).ok();
            });
        });
        let task = cx.spawn(async move |this, cx| {
            let _work = work; // dropped with this task
            while let Some(first) = rx.next().await {
                let mut batch = vec![first];
                while let Some(Some(more)) = rx.next().now_or_never() {
                    batch.push(more);
                }
                let keep_going = this
                    .update(cx, |this, cx| {
                        let mut more = true;
                        for ev in batch {
                            match ev {
                                engine::Event::Found(f) => {
                                    let mode = cx.global::<crate::settings::Settings>().search_collapse.clone();
                                    this.add(f, &mode);
                                }
                                engine::Event::Done(t) => {
                                    this.st.totals = t;
                                    this.status = Status::Done { files: t.files, matches: t.matches, truncated: t.narrowing_cut };
                                    this.running = None;
                                    more = false;
                                }
                                engine::Event::Failed(e) => {
                                    this.status = Status::Failed(e.into());
                                    this.stop();
                                    more = false;
                                }
                            }
                        }
                        this.rebuild_rows();
                        cx.notify();
                        more
                    })
                    .unwrap_or(false);
                if !keep_going {
                    break;
                }
            }
        });
        self.running = Some((ids, task));
        cx.notify();
    }

    /// Second line of the status: what was left out and why.
    pub(super) fn structural_note(&self, cx: &App) -> Option<String> {
        let t = &self.st.totals;
        let mut parts = Vec::new();
        if t.skipped_parse > 0 {
            parts.push(crate::i18n::tn("{} file skipped, parse errors", "{} files skipped, parse errors", t.skipped_parse as usize, &[]));
        }
        if t.skipped_other > 0 {
            parts.push(crate::i18n::tn("{} file skipped, binary or too large", "{} files skipped, binary or too large", t.skipped_other as usize, &[]));
        }
        if t.candidates_cut {
            parts.push(crate::i18n::tf(
                "only the first {} files were looked at (search.structural.maxFiles)",
                &[&cx.global::<crate::settings::Settings>().search_structural_max_files],
            ));
        }
        (!parts.is_empty()).then(|| parts.join("; "))
    }

    /// Replace in the listed files (all of them, or the one at `only`); the workbench confirms and writes.
    pub(super) fn request_structural_replace(&mut self, only: Option<usize>, cx: &mut Context<Self>) {
        let Status::Done { .. } = self.status else { return };
        let pick: Vec<&FileResult> = match only {
            Some(i) => self.results.get(i).into_iter().collect(),
            None => self.results.iter().collect(),
        };
        if pick.is_empty() {
            return;
        }
        let files = pick.iter().map(|f| self.abs(&f.path)).collect();
        let matches = pick.iter().map(|f| f.matches.len() as u32).sum();
        cx.emit(SearchEvent::StructuralReplace {
            files,
            family: self.st.family.id,
            pattern: self.st.pattern.read(cx).text().to_string(),
            replacement: self.replace.read(cx).text().to_string(),
            matches,
        });
    }

    #[cfg(test)]
    pub fn structural_for_test(&self) -> (bool, Option<String>, engine::Totals) {
        (self.st.on, self.st.error.clone(), self.st.totals)
    }

    #[cfg(test)]
    pub fn set_pattern_for_test(&mut self, text: &str, cx: &mut Context<Self>) {
        self.st.pattern.update(cx, |p, cx| p.set_text(text, cx));
    }

    #[cfg(test)]
    pub fn set_family_for_test(&mut self, id: &str) {
        self.st.family = st::family(id).expect("known language");
    }

    #[cfg(test)]
    pub fn status_line_for_test(&self, cx: &App) -> Option<String> {
        self.structural_note(cx)
    }
}
