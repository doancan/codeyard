//! Workbench: rendering: activity bar, explorer, timeline, outline.

use super::*;

impl Workbench {
    pub(super) fn activity_bar(&self, p: &Palette, cx: &mut Context<Self>) -> impl IntoElement {
        let changes = self.vcs.status.as_ref().map_or(0, |g| g.changes.len());
        let failed_tests = self.failed_test_count();
        let item = |id: &'static str, name: &'static str, label: &'static str, panel: Panel, badge: usize, cx: &mut Context<Self>| {
            let selected = self.chrome.panel == Some(panel);
            div()
                .id(id)
                .debug_selector(|| id.to_string())
                .group(id)
                .relative()
                .w(px(layout::ACTIVITYBAR_W))
                .h(px(layout::ACTIVITYBAR_W))
                .flex()
                .items_center()
                .justify_center()
                .cursor_pointer()
                // Hover: rounded background (same as the app's other icon buttons) + the icon brightens.
                .child(
                    div()
                        .size(px(layout::ACTIVITYBAR_W - 8.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded(px(theme::radius::MD))
                        // The selected view gets a permanent background (stronger than hover); hover only on unselected ones.
                        .when(selected, |d| d.bg(c(p.surface_active)))
                        .when(!selected, |d| d.group_hover(id, |s| s.bg(c(p.surface_hover))))
                        .child(icon(name, layout::ICON_LG, if selected { p.text } else { p.text_muted }).group_hover(id, |s| s.text_color(c(p.text)))),
                )
                .when(badge > 0, |d| {
                    d.child(
                        div()
                            .absolute()
                            .right(px(8.))
                            .bottom(px(10.))
                            .min_w(px(14.))
                            .h(px(14.))
                            .px(px(3.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded_full()
                            .bg(c(p.accent))
                            .text_color(c(p.on_accent))
                            .text_size(px(text::BADGE.0))
                            .font_weight(gpui::FontWeight::BOLD)
                            .child(if badge > 99 { "99+".to_string() } else { badge.to_string() }),
                    )
                })
                .tooltip(move |_, cx| cx.new(|_| Tooltip(label.into())).into())
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.chrome.panel = if this.chrome.panel == Some(panel) { None } else { Some(panel) };
                    if this.chrome.panel == Some(Panel::Containers) {
                        this.sync_container_monitor(cx);
                    }
                    if this.chrome.panel == Some(Panel::Explorer) {
                        window.focus(&this.roots.primary.focus_handle(cx));
                    }
                    if this.chrome.panel == Some(Panel::Remote) {
                        this.refresh_remote_view(cx);
                    }
                    if this.chrome.panel == Some(Panel::Debug) {
                        this.load_launch_catalog(cx);
                    }
                    if this.chrome.panel == Some(Panel::Testing) {
                        this.testing_opened(window, cx);
                    }
                    cx.notify();
                }))
        };
        // `tip`: tooltip only while the menu is closed so it can't cover the open menu's last row (native apps hide it too).
        let bottom = |id: &'static str, name: &'static str, label: &'static str, tip: bool| {
            div()
                .id(id)
                .group(id)
                .w(px(layout::ACTIVITYBAR_W))
                .h(px(layout::ACTIVITYBAR_W))
                .flex()
                .items_center()
                .justify_center()
                .cursor_pointer()
                .child(
                    div()
                        .size(px(layout::ACTIVITYBAR_W - 8.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded(px(theme::radius::MD))
                        .group_hover(id, |s| s.bg(c(p.surface_hover)))
                        .child(icon(name, layout::ICON_LG - 2., p.text_muted).group_hover(id, |s| s.text_color(c(p.text)))),
                )
                .when(tip, |d| d.tooltip(move |_, cx| cx.new(|_| Tooltip(label.into())).into()))
        };
        div()
            .w(px(layout::ACTIVITYBAR_W))
            .flex_none()
            .flex()
            .flex_col()
            .bg(c(p.surface))
            .border_r_1()
            .border_color(c(p.border))
            .child(item("act-explorer", "files", crate::i18n::t("Explorer (⇧⌘E)"), Panel::Explorer, 0, cx))
            .child(item("act-search", "search", crate::i18n::t("Search (⇧⌘F)"), Panel::Search, 0, cx))
            .child(item("act-scm", "git-branch", crate::i18n::t("Source Control (⌃⇧G)"), Panel::Scm, changes, cx))
            .child(item("act-debug", "bug-play", crate::i18n::t("Run and Debug (⌃⇧D)"), Panel::Debug, 0, cx))
            .child(item("act-testing", "flask-conical", crate::i18n::t("Testing"), Panel::Testing, failed_tests, cx))
            .child(item("act-remote", "screen-share", crate::i18n::t("Remote Explorer"), Panel::Remote, 0, cx))
            .when(cx.global::<Settings>().docker_enabled, |d| d.child(item("act-containers", "server", crate::i18n::t("Containers"), Panel::Containers, 0, cx)))
            .child(div().flex_1())
            .child(
                // A single "manage" button: the menu opens to the right of the icon, bottom edge aligned with it (see `open_manage_menu`).
                bottom("act-manage", "settings", crate::i18n::t("Manage: About, Settings, Color Theme…"), self.menu.is_none()).on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _: &gpui::MouseDownEvent, window, cx| {
                        cx.stop_propagation();
                        this.open_manage_menu(window, cx);
                    }),
                ),
            )
    }

    pub(super) fn view_header(&self, title: impl Into<SharedString>, p: &Palette) -> impl IntoElement {
        div()
            .h(px(layout::VIEW_HEADER_H))
            .flex_none()
            .flex()
            .items_center()
            .px(px(theme::SPACE[6]))
            .text_size(px(text::XXS.0))
            .font_weight(gpui::FontWeight::SEMIBOLD)
            .text_color(c(p.text_muted))
            .child(title.into())
    }

    /// Explorer tree events (the first and extra folders' trees use the same path).
    pub(super) fn on_tree_event(&mut self, event: &TreeEvent, window: &mut Window, cx: &mut Context<Self>) {
        let this = self;
        match event {
            TreeEvent::OpenFile { path, preview } => {
                let host = this.remote.host.clone();
                this.open_file_as(host, path.clone(), *preview, window, cx)
            }
            TreeEvent::ContextMenu { path, is_dir, position } => {
                this.open_context_menu(MenuTarget { path: path.clone(), is_dir: *is_dir, tab: None }, *position, window, cx)
            }
            TreeEvent::Rename(path) => this.start_rename(path.clone(), window, cx),
            TreeEvent::Delete(path, is_dir) => this.confirm_delete(path.clone(), *is_dir, window, cx),
            TreeEvent::Copy(path) => this.roots.file_clip = Some((path.clone(), false)),
            TreeEvent::Cut(path) => this.roots.file_clip = Some((path.clone(), true)),
            TreeEvent::Paste { dir } => this.paste_entry(dir.clone(), window, cx),
            TreeEvent::Move { from, to_dir } => this.confirm_drop(from.clone(), to_dir.clone(), window, cx),
            TreeEvent::DropFiles { paths, to_dir } => this.drop_external(paths.clone(), to_dir.clone(), window, cx),
            TreeEvent::ExpandedChanged => this.persist_tree(cx),
        }
    }

    pub(super) fn explorer_panel(&self, p: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let root = self.roots.primary.read(cx).root().map(|r| ui::file_name(r).to_uppercase());
        let Some(root) = root else {
            // No folder: an empty state saying what to do.
            return div()
                .flex()
                .flex_col()
                .size_full()
                .child(self.view_header(crate::i18n::upper(crate::i18n::t("Explorer")), p))
                .child(
                    div()
                        .px(px(theme::SPACE[6]))
                        .pt(px(theme::SPACE[4]))
                        .flex()
                        .flex_col()
                        .gap(px(theme::SPACE[4]))
                        .text_size(px(text::SM.0))
                        .text_color(c(p.text_muted))
                        .child(crate::i18n::t("No folder is open."))
                        .child(
                            div()
                                .id("explorer-open-folder")
                                .h(px(28.))
                                .flex()
                                .items_center()
                                .justify_center()
                                .rounded(px(theme::radius::SM))
                                .bg(c(p.accent))
                                .text_color(c(p.on_accent))
                                .cursor_pointer()
                                .hover(|d| d.opacity(0.9))
                                .child(crate::i18n::t("Open Folder"))
                                .on_click(cx.listener(|this, _, window, cx| this.open_folder(&OpenFolder, window, cx))),
                        )
                        .child(
                            div()
                                .id("explorer-recent")
                                .cursor_pointer()
                                .text_color(c(p.accent))
                                .hover(|d| d.underline())
                                .child(crate::i18n::t("Open recent, or connect to a host… (⌃R)"))
                                .on_click(cx.listener(|this, _, window, cx| this.show_project_switcher(window, cx))),
                        ),
                )
                .into_any_element();
        };
        let favorites: Vec<AnyElement> = self
            .roots
            .favorites
            .iter()
            .enumerate()
            .map(|(i, rel)| {
                let rel = rel.clone();
                div()
                    .id(("favorite", i))
                    .h(px(22.))
                    .w_full()
                    .flex()
                    .items_center()
                    .gap(px(theme::SPACE[3]))
                    .px(px(theme::SPACE[8]))
                    .text_size(px(text::SM.0))
                    .cursor_pointer()
                    .hover(|d| d.bg(c(p.surface_hover)))
                    .child(crate::icon_theme::file_icon_el(ui::file_name(&rel), 14., p))
                    .child(div().flex_none().text_color(c(p.text)).child(ui::file_name(&rel).to_string()))
                    .child(div().min_w_0().overflow_hidden().whitespace_nowrap().text_color(c(p.text_muted)).text_size(px(text::XS.0)).child(parent_of(&rel)))
                    .tooltip({
                        let r = rel.clone();
                        move |_, cx| cx.new(|_| Tooltip(r.clone().into())).into()
                    })
                    .on_click(cx.listener(move |this, _, window, cx| this.open_favorite(rel.clone(), window, cx)))
                    .into_any_element()
            })
            .collect();
        let has_favorites = !favorites.is_empty();
        // OPEN EDITORS: open tabs; click → go, ×: close (dot if dirty).
        // Skipped for a single open file: it'd list the same thing as the tree. Starts collapsed with more than two.
        let show_open = cx.global::<Settings>().open_editors && self.tabs.editors.len() >= 2;
        let open_rows: Vec<AnyElement> = if show_open {
            self.tabs
                .editors
                .iter()
                .enumerate()
                .map(|(ix, ed)| {
                    let e = ed.read(cx);
                    let (name, path, dirty) = (e.name.to_string(), e.path.clone(), e.is_dirty());
                    let mark = self.git_mark_of(&path);
                    let active = self.tabs.active == Some(ix) && self.tabs.focused_left;
                    let group: SharedString = format!("open-editor-{ix}").into();
                    div()
                        .id(("open-editor", ix))
                        .group(group.clone())
                        .h(px(22.))
                        .w_full()
                        .flex()
                        .items_center()
                        .gap(px(theme::SPACE[3]))
                        .pl(px(theme::SPACE[6]))
                        .pr(px(theme::SPACE[3]))
                        .text_size(px(text::SM.0))
                        .cursor_pointer()
                        .when(active, |d| d.bg(c(p.surface_active)))
                        .hover(|d| d.bg(c(p.surface_hover)))
                        .child(crate::icon_theme::file_icon_el(&name, 14., p))
                        .child(div().flex_none().text_color(c(mark.map_or(p.text, |m| m.color(p)))).child(name))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .text_color(c(p.text_muted))
                                .text_size(px(text::XS.0))
                                .child(self.rel_in_roots(&path, cx).map(|r| parent_of(&r)).unwrap_or_default()),
                        )
                        .when_some(mark, |d, m| d.child(div().flex_none().text_size(px(text::XS.0)).text_color(c(m.color(p))).child(m.letter())))
                        .child(if dirty {
                            div().flex_none().size(px(8.)).rounded_full().bg(c(p.text_muted)).into_any_element()
                        } else {
                            div()
                                .id(("open-editor-close", ix))
                                .flex_none()
                                .invisible()
                                .group_hover(group, |s| s.visible())
                                .size(px(16.))
                                .flex()
                                .items_center()
                                .justify_center()
                                .rounded(px(theme::radius::SM))
                                .hover(|d| d.bg(c(p.surface_active)))
                                .child(icon("x", 12., p.text_muted))
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    cx.stop_propagation();
                                    this.close_editor_at(ix, window, cx);
                                }))
                                .into_any_element()
                        })
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.tabs.focused_left = true;
                            this.activate(ix, window, cx);
                        }))
                        .into_any_element()
                })
                .collect()
        } else {
            Vec::new()
        };
        let open_count = open_rows.len();
        let sec = |id: &'static str| self.sections.is_open(id, id != "open-editors");
        let (open_open, fav_open, tree_open, time_open) = (sec("open-editors"), sec("favorites"), sec("folders"), sec("timeline"));
        // Section header + toggle on click.
        let head = |id: &'static str, title: &str, count: Option<usize>, open: bool, cx: &mut Context<Self>| {
            sections::header(SharedString::from(format!("sec-{id}")), title, count, open, p).debug_selector(move || format!("sec-{id}")).on_click(cx.listener(
                move |this, _, _, cx| {
                    this.sections.toggle(id, id != "open-editors");
                    this.persist_sections(cx);
                },
            ))
        };
        // Bottom edge handle (dragging down grows) and top edge (dragging up grows).
        let sash = |id: &'static str, h: f32, sign: f32, cx: &mut Context<Self>| {
            sections::sash(SharedString::from(format!("sash-{id}")), p, self.sections.dragging_section(id))
                .debug_selector(move || format!("sash-{id}"))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, ev: &gpui::MouseDownEvent, _, cx| {
                        cx.stop_propagation();
                        let current = this.sections.rendered_height(id, h);
                        this.sections.begin_drag(id, ev.position.y / px(1.), current, sign);
                    }),
                )
        };
        let open_h = self.sections.height("open-editors", (22. * open_count.min(8) as f32).max(22.));
        let time_h = self.sections.height("timeline", 170.);
        let outline_open = sec("outline");
        let outline_h = self.sections.height("outline", 170.);
        let outline_body = outline_open.then(|| self.outline_rows(p, cx));
        let outline_count = (!self.outline.symbols.is_empty()).then_some(self.outline.symbols.len());
        let in_repo = self.vcs.status.is_some();
        let timeline_body = time_open.then(|| self.timeline_rows(p, cx));
        let timeline_count = (!self.timeline.entries.is_empty()).then_some(self.timeline.entries.len());
        div()
            .flex()
            .flex_col()
            .size_full()
            .child(
                div()
                    .h(px(layout::VIEW_HEADER_H))
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap(px(theme::SPACE[1]))
                    .pl(px(theme::SPACE[6]))
                    .pr(px(theme::SPACE[3]))
                    .child(
                        div()
                            .flex_1()
                            .text_size(px(text::XXS.0))
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .text_color(c(p.text_muted))
                            .child(crate::i18n::upper(crate::i18n::t("Explorer"))),
                    )
                    .child(
                        ui::icon_button("exp-new-file", "file-plus", crate::i18n::t("New File… (⌘N)"), p)
                            .on_click(cx.listener(|this, _, window, cx| this.new_file(&NewFile, window, cx))),
                    )
                    .child(
                        ui::icon_button("exp-new-folder", "folder-plus", crate::i18n::t("New Folder…"), p)
                            .on_click(cx.listener(|this, _, window, cx| this.new_folder(window, cx))),
                    )
                    .child(
                        ui::icon_button("exp-refresh", "refresh-cw", crate::i18n::t("Refresh Explorer"), p)
                            .on_click(cx.listener(|this, _, _, cx| this.refresh_explorer(cx))),
                    )
                    .child(
                        ui::icon_button("exp-expand", "chevrons-up-down", crate::i18n::t("Expand Folders (the selected one, or the open file's folders)"), p)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.roots.primary.update(cx, |t, cx| t.expand_selected_recursive(cx));
                            })),
                    )
                    .child(ui::icon_button("exp-collapse", "square-minus", crate::i18n::t("Collapse Folders"), p).on_click(cx.listener(|this, _, _, cx| {
                        this.roots.primary.update(cx, |t, cx| t.collapse_all(cx));
                    }))),
            )
            .when(open_count > 0, |d| {
                d.child(head("open-editors", &crate::i18n::tf("Open Editors ({})", &[&open_count]), None, open_open, cx)).when(open_open, |d| {
                    d.child(div().id("open-editors").h(px(open_h)).flex_none().overflow_y_scroll().children(open_rows)).child(sash(
                        "open-editors",
                        open_h,
                        1.,
                        cx,
                    ))
                })
            })
            .when(has_favorites, |d| {
                d.child(head("favorites", crate::i18n::t("★ Favorites"), Some(self.roots.favorites.len()), fav_open, cx))
                    .when(fav_open, |d| d.children(favorites))
            })
            .child(head("folders", &root, None, tree_open, cx).font_weight(gpui::FontWeight::BOLD).text_color(c(p.text)))
            .when(tree_open, |d| d.child(div().flex_1().min_h(px(80.)).child(self.roots.primary.clone())))
            .children(
                self.roots
                    .extra_trees
                    .iter()
                    .zip(self.roots.extra_roots.iter())
                    .enumerate()
                    .flat_map(|(i, (tree, root))| {
                        let id = extra_folder_id(i);
                        let open = self.sections.is_open(id, true);
                        let title = ui::file_name(root).to_string();
                        let header = head(id, &title, None, open, cx).font_weight(gpui::FontWeight::BOLD).text_color(c(p.text)).tooltip({
                            let r = root.clone();
                            move |_, cx| cx.new(|_| Tooltip(r.clone().into())).into()
                        });
                        let body = open.then(|| div().flex_1().min_h(px(80.)).child(tree.clone()).into_any_element());
                        [Some(header.into_any_element()), body]
                    })
                    .flatten()
                    .collect::<Vec<_>>(),
            )
            .when(outline_open, |d| d.child(sash("outline", outline_h, -1., cx)))
            .child(head("outline", crate::i18n::t("Outline"), outline_count, outline_open, cx).when(outline_open, |d| d.border_t_0()))
            .when_some(outline_body, |d, body| {
                let fixed = tree_open || self.sections.has_height("outline");
                d.child(
                    div()
                        .relative()
                        .when(fixed, |d| d.h(px(outline_h)).flex_shrink())
                        .when(!fixed, |d| d.flex_1())
                        .min_h(px(40.))
                        .child(body)
                        .child(self.sections.measure("outline")),
                )
            })
            // History comes from git, so the section shows only for a folder that is a git repo.
            .when(in_repo && time_open, |d| d.child(sash("timeline", time_h, -1., cx)))
            .when(in_repo, |d| d.child(head("timeline", crate::i18n::t("Timeline"), timeline_count, time_open, cx).when(time_open, |d| d.border_t_0())))
            .when_some(timeline_body.filter(|_| in_repo), |d, body| {
                let fixed = tree_open || self.sections.has_height("timeline");
                d.child(
                    div()
                        .relative()
                        .when(fixed, |d| d.h(px(time_h)).flex_shrink())
                        .when(!fixed, |d| d.flex_1())
                        .min_h(px(40.))
                        .child(body)
                        .child(self.sections.measure("timeline")),
                )
            })
            .into_any_element()
    }

    /// TIMELINE body: the active file's commits, newest first.
    pub(super) fn timeline_rows(&self, p: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let note = |t: &str| {
            div().px(px(theme::SPACE[6])).py(px(theme::SPACE[2])).text_color(c(p.text_muted)).text_size(px(text::XS.0)).child(t.to_string()).into_any_element()
        };
        if self.timeline.key.is_none() {
            return note(crate::i18n::t("Open a file to see its history."));
        }
        let now_ms = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64);
        let local_rows: Vec<AnyElement> = self
            .timeline
            .local
            .iter()
            .take(15)
            .enumerate()
            .map(|(ix, l)| {
                let ago = crate::git::relative_time((now_ms.saturating_sub(l.at_ms) / 1000) as i64);
                div()
                    .id(("timeline-local", ix))
                    .w_full()
                    .h(px(30.))
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap(px(theme::SPACE[3]))
                    .pl(px(theme::SPACE[6]))
                    .pr(px(theme::SPACE[3]))
                    .cursor_pointer()
                    .hover(|d| d.bg(c(p.surface_hover)))
                    .child(icon("clock", 14., p.text_muted))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_size(px(text::SM.0))
                            .child(crate::i18n::tf("Saved · {}", &[&crate::git::short_ago(&ago)])),
                    )
                    .child(div().flex_none().text_size(px(text::XXS.0)).text_color(c(p.text_muted)).child(crate::i18n::t("Local")))
                    .tooltip(|_, cx| cx.new(|_| Tooltip(crate::i18n::t("Compare this saved copy with the editor").into())).into())
                    .on_click(cx.listener(move |this, _, window, cx| this.open_local_history(ix, window, cx)))
                    .into_any_element()
            })
            .collect();
        let has_local = !local_rows.is_empty();
        if let Some(e) = &self.timeline.error
            && !has_local
        {
            return note(&crate::i18n::tf("Could not read the history: {}", &[&e]));
        }
        if self.timeline.entries.is_empty() && !has_local {
            return note(if self.timeline.loading {
                crate::i18n::t("Loading…")
            } else if self.vcs.status.is_none() {
                crate::i18n::t("No saved copies yet. Save the file to start its local history.")
            } else {
                crate::i18n::t("This file has no commits or saved copies yet.")
            });
        }
        let rows: Vec<AnyElement> = self
            .timeline
            .entries
            .iter()
            .enumerate()
            .map(|(ix, e)| {
                let tip = format!("{}\n{} · {} · {}", e.subject, e.author, e.when, e.short);
                div()
                    .id(("timeline", ix))
                    .w_full()
                    .h(px(46.))
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap(px(theme::SPACE[4]))
                    .pl(px(theme::SPACE[6]))
                    .pr(px(theme::SPACE[3]))
                    .cursor_pointer()
                    .hover(|d| d.bg(c(p.surface_hover)))
                    .child(icon("git-commit", 14., p.accent))
                    .child(ui::commit_lines(
                        e.subject.clone().into(),
                        crate::git::short_ago(&e.when).into(),
                        div().overflow_hidden().whitespace_nowrap().child(e.author.clone()),
                        e.short.clone().into(),
                        false,
                        p,
                    ))
                    .tooltip(move |_, cx| cx.new(|_| Tooltip(tip.clone().into())).into())
                    .on_click(cx.listener(move |this, _, window, cx| this.open_timeline_entry(ix, window, cx)))
                    .into_any_element()
            })
            .collect();
        div().id("timeline-list").size_full().overflow_y_scroll().flex().flex_col().children(local_rows).children(rows).into_any_element()
    }

    /// Save the open folders (relative to the root) for this workspace.
    pub(super) fn persist_tree(&mut self, cx: &mut Context<Self>) {
        let (Some(ws), Some(s), Some(root)) = (self.persist.workspace_id, &self.persist.state, self.root(cx)) else { return };
        let prefix = format!("{}/", root.trim_end_matches('/'));
        let rels: Vec<String> = self.roots.primary.read(cx).open_dirs().iter().filter_map(|d| d.strip_prefix(&prefix).map(str::to_string)).take(300).collect();
        let _ = s.set(&format!("tree:{ws}"), &rels.join("\n"));
    }

    /// Restore the open folders from the previous session.
    pub(super) fn restore_tree(&mut self, cx: &mut Context<Self>) {
        let (Some(ws), Some(s), Some(root)) = (self.persist.workspace_id, &self.persist.state, self.root(cx)) else { return };
        let Some(text) = s.get(&format!("tree:{ws}")) else { return };
        let dirs: Vec<String> = text.lines().filter(|l| !l.is_empty()).map(|rel| ui::join(&root, rel)).collect();
        self.roots.primary.update(cx, |t, cx| t.restore_expanded(dirs, cx));
    }

    /// Re-extract the active file's outline (if needed).
    pub(super) fn sync_outline(&mut self, cx: &mut Context<Self>) {
        if self.chrome.panel != Some(Panel::Explorer) || !self.sections.is_open("outline", true) {
            return;
        }
        let Some(ed) = self
            .tabs
            .active
            .filter(|_| !self.tabs.diff_active && !self.tabs.large_active && !self.tabs.page_active)
            .and_then(|i| self.tabs.editors.get(i))
            .cloned()
        else {
            self.outline = OutlineState::default();
            return;
        };
        let (path, version, lines) = {
            let e = ed.read(cx);
            (e.path.clone(), e.version(), e.line_count())
        };
        let key = Some((path.clone(), version));
        if self.outline.key == key {
            return;
        }
        // Defer re-extraction until 400 ms after the last one, not on every keystroke.
        let wait = std::time::Duration::from_millis(400);
        let same_file = self.outline.key.as_ref().is_some_and(|(p, _)| *p == path);
        if let (true, Some(at)) = (same_file, self.outline.computed_at)
            && at.elapsed() < wait
        {
            if !self.outline.refresh_pending {
                self.outline.refresh_pending = true;
                cx.spawn(async move |this, cx| {
                    cx.background_executor().timer(wait).await;
                    this.update(cx, |this, cx| {
                        this.outline.refresh_pending = false;
                        cx.notify();
                    })
                    .ok();
                })
                .detach();
            }
            return;
        }
        self.outline.key = key;
        self.outline.computed_at = Some(std::time::Instant::now());
        self.outline.too_large = lines > 30_000;
        if self.outline.too_large {
            self.outline.symbols.clear();
            self.outline.supported = true;
            return;
        }
        let text = ed.read(cx).text_lines();
        match crate::outline::extract(&path, &text) {
            Some(s) => {
                self.outline.symbols = s;
                self.outline.supported = true;
            }
            None => {
                self.outline.symbols.clear();
                self.outline.supported = false;
            }
        }
    }

    /// OUTLINE body: symbols, the one at the cursor highlighted.
    pub(super) fn outline_rows(&self, p: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let note = |t: &str| {
            div().px(px(theme::SPACE[6])).py(px(theme::SPACE[2])).text_color(c(p.text_muted)).text_size(px(text::XS.0)).child(t.to_string()).into_any_element()
        };
        let Some(ed) =
            self.tabs.active.and_then(|i| self.tabs.editors.get(i)).filter(|_| !self.tabs.diff_active && !self.tabs.large_active && !self.tabs.page_active)
        else {
            return note(crate::i18n::t("Open a file to see its structure."));
        };
        if self.outline.too_large {
            return note(crate::i18n::t("The file is too large for an outline."));
        }
        if !self.outline.supported {
            return note(crate::i18n::t("No outline for this file type."));
        }
        if self.outline.symbols.is_empty() {
            return note(crate::i18n::t("No symbols found."));
        }
        let cursor_line = ed.read(cx).cursor_position().0.saturating_sub(1);
        let here = crate::outline::current(&self.outline.symbols, cursor_line);
        let path = ed.read(cx).path.clone();
        let p = *p;
        // Virtualized list: in a file with tens of thousands of symbols only the visible rows are drawn.
        gpui::uniform_list(
            "outline-rows",
            self.outline.symbols.len(),
            cx.processor(move |this, range: std::ops::Range<usize>, _window, cx| {
                range
                    .filter_map(|ix| {
                        let s = this.outline.symbols.get(ix)?;
                        let (line, path) = (s.line as u32, path.clone());
                        Some(
                            div()
                                .id(("outline", ix))
                                .w_full()
                                .h(px(22.))
                                .flex()
                                .items_center()
                                .gap(px(theme::SPACE[3]))
                                .pl(px(theme::SPACE[6] + s.depth as f32 * 12.))
                                .pr(px(theme::SPACE[3]))
                                .cursor_pointer()
                                .when(here == Some(ix), |d| d.bg(c(p.surface_active)))
                                .hover(|d| d.bg(c(p.surface_hover)))
                                .child(
                                    div()
                                        .flex_none()
                                        .w(px(14.))
                                        .text_color(c(match s.kind {
                                            crate::outline::Kind::Function => p.syntax_function,
                                            crate::outline::Kind::Type => p.syntax_type,
                                            crate::outline::Kind::Module => p.syntax_keyword,
                                            crate::outline::Kind::Heading | crate::outline::Kind::Section => p.info,
                                        }))
                                        .child(s.kind.glyph()),
                                )
                                .child(div().flex_1().min_w_0().overflow_hidden().whitespace_nowrap().text_size(px(text::SM.0)).child(s.name.clone()))
                                .child(div().flex_none().text_size(px(text::XXS.0)).text_color(c(p.line_number)).child((s.line + 1).to_string()))
                                .on_click(cx.listener(move |this, _, window, cx| this.open_location(path.clone(), line, 0, window, cx))),
                        )
                    })
                    .collect::<Vec<_>>()
            }),
        )
        .size_full()
        .into_any_element()
    }

    pub(super) fn persist_sections(&mut self, cx: &mut Context<Self>) {
        if let Some(s) = &self.persist.state {
            let _ = s.set("explorer_sections", &self.sections.encode());
        }
        cx.notify();
    }
}
