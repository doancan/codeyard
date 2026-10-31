//! Workbench: drawing the Market (the Extensions view's second tab) and a Market extension's page. The list is a
//! uniform list, so 10 000 results cost what the visible rows cost. ↑ and ↓ in the search field move the highlighted
//! row and ↵ opens it; every row and button is also a tab stop.

use gpui::{AnyElement, Context, div, px};

use super::ext_glue::{contribution_lines, high_risk_line, permission_lines};
use super::ext_view_glue::{action_button, line_list, section_title};
use super::market_glue::{ExtTab, LoadedPage, page_release};
use super::*;

/// Height of a Market row: name and version, then the description.
const ROW_H: f32 = 48.;

/// The text a screen reader says about the result list, also shown above it.
pub(super) fn market_status(m: &super::market_glue::MarketState) -> (String, bool) {
    if m.loading && m.catalog.is_none() {
        return (crate::i18n::t("Reading the registries…").to_string(), false);
    }
    if m.catalog.is_none() {
        return (m.errors.join("\n"), !m.errors.is_empty());
    }
    let n = m.results.len();
    let text = match (&m.answered, n) {
        (None, _) => crate::i18n::t("Searching…").to_string(),
        (Some(q), 0) if !q.trim().is_empty() => crate::i18n::t("No extension matches the search.").to_string(),
        (_, n) => crate::i18n::tn("{} extension", "{} extensions", n, &[]),
    };
    (text, false)
}

impl Workbench {
    /// "Installed" and "Market" under the view's title.
    pub(super) fn extension_tabs(&self, p: &Palette, cx: &mut Context<Self>) -> gpui::Div {
        let tab = |id: &'static str, label: &'static str, which: ExtTab, cx: &mut Context<Self>| {
            let on = self.cfg.exts.tab == which;
            div()
                .id(id)
                .px(px(theme::SPACE[2]))
                .py(px(theme::SPACE[1]))
                .rounded(px(theme::radius::SM))
                .border_1()
                .border_color(ui::ca(p.focus_ring, 0.))
                .cursor_pointer()
                .tab_index(0)
                .text_size(px(text::XS.0))
                .text_color(c(if on { p.text } else { p.text_muted }))
                .when(on, |d| d.bg(c(p.surface_active)).font_weight(gpui::FontWeight::SEMIBOLD))
                .hover(|d| d.bg(c(p.surface_hover)))
                .focus(|d| d.border_color(c(p.focus_ring)))
                .child(label)
                .on_click(cx.listener(move |this, _, w, cx| this.set_extensions_tab(which, w, cx)))
                .relative()
                .child(a11y::part_frame(a11y::Part::ExtControl(if which == ExtTab::Market { 1 } else { 0 }), cx))
        };
        div()
            .flex()
            .gap(px(theme::SPACE[1]))
            .px(px(theme::SPACE[3]))
            .pb(px(theme::SPACE[2]))
            .child(tab("ext-tab-installed", crate::i18n::t("Installed"), ExtTab::Installed, cx))
            .child(tab("ext-tab-market", crate::i18n::t("Market"), ExtTab::Market, cx))
    }

