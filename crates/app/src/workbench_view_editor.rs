//! Workbench: rendering: editor area, tabs, breadcrumb, title bar, empty screen.

use super::*;

pub(super) const WINDOWS_TITLEBAR_H: f32 = 34.;

impl Workbench {
    /// Editor area: a single group or two side by side (with a draggable divider between).
    pub(super) fn editor_area(&self, main: AnyElement, p: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let hint = || center(p).text_color(c(p.text_muted)).child(crate::i18n::t("Empty group: ⌘P opens a file here")).into_any_element();
        // When split, a short hint in an empty group instead of the welcome screen.
        let main =
            if self.tabs.other.is_some() && self.tabs.editors.is_empty() && self.tabs.diff.is_none() && self.tabs.open_error.is_none() { hint() } else { main };
        let focused_pane = div()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .flex()
            .flex_col()
            .when(
                (!self.tabs.editors.is_empty() && cx.global::<Settings>().show_tabs)
                    || self.tabs.diff.is_some()
                    || self.tabs.large.is_some()
                    || self.tabs.page.is_some(),
                |d| d.child(self.tabs(true, p, cx)),
            )
            .children(
                (!self.tabs.diff_active && !self.tabs.large_active && !self.tabs.page_active && cx.global::<Settings>().breadcrumbs)
                    .then(|| self.breadcrumbs(p, cx))
                    .flatten(),
            )
            // --- data ---
            .children(self.data_query_bar(p, cx))
            .child(div().flex_1().min_h_0().child(main));
        let Some(g) = &self.tabs.other else { return focused_pane.into_any_element() };
        let other_main = match g.active.and_then(|a| g.editors.get(a)) {
            Some(ed) => editor::slot_view(ed.clone(), 1 - self.focused_slot()).into_any_element(),
            None => hint(),
        };
        let other_pane = div()
            .id("other-group")
            .flex_1()
            .min_w_0()
            .min_h_0()
            .flex()
            .flex_col()
            .when(!g.editors.is_empty(), |d| d.child(self.tabs(false, p, cx)))
            .child(div().flex_1().min_h_0().child(other_main))
            // Clicking inside moves focus to this group (the editor's own click is handled too).
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    this.swap_groups();
                    cx.notify();
                }),
            );
        // Left/right: drawn on whichever side the focused group is; the focus frame is a thin accent line.
        let ratio = self.chrome.split_ratio;
        let (left, right): (AnyElement, AnyElement) = if self.tabs.focused_left {
            (focused_pane.into_any_element(), other_pane.into_any_element())
        } else {
            (other_pane.into_any_element(), focused_pane.into_any_element())
        };
        let resizing = self.chrome.resizing_split;
        div()
            .id("editor-groups")
            .flex_1()
            .min_h_0()
            .flex()
            .child(div().flex_basis(gpui::relative(ratio)).flex_shrink().min_w_0().flex().child(left))
            .child(
                div()
                    .id("group-sash")
                    .w(px(layout::SASH))
                    .flex_none()
                    .h_full()
                    .bg(c(p.border))
                    .cursor(gpui::CursorStyle::ResizeLeftRight)
                    .child(crate::ui_sash::highlight(p.accent, resizing))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, _, cx| {
                            cx.stop_propagation();
                            this.chrome.resizing_split = true;
                            cx.notify();
                        }),
                    ),
            )
            .child(div().flex_basis(gpui::relative(1. - ratio)).flex_shrink().min_w_0().flex().child(right))
            .into_any_element()
    }

    /// A group's tab bar. `focused`: whether it is the focused group (if not, clicks
    /// give focus to that group first; indices are relative to that group's list).
    pub(super) fn tabs(&self, focused: bool, p: &Palette, cx: &mut Context<Self>) -> impl IntoElement {
        let (editors, active_ix): (&[Entity<EditorView>], Option<usize>) =
            if focused { (&self.tabs.editors, self.tabs.active) } else { self.tabs.other.as_ref().map_or((&[][..], None), |g| (&g.editors[..], g.active)) };
        let diff_here = focused && (self.tabs.diff_active || self.tabs.large_active || self.tabs.page_active);
        // Files with the same name: folder name next to it (like VS Code "main.rs  a").
        let root = self.root(cx).map(|r| format!("{}/", r.trim_end_matches('/')));
        let rels: Vec<String> = editors
            .iter()
            .map(|e| {
                let p = &e.read(cx).path;
                root.as_deref().and_then(|r| p.strip_prefix(r)).unwrap_or(p).to_string()
            })
            .collect();
        let hints = twin_hints(&rels);
        let (tab_icons, close_mode) = {
            let st = cx.global::<Settings>();
            (st.tab_icons, st.tab_close.clone())
        };
        div()
            .id(if focused { "tabs" } else { "tabs-other" })
            .h(px(layout::TABBAR_H))
            .flex_none()
            .flex()
            .overflow_x_scroll()
            .when(focused, |d| d.track_scroll(&self.tabs.tab_scroll))
            // Dropping on the empty area right of the tabs: goes to the end.
            .on_drop(cx.listener(move |this, d: &DraggedTab, window, cx| {
                this.drop_tab(d.editor.clone(), d.from_focused, focused, None, window, cx);
            }))
            .bg(c(p.surface))
            .border_b_1()
            .border_color(c(p.border))
            .children(editors.iter().enumerate().map(|(ix, ed)| {
                let ed = ed.read(cx);
                let active = active_ix == Some(ix) && !diff_here;
                let dirty = ed.is_dirty();
                let (preview, pinned) = (ed.preview, ed.pinned);
                let group: SharedString = format!("tabg-{focused}-{ix}").into();
                let rel = rels[ix].clone();
                let hint = hints[ix].clone();
                let dragged = DraggedTab { editor: editors[ix].clone(), from_focused: focused, name: ed.name.clone() };
                let drop_on = editors[ix].clone();
                let accent = p.accent;
                div()
                    .id(SharedString::from(format!("tab-{focused}-{ix}")))
                    .on_drag(dragged, |d, _, _, cx| cx.new(|_| d.clone()))
                    .drag_over::<DraggedTab>(move |s, _, _, _| s.border_l_2().border_color(c(accent)))
                    .on_drop(cx.listener(move |this, d: &DraggedTab, window, cx| {
                        this.drop_tab(d.editor.clone(), d.from_focused, focused, Some(drop_on.clone()), window, cx);
                    }))
                    .group(group.clone())
                    .relative()
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap(px(theme::SPACE[3]))
                    .min_w(px(100.))
                    .max_w(px(220.))
                    .pl(px(theme::SPACE[5]))
                    .pr(px(theme::SPACE[2]))
                    .border_r_1()
                    .border_color(c(p.border))
                    .cursor_pointer()
                    .text_color(c(if active { p.text } else { p.text_muted }))
                    .when(!active, |d| d.hover(|d| d.bg(c(p.surface_hover))))
                    .when(active, |d| {
                        // Accent on the top edge only: border_color applies to all edges,
                        // hence a separate line element.
                        d.bg(c(p.bg)).child(div().absolute().top_0().left_0().right_0().h(px(1.)).bg(c(p.accent)))
                    })
                    .when(tab_icons, |d| d.child(crate::icon_theme::file_icon_el(&ed.name, layout::ICON, p)))
                    .child(
                        div()
                            .flex_1()
                            .flex()
                            .items_baseline()
                            .gap(px(theme::SPACE[2]))
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .when(preview, |d| d.italic())
                            // Like VS Code: the tab name of a modified/new file takes the git color.
                            .when_some(self.git_mark_of(&ed.path), |d, m| d.text_color(c(m.color(p))))
                            .child(ed.name.clone())
                            .when_some(hint, |d, h| d.child(div().text_size(px(text::XS.0)).text_color(c(p.text_muted)).child(h))),
                    )
                    .when(
                        active && ed.path.rsplit_once('.').is_some_and(|(_, ext)| ext.eq_ignore_ascii_case("md") || ext.eq_ignore_ascii_case("markdown")),
                        |d| {
                            d.child(ui::icon_button("markdown-read", "eye", crate::i18n::t("Read"), p).on_click(cx.listener(|this, _, window, cx| {
                                cx.stop_propagation();
                                this.open_markdown_preview(&OpenMarkdownPreview, window, cx);
                            })))
                        },
                    )
                    .when(active && cx.global::<Settings>().web_preview_enabled && crate::web_preview::is_html(&ed.path), |d| {
                        d.child(
                            ui::icon_button("html-preview", "eye", crate::i18n::t("Preview HTML in your browser"), p)
                                .debug_selector(|| "html-preview".into())
                                .on_click(cx.listener(|this, _, window, cx| {
                                    cx.stop_propagation();
                                    this.open_web_preview(window, cx);
                                })),
                        )
                    })
                    .tooltip(move |_, cx| cx.new(|_| Tooltip(rel.clone().into())).into())
                    .when(pinned && !dirty, |d| {
                        // Pinned: a pin instead of close (click: unpin).
                        d.child(
                            div()
                                .id(SharedString::from(format!("tab-pin-{focused}-{ix}")))
                                .size(px(20.))
                                .flex()
                                .items_center()
                                .justify_center()
                                .rounded(px(theme::radius::SM))
                                .hover(|d| d.bg(c(p.surface_hover)))
                                .child(icon("pin", 13., p.text_muted))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.ensure_group(focused);
                                    cx.stop_propagation();
                                    this.toggle_pin(ix, cx);
                                })),
                        )
                    })
                    .when((!pinned || dirty) && (close_mode != "off" || dirty), |d| {
                        d.child(
                            // Close: visible on the active tab and on hover; a dot if dirty.
                            div()
                                .id(SharedString::from(format!("tab-close-{focused}-{ix}")))
                                .size(px(20.))
                                .flex()
                                .items_center()
                                .justify_center()
                                .rounded(px(theme::radius::SM))
                                .hover(|d| d.bg(c(p.surface_hover)))
                                .when(!active && !dirty && close_mode == "hover", |d| d.opacity(0.).group_hover(group.clone(), |s| s.opacity(1.)))
                                .child(if dirty {
                                    div().size(px(8.)).rounded_full().bg(c(p.text)).into_any_element()
                                } else {
                                    icon("x", 14., p.text_muted).into_any_element()
                                })
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.ensure_group(focused);
                                    cx.stop_propagation();
                                    this.close_editor_at(ix, window, cx);
                                })),
                        )
                    })
                    .on_mouse_down(
                        MouseButton::Middle,
                        cx.listener(move |this, _, window, cx| {
                            this.ensure_group(focused);
                            this.close_editor_at(ix, window, cx)
                        }),
                    )
                    .on_mouse_down(
                        MouseButton::Right,
                        cx.listener(move |this, ev: &gpui::MouseDownEvent, window, cx| {
                            this.ensure_group(focused);
                            this.open_tab_menu(ix, ev.position, window, cx);
                        }),
                    )
                    .on_click(cx.listener(move |this, ev: &gpui::ClickEvent, window, cx| {
                        this.record_nav(cx);
                        this.ensure_group(focused);
                        if ev.click_count() >= 2 {
                            this.tabs.editors[ix].update(cx, |e, _| e.preview = false); // double click: make permanent
                        }
                        this.activate(ix, window, cx);
                    }))
            }))
            .when_some(self.tabs.large.clone().filter(|_| focused), |d, view| {
                let active = self.tabs.large_active;
                let name = view.read(cx).name.clone();
                let (glyph, suffix) =
                    if view.read(cx).is_image() { ("file-image", crate::i18n::t("preview")) } else { ("file-text", crate::i18n::t("read-only")) };
                d.child(
                    div()
                        .id("tab-large")
                        .relative()
                        .flex()
                        .flex_none()
                        .items_center()
                        .gap(px(theme::SPACE[3]))
                        .pl(px(theme::SPACE[5]))
                        .pr(px(theme::SPACE[2]))
                        .border_r_1()
                        .border_color(c(p.border))
                        .cursor_pointer()
                        .text_color(c(if active { p.text } else { p.text_muted }))
                        .when(!active, |d| d.hover(|d| d.bg(c(p.surface_hover))))
                        .when(active, |d| d.bg(c(p.bg)).child(div().absolute().top_0().left_0().right_0().h(px(1.)).bg(c(p.accent))))
                        .child(icon(glyph, layout::ICON, p.text_muted))
                        .child(div().whitespace_nowrap().child(format!("{name} ({suffix})")))
                        .child(
                            div()
                                .id("tab-large-close")
                                .size(px(20.))
                                .flex()
                                .items_center()
                                .justify_center()
                                .rounded(px(theme::radius::SM))
                                .hover(|d| d.bg(c(p.surface_hover)))
                                .child(icon("x", 14., p.text_muted))
                                .on_click(cx.listener(|this, _, window, cx| {
                                    cx.stop_propagation();
                                    this.close_large(window, cx);
                                })),
                        )
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.tabs.large_active = true;
                            this.tabs.page_active = false;
                            this.tabs.diff_active = false;
                            if let Some(v) = &this.tabs.large {
                                window.focus(&v.focus_handle(cx));
                            }
                            cx.notify();
                        })),
                )
            })
            .when_some(self.tabs.diff.clone().filter(|_| focused), |d, view| {
                let active = self.tabs.diff_active;
                let name = view.read(cx).name.clone();
                d.child(
                    div()
                        .id("tab-diff")
                        .relative()
                        .flex()
                        .flex_none()
                        .items_center()
                        .gap(px(theme::SPACE[3]))
                        .pl(px(theme::SPACE[5]))
                        .pr(px(theme::SPACE[2]))
                        .border_r_1()
                        .border_color(c(p.border))
                        .cursor_pointer()
                        .text_color(c(if active { p.text } else { p.text_muted }))
                        .when(!active, |d| d.hover(|d| d.bg(c(p.surface_hover))))
                        .when(active, |d| d.bg(c(p.bg)).child(div().absolute().top_0().left_0().right_0().h(px(1.)).bg(c(p.accent))))
                        .child(icon("git-compare", layout::ICON, p.info))
                        .child(div().italic().whitespace_nowrap().child(crate::i18n::tf("{} (diff)", &[&name])))
                        .child(
                            div()
                                .id("tab-diff-close")
                                .size(px(20.))
                                .flex()
                                .items_center()
                                .justify_center()
                                .rounded(px(theme::radius::SM))
                                .hover(|d| d.bg(c(p.surface_hover)))
                                .child(icon("x", 14., p.text_muted))
                                .on_click(cx.listener(|this, _, window, cx| {
                                    cx.stop_propagation();
                                    this.close_diff(window, cx);
                                })),
                        )
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.tabs.diff_active = true;
                            this.tabs.page_active = false;
                            this.tabs.large_active = false;
                            if let Some(v) = &this.tabs.diff {
                                window.focus(&v.focus_handle(cx));
                            }
                            cx.notify();
                        })),
                )
            })
            .when_some(self.tabs.page.clone().filter(|_| focused), |d, page| {
                let active = self.tabs.page_active;
                let (glyph, title): (&str, String) = match &page {
                    Page::Welcome => ("rocket", crate::i18n::t("Welcome").into()),
                    Page::Containers => ("server", crate::i18n::t("Container Resources").into()),
                    Page::Settings(_) => ("settings", crate::i18n::t("Settings").into()),
                    Page::Markdown(v) => ("eye", v.read(cx).title(cx)),
                    Page::Shortcuts(_) => ("keyboard", crate::i18n::t("Keyboard Shortcuts").into()),
                    Page::Merge(v) => ("git-compare", v.read(cx).title(cx)),
                    Page::Compare(v) => ("git-compare", v.read(cx).title()),
                };
                d.child(
                    div()
                        .id("tab-page")
                        .relative()
                        .flex()
                        .flex_none()
                        .items_center()
                        .gap(px(theme::SPACE[3]))
                        .pl(px(theme::SPACE[5]))
                        .pr(px(theme::SPACE[2]))
                        .border_r_1()
                        .border_color(c(p.border))
                        .cursor_pointer()
                        .text_color(c(if active { p.text } else { p.text_muted }))
                        .when(!active, |d| d.hover(|d| d.bg(c(p.surface_hover))))
                        .when(active, |d| d.bg(c(p.bg)).child(div().absolute().top_0().left_0().right_0().h(px(1.)).bg(c(p.accent))))
                        .child(icon(glyph, layout::ICON, p.accent))
                        .child(div().whitespace_nowrap().child(title))
                        .child(
                            div()
                                .id("tab-page-close")
                                .size(px(20.))
                                .flex()
                                .items_center()
                                .justify_center()
                                .rounded(px(theme::radius::SM))
                                .hover(|d| d.bg(c(p.surface_hover)))
                                .child(icon("x", 14., p.text_muted))
                                .on_click(cx.listener(|this, _, _, cx| {
                                    cx.stop_propagation();
                                    this.close_page(cx);
                                })),
                        )
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.tabs.page_active = true;
                            this.tabs.diff_active = false;
                            this.tabs.large_active = false;
                            if let Some(Page::Settings(v)) = &this.tabs.page {
                                v.read(cx).focus_search(window, cx);
                            }
                            cx.notify();
                        })),
                )
            })
    }

    /// File path relative to the root folder: src › app › main.rs
    /// The path above the editor. A folder item shows that folder in the explorer, a file item shows the file.
    pub(super) fn breadcrumbs(&self, p: &Palette, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let path = self.active_editor()?.read(cx).path.clone();
        let root = self.roots.primary.read(cx).root().map(str::to_string);
        let crumbs = crumb_paths(root.as_deref(), &path);
        let last = crumbs.len().saturating_sub(1);
        let mut items = Vec::new();
        for (i, (part, abs)) in crumbs.into_iter().enumerate() {
            if i > 0 {
                items.push(icon("chevron-right", 12., p.text_muted).into_any_element());
            }
            let file = i == last;
            let target = abs;
            items.push(
                div()
                    .id(SharedString::from(format!("crumb-{i}")))
                    .flex()
                    .items_center()
                    .gap(px(theme::SPACE[1]))
                    .px(px(theme::SPACE[1]))
                    .rounded(px(theme::radius::SM))
                    .cursor_pointer()
                    .hover(|d| d.bg(c(p.surface_hover)).text_color(c(p.text)))
                    .when(file, |d| d.text_color(c(p.text)).child(crate::icon_theme::file_icon_el(&part, 12., p)))
                    .child(part)
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.chrome.panel = Some(Panel::Explorer);
                        this.roots.primary.update(cx, |t, cx| if file { t.reveal(&target, cx) } else { t.reveal_dir(&target, cx) });
                        window.focus(&this.roots.primary.focus_handle(cx));
                        cx.notify();
                    }))
                    .into_any_element(),
            );
        }
        Some(
            div()
                .h(px(layout::BREADCRUMBS_H))
                .flex_none()
                .flex()
                .items_center()
                .gap(px(theme::SPACE[1]))
                .px(px(theme::SPACE[5]))
                .bg(c(p.bg))
                .text_size(px(text::XS.0))
                .text_color(c(p.text_muted))
                .overflow_hidden()
                .whitespace_nowrap()
                .children(items),
        )
    }

    /// Native hit-test areas preserve Windows dragging, snapping and WM_CLOSE guards.
    pub(super) fn windows_title_bar(&self, window: &Window, p: &Palette, _cx: &mut Context<Self>) -> impl IntoElement {
        let p = *p;
        let control = |id: &'static str, glyph: &'static str, label: &'static str, area: gpui::WindowControlArea| {
            div()
                .id(id)
                .debug_selector(move || id.into())
                .w(px(46.))
                .h_full()
                .flex_none()
                .flex()
                .items_center()
                .justify_center()
                .window_control_area(area)
                .hover(move |s| s.bg(c(if id == "window-close" { ui::tinted_bg(p.surface, p.danger, 0.28, p.text) } else { p.surface_hover })))
                .tooltip(move |_, cx| cx.new(|_| Tooltip(label.into())).into())
                .child(icon(glyph, 14., p.text))
        };
        div()
            .id("windows-title-bar")
            // Keep the focus-tracking workbench from preventing native caption defaults.
            // Occlusion excludes its hitbox without stopping event propagation to Windows.
            .occlude()
            .pt(px(3.))
            .debug_selector(|| "windows-title-bar".into())
            .h(px(WINDOWS_TITLEBAR_H))
            .flex_none()
            .flex()
            .items_center()
            .bg(c(p.surface))
            .border_b_1()
            .border_color(c(p.border))
            .child(
                div()
                    .id("window-drag-region")
                    .debug_selector(|| "window-drag-region".into())
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .px(px(theme::SPACE[4]))
                    .flex()
                    .items_center()
                    .gap(px(theme::SPACE[4]))
                    .window_control_area(gpui::WindowControlArea::Drag)
                    .child(div().text_size(px(text::XS.0)).font_weight(gpui::FontWeight::SEMIBOLD).text_color(c(p.text)).child("codeyard")) // i18n-keep: application name.
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_size(px(text::XS.0))
                            .text_color(c(if window.is_window_active() { p.text } else { p.text_muted }))
                            .child(if self.title == "codeyard" { String::new() } else { self.title.clone() }),
                    ),
            )
            .child(control("window-minimize", "minus", crate::i18n::t("Minimize window"), gpui::WindowControlArea::Min))
            .child(control(
                "window-maximize",
                if window.is_maximized() { "copy" } else { "square" },
                if window.is_maximized() { crate::i18n::t("Restore window") } else { crate::i18n::t("Maximize window") },
                gpui::WindowControlArea::Max,
            ))
            .child(control("window-close", "x", crate::i18n::t("Close window"), gpui::WindowControlArea::Close))
    }

    /// macOS: the native title is transparent, we draw it (so it follows the theme color). Back /
    /// forward, then the command center (the folder name; click to search files) in the middle.
    pub(super) fn title_bar(&self, p: &Palette, cx: &mut Context<Self>) -> impl IntoElement {
        let root = self.roots.primary.read(cx).root().map(|r| ui::file_name(r).to_string());
        let remote = match &self.remote.conn {
            Conn::Connected { alias } | Conn::Reconnecting { alias, .. } => Some(alias.to_string()),
            _ => None,
        };
        let label: SharedString = match (&root, &remote) {
            (Some(r), Some(a)) => format!("{r} [SSH: {a}]").into(),
            (Some(r), None) => r.clone().into(),
            (None, Some(a)) => format!("[SSH: {a}]").into(),
            (None, None) => crate::i18n::t("Search").into(),
        };
        let tip: SharedString = match &root {
            Some(r) => crate::i18n::tf("Search {}", &[r]).into(),
            None => crate::i18n::t("Search files by name").into(),
        };
        let shortcut = Command { label: String::new(), run: Run::Action(Box::new(QuickOpen)) }.shortcut(cx);
        let tip: SharedString = match shortcut {
            Some(k) => format!("{tip} ({k})").into(),
            None => tip,
        };
        let nav = |id: &'static str, glyph: &'static str, enabled: bool, tip: &'static str| {
            div()
                .id(id)
                .size(px(22.))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(theme::radius::MD))
                .child(icon(glyph, layout::ICON, if enabled { p.text_muted } else { ui::mix(p.surface, p.text_muted, 0.45) }))
                .when(enabled, |d| d.cursor_pointer().hover(|d| d.bg(c(p.surface_hover))))
                .tooltip(move |_, cx| cx.new(|_| Tooltip(tip.into())).into())
        };
        let (can_back, can_forward) = (!self.nav.back.is_empty(), !self.nav.forward.is_empty());
        div()
            .id("title-bar")
            .h(px(layout::TITLEBAR_H))
            .flex_none()
            .flex()
            .items_center()
            .bg(c(p.surface))
            .border_b_1()
            .border_color(c(p.border))
            .on_click(|ev, window, _| {
                if ev.click_count() == 2 {
                    window.titlebar_double_click();
                }
            })
            // Room for the traffic lights, mirrored on the right so the navigation group stays in the middle of the window.
            .child(div().w(px(80.)).flex_none())
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .gap(px(theme::SPACE[1]))
                    .child(nav("tb-back", "arrow-left", can_back, crate::i18n::t("Back")).when(can_back, |d| {
                        d.on_click(cx.listener(|this, _, w, cx| {
                            cx.stop_propagation();
                            this.go_back(&GoBack, w, cx);
                        }))
                    }))
                    .child(nav("tb-forward", "arrow-right", can_forward, crate::i18n::t("Forward")).when(can_forward, |d| {
                        d.on_click(cx.listener(|this, _, w, cx| {
                            cx.stop_propagation();
                            this.go_forward(&GoForward, w, cx);
                        }))
                    }))
                    .child(
                        div()
                            .id("command-center")
                            .ml(px(theme::SPACE[2]))
                            // Sits a point below the middle, level with the traffic lights' optical centre.
                            .mt(px(2.))
                            .w(gpui::relative(0.38))
                            .max_w(px(600.))
                            .min_w(px(160.))
                            .h(px(24.))
                            .px(px(12.))
                            .flex()
                            .items_center()
                            .rounded(px(6.))
                            .border_1()
                            // Drawn on the title bar itself: only a faint outline marks the box until it is hovered.
                            .border_color(c(ui::mix(p.surface, p.text, 0.08)))
                            .text_size(px(text::XS.0))
                            .text_color(c(p.text_muted))
                            .cursor_pointer()
                            .hover(|d| d.bg(c(p.surface_hover)).border_color(c(p.border)).text_color(c(p.text)))
                            .tooltip(move |_, cx| cx.new(|_| Tooltip(tip.clone())).into())
                            .on_click(cx.listener(|this, _, w, cx| {
                                cx.stop_propagation();
                                this.quick_open(&QuickOpen, w, cx);
                            }))
                            .child(div().min_w_0().overflow_hidden().whitespace_nowrap().text_ellipsis().child(label)),
                    ),
            )
            .child(div().w(px(80.)).flex_none())
    }

    /// An action row on the welcome screen: icon, name, shortcut (from the keymap).
    pub(super) fn start_row(
        &self,
        id: &'static str,
        glyph: &'static str,
        label: &'static str,
        action: impl gpui::Action,
        p: &Palette,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let shortcut = Command { label: label.into(), run: Run::Action(action.boxed_clone()) }.shortcut(cx);
        div()
            .id(id)
            .h(px(28.))
            .flex()
            .items_center()
            .gap(px(theme::SPACE[4]))
            .px(px(theme::SPACE[3]))
            .rounded(px(theme::radius::MD))
            .cursor_pointer()
            .hover(|d| d.bg(c(p.surface_hover)))
            .child(icon(glyph, layout::ICON, p.accent))
            .child(div().flex_1().text_color(c(p.text)).child(label))
            .when_some(shortcut, |d, k| d.child(kbd(k, p)))
            .on_click(move |_, window, cx| window.dispatch_action(action.boxed_clone(), cx))
    }

    pub(super) fn empty_state(&self, p: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let has_root = self.roots.primary.read(cx).root().is_some();
        let idle = self.tabs.opening.is_none();
        // Like VS Code: the welcome page greets an empty window; once a folder is open (local or over SSH) the editor area stays quiet.
        if idle && !has_root && cx.global::<Settings>().shows_welcome() {
            return self.welcome_view(p, cx);
        }
        if idle {
            let (_, _, mark) = super::welcome_glue::brand_colors(p);
            return center(p).child(icon("codeyard-mark", 96., mark).text_color(ui::ca(mark, 0.18))).into_any_element();
        }
        if has_root {
            let opening = self.tabs.opening.is_some();
            return center(p)
                .child(div().text_color(c(p.text_muted)).child(if opening {
                    crate::i18n::t("Opening…")
                } else {
                    crate::i18n::t("Pick a file from the tree, or:")
                }))
                .child(
                    div()
                        .w(px(320.))
                        .flex()
                        .flex_col()
                        .gap(px(theme::SPACE[1]))
                        .child(self.start_row("es-file", "search", crate::i18n::t("Go to File"), QuickOpen, p, cx))
                        .child(self.start_row("es-cmd", "terminal", crate::i18n::t("Command Palette"), CommandPalette, p, cx))
                        .child(self.start_row("es-side", "panel-left", crate::i18n::t("Toggle Sidebar"), ToggleSidebar, p, cx))
                        .child(self.start_row("es-theme", "palette", crate::i18n::t("Color Theme"), SelectTheme, p, cx)),
                )
                .into_any_element();
        }

        let recent = self.persist.recent.clone();
        let heading = |t: String| div().text_size(px(text::XXS.0)).font_weight(gpui::FontWeight::SEMIBOLD).text_color(c(p.text_muted)).child(t);
        center(p)
            .child(
                div()
                    .w(px(680.))
                    .flex()
                    .flex_col()
                    .gap(px(theme::SPACE[8]))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(theme::SPACE[2]))
                            .child(div().text_size(px(text::DISPLAY.0)).font_weight(gpui::FontWeight::SEMIBOLD).child("codeyard")) // i18n-keep: product name
                            .child(div().text_color(c(p.text_muted)).child(crate::i18n::t("Lightweight editing, on this machine or over SSH."))),
                    )
                    .child(
                        div()
                            .flex()
                            .gap(px(theme::SPACE[9]))
                            .child(
                                div()
                                    .w(px(300.))
                                    .flex()
                                    .flex_col()
                                    .gap(px(theme::SPACE[1]))
                                    .child(heading(crate::i18n::upper(crate::i18n::t("Start"))))
                                    .child(self.start_row("st-open", "folder", crate::i18n::t("Open Folder…"), OpenFolder, p, cx))
                                    .child(self.start_row("st-ssh", "server", crate::i18n::t("Connect over SSH"), ShowRemote, p, cx))
                                    .child(self.start_row("st-cmd", "search", crate::i18n::t("Command Palette"), CommandPalette, p, cx))
                                    .child(self.start_row("st-theme", "palette", crate::i18n::t("Color Theme"), SelectTheme, p, cx)),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .flex()
                                    .flex_col()
                                    .gap(px(theme::SPACE[1]))
                                    .child(heading(crate::i18n::upper(crate::i18n::t("Recent"))))
                                    .when(recent.is_empty(), |d| {
                                        d.child(div().px(px(theme::SPACE[3])).text_color(c(p.text_muted)).child(crate::i18n::t("Nothing yet.")))
                                    })
                                    .children(recent.into_iter().enumerate().map(|(i, r)| {
                                        let remote = r.host.clone();
                                        let name = ui::file_name(&r.path).to_string();
                                        let detail = match &remote {
                                            Some(h) => format!("ssh://{h}{}", r.path),
                                            None => tilde(&r.path),
                                        };
                                        div()
                                            .id(("recent", i))
                                            .h(px(28.))
                                            .flex()
                                            .items_center()
                                            .gap(px(theme::SPACE[4]))
                                            .px(px(theme::SPACE[3]))
                                            .rounded(px(theme::radius::MD))
                                            .cursor_pointer()
                                            .hover(|d| d.bg(c(p.surface_hover)))
                                            .child(icon(if remote.is_some() { "server" } else { "folder" }, layout::ICON, p.text_muted))
                                            .child(div().flex_none().text_color(c(p.text)).child(name))
                                            .child(
                                                div()
                                                    .flex_1()
                                                    .min_w_0()
                                                    .overflow_hidden()
                                                    .whitespace_nowrap()
                                                    .text_size(px(text::XS.0))
                                                    .text_color(c(p.text_muted))
                                                    .child(detail),
                                            )
                                            .on_click(cx.listener(move |this, _, window, cx| this.open_recent(r.clone(), window, cx)))
                                    })),
                            ),
                    ),
            )
            .into_any_element()
    }
}
