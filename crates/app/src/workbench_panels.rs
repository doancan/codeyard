//! Workbench: bottom panels (Output, Problems, Ports), zoom, diff and large-file tabs.

use super::*;

impl Workbench {
    /// Zoom: editor and terminal font size (for the session; applied on top if the
    /// settings file changes).
    pub(super) fn apply_zoom(&mut self, delta: f32, window: &mut Window, cx: &mut Context<Self>) {
        self.chrome.zoom = if delta == 0. { 0. } else { (self.chrome.zoom + delta).clamp(-6., 16.) };
        let mut s = cx.global::<Settings>().clone();
        s.editor_font_size = (self.chrome.zoom_base.0 + self.chrome.zoom).max(6.);
        s.editor_line_height = (self.chrome.zoom_base.1 + self.chrome.zoom * 1.5).max(10.);
        s.terminal_font_size = (self.chrome.zoom_base.2 + self.chrome.zoom).max(6.);
        cx.set_global(s);
        self.notice = Some(Notice::Info(crate::i18n::tf("Zoom: {}", &[&format!("{:+}", self.chrome.zoom)]).into()));
        window.refresh();
        cx.notify();
    }

    pub(super) fn zoom_in(&mut self, _: &ZoomIn, window: &mut Window, cx: &mut Context<Self>) {
        self.apply_zoom(1., window, cx);
    }

    pub(super) fn zoom_out(&mut self, _: &ZoomOut, window: &mut Window, cx: &mut Context<Self>) {
        self.apply_zoom(-1., window, cx);
    }

    pub(super) fn zoom_reset(&mut self, _: &ZoomReset, window: &mut Window, cx: &mut Context<Self>) {
        self.apply_zoom(0., window, cx);
    }

    pub(super) fn show_ports(&mut self, _: &ShowPorts, _: &mut Window, cx: &mut Context<Self>) {
        if self.chrome.panel_open && self.chrome.bottom == Bottom::Ports {
            self.chrome.panel_open = false;
        } else {
            self.chrome.panel_open = true;
            self.chrome.bottom = Bottom::Ports;
        }
        cx.notify();
    }

    pub(super) fn show_output(&mut self, _: &ShowOutput, _: &mut Window, cx: &mut Context<Self>) {
        if self.chrome.panel_open && self.chrome.bottom == Bottom::Output {
            self.chrome.panel_open = false;
        } else {
            self.chrome.panel_open = true;
            self.chrome.bottom = Bottom::Output;
        }
        cx.notify();
    }

    /// Append a line to Output (the last 5,000 lines are kept).
    pub(super) fn log_output(&mut self, line: String) {
        if self.lsp.output.len() >= 5_000 {
            self.lsp.output.pop_front();
        }
        self.lsp.output.push_back(line);
    }

    #[cfg(test)]
    pub fn output_lines(&self) -> Vec<String> {
        self.lsp.output.iter().cloned().collect()
    }

    /// Last 400 lines, pinned to the bottom.
    pub(super) fn output_view(&self, p: &Palette) -> AnyElement {
        if self.lsp.output.is_empty() {
            return div()
                .px(px(theme::SPACE[6]))
                .py(px(theme::SPACE[3]))
                .text_color(c(p.text_muted))
                .child(crate::i18n::t("No output yet. Language server logs appear here."))
                .into_any_element();
        }
        let skip = self.lsp.output.len().saturating_sub(400);
        let rows: Vec<AnyElement> = self
            .lsp
            .output
            .iter()
            .skip(skip)
            .map(|l| {
                div()
                    .px(px(theme::SPACE[4]))
                    .whitespace_nowrap()
                    .font_family(crate::ui::MONO_FONT)
                    .text_size(px(text::XS.0))
                    .child(l.clone())
                    .into_any_element()
            })
            .collect();
        div().id("output").size_full().overflow_y_scroll().children(rows).into_any_element()
    }

    pub(super) fn show_problems(&mut self, _: &ShowProblems, _: &mut Window, cx: &mut Context<Self>) {
        // Already open: close it.
        if self.chrome.panel_open && self.chrome.bottom == Bottom::Problems {
            self.chrome.panel_open = false;
        } else {
            self.chrome.panel_open = true;
            self.chrome.bottom = Bottom::Problems;
        }
        cx.notify();
    }

    #[cfg(test)]
    pub fn problems(&self) -> Vec<(String, usize)> {
        self.all_problems().iter().map(|(p, d)| (p.clone(), d.len())).collect()
    }

