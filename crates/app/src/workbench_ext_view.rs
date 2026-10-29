//! Workbench: drawing the Extensions view (sidebar list) and the extension page (details, actions, consent sheet).
//! Every control is a tab stop and answers Enter and Space like a click.

use gpui::{AnyElement, Context, SharedString, div, px};

use super::ext_glue::{ExtEntry, contribution_lines, high_risk_line, permission_lines, source_line};
use super::*;

/// A text button that takes keyboard focus.
fn action_button(id: &'static str, label: &'static str, p: &Palette) -> gpui::Stateful<gpui::Div> {
    super::container_monitor_glue::button(id, label, p)
        .tab_index(0)
        .border_1()
        .border_color(ui::ca(p.focus_ring, 0.))
        .focus(|d| d.border_color(c(p.focus_ring)))
}

fn section_title(title: &str, p: &Palette) -> gpui::Div {
    div()
        .pt(px(theme::SPACE[4]))
        .pb(px(theme::SPACE[1]))
        .text_size(px(text::XXS.0))
        .font_weight(gpui::FontWeight::SEMIBOLD)
        .text_color(c(p.text_muted))
        .child(crate::i18n::upper(title))
}

/// A bulleted list of plain lines, or `empty` in muted text.
fn line_list(lines: Vec<String>, empty: &str, p: &Palette) -> gpui::Div {
    let mut d = div().flex().flex_col().gap(px(theme::SPACE[1]));
    if lines.is_empty() {
        return d.child(div().text_color(c(p.text_muted)).child(empty.to_string()));
    }
    for l in lines {
        d = d.child(div().flex().gap(px(theme::SPACE[2])).child(div().flex_none().text_color(c(p.text_muted)).child("•")).child(div().min_w_0().child(l)));
    }
    d
}

