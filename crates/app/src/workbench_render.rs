//! Workbench: rendering of the whole window (`Render`).

use super::*;

impl Workbench {
    /// Error card: bottom right, above the status bar; closed with ×.
    fn error_toast(&self, p: &Palette, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let Some(Notice::Error(message)) = &self.notice else { return None };
        Some(
            div()
                .id("error-toast")
                .absolute()
                .right(px(theme::SPACE[6]))
                .bottom(px(layout::STATUSBAR_H + theme::SPACE[4]))
                .w(px(420.))
                .flex()
                .items_start()
                .gap(px(theme::SPACE[3]))
                .p(px(theme::SPACE[4]))
                .bg(c(p.surface_raised))
                .border_1()
                .border_color(c(p.border))
                .rounded(px(theme::radius::MD))
                .shadow_lg()
                .text_size(px(text::SM.0))
                .text_color(c(p.text))
                .occlude()
                .child(div().flex_none().pt(px(1.)).child(icon("circle-x", 16., p.danger)))
                .child(div().flex_1().min_w_0().child(message.clone()))
                .when(matches!(self.remote.conn, Conn::Failed { .. }), |d| {
                    d.child(
                        div()
                            .id("error-toast-reconnect")
                            .flex_none()
                            .h(px(22.))
                            .px(px(theme::SPACE[4]))
                            .flex()
                            .items_center()
                            .rounded(px(theme::radius::SM))
                            .bg(c(p.accent))
                            .text_color(c(p.on_accent))
                            .text_size(px(text::XS.0))
                            .cursor_pointer()
                            .hover(|d| d.opacity(0.9))
                            .child(crate::i18n::t("Reconnect"))
                            .on_click(cx.listener(|this, _, window, cx| this.reconnect_lost(window, cx))),
                    )
                })
                .child(
                    div()
                        .id("error-toast-close")
                        .flex_none()
                        .size(px(20.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded(px(theme::radius::SM))
                        .cursor_pointer()
                        .hover(|d| d.bg(c(p.surface_hover)))
                        .child(icon("x", 14., p.text_muted))
                        .tooltip(|_, cx| cx.new(|_| Tooltip(crate::i18n::t("Close").into())).into())
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.notice = None;
                            cx.notify();
                        })),
                ),
        )
    }

    /// A side view's content, for whichever side bar shows it.
    pub(super) fn side_view(&mut self, panel: Panel, p: &Palette, cx: &mut Context<Self>) -> AnyElement {
        match panel {
            Panel::Explorer => self.explorer_panel(p, cx),
            Panel::Search => div()
                .flex()
                .flex_col()
                .size_full()
                .child(self.view_header(crate::i18n::upper(crate::i18n::t("Search")), p))
                .child(div().flex_1().min_h_0().child(self.search.clone()))
                .into_any_element(),
            Panel::Scm => self.scm.clone().into_any_element(),
            Panel::Debug => self.run_debug_view(p, cx),
            Panel::Testing => self.testing_view(p, cx),
            Panel::Remote => self.remote_view(p, cx),
            Panel::Containers => self.explorer_panel(p, cx),
            Panel::Extensions => self.extensions_view(p, cx),
            Panel::ExtView => self.ext_view_in(ext_manifest::ViewLocation::Sidebar, p, cx),
        }
    }

    fn finish_resize(&mut self, cx: &mut Context<Self>) {
        if self.sections.end_drag() {
            self.persist_sections(cx);
        }
        if std::mem::take(&mut self.chrome.resizing_sidebar) {
            if let Some(s) = &self.persist.state {
                let _ = s.set("sidebar_w", &format!("{:.0}", self.chrome.sidebar_w));
            }
            cx.notify();
        }
        if std::mem::take(&mut self.chrome.resizing_split) {
            self.tabs.resizing = None;
            self.persist_session(cx);
            cx.notify();
        }
        if std::mem::take(&mut self.chrome.resizing_term_list) {
            if let Some(s) = &self.persist.state {
                let _ = s.set("terminal_list_w", &format!("{:.0}", self.chrome.term_list_w));
            }
            cx.notify();
        }
        if std::mem::take(&mut self.chrome.resizing_panel) {
            if let Some(s) = &self.persist.state {
                let _ = s.set("panel_h", &format!("{:.0}", self.chrome.panel_h));
                let _ = s.set("panel_w", &format!("{:.0}", self.chrome.panel_w));
            }
            cx.notify();
        }
    }

    /// The open context menu over the window, from `top` down (a click outside closes it).
    pub(super) fn menu_layer(&self, top: f32, cx: &mut Context<Self>) -> Option<AnyElement> {
        let (menu, position, anchor) = self.menu.as_ref().map(|m| (m.menu.clone(), m.position, m.menu.read(cx).anchor()))?;
        Some(
            deferred(
                div()
                    .id("menu-backdrop")
                    .absolute()
                    .top(px(top))
                    .left_0()
                    .right_0()
                    .bottom_0()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, window, cx| {
                            this.close_menu(window, cx);
                        }),
                    )
                    .on_mouse_down(
                        MouseButton::Right,
                        cx.listener(|this, _, window, cx| {
                            this.close_menu(window, cx);
                        }),
                    )
                    .child(gpui::anchored().position(position).anchor(anchor).snap_to_window().child(menu)),
            )
            .into_any_element(),
        )
    }

    /// The open picker (command palette, quick open, go to line), `top` below the window's top edge.
    pub(super) fn picker_layer(&self, top: f32, cx: &mut Context<Self>) -> Option<AnyElement> {
        let picker = self.overlay.as_ref()?.picker.clone();
        Some(
            deferred(
                div()
                    .id("picker-backdrop")
                    .absolute()
                    .top_0()
                    .left_0()
                    .size_full()
                    .flex()
                    .justify_center()
                    .pt(px(top))
                    // Click outside = Esc.
                    .on_mouse_down(MouseButton::Left, cx.listener(|this, _, window, cx| this.on_overlay_event(&PickerEvent::Dismiss, window, cx)))
                    .child(div().occlude().child(picker)),
            )
            .into_any_element(),
        )
    }
}