    pub(super) fn problems_view(&self, p: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let all = if self.jobs.problems.is_empty() { std::borrow::Cow::Borrowed(&self.lsp.diagnostics) } else { std::borrow::Cow::Owned(self.all_problems()) };
        if all.is_empty() {
            let msg = if self.lsp.servers.is_empty() {
                crate::i18n::t("No problems. Language servers start when you open a file.")
            } else {
                crate::i18n::t("No problems.")
            };
            return div().px(px(theme::SPACE[6])).py(px(theme::SPACE[3])).text_color(c(p.text_muted)).child(msg).into_any_element();
        }
        let root = self.root(cx).unwrap_or_default();
        let mut rows: Vec<AnyElement> = Vec::new();
        for (path, diags) in all.iter() {
            let rel = path.strip_prefix(&format!("{}/", root.trim_end_matches('/'))).unwrap_or(path);
            rows.push(
                div()
                    .h(px(layout::ROW_H))
                    .flex()
                    .items_center()
                    .gap(px(theme::SPACE[2]))
                    .px(px(theme::SPACE[4]))
                    .child(crate::icon_theme::file_icon_el(ui::file_name(path), layout::ICON, p))
                    .child(ui::file_name(path).to_string())
                    .child(
                        div().text_color(c(p.text_muted)).text_size(px(text::XS.0)).child(rel.rsplit_once('/').map(|(d, _)| d.to_string()).unwrap_or_default()),
                    )
                    .child(
                        div()
                            .px(px(theme::SPACE[2]))
                            .rounded(px(theme::radius::LG))
                            .bg(c(p.surface_active))
                            .text_size(px(text::XS.0))
                            .child(diags.len().to_string()),
                    )
                    .into_any_element(),
            );
            let mut sorted: Vec<&crate::lsp::Diagnostic> = diags.iter().collect();
            sorted.sort_by_key(|d| (d.severity, d.start));
            for (i, d) in sorted.into_iter().enumerate() {
                let (glyph, color) = match d.severity {
                    1 => ("circle-x", p.danger),
                    2 => ("alert", p.warning),
                    _ => ("info", p.info),
                };
                let (path, line, col) = (path.clone(), d.start.0, d.start.1);
                let id = SharedString::from(format!("problem-{path}-{i}"));
                rows.push(
                    div()
                        .id(id)
                        .h(px(layout::ROW_H))
                        .flex()
                        .items_center()
                        .gap(px(theme::SPACE[2]))
                        .pl(px(theme::SPACE[8]))
                        .pr(px(theme::SPACE[4]))
                        .whitespace_nowrap()
                        .overflow_hidden()
                        .cursor_pointer()
                        .hover(|s| s.bg(c(p.surface_hover)))
                        .child(icon(glyph, 14., color))
                        .child(div().flex_shrink().min_w_0().overflow_hidden().child(d.message.lines().next().unwrap_or_default().to_string()))
                        .child(div().flex_none().text_size(px(text::XS.0)).text_color(c(p.text_muted)).child(format!(
                            "{} [{}:{}]",
                            d.source.as_deref().map_or("", |s| if s == "task" { crate::i18n::t("task") } else { s }),
                            line + 1,
                            col + 1
                        )))
                        .on_click(cx.listener(move |this, _, window, cx| this.open_location(path.clone(), line, col, window, cx)))
                        .into_any_element(),
                );
            }
        }
        div().id("problems").size_full().overflow_y_scroll().children(rows).into_any_element()
    }

    /// `--panel`: which sidebar is visible at startup.
    pub fn show_panel_named(&mut self, name: &str, window: &mut Window, cx: &mut Context<Self>) {
        match name {
            "scm" | "git" => self.show_scm(&ShowScm, window, cx),
            "search" => self.find_in_files(&FindInFiles, window, cx),
            "debug" => self.show_run_debug(&ShowRunAndDebug, window, cx),
            "testing" | "tests" => self.show_testing(&ShowTesting, window, cx),
            "remote" => self.show_remote_explorer(&remote_glue::ShowRemoteExplorer, window, cx),
            "welcome" => self.show_welcome(&ShowWelcome, window, cx),
            "shortcuts" => self.show_shortcuts_page(&ShowShortcutsPage, window, cx),
            "preview" => self.open_markdown_preview(&OpenMarkdownPreview, window, cx),
            "settings" => self.open_settings_page(&OpenSettingsPage, window, cx),
            _ => self.show_explorer(&ShowExplorer, window, cx),
        }
    }

    pub(super) fn show_scm(&mut self, _: &ShowScm, window: &mut Window, cx: &mut Context<Self>) {
        self.chrome.panel = Some(Panel::Scm);
        self.scm.read(cx).focus_message(window, cx);
        self.refresh_git(0, cx);
        cx.notify();
    }

