//! Workbench: bottom panels (Output, Problems, Ports), zoom, diff and large-file tabs.

use super::*;

/// How long the panel takes to maximize or restore.
const PANEL_ANIM: std::time::Duration = std::time::Duration::from_millis(180);
#[cfg(test)]
pub(super) const PANEL_ANIM_FOR_TEST: std::time::Duration = PANEL_ANIM;

#[cfg(test)]
#[path = "workbench_panel_tests.rs"]
mod panel_tests;

/// "Interface scale: 110%" in the interface language (Turkish puts the sign first: "%110").
pub(crate) fn interface_scale_notice(scale: f32) -> String {
    crate::i18n::tf("Interface scale: {}%", &[&settings::ui_scale_percent(scale)])
}

impl Workbench {
    /// Zoom: the interface scale (`ui.scale`) of every window. The editor and terminal font sizes stay what the user
    /// set; the window scales them once together with everything else. The new value is written to the user's
    /// settings file once and applied right away; other windows follow through `App::set_ui_scale` and, when they
    /// re-read the file, find the same value and do nothing.
    pub(super) fn set_interface_scale(&mut self, scale: f32, window: &mut Window, cx: &mut Context<Self>) {
        let scale = if (settings::UI_SCALE_MIN..=settings::UI_SCALE_MAX).contains(&scale) { scale } else { 1. };
        let current = cx.global::<Settings>().ui_scale;
        if scale != current {
            let session = cx.try_global::<settings::SessionScale>().copied().unwrap_or_default();
            let file_before = session.file_value(current);
            cx.set_global(settings::SessionScale(None));
            if self.config_paths().is_some() {
                self.write_setting("ui.scale", settings::ui_scale_value(scale), window, cx);
            }
            // Without a settings folder, or if the file could not be written, the scale still applies for this session
            // and survives settings reloads until the file's own value changes.
            if cx.global::<Settings>().ui_scale != scale {
                cx.set_global(settings::SessionScale(Some(settings::UnsavedScale { scale, file: file_before })));
                cx.global_mut::<Settings>().ui_scale = scale;
            }
            cx.set_ui_scale(scale);
        }
        if !matches!(self.notice, Some(Notice::Error(_))) {
            self.notice = Some(Notice::Info(interface_scale_notice(scale).into()));
        }
        cx.notify();
    }

    pub(super) fn zoom_in(&mut self, _: &ZoomIn, window: &mut Window, cx: &mut Context<Self>) {
        let next = settings::step_ui_scale(cx.global::<Settings>().ui_scale, true);
        self.set_interface_scale(next, window, cx);
    }

    pub(super) fn zoom_out(&mut self, _: &ZoomOut, window: &mut Window, cx: &mut Context<Self>) {
        let next = settings::step_ui_scale(cx.global::<Settings>().ui_scale, false);
        self.set_interface_scale(next, window, cx);
    }

    pub(super) fn zoom_reset(&mut self, _: &ZoomReset, window: &mut Window, cx: &mut Context<Self>) {
        self.set_interface_scale(1., window, cx);
    }

    /// Bottom ↔ right. A maximized panel covers the editor wherever it is docked, so moving it also restores its size:
    /// otherwise the move would not show.
    pub(super) fn toggle_panel_position(&mut self, cx: &mut Context<Self>) {
        self.chrome.panel_right = !self.chrome.panel_right;
        self.chrome.panel_max = false;
        self.chrome.panel_open = true;
        if let Some(s) = &self.persist.state {
            let _ = s.set("panel_right", if self.chrome.panel_right { "1" } else { "0" });
        }
        cx.notify();
    }

    /// Maximize or restore. An open panel grows towards the top (or the left when docked right) and shrinks back, unless
    /// motion is reduced.
    pub(super) fn toggle_maximized_panel(&mut self, cx: &mut Context<Self>) {
        let animate = self.chrome.panel_open && self.chrome.panel_area.is_some() && !crate::system_prefs::reduce_motion(cx);
        self.chrome.panel_max = !self.chrome.panel_max;
        self.chrome.panel_open = true;
        self.chrome.panel_anim = animate.then(|| (std::time::Instant::now(), self.chrome.panel_max));
        cx.notify();
    }