impl Render for Workbench {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Accept restored legacy sidebar routes, but keep a single inventory surface.
        if self.view_shown(Panel::Containers) && cx.global::<Settings>().docker_enabled {
            self.show_containers(cx);
        }
        let me = window.window_handle().window_id();
        self.chrome.a11y_bounds.clear(me);
        let open: Vec<gpui::WindowId> = std::iter::once(me).chain(self.chrome.view_windows.iter().map(|w| w.window.window_id())).collect();
        self.chrome.a11y_bounds.retain(&open);
        self.step_panel_anim(window);
        // The repository search waits until Source Control is on screen (connecting opens a home folder: no scan unasked).
        if self.view_shown(Panel::Scm) {
            self.discover_if_pending(cx);
        }
        self.update_title(window, cx);
        self.chrome.window_bounds = Some(window.window_bounds());
        self.sync_manage_menu(window, cx);
        self.track_notice(cx);
        self.sync_watches(cx);
        self.sync_ext_scm(window, cx);
        self.sync_slots(cx);
        self.sync_timeline(cx);
        self.sync_outline(cx);
        self.sync_ext_views_frame(cx);
        self.sync_update_window(window, cx);
        let remote = self.remote.host.is_remote();
        let project = self.root(cx).map(std::path::PathBuf::from);
        self.chrome.resources.update(cx, |monitor, cx| monitor.set_project(project, remote, cx));
        // Whatever had the keyboard went away (a tab closed with the old folder when a workspace opened, for one): the
        // keyboard stays in the window, or no shortcut works until something is clicked.
        if window.focused(cx).is_none() {
            window.focus(&self.focus_handle);
        }
        if let Some(ws) = self.persist.pending_restore.take() {
            self.restore_tabs(ws, window, cx);
            // A terminal asked for before the restore started, when no restore of the panel followed after all.
            if !self.persist.panel_restoring {
                self.open_wanted_terminal(window, cx);
            }
        }
        let p = *palette(cx);
        let main: AnyElement = match (self.tab_editor(), &self.tabs.open_error) {
            (_, Some((path, msg))) => center(&p)
                .child(icon("alert", 32., p.warning))
                .child(div().font_weight(gpui::FontWeight::SEMIBOLD).child(ui::file_name(path).to_string()))
                .child(div().max_w(px(480.)).text_color(c(p.text_muted)).child(msg.clone()))
                .when(msg.contains("binary file") || *msg == crate::i18n::t("This looks like a binary file, so it was not opened as text."), |d| {
                    let target = path.clone();
                    let entity = cx.entity();
                    d.child(button("open-hex", crate::i18n::t("Open as Hex"), true, &p, move |_, window, cx| {
                        let target = target.clone();
                        entity.update(cx, |this, cx| this.open_hex_of(target, window, cx));
                    }))
                })
                .into_any_element(),
            (_, None) if self.tabs.page_active && self.tabs.page.is_some() => match self.tabs.page.clone().unwrap() {
                Page::Welcome => self.welcome_view(&p, cx),
                Page::About(view) => view.into_any_element(),
                Page::Containers => self.container_dashboard_view(&p, cx),
                Page::Extension => self.extension_page(&p, cx),
                Page::ExtensionInspector => self.extension_inspector(&p, cx),
                Page::Settings(view) => view.into_any_element(),
                Page::Markdown(view) => view.into_any_element(),
                Page::Shortcuts(view) => view.into_any_element(),
                Page::LanguageServices(view) => view.into_any_element(),
                Page::Merge(view) => view.into_any_element(),
                Page::Compare(view) => view.into_any_element(),
                Page::Search(view) => view.into_any_element(),
                Page::ExtView(id) => self.ext_view_frame(&id, &p, cx),
                Page::Terminal(view) => self.editor_terminal_view(view, &p),
            },
            (_, None) if self.tabs.diff_active && self.tabs.diff.is_some() => self.tabs.diff.clone().unwrap().into_any_element(),
            (_, None) if self.tabs.large_active && self.tabs.large.is_some() => self.tabs.large.clone().unwrap().into_any_element(),
            (Some(ed), None) => editor::slot_view(ed.clone(), self.focused_slot()).into_any_element(),
            (None, None) => self.empty_state(&p, cx),
        };
        // A view asked for in the primary side bar that lives in the secondary one opens there.
        self.place_views();
        self.place_window_views(window, cx);
        let sidebar = self.chrome.panel.map(|panel| self.side_view(panel, &p, cx));
        let secondary = self.chrome.secondary.map(|panel| self.side_view(panel, &p, cx));
        // While a view is dragged: drop zones over each side (at least `ZONE_MIN` wide, also where no side bar is open).
        const ZONE_MIN: f32 = 180.;
        let bar = if cx.global::<Settings>().activity_bar { layout::ACTIVITYBAR_W } else { 0. };
        let left_w = (bar + if sidebar.is_some() { self.chrome.sidebar_w } else { 0. }).max(ZONE_MIN);
        let right_w = if secondary.is_some() { self.chrome.sidebar_w } else { ZONE_MIN };
        let drop_zones = self.view_drop_zones(left_w, right_w, &p, cx);