    /// The Market tab: search field, a status line, and the results.
    pub(super) fn market_list(&self, p: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let m = &self.cfg.exts.market;
        let field = m.search.clone().map(|f| {
            div()
                .key_context("MarketSearch")
                .on_action(cx.listener(Self::market_previous))
                .on_action(cx.listener(Self::market_next))
                .mx(px(theme::SPACE[3]))
                .mb(px(theme::SPACE[2]))
                .px(px(theme::SPACE[2]))
                .py(px(theme::SPACE[1]))
                .rounded(px(theme::radius::SM))
                .border_1()
                .border_color(c(p.border_input))
                .bg(c(p.bg))
                .child(f)
                .relative()
                .child(a11y::part_frame(a11y::Part::ExtControl(2), cx))
        });
        let (status, error) = market_status(m);
        let stale: Vec<String> = m
            .listings
            .iter()
            .filter(|l| l.verified.stale)
            .map(|l| crate::i18n::tf("The {} registry's list is more than 3 days old; versions withdrawn since may still show.", &[&l.registry.name]))
            .collect();
        let line = |s: String, color| div().px(px(theme::SPACE[6])).pb(px(theme::SPACE[1])).text_size(px(text::XS.0)).text_color(c(color)).child(s);
        let mut col = div().flex().flex_col().flex_1().min_h_0().children(field).child(
            line(status, if error { p.danger } else { p.text_muted })
                .flex()
                .items_center()
                .gap(px(theme::SPACE[2]))
                .when(m.loading, |d| d.child(ui::spinner("market-loading", layout::ICON - 4., p.text_muted))),
        );
        if m.catalog.is_some() {
            col = col.children(m.errors.iter().map(|e| line(e.clone(), p.danger))).children(stale.into_iter().map(|s| line(s, p.warning)));
        }
        let n = m.results.len();
        if n > 0 && m.catalog.is_some() {
            let p = *p;
            col = col.child(
                div().flex_1().min_h_0().relative().child(a11y::part_frame(a11y::Part::ExtControl(3), cx)).child(
                    gpui::uniform_list(
                        "market-rows",
                        n,
                        cx.processor(move |this, range: std::ops::Range<usize>, _window, cx| range.filter_map(|ix| this.market_row(ix, &p, cx)).collect()),
                    )
                    .track_scroll(m.scroll.clone())
                    .size_full(),
                ),
            );
        }
        col.into_any_element()
    }