    /// Ends a finished (or overtaken) maximize/restore animation, or asks for its next frame.
    pub(super) fn step_panel_anim(&mut self, window: &mut Window) {
        let Some((start, maximizing)) = self.chrome.panel_anim else {
            return;
        };
        if maximizing != self.chrome.panel_max || !self.chrome.panel_open || start.elapsed() >= PANEL_ANIM {
            self.chrome.panel_anim = None;
        } else {
            window.request_animation_frame();
        }
    }

    /// The panel's size along its docking axis while it animates: from its own size to the whole area, eased.
    pub(super) fn animated_panel_size(&self) -> Option<f32> {
        let (start, maximizing) = self.chrome.panel_anim?;
        let area = self.chrome.panel_area?;
        let full = if self.chrome.panel_right { area.size.width } else { area.size.height } / px(1.);
        let own = if self.chrome.panel_right { self.chrome.panel_w } else { self.chrome.panel_h }.min(full);
        let t = gpui::ease_in_out((start.elapsed().as_secs_f32() / PANEL_ANIM.as_secs_f32()).min(1.));
        let open = if maximizing { t } else { 1. - t };
        Some(own + (full - own) * open)
    }

    /// Records the editor-and-panel area after layout, without drawing anything.
    pub(super) fn panel_area_probe(cx: &Context<Self>) -> impl IntoElement + use<> {
        let entity = cx.entity().downgrade();
        gpui::canvas(
            |bounds, _, _| bounds,
            move |_, bounds, _, cx| {
                entity.update(cx, |wb, _| wb.chrome.panel_area = Some(bounds)).ok();
            },
        )
        .absolute()
        .top_0()
        .left_0()
        .size_full()
    }

    /// The panel's width in a window `total` wide: its own when docked right, else the editor area's.
    pub(super) fn panel_width(&self, total: f32) -> f32 {
        if self.chrome.panel_right && !self.chrome.panel_max {
            return self.chrome.panel_w;
        }
        let sidebar = if self.chrome.panel.is_some() && self.chrome.zen.is_none() { self.chrome.sidebar_w } else { 0. };
        total - layout::ACTIVITYBAR_W - sidebar
    }

    #[cfg(test)]
    pub fn panel_layout_for_test(&self) -> (bool, bool, bool) {
        (self.chrome.panel_open, self.chrome.panel_right, self.chrome.panel_max)
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
        let root = self.root(cx);
        let rows: Vec<AnyElement> = problem_rows(&all, root.as_deref())
            .into_iter()
            .enumerate()
            .map(|(i, row)| match row {
                ProblemRow::File { path, dir, count } => div()
                    .h(px(layout::ROW_H))
                    .flex()
                    .items_center()
                    .gap(px(theme::SPACE[2]))
                    .px(px(theme::SPACE[4]))
                    .child(crate::icon_theme::file_icon_el(ui::file_name(&path), layout::ICON, p))
                    .child(ui::file_name(&path).to_string())
                    .child(div().text_color(c(p.text_muted)).text_size(px(text::XS.0)).child(dir))
                    .child(
                        div().px(px(theme::SPACE[2])).rounded(px(theme::radius::LG)).bg(c(p.surface_active)).text_size(px(text::XS.0)).child(count.to_string()),
                    )
                    .into_any_element(),
                ProblemRow::Problem { severity, text, detail, .. } => {
                    let (glyph, color) = match severity {
                        1 => ("circle-x", p.danger),
                        2 => ("alert", p.warning),
                        _ => ("info", p.info),
                    };
                    problem_row(i, theme::SPACE[8], p, cx)
                        .child(icon(glyph, 14., color))
                        .child(div().flex_shrink().min_w_0().overflow_hidden().child(text))
                        .child(div().flex_none().text_size(px(text::XS.0)).text_color(c(p.text_muted)).child(detail))
                        .into_any_element()
                }
                ProblemRow::Related { text, .. } => problem_row(i, theme::SPACE[8] + 22., p, cx)
                    .text_color(c(p.text_muted))
                    .child(div().min_w_0().overflow_hidden().child(text))
                    .into_any_element(),
            })
            .collect();
        div().id("problems").size_full().overflow_y_scroll().children(rows).into_any_element()
    }