        self.with_actions(div().id("workbench"), cx)
            .key_context("Workbench")
            .track_focus(&self.focus_handle)
            .capture_key_down(cx.listener(|this, _, _, cx| {
                ui::set_keyboard_focus(true, cx);
                this.leave_editor_window();
            }))
            .capture_any_mouse_down(cx.listener(|this, _, _, cx| {
                ui::set_keyboard_focus(false, cx);
                this.leave_editor_window();
            }))
            .size_full()
            .flex()
            .flex_col()
            .bg(c(p.surface))
            .font_family(cx.global::<Settings>().ui_font.clone().map_or(SharedString::from(ui::UI_FONT), SharedString::from))
            .text_size(px(text::SM.0))
            .line_height(px(text::SM.1))
            .text_color(c(p.text))
            // Sidebar resizing: the drag is tracked everywhere in the window.
            .on_mouse_move(cx.listener(|this, ev: &gpui::MouseMoveEvent, window, cx| {
                // A release outside the window may not have delivered a mouse-up event.
                if ev.pressed_button != Some(MouseButton::Left) {
                    this.finish_resize(cx);
                    return;
                }
                if this.sections.dragging() && this.sections.drag_to(ev.position.y / px(1.), window.viewport_size().height / px(1.) - 220.) {
                    cx.notify();
                }
                if this.chrome.resizing_sidebar {
                    let x = ev.position.x / px(1.) - layout::ACTIVITYBAR_W;
                    this.chrome.sidebar_w = x.clamp(layout::SIDEBAR_MIN, layout::SIDEBAR_MAX);
                    cx.notify();
                }
                if let Some(resize) = this.tabs.resizing.clone() {
                    if this.tabs.contains_key(resize.anchor) && this.tabs.revision == resize.revision {
                        let bounds = resize.bounds.get();
                        let (position, start, size) = match resize.axis {
                            crate::editor_layout::Axis::Horizontal => (ev.position.x, bounds.origin.x, bounds.size.width),
                            crate::editor_layout::Axis::Vertical => (ev.position.y, bounds.origin.y, bounds.size.height),
                        };
                        let ratio = (position - start) / size.max(px(1.));
                        if this.tabs.layout.set_ratio(resize.split, ratio) {
                            cx.notify();
                        }
                    } else {
                        this.finish_resize(cx);
                    }
                }
                // Docked left, the panel starts at the editor area's left edge and is resized from its right edge.
                let left_dock = (this.chrome.panel_dock == docks_glue::PanelDock::Left).then(|| this.chrome.panel_area.map_or(0., |a| a.origin.x / px(1.)));
                if this.chrome.resizing_term_list {
                    // The list sits at the right edge of the panel: the right edge of the window, or of a panel docked left.
                    let total = window.viewport_size().width / px(1.);
                    let right = left_dock.map_or(total, |x| x + this.chrome.panel_w);
                    this.chrome.term_list_w = term_list_width(right - ev.position.x / px(1.), this.panel_width(total));
                    cx.notify();
                }
                if this.chrome.resizing_panel && this.chrome.panel_dock.beside() {
                    let total = window.viewport_size().width / px(1.);
                    let w = match left_dock {
                        Some(x) => ev.position.x / px(1.) - x,
                        None => total - ev.position.x / px(1.),
                    };
                    this.chrome.panel_w = w.clamp(PANEL_MIN * 2., (total - 300.).max(PANEL_MIN * 2.));
                    cx.notify();
                } else if this.chrome.resizing_panel {
                    let total = window.viewport_size().height / px(1.);
                    let h = total - ev.position.y / px(1.) - layout::STATUSBAR_H;
                    this.chrome.panel_h = h.clamp(PANEL_MIN, (total - 200.).max(PANEL_MIN));
                    cx.notify();
                }
            }))
            // Finish before editor/terminal handlers can consume the release.
            .capture_any_mouse_up(cx.listener(|this, ev: &gpui::MouseUpEvent, window, cx| {
                if ev.button == MouseButton::Left {
                    this.finish_resize(cx);
                    // The drop zones go; a drop on one is still delivered for this release.
                    this.end_view_drag(cx);
                    // A tab released inside the window is the drop targets' business.
                    this.end_tab_drag(ev.position, window, cx);
                }
            }))
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|this, ev: &gpui::MouseUpEvent, window, cx| {
                    this.finish_resize(cx);
                    // A tab released outside the window leaves it.
                    this.end_tab_drag(ev.position, window, cx);
                }),
            )
            .on_action(cx.listener(|this, _: &crate::menus::FileMenu, w, cx| this.open_application_menu_by_index(0, w, cx)))
            .on_action(cx.listener(|this, _: &crate::menus::EditMenu, w, cx| this.open_application_menu_by_index(1, w, cx)))
            .on_action(cx.listener(|this, _: &crate::menus::SelectionMenu, w, cx| this.open_application_menu_by_index(2, w, cx)))
            .on_action(cx.listener(|this, _: &crate::menus::ViewMenu, w, cx| this.open_application_menu_by_index(3, w, cx)))
            .on_action(cx.listener(|this, _: &crate::menus::GoMenu, w, cx| this.open_application_menu_by_index(4, w, cx)))
            .on_action(cx.listener(|this, _: &crate::menus::TerminalMenu, w, cx| this.open_application_menu_by_index(5, w, cx)))
            .on_action(cx.listener(|this, _: &crate::menus::WindowMenu, w, cx| this.open_application_menu_by_index(6, w, cx)))
            .on_action(cx.listener(|this, _: &crate::menus::HelpMenu, w, cx| this.open_application_menu_by_index(7, w, cx)))
            .when(cfg!(target_os = "macos"), |d| d.child(self.title_bar(window, &p, cx)))
            .when(cfg!(target_os = "windows") && !window.is_fullscreen(), |d| d.child(self.windows_title_bar(window, &p, cx)))
            .when(!cfg!(target_os = "macos") && self.chrome.zen.is_none(), |d| d.child(self.application_menu_bar(&p, cx)))
            .when(self.docker_connected() && self.remote.docker.monitor.return_workspace.is_some(), |d| {
                d.child(
                    div()
                        .id("container-return-workspace")
                        .debug_selector(|| "container-return-workspace".into())
                        .w_full()
                        .min_h(px(30.))
                        .px(px(theme::SPACE[5]))
                        .flex()
                        .items_center()
                        .gap(px(theme::SPACE[3]))
                        .bg(c(p.surface_active))
                        .text_color(c(p.text))
                        .text_size(px(text::SM.0))
                        .cursor_pointer()
                        .child(icon("arrow-left", 14., p.text))
                        .child(crate::i18n::t("Back to SSH workspace and container details"))
                        .on_click(cx.listener(|this, _, window, cx| this.return_from_container(window, cx))),
                )
            })
            .child(
                div()
                    .relative()
                    .flex()
                    .flex_1()
                    .min_h_0()
                    .when(self.chrome.zen.is_none() && cx.global::<Settings>().activity_bar, |d| d.child(self.activity_bar(&p, cx)))
                    .when_some(sidebar, |d, s| {
                        let resizing = self.chrome.resizing_sidebar;
                        d.child(
                            div().relative().w(px(self.chrome.sidebar_w)).flex_none().bg(c(p.surface)).border_r_1().border_color(c(p.border)).child(s).child(
                                div()
                                    .id("sidebar-sash")
                                    .absolute()
                                    // Keep the explorer row underneath from receiving the mouse press (drag and drop).
                                    .occlude()
                                    .top_0()
                                    .bottom_0()
                                    .right(px(-layout::SASH / 2.))
                                    .w(px(layout::SASH))
                                    .cursor(gpui::CursorStyle::ResizeLeftRight)
                                    .child(crate::ui_sash::highlight(p.accent, resizing))
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(|this, _, _, cx| {
                                            this.chrome.resizing_sidebar = true;
                                            cx.notify();
                                        }),
                                    ),
                            ),
                        )
                    })
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .relative()
                            .when(!self.chrome.panel_dock.beside(), |d| d.flex_col())
                            .child(Self::panel_area_probe(cx))
                            // Docked left, the panel comes before the editor.
                            .when(self.chrome.panel_open && self.chrome.panel_dock == docks_glue::PanelDock::Left, |d| d.child(self.panel(&p, cx)))
                            // A maximized panel takes the editor's place; while it grows or shrinks, the editor gives way.
                            .when(!(self.chrome.panel_open && self.chrome.panel_max) || self.chrome.panel_anim.is_some(), |d| {
                                d.child(
                                    div()
                                        .debug_selector(|| "editor-area".into())
                                        .relative()
                                        .overflow_hidden()
                                        .flex_1()
                                        .min_w_0()
                                        .min_h_0()
                                        .flex()
                                        .flex_col()
                                        .child(self.editor_area(main, &p, cx))
                                        .children(self.debug_toolbar(&p, cx)),
                                )
                            })
                            .when(self.chrome.panel_open && self.chrome.panel_dock != docks_glue::PanelDock::Left, |d| d.child(self.panel(&p, cx))),
                    )
                    // The secondary side bar: views moved to the right of the editor.
                    .when_some(secondary.filter(|_| self.chrome.zen.is_none()), |d, s| {
                        d.child(
                            div()
                                .debug_selector(|| "secondary-side-bar".into())
                                .w(px(self.chrome.sidebar_w))
                                .flex_none()
                                .bg(c(p.surface))
                                .border_l_1()
                                .border_color(c(p.border))
                                .child(s),
                        )
                    })
                    .children(drop_zones),
            )
            .when(self.chrome.zen.is_none() && cx.global::<Settings>().status_bar, |d| d.child(self.status_bar(&p, window.viewport_size().width / px(1.), cx)))
            .children(self.error_toast(&p, cx))
            .children(self.update_dialog(&p, cx))
            // A menu or picker opened from an editor's own window is drawn there.
            .when(self.hosted_editor().is_none() && self.view_host_window().is_none(), |d| {
                let menu_top = if self.menu.as_ref().is_some_and(|m| m.app_path.is_some()) { self.application_menu_bottom(window) } else { 0. };
                let picker_top = if cfg!(target_os = "macos") {
                    layout::TITLEBAR_H + 4.
                } else if cfg!(target_os = "windows") && !window.is_fullscreen() {
                    view_editor_glue::WINDOWS_TITLEBAR_H + 4.
                } else {
                    theme::SPACE[4]
                };
                d.children(self.menu_layer(menu_top, cx)).children(self.picker_layer(picker_top, cx))
            })
    }
}