    fn market_row(&self, ix: usize, p: &Palette, cx: &mut Context<Self>) -> Option<AnyElement> {
        let m = &self.cfg.exts.market;
        let item = m.catalog.as_ref()?.item(*m.results.get(ix)?);
        let on_page = m.page.as_ref().is_some_and(|pg| pg.registry == item.registry && pg.id == item.id) && matches!(self.tabs.page, Some(Page::Extension));
        let highlighted = m.cursor == ix;
        let version = item.latest.as_ref().map_or_else(|| crate::i18n::t("withdrawn").to_string(), ToString::to_string);
        let installed = self.installed_version(&item.id).is_some();
        let several = m.listings.len() > 1;
        let second = if several { format!("{} · {}", item.id, item.registry) } else { item.id.to_string() };
        Some(
            div()
                .id(("market-row", ix))
                .debug_selector(move || format!("market-row-{ix}"))
                .h(px(ROW_H))
                .w_full()
                .flex()
                .flex_col()
                .justify_center()
                .px(px(theme::SPACE[6]))
                .border_1()
                .border_color(ui::ca(p.focus_ring, 0.))
                .cursor_pointer()
                .tab_index(0)
                .when(on_page, |d| d.bg(c(p.surface_active)))
                .when(highlighted && !on_page, |d| d.bg(c(p.surface_hover)))
                .hover(|d| d.bg(c(p.surface_hover)))
                .focus(|d| d.border_color(c(p.focus_ring)))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(theme::SPACE[2]))
                        .child(div().min_w_0().truncate().text_color(c(p.text)).child(item.name.clone()))
                        .child(div().flex_none().text_size(px(text::XS.0)).text_color(c(p.text_muted)).child(version))
                        .child(div().flex_1())
                        .when(installed, |d| {
                            d.child(div().flex_none().text_size(px(text::XXS.0)).text_color(c(p.text_muted)).child(crate::i18n::t("Installed")))
                        })
                        .when(item.deprecated.is_some(), |d| {
                            d.child(div().flex_none().text_size(px(text::XXS.0)).text_color(c(p.warning)).child(crate::i18n::t("Deprecated")))
                        }),
                )
                .child(div().truncate().text_size(px(text::XS.0)).text_color(c(p.text_muted)).child(if item.description.is_empty() {
                    second
                } else {
                    format!("{} — {second}", item.description)
                }))
                .on_click(cx.listener(move |this, _, w, cx| this.open_market_row(ix, w, cx)))
                .relative()
                .child(a11y::part_frame(a11y::Part::Market(ix), cx))
                .into_any_element(),
        )
    }

    /// The page of a Market extension: what the index says, then the manifest and README from its details file.
    pub(super) fn market_page(&self, page: gpui::Stateful<gpui::Div>, column: gpui::Div, p: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let m = &self.cfg.exts.market;
        let (Some(pg), Some((listing, entry))) = (&m.page, m.page_entry()) else {
            return page
                .child(column.child(div().text_color(c(p.text_muted)).child(crate::i18n::t("This extension is no longer in the registry."))))
                .into_any_element();
        };
        let id = &pg.id;
        let release = page_release(&listing.verified, id, entry);
        let installable = listing.verified.index.latest(id, false).is_some();
        let installed = self.installed_version(id);
        let loaded = pg.loaded.as_ref().and_then(|r| r.as_ref().ok());
        let license = loaded.map(|l| l.manifest.license.clone());
        let mut heading_line = crate::i18n::tf("{0} · version {1}", &[id, &release.version]);
        if let Some(l) = &license {
            heading_line.push_str(&format!(" · {}", crate::i18n::tf("{} license", &[l])));
        }
        let downloads: u64 = entry.versions.iter().map(|r| r.downloads).sum();
        let mut col = column
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(theme::SPACE[1]))
                    .child(div().text_size(px(text::LG.0)).font_weight(gpui::FontWeight::SEMIBOLD).child(entry.name.clone()))
                    .child(div().text_color(c(p.text_muted)).child(heading_line)),
            )
            .child(
                div()
                    .text_color(c(p.text_muted))
                    .child(crate::i18n::tf("From the {0} registry, published by {1}", &[&listing.registry.name, &entry.publisher])),
            )
            .child(div().text_color(c(p.text_muted)).child(crate::i18n::tn(
                "{} download, including updates",
                "{} downloads, including updates",
                downloads as usize,
                &[],
            )))
            .when(!entry.description.is_empty(), |d| d.child(div().child(entry.description.clone())));
        if let ext_core::yard::Status::Deprecated { replacement } = &release.status {
            col = col.child(div().text_color(c(p.warning)).child(match replacement {
                Some(r) => crate::i18n::tf("Deprecated by its publisher: use {} instead.", &[r]),
                None => crate::i18n::t("Deprecated by its publisher.").to_string(),
            }));
        }
        if listing.verified.stale {
            col =
                col.child(div().text_color(c(p.warning)).child(crate::i18n::tf(
                    "The {} registry's list is more than 3 days old; versions withdrawn since may still show.",
                    &[&listing.registry.name],
                )));
        }
        // Install, Reinstall or Update; nothing when no version can be installed or it runs code.
        let action: Option<&'static str> = match (&installed, installable && (!release.code || crate::ext_runtime::code_runs_here(&id).is_ok())) {
            (_, false) => None,
            (Some(v), true) if *v == release.version.to_string() => Some(crate::i18n::t("Reinstall")),
            (Some(_), true) => Some(crate::i18n::t("Update")),
            (None, true) => Some(crate::i18n::t("Install")),
        };
        let mut actions = div().flex().flex_wrap().items_center().gap(px(theme::SPACE[2])).pt(px(theme::SPACE[2]));
        if let Some(label) = action {
            actions = actions.child(action_button("market-install", label, p).on_click(cx.listener(|this, _, w, cx| this.install_from_market(w, cx))));
        }
        if let Some(v) = &installed {
            let installed_id = id.to_string();
            actions = actions.child(div().text_color(c(p.text_muted)).child(crate::i18n::tf("Version {} is installed.", &[v]))).child(
                action_button("market-show-installed", crate::i18n::t("Show Installed"), p).on_click(cx.listener(move |this, _, _, cx| {
                    this.select_extension(&installed_id, cx);
                })),
            );
        }
        let why_not = if !installable {
            Some(crate::i18n::t("No version of this extension can be installed: they were withdrawn or revoked.").to_string())
        } else if let Some(Err(why)) = release.code.then(|| crate::ext_runtime::code_runs_here(&id)) {
            Some(why)
        } else {
            None
        };
        col = col.child(actions).children(why_not.map(|w| div().text_color(c(p.text_muted)).child(w)));
        col = match &pg.loaded {
            None => col.child(section_title(crate::i18n::t("Permissions"), p)).child(div().text_color(c(p.text_muted)).child(crate::i18n::t("Loading…"))),
            Some(Err(e)) => col
                .child(section_title(crate::i18n::t("Permissions"), p))
                .child(div().text_color(c(p.danger)).child(crate::i18n::tf("Cannot show this extension's details: {}", &[e]))),
            Some(Ok(l)) => col.child(self.market_permissions(l, p)).child(section_title(crate::i18n::t("Contributes"), p)).child(line_list(
                contribution_lines(&l.manifest),
                crate::i18n::t("Nothing"),
                p,
            )),
        };
        col = col.child(section_title(crate::i18n::t("Versions"), p)).child(version_lines(entry, p));
        if let Some(l) = loaded {
            col = col.child(section_title(crate::i18n::t("README"), p));
            col = match &l.readme {
                None => col.child(div().text_color(c(p.text_muted)).child(crate::i18n::t("This extension has no README."))),
                Some(blocks) => {
                    let link = cx.entity().downgrade();
                    col.child(div().flex().flex_col().gap(px(theme::SPACE[2])).children(crate::markdown::untrusted_document_view(
                        blocks,
                        "market-readme",
                        p,
                        move |url, window, cx| {
                            link.update(cx, |this, cx| this.confirm_open_link(url.to_string(), window, cx)).ok();
                        },
                    )))
                    .when(l.readme_cut, |d| {
                        d.child(div().text_color(c(p.text_muted)).child(crate::i18n::t("The README is shortened here; the full one is in the package.")))
                    })
                }
            };
        }
        page.child(col).into_any_element()
    }

    fn market_permissions(&self, l: &LoadedPage, p: &Palette) -> gpui::Div {
        let m = &l.manifest;
        div()
            .flex()
            .flex_col()
            .gap(px(theme::SPACE[1]))
            .child(section_title(crate::i18n::t("Permissions"), p))
            .children(high_risk_line(m).map(|r| div().font_weight(gpui::FontWeight::SEMIBOLD).text_color(c(p.danger)).child(r)))
            .child(line_list(permission_lines(&m.permissions), crate::i18n::t("None: it only adds data to Codeyard."), p))
            .children(m.permissions.why.clone().map(|why| {
                div()
                    .pl(px(theme::SPACE[3]))
                    .border_l_2()
                    .border_color(c(p.border))
                    .text_color(c(p.text_muted))
                    .child(crate::i18n::tf("{0} says why: “{1}”", &[&m.id.publisher(), &why]))
            }))
    }

    /// A README link: the full address is shown and the user agrees before the browser opens it.
    pub(super) fn confirm_open_link(&mut self, url: String, window: &mut Window, cx: &mut Context<Self>) {
        if !crate::markdown::is_safe_link(&url) {
            return;
        }
        let answer = window.prompt(
            PromptLevel::Info,
            crate::i18n::t("Open this link in your browser?"),
            Some(&url),
            &[crate::i18n::t("Open"), crate::i18n::t("Cancel")],
            cx,
        );
        cx.spawn(async move |_, cx| {
            if answer.await == Ok(0) {
                cx.update(|cx| cx.open_url(&url)).ok();
            }
        })
        .detach();
    }
}