    /// A click on Problems row `i`: a problem or a related location opens at its position.
    pub(super) fn open_problem_row(&mut self, i: usize, window: &mut Window, cx: &mut Context<Self>) {
        let root = self.root(cx);
        if let Some(ProblemRow::Problem { path, line, col, .. } | ProblemRow::Related { path, line, col, .. }) =
            problem_rows(&self.all_problems(), root.as_deref()).into_iter().nth(i)
        {
            self.open_location(path, line, col, window, cx);
        }
    }

    #[cfg(test)]
    pub fn problem_rows_for_test(&self, cx: &App) -> Vec<ProblemRow> {
        problem_rows(&self.all_problems(), self.root(cx).as_deref())
    }

    #[cfg(test)]
    pub fn click_problem_row_for_test(&mut self, i: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.open_problem_row(i, window, cx);
    }

    /// `--panel`: which sidebar is visible at startup.
    pub fn show_panel_named(&mut self, name: &str, window: &mut Window, cx: &mut Context<Self>) {
        match name {
            "scm" | "git" => self.show_scm(&ShowScm, window, cx),
            "search" => self.find_in_files(&FindInFiles, window, cx),
            "debug" => self.show_run_debug(&ShowRunAndDebug, window, cx),
            "testing" | "tests" => self.show_testing(&ShowTesting, window, cx),
            "extensions" => self.show_extensions(&ShowExtensions, window, cx),
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
        self.chrome.panel_max = false;
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
        self.chrome.panel_max = false;
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

/// One row of the Problems panel: a file, one of its problems, or a related location of that problem.
#[derive(Debug, Clone, PartialEq)]
pub enum ProblemRow {
    File { path: String, dir: String, count: usize },
    Problem { path: String, severity: u8, text: String, detail: String, line: u32, col: u32 },
    Related { text: String, path: String, line: u32, col: u32 },
}

/// The Problems panel's rows: files in path order, their problems by severity and position, each followed by its related
/// locations.
pub fn problem_rows(all: &std::collections::BTreeMap<String, Vec<crate::lsp::Diagnostic>>, root: Option<&str>) -> Vec<ProblemRow> {
    let mut rows = Vec::new();
    for (path, diags) in all {
        let rel = crate::lsp::related_path(path, root);
        let mut sorted: Vec<&crate::lsp::Diagnostic> = diags.iter().filter(|d| !is_related_copy(d, path, diags)).collect();
        rows.push(ProblemRow::File { path: path.clone(), dir: rel.rsplit_once('/').map(|(d, _)| d.to_string()).unwrap_or_default(), count: sorted.len() });
        sorted.sort_by_key(|d| (d.severity, d.start));
        for d in sorted {
            let source = d.source.as_deref().map_or("", |s| if s == "task" { crate::i18n::t("task") } else { s });
            rows.push(ProblemRow::Problem {
                path: path.clone(),
                severity: d.severity,
                text: d.headline().lines().next().unwrap_or_default().to_string(),
                detail: format!("{source} [{}:{}]", d.start.0.saturating_add(1), d.start.1.saturating_add(1)),
                line: d.start.0,
                col: d.start.1,
            });
            for r in &d.related {
                rows.push(ProblemRow::Related { text: crate::lsp::related_label(r, path, root), path: r.path.clone(), line: r.start.0, col: r.start.1 });
            }
        }
    }
    rows
}

/// Some servers (rust-analyzer) also publish each related location of a diagnostic as a hint of its own, which would show
/// the same text twice in Problems: a hint is left out when a related row of another diagnostic in the file says the same at
/// the same place, or when it points at another diagnostic with the same message (the copy at a macro call).
fn is_related_copy(d: &crate::lsp::Diagnostic, path: &str, all: &[crate::lsp::Diagnostic]) -> bool {
    d.severity == 4
        && all.iter().filter(|o| !std::ptr::eq(*o, d)).any(|o| {
            o.related.iter().any(|r| r.path == path && r.start == d.start && r.message == d.message)
                || (o.severity < 4 && o.message == d.message && d.related.iter().any(|r| r.path == path && r.start == o.start))
        })
}

/// A clickable Problems row indented by `indent`.
fn problem_row(i: usize, indent: f32, p: &Palette, cx: &mut Context<Workbench>) -> gpui::Stateful<gpui::Div> {
    div()
        .id(("problem", i))
        .h(px(layout::ROW_H))
        .flex()
        .items_center()
        .gap(px(theme::SPACE[2]))
        .pl(px(indent))
        .pr(px(theme::SPACE[4]))
        .whitespace_nowrap()
        .overflow_hidden()
        .cursor_pointer()
        .hover(|s| s.bg(c(p.surface_hover)))
        .on_click(cx.listener(move |this, _, window, cx| this.open_problem_row(i, window, cx)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lsp::{Diagnostic, Related};

    fn rel(line: u32, col: u32, message: &str) -> Related {
        Related { path: "/p/src/lib.rs".into(), start: (line, col), end: (line, col + 1), message: message.into() }
    }

    fn diag(severity: u8, at: (u32, u32), message: &str, related: Vec<Related>) -> Diagnostic {
        Diagnostic { start: at, end: at, severity, message: message.into(), code: Some("E0382".into()), related, ..Default::default() }
    }

    /// The shapes rust-analyzer 1.99 publishes for a use after move: the error, its related locations again as hints, and a
    /// hint at the macro call. Problems shows the error and its related rows once.
    #[test]
    fn hints_that_repeat_a_related_location_are_left_out_of_problems() {
        let msg = "borrow of moved value: `s`\nvalue borrowed here after move";
        let error = diag(1, (3, 15), msg, vec![rel(3, 4, "Error originated from macro call here"), rel(2, 12, "value moved here"), rel(1, 8, "move occurs")]);
        let diags = vec![
            diag(4, (3, 4), msg, vec![rel(3, 15, "Exact error occurred here")]),
            error,
            diag(4, (2, 12), "value moved here", vec![rel(3, 15, "original diagnostic")]),
            diag(4, (1, 8), "move occurs", vec![rel(3, 15, "original diagnostic")]),
            // A hint of its own stays.
            diag(4, (0, 0), "unrelated hint", vec![]),
        ];
        let all = std::collections::BTreeMap::from([("/p/src/lib.rs".to_string(), diags)]);
        let rows = problem_rows(&all, Some("/p"));
        let problems: Vec<&str> = rows.iter().filter_map(|r| if let ProblemRow::Problem { text, .. } = r { Some(text.as_str()) } else { None }).collect();
        assert_eq!(problems, ["E0382: borrow of moved value: `s`", "E0382: unrelated hint"]);
        assert!(matches!(&rows[0], ProblemRow::File { count: 2, dir, .. } if dir == "src"));
        assert_eq!(rows.iter().filter(|r| matches!(r, ProblemRow::Related { .. })).count(), 3);
        // An error is never left out, even when a related row repeats it.
        let twice = vec![diag(1, (2, 12), "value moved here", vec![]), diag(1, (3, 15), msg, vec![rel(2, 12, "value moved here")])];
        let all = std::collections::BTreeMap::from([("/p/src/lib.rs".to_string(), twice)]);
        assert!(matches!(&problem_rows(&all, Some("/p"))[0], ProblemRow::File { count: 2, .. }));
    }
}