#[cfg(test)]
mod resize_tests {
    use super::*;
    use gpui::{Modifiers, TestAppContext, VisualTestContext, point};

    #[gpui::test]
    fn status_bar_keeps_resource_control_visible_with_long_content(cx: &mut TestAppContext) {
        let (wb, cx) = crate::qa_workbench_tests::setup(cx);
        let project = crate::qa_workbench_tests::project("status-overflow");
        crate::qa_workbench_tests::open_in(&wb, &project.join("src/main.rs"), cx);
        wb.update(cx, |wb, cx| {
            wb.vcs.branch = Some("feature/very-long-branch-name-".repeat(12).into());
            wb.notice = Some(Notice::Info("A long workbench notification. ".repeat(80).into()));
            cx.notify();
        });
        for width in [480., 800., 1280.] {
            cx.simulate_resize(gpui::size(px(width), px(800.)));
            cx.run_until_parked();
            let bar = cx.debug_bounds("status-bar").unwrap();
            let resources = cx.debug_bounds("sb-resources").unwrap();
            assert_eq!(bar.size.height, px(28.));
            assert!(resources.left() >= bar.left() && resources.right() <= bar.right(), "resource control at {width}: {resources:?} inside {bar:?}");
            assert!(resources.size.width >= px(28.) && resources.size.height >= px(24.));
        }
    }