impl Workbench {
    pub(super) fn extensions_view(&self, p: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let off = crate::ext::turned_off();
        let header = div()
            .h(px(layout::VIEW_HEADER_H))
            .flex_none()
            .flex()
            .items_center()
            .justify_between()
            .pl(px(theme::SPACE[6]))
            .pr(px(theme::SPACE[3]))
            .text_size(px(text::XXS.0))
            .font_weight(gpui::FontWeight::SEMIBOLD)
            .text_color(c(p.text_muted))
            .child(crate::i18n::upper(crate::i18n::t("Extensions")))
            .when(!off, |d| {
                let tool = |id: &'static str, glyph: &'static str, tip: &'static str| {
                    ui::icon_button(id, glyph, tip, p).tab_index(0).focus(|d| d.bg(c(p.surface_active)))
                };
                d.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(theme::SPACE[1]))
                        .child(
                            tool("ext-install-file", "file-plus", crate::i18n::t("Install from File…"))
                                .on_click(cx.listener(|this, _, w, cx| this.install_extension_from_file(&InstallExtensionFromFile, w, cx))),
                        )
                        .child(
                            tool("ext-install-folder", "folder-plus", crate::i18n::t("Install from Folder…"))
                                .on_click(cx.listener(|this, _, w, cx| this.install_extension_from_folder(&InstallExtensionFromFolder, w, cx))),
                        )
                        .child(tool("ext-refresh", "refresh-cw", crate::i18n::t("Refresh")).on_click(cx.listener(|this, _, _, cx| {
                            this.refresh_extension_list();
                            cx.notify();
                        }))),
                )
            });
        let muted = |s: String| div().px(px(theme::SPACE[6])).py(px(theme::SPACE[2])).text_color(c(p.text_muted)).child(s);
        let mut body = div().id("ext-list").flex().flex_col().flex_1().min_h_0().overflow_y_scroll();
        if off {
            body = body
                .child(muted(crate::i18n::t("Extensions are turned off for this session (--no-extensions). Start Codeyard without it to load them.").into()));
        } else {
            let s = &self.cfg.exts;
            let rows = self.visible_extensions(cx);
            body = body.child(div().px(px(theme::SPACE[6])).child(section_title(&crate::i18n::tf("Installed ({})", &[&s.list.len()]), p)));
            if let Some(e) = &s.error {
                body = body.child(muted(e.clone()).text_color(c(p.danger)));
            } else if s.list.is_empty() {
                body = body.child(muted(crate::i18n::t("No extensions installed. Install one from a .cyx file or a folder.").into()));
            } else if rows.is_empty() {
                body = body.child(muted(crate::i18n::t("No installed extension matches the search.").into()));
            }
            for i in rows {
                body = body.child(self.extension_row(i, &s.list[i], p, cx));
            }
            body = body.child(div().px(px(theme::SPACE[6])).child(section_title(crate::i18n::t("Built-in"), p))).child(muted(
                crate::i18n::t("Themes, languages, language servers, snippets and key bindings that come with Codeyard are built in and always on.").into(),
            ));
        }
        div()
            .id("extensions-view")
            .flex()
            .flex_col()
            .size_full()
            .text_size(px(text::SM.0))
            .child(header)
            .when(!off, |d| {
                d.children(self.cfg.exts.filter.clone().map(|f| {
                    div()
                        .mx(px(theme::SPACE[3]))
                        .mb(px(theme::SPACE[2]))
                        .px(px(theme::SPACE[2]))
                        .py(px(theme::SPACE[1]))
                        .rounded(px(theme::radius::SM))
                        .border_1()
                        .border_color(c(p.border_input))
                        .bg(c(p.bg))
                        .child(f)
                }))
            })
            .child(body)
            .into_any_element()
    }

    fn extension_row(&self, ix: usize, e: &ExtEntry, p: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let id = e.manifest.id.to_string();
        let selected = self.cfg.exts.selected.as_deref() == Some(id.as_str()) && matches!(self.tabs.page, Some(Page::Extension));
        let state = if e.disabled { crate::i18n::t("Disabled") } else { "" };
        div()
            .id(("ext-row", ix))
            .debug_selector(move || format!("ext-row-{ix}"))
            .w_full()
            .flex_none()
            .flex()
            .flex_col()
            .px(px(theme::SPACE[6]))
            .py(px(theme::SPACE[1]))
            .border_1()
            .border_color(ui::ca(p.focus_ring, 0.))
            .cursor_pointer()
            .tab_index(0)
            .when(selected, |d| d.bg(c(p.surface_active)))
            .hover(|d| d.bg(c(p.surface_hover)))
            .focus(|d| d.border_color(c(p.focus_ring)))
            .when(e.disabled, |d| d.opacity(0.7))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(theme::SPACE[2]))
                    .child(div().min_w_0().truncate().text_color(c(p.text)).child(e.manifest.name.clone()))
                    .child(div().flex_none().text_size(px(text::XS.0)).text_color(c(p.text_muted)).child(e.manifest.version.to_string()))
                    .child(div().flex_1())
                    .when(!state.is_empty(), |d| d.child(div().flex_none().text_size(px(text::XXS.0)).text_color(c(p.text_muted)).child(state))),
            )
            .child(div().truncate().text_size(px(text::XS.0)).text_color(c(p.text_muted)).child(id.clone()))
            .on_click(cx.listener(move |this, _, _, cx| this.select_extension(&id, cx)))
            .into_any_element()
    }

    /// The page in the editor area: the consent sheet while a package waits for an answer, else the chosen extension.
    pub(super) fn extension_page(&self, p: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let s = &self.cfg.exts;
        let page = div().id("extension-page").size_full().overflow_y_scroll().px(px(theme::SPACE[8])).py(px(theme::SPACE[6])).text_size(px(text::SM.0));
        let column = div().max_w(px(720.)).flex().flex_col().gap(px(theme::SPACE[2])).text_color(c(p.text));
        if let Some(pending) = &s.pending {
            let m = &pending.manifest;
            let (install, cancel) = s.consent_focus.clone().expect("created with the sheet");
            let risk = high_risk_line(m);
            let buttons = div()
                .flex()
                .gap(px(theme::SPACE[2]))
                .pt(px(theme::SPACE[4]))
                .child(
                    action_button("ext-consent-install", crate::i18n::t("Install"), p)
                        .track_focus(&install)
                        .on_click(cx.listener(|this, _, w, cx| this.confirm_consent(w, cx))),
                )
                .child(
                    action_button("ext-consent-cancel", crate::i18n::t("Cancel"), p)
                        .track_focus(&cancel)
                        .on_click(cx.listener(|this, _, _, cx| this.cancel_consent(cx))),
                );
            // Escape answers Cancel, as in every other dialog.
            return page
                .key_context("ExtensionConsent")
                .on_action(cx.listener(Self::cancel_consent_key))
                .child(
                    column
                        .child(self.extension_heading(&crate::i18n::tf("Install {}?", &[&m.name]), m, p))
                        .child(div().text_color(c(p.text_muted)).child(source_line(&pending.source)))
                        .children(m.description.clone().map(|d| div().child(d)))
                        .child(self.extension_permissions(m, risk, p))
                        .child(section_title(crate::i18n::t("Contributes"), p))
                        .child(line_list(contribution_lines(m), crate::i18n::t("Nothing"), p))
                        .children(pending.warnings.iter().map(|w| div().pt(px(theme::SPACE[2])).text_color(c(p.danger)).child(w.clone())))
                        .child(buttons),
                )
                .into_any_element();
        }
        let Some(e) = self.selected_extension() else {
            return page
                .child(column.child(div().text_color(c(p.text_muted)).child(crate::i18n::t("Choose an extension in the Extensions view."))))
                .into_any_element();
        };
        let m = e.manifest.clone();
        let id = m.id.clone();
        let actions = {
            let (on_id, un_id, back_id) = (id.clone(), id.clone(), id.clone());
            let verify_id = id.to_string();
            let enabled = !e.disabled;
            div()
                .flex()
                .flex_wrap()
                .gap(px(theme::SPACE[2]))
                .pt(px(theme::SPACE[2]))
                .child(
                    action_button("ext-toggle", if enabled { crate::i18n::t("Disable") } else { crate::i18n::t("Enable") }, p)
                        .on_click(cx.listener(move |this, _, w, cx| this.set_extension_enabled(on_id.clone(), !enabled, w, cx))),
                )
                .child(
                    action_button("ext-uninstall", crate::i18n::t("Uninstall…"), p)
                        .on_click(cx.listener(move |this, _, w, cx| this.uninstall_extension(un_id.clone(), w, cx))),
                )
                .when_some(e.previous.clone(), |d, v| {
                    d.child(
                        action_button("ext-rollback", crate::i18n::t("Roll Back"), p)
                            .tooltip(move |_, cx| cx.new(|_| Tooltip(crate::i18n::tf("Go back to version {}", &[&v]).into())).into())
                            .on_click(cx.listener(move |this, _, w, cx| this.roll_back_extension(back_id.clone(), w, cx))),
                    )
                })
                .child(
                    action_button("ext-verify", crate::i18n::t("Verify Files"), p)
                        .on_click(cx.listener(move |this, _, _, cx| this.verify_extension(&verify_id, cx))),
                )
        };
        let state: SharedString =
            if e.disabled { crate::i18n::t("Disabled: nothing it contributes applies.").into() } else { crate::i18n::t("Enabled").into() };
        page.child(
            column
                .child(self.extension_heading(&m.name, &m, p))
                .child(div().text_color(c(p.text_muted)).child(source_line(&e.source)))
                .child(div().text_color(c(if e.disabled { p.warning } else { p.text_muted })).child(state))
                .children(m.description.clone().map(|d| div().child(d)))
                .child(actions)
                .child(section_title(crate::i18n::t("Contributes"), p))
                .child(line_list(contribution_lines(&m), crate::i18n::t("Nothing"), p))
                .child(self.extension_permissions(&m, high_risk_line(&m), p)),
        )
        .into_any_element()
    }

    fn extension_heading(&self, title: &str, m: &ext_manifest::Manifest, p: &Palette) -> gpui::Div {
        div()
            .flex()
            .flex_col()
            .gap(px(theme::SPACE[1]))
            .child(div().text_size(px(text::LG.0)).font_weight(gpui::FontWeight::SEMIBOLD).child(title.to_string()))
            .child(div().text_color(c(p.text_muted)).child(crate::i18n::tf("{0} · version {1} · {2} license", &[&m.id, &m.version, &m.license])))
    }

    /// Permissions in plain words, the author's reason as a quote and the red High-risk line.
    fn extension_permissions(&self, m: &ext_manifest::Manifest, risk: Option<String>, p: &Palette) -> gpui::Div {
        let lines = permission_lines(&m.permissions);
        div()
            .flex()
            .flex_col()
            .gap(px(theme::SPACE[1]))
            .child(section_title(crate::i18n::t("Permissions"), p))
            .children(risk.map(|r| div().debug_selector(|| "ext-high-risk".into()).font_weight(gpui::FontWeight::SEMIBOLD).text_color(c(p.danger)).child(r)))
            .child(line_list(lines, crate::i18n::t("None: it only adds data to Codeyard."), p))
            .children(m.permissions.why.clone().map(|why| {
                div()
                    .pl(px(theme::SPACE[3]))
                    .border_l_2()
                    .border_color(c(p.border))
                    .text_color(c(p.text_muted))
                    .child(crate::i18n::tf("{0} says why: “{1}”", &[&m.id.publisher(), &why]))
            }))
    }
}