    /// Open the diff tab (replacing the previous one). `staged`: HEAD ↔ index, otherwise index ↔ working tree.
    pub fn open_diff(&mut self, rel: String, staged: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(root) = self.vcs.status.as_ref().map(|g| g.root.clone()) else { return };
        let host = self.remote.host.clone();
        let (r, path) = (root.clone(), rel.clone());
        // If it was renamed, the content in HEAD is at the old path.
        let orig = self.vcs.status.as_ref().and_then(|g| g.changes.iter().find(|c| c.path == rel).and_then(|c| c.orig.clone())).unwrap_or_else(|| rel.clone());
        let task = cx.background_executor().spawn(async move {
            let h = host.as_ref();
            if staged {
                (crate::git::show(h, &r, "HEAD", &orig), crate::git::show(h, &r, "", &path))
            } else {
                let disk = h.read(&format!("{}/{path}", r.trim_end_matches('/'))).ok().map(|(d, _)| String::from_utf8_lossy(&d).replace("\r\n", "\n"));
                (crate::git::show(h, &r, "", &path), disk)
            }
        });
        cx.spawn_in(window, async move |this, cx| {
            let (old, new) = task.await;
            this.update_in(cx, |this, window, cx| {
                let view = cx.new(|cx| crate::diff::DiffView::new(rel, staged, old.as_deref().unwrap_or(""), new.as_deref().unwrap_or(""), cx));
                window.focus(&view.focus_handle(cx));
                this.tabs.diff = Some(view);
                this.tabs.diff_active = true;
                this.tabs.page_active = false;
                this.tabs.large_active = false;
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Open the image in a preview (read-only tab, replacing the previous one).
    pub(super) fn open_image(&mut self, path: String, format: gpui::ImageFormat, window: &mut Window, cx: &mut Context<Self>) {
        self.record_nav(cx);
        self.remember_file(&path);
        let host = self.remote.host.clone();
        let view = cx.new(|cx| crate::large::LargeFileView::new_image(host, path, format, cx));
        window.focus(&view.focus_handle(cx));
        self.tabs.large = Some(view);
        self.tabs.large_active = true;
        self.tabs.page_active = false;
        self.tabs.diff_active = false;
        cx.notify();
    }

    /// Open a large file in a read-only view (replacing the previous one).
    pub(super) fn open_large(&mut self, path: String, window: &mut Window, cx: &mut Context<Self>) {
        let host = self.remote.host.clone();
        let view = cx.new(|cx| crate::large::LargeFileView::new(host, path, cx));
        view.update(cx, |v, cx| {
            v.watch(cx);
            // Log files follow the tail when opened.
            if crate::large::looks_like_log(&v.path) {
                v.set_follow(true, cx);
            }
        });
        window.focus(&view.focus_handle(cx));
        self.tabs.large = Some(view);
        self.tabs.large_active = true;
        self.tabs.page_active = false;
        self.tabs.diff_active = false;
        cx.notify();
    }

    /// "File: Follow (Tail) Active File": open the active file (whatever its size) in a read-only
    /// view that sticks to the end; a log file is followed while it grows on the server. If already in follow view, toggle it off.
    pub(super) fn follow_file(&mut self, _: &FollowFile, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(v) = self.large_current(cx).filter(|v| !v.read(cx).is_image()) {
            v.update(cx, |v, cx| v.set_follow(!v.is_following(), cx));
            return;
        }
        let Some(path) = self.active_editor().map(|e| e.read(cx).path.clone()).filter(|p| !p.is_empty()) else {
            self.notice = Some(Notice::Info(crate::i18n::t("Open a file first.").into()));
            cx.notify();
            return;
        };
        self.follow_path(path, window, cx);
    }

    /// Open the file (whatever its size) in follow view: also reached from the explorer and tab menus.
    pub(super) fn follow_path(&mut self, path: String, window: &mut Window, cx: &mut Context<Self>) {
        self.open_large(path, window, cx);
        if let Some(v) = self.tabs.large.clone() {
            v.update(cx, |v, cx| v.set_follow(true, cx));
        }
    }

    /// "File: Filter Log Lines…": lines matching a pattern (`grep` on the server; only the last 256 MB and at most 5000 lines) in a tab.
    pub(super) fn filter_log(&mut self, _: &FilterLog, window: &mut Window, cx: &mut Context<Self>) {
        let path = self
            .large_current(cx)
            .filter(|v| !v.read(cx).is_image())
            .map(|v| v.read(cx).path.clone())
            .or_else(|| self.active_editor().map(|e| e.read(cx).path.clone()))
            .filter(|p| !p.is_empty());
        let Some(path) = path else {
            self.notice = Some(Notice::Info(crate::i18n::t("Open a file first.").into()));
            cx.notify();
            return;
        };
        self.show_input(
            InputPurpose::FilterLog { path },
            crate::i18n::t("Filter"),
            &crate::i18n::tf("Show lines matching (regular expression, case-insensitive), e.g. {}", &[&"ERROR|timeout"]),
            "",
            window,
            cx,
        );
    }

    pub(super) fn run_filter_log(&mut self, path: String, pattern: String, window: &mut Window, cx: &mut Context<Self>) {
        let pattern = pattern.trim().to_string();
        if pattern.is_empty() {
            return;
        }
        let host = self.remote.host.clone();
        self.notice = Some(Notice::Info(crate::i18n::tf("Filtering {0} for “{1}”…", &[&ui::file_name(&path), &pattern]).into()));
        cx.notify();
        let (p, pat) = (path.clone(), pattern.clone());
        let task = cx.background_executor().spawn(async move {
            let q = |s: &str| format!("'{}'", s.replace('\'', "'\\''"));
            // `cut`: limit a single line to 2000 characters (so minified JSON logs don't lock up the screen).
            let script = format!("tail -c 268435456 -- {} | grep -a -i -E -e {} | cut -c1-2000 | tail -n 5000", q(&p), q(&pat));
            let tools = |why: String| crate::i18n::tf("Filtering needs sh, tail, grep and cut on the host (on Windows: Git Bash or WSL). {}", &[&why]);
            let out = host.exec("sh", &["-c", &script], None, None).map_err(|e| tools(e.to_string()))?;
            if out.code == Some(127) {
                return Err(tools(crate::i18n::t("They were not found.").into()));
            }
            // grep exits 1 on no match (fine); for a bad pattern or unreadable file, show its message.
            let err = out.stderr_str();
            if !err.trim().is_empty() && out.stdout.is_empty() {
                let first = err.lines().next().unwrap_or("filter failed");
                return Err(
                    if first.contains("parentheses")
                        || first.contains("brackets")
                        || first.contains("repetition")
                        || first.contains("Unmatched")
                        || first.contains("Missing")
                        || first.contains("invalid")
                        || first.contains("Invalid")
                    {
                        crate::i18n::tf("“{0}” is not a valid regular expression ({1}). Escape special characters with a backslash.", &[&pat, &first])
                    } else {
                        first.to_string()
                    },
                );
            }
            Ok(String::from_utf8_lossy(&out.stdout).into_owned())
        });
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            this.update_in(cx, |this, window, cx| match result {
                Ok(text) if text.is_empty() => {
                    this.notice = Some(Notice::Info(crate::i18n::tf("No lines match “{}”.", &[&pattern]).into()));
                    cx.notify();
                }
                Ok(text) => {
                    let n = text.lines().count();
                    let name = format!("{}.filtered.log", ui::file_name(&path));
                    this.create_scratch_with(name, &text, window, cx);
                    let msg = if n >= 5000 {
                        crate::i18n::tn("{0} line matches “{1}” (showing the last 5000).", "{0} lines match “{1}” (showing the last 5000).", n, &[&pattern])
                    } else {
                        crate::i18n::tn("{0} line matches “{1}”.", "{0} lines match “{1}”.", n, &[&pattern])
                    };
                    this.notice = Some(Notice::Info(msg.into()));
                    cx.notify();
                }
                Err(e) => {
                    this.notice = Some(Notice::Error(e.into()));
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    fn large_current(&self, _: &App) -> Option<Entity<crate::large::LargeFileView>> {
        self.tabs.large.clone().filter(|_| self.tabs.large_active)
    }

    pub(super) fn close_large(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.tabs.large = None;
        self.tabs.large_active = false;
        if let Some(ed) = self.active_editor() {
            window.focus(&ed.focus_handle(cx));
        }
        cx.notify();
    }

    #[cfg(test)]
    pub fn large_view(&self) -> Option<Entity<crate::large::LargeFileView>> {
        self.tabs.large.clone().filter(|_| self.tabs.large_active)
    }

    pub(super) fn close_diff(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.tabs.diff = None;
        self.tabs.diff_active = false;
        if let Some(ed) = self.active_editor() {
            window.focus(&ed.focus_handle(cx));
        }
        cx.notify();
    }

    #[cfg(test)]
    pub fn diff_view(&self) -> Option<Entity<crate::diff::DiffView>> {
        self.tabs.diff.clone().filter(|_| self.tabs.diff_active)
    }

    #[cfg(test)]
    pub fn scm_panel(&self) -> Entity<crate::scm::ScmPanel> {
        self.scm.clone()
    }
}