    #[gpui::test]
    fn dense_status_bar_preserves_controls_in_a_narrow_window(cx: &mut TestAppContext) {
        let (wb, cx) = crate::qa_workbench_tests::setup(cx);
        let project = crate::qa_workbench_tests::project("status-dense");
        let path = project.join("src/main.rs");
        std::fs::write(&path, "<<<<<<< HEAD\nfn first() {}\n=======\nfn second() {}\n>>>>>>> qa\n").unwrap();
        crate::qa_workbench_tests::open_in(&wb, &path, cx);
        wb.update(cx, |wb, cx| {
            wb.active_editor().unwrap().update(cx, |ed, cx| ed.insert_at_for_test(0, "// unsaved\n", cx));
            wb.vcs.branch = Some("feature/long-branch-name-".repeat(12).into());
            wb.vcs.status = Some(std::sync::Arc::new(crate::git::Status { ahead: 1294, behind: 982, ..Default::default() }));
            wb.lsp.progress = Some("Indexing a large workspace… ".repeat(20).into());
            wb.update.state = update_glue::UpdateState::Ready { version: "999.999.999".into(), plan: None };
            cx.notify();
        });
        for width in [480., 640., 800., 1100., 1280., 1920.] {
            cx.simulate_resize(gpui::size(px(width), px(800.)));
            cx.run_until_parked();
            let bar = cx.debug_bounds("status-bar").unwrap();
            let resources = cx.debug_bounds("sb-resources").unwrap();
            assert!(
                resources.left() >= bar.left() && resources.right() + px(32.) <= bar.right(),
                "resource and terminal controls at {width}: {resources:?} inside {bar:?}"
            );
            for id in ["sb-conflicts", "sb-unsaved", "sb-update"] {
                let item = cx.debug_bounds(id).unwrap();
                assert!(item.left() >= bar.left() && item.right() <= bar.right(), "{id} at {width}: {item:?} inside {bar:?}");
            }
        }
    }

