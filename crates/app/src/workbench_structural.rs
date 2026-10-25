//! Workbench: structural search and replace. The commands open the Search panel in structural mode; Replace checks that
//! the files would still parse, asks, and then writes through `apply_file_edits`, the same path Replace in Files takes.

use std::collections::HashMap;

use syntax::structural as st;

use super::*;
use crate::structural as engine;

impl Workbench {
    pub(super) fn structural_search(&mut self, _: &crate::search::StructuralSearch, window: &mut Window, cx: &mut Context<Self>) {
        self.open_structural(false, window, cx);
    }

    pub(super) fn structural_replace(&mut self, _: &crate::search::StructuralReplace, window: &mut Window, cx: &mut Context<Self>) {
        self.open_structural(true, window, cx);
    }

    /// The pattern starts in the language of the active file, with the selection (if any) as the pattern.
    fn open_structural(&mut self, replace: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.chrome.panel = Some(Panel::Search);
        let (family, selection) = match self.active_editor() {
            Some(e) => {
                let e = e.read(cx);
                (st::family_for_path(&e.path), e.selected_text())
            }
            None => (None, None),
        };
        self.search.update(cx, |s, cx| s.open_structural(family, selection, replace, window, cx));
        cx.notify();
    }

    /// The panel asks for the unsaved text of the open files it will parse; their disk copies may differ.
    pub(super) fn structural_buffers(&mut self, generation: u32, family: &str, cx: &mut Context<Self>) {
        let Some(family) = st::family(family) else { return };
        let open: HashMap<String, String> = self
            .all_editors()
            .into_iter()
            .filter_map(|e| {
                let e = e.read(cx);
                (e.is_dirty() && family.language_of(&e.path).is_some()).then(|| (e.path.clone(), e.buffer_text()))
            })
            .collect();
        self.search.update(cx, |s, cx| s.run_structural(generation, open, cx));
    }

    /// Files whose result would not parse: the check runs on the text the replacement would be applied to (the unsaved
    /// text for open files, a fresh read for the others).
    fn broken_after_replace(
        &mut self,
        files: &[String],
        family: &'static st::Family,
        pattern: Arc<st::Pattern>,
        template: String,
        cx: &mut Context<Self>,
    ) -> gpui::Task<Vec<String>> {
        let mut open: HashMap<String, String> = HashMap::new();
        for e in self.all_editors() {
            let e = e.read(cx);
            if files.contains(&e.path) {
                open.insert(e.path.clone(), e.buffer_text());
            }
        }
        let (host, files) = (self.remote.host.clone(), files.to_vec());
        cx.background_executor().spawn(async move {
            files
                .into_iter()
                .filter(|path| {
                    let Some(lang) = family.language_of(path) else { return false };
                    let text = match open.get(path) {
                        Some(t) => Some(t.clone()),
                        None => host.read(path).ok().and_then(|(data, _)| engine::editor_text(&data)),
                    };
                    text.is_some_and(|t| engine::result_parses(&pattern, lang, &t, &template) == Some(false))
                })
                .collect()
        })
    }

    /// Replace from the Search panel: only files under the opened folders are touched, the pattern is compiled again from
    /// what the panel holds, and a file whose result would not parse is offered for skipping before anything is written.
    pub(crate) fn confirm_structural_replace(
        &mut self,
        files: Vec<String>,
        family: &'static str,
        pattern: String,
        replacement: String,
        matches: u32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(family) = st::family(family) else { return };
        let compiled = match st::compile(family.language(), &pattern) {
            Ok(p) => Arc::new(p),
            Err(e) => {
                self.notice = Some(Notice::Error(crate::search::describe_pattern_error(&pattern, &e).into()));
                cx.notify();
                return;
            }
        };
        let files: Vec<String> = files.into_iter().filter(|f| self.root_of(f, cx).is_some()).collect();
        if files.is_empty() {
            return;
        }
        let check = self.broken_after_replace(&files, family, compiled.clone(), replacement.clone(), cx);
        cx.spawn_in(window, async move |this, cx| {
            let broken = check.await;
            this.update_in(cx, |this, window, cx| this.ask_structural_replace(files, broken, family, compiled, replacement, matches, window, cx)).ok();
        })
        .detach();
    }

    fn ask_structural_replace(
        &mut self,
        files: Vec<String>,
        broken: Vec<String>,
        family: &'static st::Family,
        pattern: Arc<st::Pattern>,
        replacement: String,
        matches: u32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let n = files.len();
        let (m, r) = (matches as usize, &replacement);
        let title = if n == 1 {
            crate::i18n::tn("Replace {0} match in 1 file with “{1}”?", "Replace {0} matches in 1 file with “{1}”?", m, &[r])
        } else {
            crate::i18n::tn("Replace {0} match in {1} files with “{2}”?", "Replace {0} matches in {1} files with “{2}”?", m, &[&n, r])
        };
        // What each button does, in the order they are shown; the last one always cancels.
        #[derive(Clone, Copy, PartialEq)]
        enum Choice {
            All,
            SkipBroken,
            Cancel,
        }
        let (detail, choices) = if broken.is_empty() {
            (crate::i18n::t("Open files can be undone with ⌘Z; closed files are written to disk.").to_string(), vec![(Choice::All, crate::i18n::t("Replace"))])
        } else {
            let names: Vec<&str> = broken.iter().take(5).map(|p| ui::file_name(p)).collect();
            let list = if broken.len() > 5 { format!("{}, …", names.join(", ")) } else { names.join(", ") };
            let detail = crate::i18n::tn("The result would not parse in {0} file: {1}", "The result would not parse in {0} files: {1}", broken.len(), &[&list]);
            let mut choices = Vec::new();
            if broken.len() < n {
                choices.push((Choice::SkipBroken, crate::i18n::t("Skip Those Files")));
            }
            choices.push((Choice::All, crate::i18n::t("Replace Anyway")));
            (detail, choices)
        };
        let mut choices = choices;
        choices.push((Choice::Cancel, crate::i18n::t("Cancel")));
        let labels: Vec<&str> = choices.iter().map(|(_, l)| *l).collect();
        let answer = window.prompt(PromptLevel::Warning, &title, Some(&detail), &labels, cx);
        cx.spawn_in(window, async move |this, cx| {
            let files = match answer.await.ok().and_then(|i| choices.get(i)).map(|(c, _)| *c) {
                Some(Choice::All) => files,
                Some(Choice::SkipBroken) => files.into_iter().filter(|f| !broken.contains(f)).collect(),
                _ => return,
            };
            this.update(cx, |this, cx| this.replace_structural_now(files, family, pattern, replacement, cx)).ok();
        })
        .detach();
    }

    /// Write the replacements, no questions asked (the caller has asked).
    pub(crate) fn replace_structural_now(
        &mut self,
        files: Vec<String>,
        family: &'static st::Family,
        pattern: Arc<st::Pattern>,
        template: String,
        cx: &mut Context<Self>,
    ) {
        if files.is_empty() {
            return;
        }
        let edits_for: super::replace_glue::EditsFor = Arc::new(move |path, b| match family.language_of(path) {
            Some(lang) => engine::buffer_edits(&pattern, lang, b, &template),
            None => Vec::new(),
        });
        self.apply_file_edits(files, edits_for, cx);
    }
}