/// One line per version, newest first: date, downloads and status.
pub(super) fn version_line(r: &ext_core::yard::Release) -> String {
    use ext_core::yard::Status;
    let downloads = crate::i18n::tn("{} download", "{} downloads", r.downloads as usize, &[]);
    let mut line = format!("{} · {} · {downloads}", r.version, crate::texttools::iso_date(r.published));
    let status = match &r.status {
        Status::Ok => None,
        Status::Deprecated { replacement: Some(x) } => Some(crate::i18n::tf("deprecated, use {}", &[x])),
        Status::Deprecated { replacement: None } => Some(crate::i18n::t("deprecated").to_string()),
        Status::Yanked => Some(crate::i18n::t("withdrawn by its publisher").to_string()),
        Status::Revoked { reason } => Some(crate::i18n::tf("revoked: {}", &[reason])),
    };
    if r.is_pre_release() {
        line.push_str(&format!(" · {}", crate::i18n::t("pre-release")));
    }
    if let Some(s) = status {
        line.push_str(&format!(" · {s}"));
    }
    line
}

fn version_lines(entry: &ext_core::yard::Entry, p: &Palette) -> gpui::Div {
    let mut d = div().flex().flex_col().gap(px(theme::SPACE[1]));
    for r in entry.versions.iter().rev() {
        let bad = matches!(r.status, ext_core::yard::Status::Revoked { .. } | ext_core::yard::Status::Yanked);
        d = d.child(div().text_color(c(if bad { p.text_muted } else { p.text })).child(version_line(r)));
    }
    d
}