    #[gpui::test]
    fn explorer_sections_resize_from_above_their_headers(cx: &mut TestAppContext) {
        let (wb, cx) = crate::qa_workbench_tests::setup(cx);
        let project = crate::qa_workbench_tests::project("section-edges");
        crate::qa_workbench_tests::open(&wb, &project, cx);
        wb.update(cx, |wb, cx| {
            wb.chrome.panel = Some(Panel::Explorer);
            wb.vcs.status = Some(std::sync::Arc::new(crate::git::Status::default()));
            cx.notify();
        });
        cx.run_until_parked();
        for (id, sash_id, header_id) in [("outline", "sash-outline", "sec-outline"), ("timeline", "sash-timeline", "sec-timeline")] {
            let sash = cx.debug_bounds(sash_id).unwrap();
            let header = cx.debug_bounds(header_id).unwrap();
            assert!(sash.bottom() <= header.top(), "{id}: the resize target must be above the header");
            let at = sash.center();
            let before = wb.read_with(cx, |wb, _| wb.sections.height(id, 170.));
            cx.simulate_mouse_down(at, MouseButton::Left, Modifiers::none());
            cx.simulate_mouse_move(at - point(px(0.), px(40.)), Some(MouseButton::Left), Modifiers::none());
            cx.simulate_mouse_up(at - point(px(0.), px(40.)), MouseButton::Left, Modifiers::none());
            assert_eq!(wb.read_with(cx, |wb, _| wb.sections.height(id, 170.)), before + 40.);
            assert!(wb.read_with(cx, |wb, _| wb.sections.is_open(id, true)), "resizing must not collapse the section");
            assert!(!wb.read_with(cx, |wb, _| wb.sections.dragging()));
        }
    }

    #[cfg(windows)]
    #[gpui::test]
    fn windows_titlebar_mouse_events_preserve_native_drag_and_close_defaults(cx: &mut TestAppContext) {
        let (_wb, cx) = crate::qa_workbench_tests::setup(cx);
        cx.run_until_parked();
        for id in ["window-drag-region", "window-minimize", "window-maximize", "window-close"] {
            let position = cx.debug_bounds(id).unwrap().center();
            for click_count in [1, 2] {
                cx.update(|window, cx| {
                    window.dispatch_event(
                        gpui::PlatformInput::MouseMove(gpui::MouseMoveEvent { position, pressed_button: None, modifiers: Modifiers::none() }),
                        cx,
                    );
                    let down = window.dispatch_event(
                        gpui::PlatformInput::MouseDown(gpui::MouseDownEvent {
                            button: MouseButton::Left,
                            position,
                            modifiers: Modifiers::none(),
                            click_count,
                            first_mouse: false,
                        }),
                        cx,
                    );
                    assert!(down.propagate, "{id} must reach native non-client handling");
                    assert!(!down.default_prevented, "{id} must not trigger the root focus handler");
                    let up = window.dispatch_event(
                        gpui::PlatformInput::MouseUp(gpui::MouseUpEvent { button: MouseButton::Left, position, modifiers: Modifiers::none(), click_count }),
                        cx,
                    );
                    assert!(up.propagate, "{id} must reach native button release handling");
                });
            }
        }
    }

