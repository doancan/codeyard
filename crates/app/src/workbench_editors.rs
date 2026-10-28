//! Workbench tab and file commands: close all/others, next/previous
//! tab, keep preview, copy path, compare two files, new folder,
//! refresh tree, close folder.

use std::sync::Arc;

use gpui::{AppContext as _, Context, Focusable, Window};
use host::Host;

use super::{EditorOpt, Notice, OpenWithKind, OverlayKind, PickItem, PickSource, Picker, Settings, Workbench, editor, parent_of};
use crate::ui;

impl Workbench {
    pub(super) fn open_file(&mut self, path: String, window: &mut Window, cx: &mut Context<Self>) {
        let host = self.remote.host.clone();
        self.open_file_on(host, path, window, cx);
    }

    pub(super) fn open_file_on(&mut self, host: Arc<dyn Host>, path: String, window: &mut Window, cx: &mut Context<Self>) {
        self.open_file_as(host, path, false, window, cx);
    }

    /// `preview`: preview tab. Opens in place of a non-dirty preview tab if there is one
    /// (so tabs don't pile up while browsing).
    pub(super) fn open_file_as(&mut self, host: Arc<dyn Host>, path: String, preview: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.chrome.panel_max = false;
        let path = if host.is_remote() { path } else { ui::local_path(std::path::Path::new(&path)) };
        if let Some(format) = crate::large::image_format(&path) {
            return self.open_image(path, format, window, cx);
        }
        if self.active_editor().is_some_and(|e| e.read(cx).path != path) {
            self.record_nav(cx);
        }
        // If open in the other group, the same document shows in this group too (no second copy is loaded).
        if !self.tabs.editors.iter().any(|e| e.read(cx).path == path)
            && let Some(ed) = self.tabs.groups.values().flat_map(|g| &g.editors).find(|e| e.read(cx).path == path && e.read(cx).uses_host(&host)).cloned()
        {
            ed.update(cx, |e, _| e.preview = e.preview && preview);
            self.tabs.editors.push(ed);
            let ix = self.tabs.editors.len() - 1;
            self.activate(ix, window, cx);
            return;
        }
        if let Some(ix) = self.tabs.editors.iter().position(|e| e.read(cx).path == path) {
            if !preview {
                self.tabs.editors[ix].update(cx, |e, _| e.preview = false);
            }
            self.activate(ix, window, cx);
            return;
        }
        self.tabs.opening = Some(path.clone());
        self.tabs.open_error = None;
        let editor_host = host.clone();
        let group_key = self.tabs.current_key();
        let workspace_host = self.remote.host.clone();
        let p = path.clone();
        let fallback = cx.global::<Settings>().fallback_encoding;
        let task = cx.background_executor().spawn(async move { editor::load(host.as_ref(), &p, fallback) });
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            this.update_in(cx, |this, window, cx| {
                if !this.tabs.contains_key(group_key)
                    || !Arc::ptr_eq(&this.remote.host, &workspace_host)
                    || this.tabs.group(group_key.id).and_then(|g| g.opening.as_deref()) != Some(path.as_str())
                {
                    return; // something else was opened meanwhile
                }
                let previous_group = this.tabs.current_key();
                this.tabs.focus(group_key);
                this.tabs.opening = None;
                let shared = this.all_editors().into_iter().find(|ed| {
                    let e = ed.read(cx);
                    e.path == path && e.uses_host(&editor_host)
                });
                if let Some(shared) = shared {
                    shared.update(cx, |e, _| e.preview &= preview);
                    let index = this.tabs.editors.iter().position(|ed| *ed == shared).unwrap_or_else(|| {
                        this.tabs.editors.push(shared);
                        this.tabs.editors.len() - 1
                    });
                    this.activate(index, window, cx);
                    if previous_group != group_key && this.tabs.contains_key(previous_group) {
                        this.focus_group_key(previous_group, window, cx);
                    }
                    return;
                }
                match result {
                    Ok(loaded) => {
                        let reuse = preview
                            .then(|| {
                                this.tabs.editors.iter().position(|e| {
                                    let e = e.read(cx);
                                    e.preview && !e.is_dirty()
                                })
                            })
                            .flatten();
                        let ed = this.add_editor(editor_host, path, loaded, window, cx);
                        ed.update(cx, |e, _| e.preview = preview);
                        // Mixed line endings collapse to one style on save (can cause a big git diff): warn beforehand.
                        let (mixed, name) = ed.read_with(cx, |e, _| (e.has_mixed_line_endings(), e.line_ending_name()));
                        if mixed {
                            this.notice = Some(Notice::Info(
                                crate::i18n::tf(
                                    "This file mixes line endings; saving writes all of them as {}, like its first line. Change it in the status bar.",
                                    &[&name],
                                )
                                .into(),
                            ));
                        }
                        let ix = match reuse {
                            Some(old) => {
                                // Take the preview tab's place (same position).
                                let new = this.tabs.editors.pop().expect("just pushed");
                                this.tabs.editors[old] = new;
                                old
                            }
                            None => this.tabs.editors.len() - 1,
                        };
                        this.activate(ix, window, cx);
                    }
                    // Too big to read in one go: read-only view loaded in pieces.
                    Err(editor::OpenError::Host(host::HostError::Op(proto::Error::TooLarge { .. }))) => {
                        this.open_large(path, window, cx);
                    }
                    Err(e) => {
                        this.tabs.open_error = Some((path, e.to_string().into()));
                        cx.notify();
                    }
                }
                if previous_group != group_key && this.tabs.contains_key(previous_group) {
                    this.focus_group_key(previous_group, window, cx);
                }
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    pub(super) fn close_all_editors(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let all: Vec<usize> = (0..self.tabs.editors.len()).collect();
        self.close_tabs(all, window, cx);
    }

    pub(super) fn close_other_editors(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let keep = self.tabs.active;
        let others: Vec<usize> = (0..self.tabs.editors.len()).filter(|i| Some(*i) != keep).collect();
        self.close_tabs(others, window, cx);
    }

    /// ⌃Tab / ⌥⌘→: next (or previous) tab in the group, wraps around at the end.
    pub(super) fn cycle_editor(&mut self, forward: bool, window: &mut Window, cx: &mut Context<Self>) {
        let n = self.tabs.editors.len();
        if n == 0 {
            return;
        }
        let cur = self.tabs.active.unwrap_or(0);
        let next = if forward { (cur + 1) % n } else { (cur + n - 1) % n };
        self.record_nav(cx);
        self.activate(next, window, cx);
    }

    /// Make the preview tab permanent (VS Code "Keep Editor").
    pub(super) fn keep_editor(&mut self, cx: &mut Context<Self>) {
        if let Some(ed) = self.active_editor() {
            ed.update(cx, |e, _| e.preview = false);
            cx.notify();
        }
    }

    pub(super) fn copy_active_path(&mut self, relative: bool, cx: &mut Context<Self>) {
        let Some(path) = self.active_editor().map(|e| e.read(cx).path.clone()) else { return };
        let text = if relative { self.rel_in_roots(&path, cx).unwrap_or(path) } else { path };
        cx.write_to_clipboard(gpui::ClipboardItem::new_string(text.clone()));
        self.notice = Some(Notice::Info(crate::i18n::tf("Copied {}", &[&text]).into()));
        cx.notify();
    }

    /// Compare the active file side by side with another file picked in ⌘P.
    pub(super) fn compare_active_with(&mut self, other: String, window: &mut Window, cx: &mut Context<Self>) {
        let Some(ed) = self.active_editor().cloned() else { return };
        let (path, current) = {
            let e = ed.read(cx);
            (e.path.clone(), e.text_lines().join("\n"))
        };
        let host = self.remote.host.clone();
        let o = other.clone();
        let task = cx.background_executor().spawn(async move { host.read(&o).map(|(d, _)| String::from_utf8_lossy(&d).replace("\r\n", "\n")) });
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            this.update_in(cx, |this, window, cx| match result {
                Ok(text) => {
                    let title = format!("{} ↔ {}", ui::file_name(&other), ui::file_name(&path));
                    let view = cx.new(|cx| crate::diff::DiffView::new(title, false, &text, &current, cx).with_label(crate::i18n::t("Compare")));
                    window.focus(&view.focus_handle(cx));
                    this.tabs.diff = Some(view);
                    this.tabs.diff_active = true;
                    this.tabs.page_active = false;
                    this.tabs.large_active = false;
                    cx.notify();
                }
                Err(e) => {
                    this.notice = Some(Notice::Error(crate::i18n::tf("Could not read {0}: {1}", &[&other, &e]).into()));
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    /// Compare two files side by side (tree "Select for Compare" → "Compare with").
    pub(super) fn compare_paths(&mut self, a: String, b: String, window: &mut Window, cx: &mut Context<Self>) {
        let host = self.remote.host.clone();
        let (pa, pb) = (a.clone(), b.clone());
        let task = cx.background_executor().spawn(async move {
            let read = |p: &str| host.read(p).map(|(d, _)| String::from_utf8_lossy(&d).replace("\r\n", "\n")).map_err(|e| format!("{p}: {e}"));
            Ok::<_, String>((read(&pa)?, read(&pb)?))
        });
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            this.update_in(cx, |this, window, cx| match result {
                Ok((left, right)) => {
                    let title = format!("{} ↔ {}", ui::file_name(&a), ui::file_name(&b));
                    let view = cx.new(|cx| crate::diff::DiffView::new(title, false, &left, &right, cx).with_label(crate::i18n::t("Compare")));
                    window.focus(&view.focus_handle(cx));
                    this.tabs.diff = Some(view);
                    this.tabs.diff_active = true;
                    this.tabs.page_active = false;
                    this.tabs.large_active = false;
                    cx.notify();
                }
                Err(e) => {
                    this.notice = Some(Notice::Error(crate::i18n::tf("Could not compare: {}", &[&e]).into()));
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    /// New folder: in the active file's folder (or the root if none).
    pub(super) fn new_folder(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(root) = self.root(cx) else { return };
        let roots = self.all_roots(cx);
        let base = self
            .target_dir_any(cx)
            .or_else(|| self.active_editor().map(|e| parent_of(&e.read(cx).path)))
            .filter(|d| roots.iter().any(|r| d.starts_with(r.as_str())))
            .or_else(|| self.active_root(cx))
            .unwrap_or(root);
        self.start_create(base, true, window, cx);
    }

    pub(super) fn refresh_explorer(&mut self, cx: &mut Context<Self>) {
        self.roots.file_index = None;
        self.roots.primary.update(cx, |t, cx| t.reload_all(cx));
        self.refresh_git(0, cx);
    }

    /// Close the folder: back to the welcome screen (tabs are saved to the session).
    pub(super) fn close_folder(&mut self, cx: &mut Context<Self>) {
        if self.root(cx).is_none() {
            return;
        }
        self.flush_backups(cx);
        if (!cx.global::<crate::settings::Settings>().hot_exit || self.has_unbacked_dirty(cx)) && self.refuse_if_dirty(cx) {
            return;
        }
        self.persist_session(cx);
        self.invalidate_file_operation();
        self.shutdown_lsp(cx);
        self.persist.backed_up.clear();
        self.tabs = Default::default();
        self.tabs.active = None;
        self.tabs.diff = None;
        self.tabs.large = None;
        self.vcs.status = None;
        self.vcs.branch = None;
        self.roots.file_index = None;
        self.persist.workspace_id = None;
        let host = self.remote.host.clone();
        self.roots.primary.update(cx, |t, cx| t.set_root(host.clone(), None, cx));
        self.search.update(cx, |s, cx| s.set_context(host, None, cx));
        self.reload_recent(cx);
    }

    /// Option list (end-of-line and indentation in the status bar).
    pub(super) fn show_options(&mut self, title: &str, opts: Vec<(EditorOpt, String, Option<String>)>, window: &mut Window, cx: &mut Context<Self>) {
        if self.active_editor().is_none() {
            return;
        }
        if self.overlay.is_some() {
            self.close_overlay(window, cx);
        }
        let items = opts.iter().map(|(_, l, d)| PickItem { label: l.clone().into(), detail: d.clone().map(Into::into), keys: None }).collect();
        let picker = cx.new(|cx| Picker::new(vec![PickSource::List { prefix: "", items }], title, "", None, cx));
        self.show_overlay(picker, OverlayKind::EditorOption(opts.into_iter().map(|(o, ..)| o).collect()), window, cx);
    }

    /// "Open With…": open the same file in another view.
    pub(super) fn show_open_with(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let opts = vec![
            (EditorOpt::OpenWith(OpenWithKind::Hex), crate::i18n::t("Hex dump (read-only copy)").to_string(), Some(crate::i18n::t("binary").to_string())),
            (EditorOpt::OpenWith(OpenWithKind::MarkdownPreview), crate::i18n::t("Markdown preview").to_string(), Some(".md".to_string())),
            (
                EditorOpt::OpenWith(OpenWithKind::CompareSaved),
                crate::i18n::t("Compare with the saved version").to_string(),
                Some(crate::i18n::t("diff").to_string()),
            ),
        ];
        self.show_options(crate::i18n::t("Open the active file with…"), opts, window, cx);
    }

    /// Add `*.ext → language` to `files.associations` (existing kept) and reopen the active editor.
    pub(super) fn associate_extension(&mut self, lang_ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(path) = self.active_editor().map(|e| e.read(cx).path.clone()) else { return };
        let Some(ext) = path.rsplit_once('.').map(|(_, x)| x.to_ascii_lowercase()).filter(|x| !x.is_empty() && !x.contains('/')) else { return };
        let lang = editor::LANGUAGES[lang_ix].1;
        let glob = format!("*.{ext}");
        let mut map = serde_json::Map::new();
        for (g, l) in cx.global::<Settings>().file_associations.iter().filter(|(g, _)| *g != glob) {
            map.insert(g.clone(), serde_json::Value::String(l.clone()));
        }
        map.insert(glob, serde_json::Value::String(lang.to_string()));
        self.write_setting("files.associations", serde_json::Value::Object(map), window, cx);
    }

    /// "Change Language Mode": this file's language, or a persistent association for its extension.
    pub(super) fn show_language_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(ed) = self.active_editor().cloned() else { return };
        let (current, ext) = {
            let e = ed.read(cx);
            (e.language(), e.path.rsplit_once('.').map(|(_, x)| x.to_ascii_lowercase()).filter(|x| !x.is_empty() && !x.contains('/')))
        };
        let mark = |l: &str| (current == l).then(|| crate::i18n::t("current").to_string());
        let mut opts: Vec<(EditorOpt, String, Option<String>)> =
            editor::LANGUAGES.iter().map(|&(name, ext)| (EditorOpt::Language(Some(ext)), name.to_string(), mark(name))).collect();
        opts.push((EditorOpt::Language(None), crate::i18n::t("Plain Text").to_string(), mark("Plain Text")));
        if let Some(x) = ext.as_deref() {
            for (i, &(name, _)) in editor::LANGUAGES.iter().enumerate() {
                opts.push((EditorOpt::Associate(i), crate::i18n::tf("Always open *.{0} as {1}", &[&x, &name]), Some(crate::i18n::t("setting").into())));
            }
        }
        self.show_options(crate::i18n::t("Language mode for this file"), opts, window, cx);
    }

    pub(super) fn show_eol_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let current = self.active_editor().map(|e| e.read(cx).line_ending());
        let mark = |l: &str| (current == Some(l)).then(|| crate::i18n::t("current").to_string());
        let opts = vec![
            (EditorOpt::Eol { crlf: false }, "LF (macOS, Linux)".to_string(), mark("LF")),
            (EditorOpt::Eol { crlf: true }, "CRLF (Windows)".to_string(), mark("CRLF")),
        ];
        self.show_options(crate::i18n::t("Line endings for this file (applied on save)"), opts, window, cx);
    }

    pub(super) fn show_indent_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let current = self.active_editor().map(|e| e.read(cx).indent_key());
        let mark = |l: &str| (current.as_deref() == Some(l)).then(|| crate::i18n::t("current").to_string());
        let opts = vec![
            (EditorOpt::Indent(Some(2)), crate::i18n::t("Indent using 2 spaces").to_string(), mark("2 spaces")),
            (EditorOpt::Indent(Some(4)), crate::i18n::t("Indent using 4 spaces").to_string(), mark("4 spaces")),
            (EditorOpt::Indent(Some(8)), crate::i18n::t("Indent using 8 spaces").to_string(), mark("8 spaces")),
            (EditorOpt::Indent(None), crate::i18n::t("Indent using tabs").to_string(), mark("Tabs")),
            (EditorOpt::Convert { to_tabs: false }, crate::i18n::t("Convert indentation to spaces").to_string(), Some(crate::i18n::t("whole file").into())),
            (EditorOpt::Convert { to_tabs: true }, crate::i18n::t("Convert indentation to tabs").to_string(), Some(crate::i18n::t("whole file").into())),
        ];
        self.show_options(crate::i18n::t("Indentation for this file"), opts, window, cx);
    }
}