    #[cfg(windows)]
    #[gpui::test]
    fn windows_custom_titlebar_keeps_controls_separate_at_minimum_width(cx: &mut TestAppContext) {
        let (_wb, cx) = crate::qa_workbench_tests::setup(cx);
        for width in [1280., 480.] {
            cx.simulate_resize(gpui::size(px(width), px(800.)));
            cx.run_until_parked();
            let bar = cx.debug_bounds("windows-title-bar").unwrap();
            let drag = cx.debug_bounds("window-drag-region").unwrap();
            let min = cx.debug_bounds("window-minimize").unwrap();
            let max = cx.debug_bounds("window-maximize").unwrap();
            let close = cx.debug_bounds("window-close").unwrap();
            assert_eq!(bar.size.height, px(34.));
            assert!(drag.size.width > px(0.));
            assert!(drag.right() <= min.left());
            assert!(min.right() <= max.left());
            assert!(max.right() <= close.left());
            assert!(close.right() <= bar.right());
            assert!(drag.top() > bar.top(), "keep the native top resize strip clear");
        }
    }

    #[cfg(windows)]
    #[gpui::test]
    fn windows_workspace_paths_reuse_tabs_with_both_separators(cx: &mut TestAppContext) {
        use crate::qa_workbench_tests::{open, open_in, project, setup};
        let dir = project("windows-şablon boşluk");
        let path = dir.join("src/main.rs");
        let (wb, cx) = setup(cx);
        open(&wb, &dir, cx);
        open_in(&wb, &path, cx);
        let root = wb.read_with(cx, |wb, cx| wb.root(cx).unwrap());
        assert!(!root.contains('\\'), "{root}");
        assert!(!root.starts_with("//?/"), "{root}");
        let normalized = ui::local_path(&path);
        wb.update_in(cx, |wb, window, cx| wb.open_file(normalized.clone(), window, cx));
        cx.run_until_parked();
        let paths = wb.read_with(cx, |wb, cx| wb.open_paths(cx));
        assert_eq!(paths, vec![normalized]);
    }

    // Exercise the real event dispatch, including releases outside the root hitbox and
    // missed mouse-up events when a drag leaves the window.
    #[gpui::test]
    fn resize_stops_on_release_or_unpressed_move(cx: &mut TestAppContext) {
        let (wb, cx) = crate::qa_workbench_tests::setup(cx);
        for release in [Some(point(px(500.), px(200.))), Some(point(px(-10.), px(-10.))), None] {
            wb.update(cx, |wb, cx| {
                wb.chrome.resizing_sidebar = true;
                wb.chrome.resizing_panel = true;
                wb.chrome.resizing_split = true;
                cx.notify();
            });
            cx.run_until_parked();
            let sizes = wb.read_with(cx, |wb, _| (wb.chrome.sidebar_w, wb.chrome.panel_h, wb.chrome.panel_w));
            if let Some(position) = release {
                // An unrelated button must not end a left-button drag.
                cx.simulate_mouse_up(position, MouseButton::Right, Modifiers::none());
                assert!(wb.read_with(cx, |wb, _| wb.chrome.resizing_sidebar && wb.chrome.resizing_panel && wb.chrome.resizing_split));
                cx.simulate_mouse_up(position, MouseButton::Left, Modifiers::none());
                assert!(wb.read_with(cx, |wb, _| !wb.chrome.resizing_sidebar && !wb.chrome.resizing_panel && !wb.chrome.resizing_split));
            }
            cx.simulate_mouse_move(point(px(600.), px(300.)), None, Modifiers::none());
            assert!(wb.read_with(cx, |wb, _| !wb.chrome.resizing_sidebar && !wb.chrome.resizing_panel && !wb.chrome.resizing_split));
            assert_eq!(wb.read_with(cx, |wb, _| (wb.chrome.sidebar_w, wb.chrome.panel_h, wb.chrome.panel_w)), sizes);
        }
    }

    // Moving a maximized panel used to flip only the docking side, which a maximized panel ignores: nothing moved.
    #[gpui::test]
    fn moving_a_maximized_panel_restores_its_size(cx: &mut TestAppContext) {
        let (wb, cx) = crate::qa_workbench_tests::setup(cx);
        let start = wb.read_with(cx, |wb, _| wb.panel_layout_for_test().1);
        cx.dispatch_action(ToggleMaximizedPanel);
        assert_eq!(wb.read_with(cx, |wb, _| wb.panel_layout_for_test()), (true, start, true));
        cx.dispatch_action(TogglePanelPosition);
        assert_eq!(wb.read_with(cx, |wb, _| wb.panel_layout_for_test()), (true, !start, false), "moved and back to its own size");
        cx.dispatch_action(ToggleMaximizedPanel);
        assert_eq!(wb.read_with(cx, |wb, _| wb.panel_layout_for_test()), (true, !start, true), "maximizing on the new side");
    }

    /// Maximize grows the open panel towards the top and restore shrinks it back, both animated; reduced motion jumps.
    #[gpui::test]
    fn maximize_and_restore_animate_the_panel_size(cx: &mut TestAppContext) {
        let (wb, cx) = crate::qa_workbench_tests::setup(cx);
        wb.update(cx, |wb, cx| {
            wb.chrome.panel_dock = docks_glue::PanelDock::Bottom;
            wb.chrome.panel_open = true;
            cx.notify();
        });
        cx.run_until_parked();
        let own = cx.debug_bounds("panel").unwrap();
        let editor = cx.debug_bounds("editor-area").unwrap();
        let full = own.bottom() - editor.top();
        let frame = |cx: &mut VisualTestContext| {
            cx.update(|window, _| window.refresh());
            cx.run_until_parked();
        };

        cx.dispatch_action(ToggleMaximizedPanel);
        frame(cx);
        let growing = cx.debug_bounds("panel").unwrap();
        assert!(wb.read_with(cx, |wb, _| wb.chrome.panel_anim.is_some()), "an open panel animates");
        assert!(growing.size.height >= own.size.height && growing.size.height < full, "it starts from its own size: {growing:?}");
        assert_eq!(growing.bottom(), own.bottom(), "it grows towards the top, its bottom stays");
        std::thread::sleep(crate::workbench::panels_glue::PANEL_ANIM_FOR_TEST + std::time::Duration::from_millis(60));
        frame(cx);
        frame(cx);
        assert!(wb.read_with(cx, |wb, _| wb.chrome.panel_anim.is_none() && wb.chrome.panel_max));
        assert_eq!(cx.debug_bounds("panel").unwrap().size.height, full, "maximized: the whole area");

        cx.dispatch_action(ToggleMaximizedPanel);
        frame(cx);
        let shrinking = cx.debug_bounds("panel").unwrap();
        assert!(shrinking.size.height > own.size.height && shrinking.size.height <= full, "restore shrinks from the top: {shrinking:?}");
        std::thread::sleep(crate::workbench::panels_glue::PANEL_ANIM_FOR_TEST + std::time::Duration::from_millis(60));
        frame(cx);
        frame(cx);
        assert_eq!(cx.debug_bounds("panel").unwrap().size.height, own.size.height, "back to its own size");

        // Reduced motion: no animation at all.
        cx.update(|_, cx| cx.global_mut::<Settings>().reduce_motion = Some(true));
        cx.dispatch_action(ToggleMaximizedPanel);
        assert!(wb.read_with(cx, |wb, _| wb.chrome.panel_anim.is_none() && wb.chrome.panel_max));
    }

    /// Opening a file while the panel grows ends the animation with the editor visible.
    #[gpui::test]
    fn an_overtaken_maximize_animation_ends_cleanly(cx: &mut TestAppContext) {
        let (wb, cx) = crate::qa_workbench_tests::setup(cx);
        wb.update(cx, |wb, cx| {
            wb.chrome.panel_open = true;
            cx.notify();
        });
        cx.run_until_parked();
        cx.dispatch_action(ToggleMaximizedPanel);
        wb.update(cx, |wb, cx| {
            wb.chrome.panel_max = false;
            cx.notify();
        });
        cx.update(|window, _| window.refresh());
        cx.run_until_parked();
        assert!(wb.read_with(cx, |wb, _| wb.chrome.panel_anim.is_none()), "an animation the state no longer wants is dropped");
        assert!(cx.debug_bounds("editor-area").is_some());
    }

    // The terminal list's left edge drags like the other sashes, keeps room for the terminals and is remembered.
    #[gpui::test]
    fn the_terminal_list_resizes_by_its_edge(cx: &mut TestAppContext) {
        let (wb, cx) = crate::qa_workbench_tests::setup(cx);
        let total = cx.update(|window, _| window.viewport_size().width / px(1.));
        wb.update(cx, |wb, cx| {
            wb.chrome.resizing_term_list = true;
            cx.notify();
        });
        cx.simulate_mouse_move(point(px(total - 300.), px(300.)), Some(MouseButton::Left), Modifiers::none());
        assert_eq!(wb.read_with(cx, |wb, _| wb.chrome.term_list_w), 300.);
        // Far to the left: as wide as allowed, never wider.
        cx.simulate_mouse_move(point(px(10.), px(300.)), Some(MouseButton::Left), Modifiers::none());
        assert_eq!(wb.read_with(cx, |wb, _| wb.chrome.term_list_w), TERM_LIST_MAX);
        // Close to the right edge: the narrowest.
        cx.simulate_mouse_move(point(px(total - 50.), px(300.)), Some(MouseButton::Left), Modifiers::none());
        assert_eq!(wb.read_with(cx, |wb, _| wb.chrome.term_list_w), TERM_LIST_MIN);
        cx.simulate_mouse_up(point(px(total - 50.), px(300.)), MouseButton::Left, Modifiers::none());
        assert!(!wb.read_with(cx, |wb, _| wb.chrome.resizing_term_list));
        // A narrow panel keeps room for the terminals beside the list.
        assert_eq!(term_list_width(400., 500.), 300.);
        assert_eq!(term_list_width(400., 250.), TERM_LIST_MIN, "never narrower than the minimum");
    }
}
