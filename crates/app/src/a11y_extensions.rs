//! The Extensions view's AccessKit nodes: its two tabs (Installed, Market), the search field, a live status that says
//! how many extensions match (spoken as the user types), the list, and a Market extension's page with its Install
//! button. Actions are checked against the list on screen before they run.

use super::*;
use crate::workbench::market_glue::{ExtTab, page_release};

#[derive(Clone, PartialEq, Debug)]
pub(crate) struct ExtSnap {
    pub market: bool,
    pub search_label: String,
    pub query: String,
    pub status: String,
    pub error: bool,
    pub list: ListSnap,
    /// What each row is (an extension id, with its registry in the Market), so an action on a row that moved does
    /// nothing.
    keys: Vec<String>,
    pub search_focused: bool,
}

/// A Market extension's page in the editor area.
#[derive(Clone, PartialEq, Debug)]
pub(crate) struct ExtPageSnap {
    pub title: String,
    /// What the page says, one line each (heading, publisher, downloads, permissions, versions).
    pub lines: Vec<String>,
    /// The Install (or Update, Reinstall) button's label, when there is one.
    pub install: Option<String>,
    key: String,
}

fn market_key(registry: &str, id: &ext_manifest::ExtId) -> String {
    format!("{registry}/{id}")
}

pub(super) fn append_tree(view: &ExtSnap, nodes: &mut Vec<(NodeId, Node)>) {
    let mut group = Node::new(Role::Group);
    group.set_label(t("Extensions"));

    let mut tabs = Node::new(Role::TabList);
    tabs.set_label(t("Extensions"));
    for (id, label, on) in [(EXT_TAB_INSTALLED, t("Installed"), !view.market), (EXT_TAB_MARKET, t("Market"), view.market)] {
        let mut tab = Node::new(Role::Tab);
        tab.set_label(label);
        tab.set_selected(on);
        tab.add_action(Action::Click);
        nodes.push((id, tab));
    }
    tabs.set_children(vec![EXT_TAB_INSTALLED, EXT_TAB_MARKET]);
    nodes.push((EXT_TABS, tabs));

    let mut search = Node::new(Role::SearchInput);
    search.set_label(view.search_label.clone());
    search.set_value(view.query.clone());
    search.add_action(Action::Focus);
    search.set_controls(vec![EXT_LIST]);
    if let Some(o) = selected_option(&view.list, EXT_ROW) {
        search.set_active_descendant(o);
    }
    nodes.push((EXT_SEARCH, search));

    // Spoken when it changes: "12 extensions" while typing, or why nothing is listed.
    let mut status = Node::new(if view.error { Role::Alert } else { Role::Status });
    status.set_label(view.status.clone());
    status.set_live(if view.error { Live::Assertive } else { Live::Polite });
    status.set_live_atomic();
    nodes.push((EXT_STATUS, status));

    let label = if view.market { t("Market") } else { t("Installed") };
    let list = list_nodes(&view.list, label, EXT_ROW, nodes);
    nodes.push((EXT_LIST, list));

    group.set_children(vec![EXT_TABS, EXT_SEARCH, EXT_STATUS, EXT_LIST]);
    nodes.push((EXT, group));
}

pub(super) fn append_page(page: &ExtPageSnap, nodes: &mut Vec<(NodeId, Node)>) {
    let mut group = Node::new(Role::Group);
    group.set_label(page.title.clone());
    let mut children = Vec::new();
    for (i, line) in page.lines.iter().enumerate() {
        let id = NodeId(EXT_PAGE_LINE + i as u64);
        let mut n = Node::new(Role::Label);
        n.set_label(line.clone());
        nodes.push((id, n));
        children.push(id);
    }
    if let Some(label) = &page.install {
        let mut b = Node::new(Role::Button);
        b.set_label(label.clone());
        b.add_action(Action::Click);
        nodes.push((EXT_PAGE_INSTALL, b));
        children.push(EXT_PAGE_INSTALL);
    }
    group.set_children(children);
    nodes.push((EXT_PAGE, group));
}

impl Workbench {
    /// The view while it is on screen (not with `--no-extensions`).
    pub(in crate::workbench) fn a11y_extensions(&self, window: &Window, cx: &App) -> Option<ExtSnap> {
        if self.chrome.panel != Some(Panel::Extensions) || crate::ext::turned_off() {
            return None;
        }
        let s = &self.cfg.exts;
        let market = s.tab == ExtTab::Market;
        let field = if market { s.market.search.as_ref() } else { s.filter.as_ref() };
        let (query, search_focused) = field.map_or((String::new(), false), |f| (f.read(cx).text().to_string(), f.focus_handle(cx).is_focused(window)));
        if market {
            let m = &s.market;
            let (status, error) = super::market_view_glue::market_status(m);
            let (mut rows, mut keys) = (Vec::new(), Vec::new());
            if let Some(catalog) = &m.catalog {
                for &i in m.results.iter().take(MAX_OPTIONS) {
                    let item = catalog.item(i);
                    let version = item.latest.as_ref().map_or_else(|| t("withdrawn").to_string(), ToString::to_string);
                    let mut detail = format!("{version}, {}", item.id);
                    if !item.description.is_empty() {
                        detail = format!("{detail}. {}", item.description);
                    }
                    rows.push((item.name.clone(), Some(detail)));
                    keys.push(market_key(&item.registry, &item.id));
                }
            }
            let list = ListSnap { rows, rows_total: m.results.len(), selected: (!m.results.is_empty()).then_some(m.cursor) };
            return Some(ExtSnap { market, search_label: t("Search the Market").into(), query, status, error, list, keys, search_focused });
        }
        let visible = self.visible_extensions(cx);
        let (status, error) = match &s.error {
            Some(e) => (e.clone(), true),
            None if s.list.is_empty() => (t("No extensions installed. Install one from a .cyx file or a folder.").to_string(), false),
            None if visible.is_empty() => (t("No installed extension matches the search.").to_string(), false),
            None => (tn("{} extension", "{} extensions", visible.len(), &[]), false),
        };
        let rows = visible
            .iter()
            .take(MAX_OPTIONS)
            .map(|&i| {
                let e = &s.list[i];
                let mut detail = format!("{}, {}", e.manifest.version, e.manifest.id);
                if e.disabled {
                    detail = format!("{detail}, {}", t("Disabled"));
                }
                (e.manifest.name.clone(), Some(detail))
            })
            .collect();
        let keys = visible.iter().take(MAX_OPTIONS).map(|&i| s.list[i].manifest.id.to_string()).collect();
        let selected = s.selected.as_ref().and_then(|id| visible.iter().position(|&i| s.list[i].manifest.id.as_str() == id));
        let list = ListSnap { rows, rows_total: visible.len(), selected };
        Some(ExtSnap { market, search_label: t("Search installed extensions").into(), query, status, error, list, keys, search_focused })
    }

    /// A Market extension's page while it is in front.
    pub(in crate::workbench) fn a11y_extension_page(&self) -> Option<ExtPageSnap> {
        if !matches!(self.tabs.page, Some(Page::Extension)) || !self.tabs.page_active || self.cfg.exts.pending.is_some() {
            return None;
        }
        let m = &self.cfg.exts.market;
        let (page, (listing, entry)) = (m.page.as_ref()?, m.page_entry()?);
        let release = page_release(&listing.verified, &page.id, entry);
        let downloads: u64 = entry.versions.iter().map(|r| r.downloads).sum();
        let mut lines = vec![
            tf("{0} · version {1}", &[&page.id, &release.version]),
            tf("From the {0} registry, published by {1}", &[&listing.registry.name, &entry.publisher]),
            tn("{} download, including updates", "{} downloads, including updates", downloads as usize, &[]),
        ];
        if !entry.description.is_empty() {
            lines.push(entry.description.clone());
        }
        if let Some(Ok(l)) = &page.loaded {
            lines.extend(super::ext_glue::high_risk_line(&l.manifest));
            let perms = super::ext_glue::permission_lines(&l.manifest.permissions);
            lines.push(if perms.is_empty() { t("Permissions: none.").to_string() } else { tf("Permissions: {}", &[&perms.join("; ")]) });
        }
        lines.extend(entry.versions.iter().rev().map(super::market_view_glue::version_line));
        let installable = listing.verified.index.latest(&page.id, false).is_some() && !release.code;
        let install = installable.then(|| match self.installed_version(&page.id) {
            Some(v) if v == release.version.to_string() => t("Reinstall").to_string(),
            Some(_) => t("Update").to_string(),
            None => t("Install").to_string(),
        });
        Some(ExtPageSnap { title: entry.name.clone(), lines, install, key: market_key(&page.registry, &page.id) })
    }

    pub(super) fn a11y_ext_action(&mut self, req: &ActionRequest, last: &Snapshot, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let id = req.target_node;
        let ours = [EXT, EXT_TABS, EXT_TAB_INSTALLED, EXT_TAB_MARKET, EXT_SEARCH, EXT_STATUS, EXT_LIST, EXT_PAGE, EXT_PAGE_INSTALL].contains(&id)
            || (EXT_ROW..EXT_ROW + BLOCK).contains(&id.0)
            || (EXT_PAGE_LINE..EXT_PAGE_LINE + BLOCK).contains(&id.0);
        if !ours {
            return false;
        }
        match (req.action, id) {
            (Action::Click, EXT_PAGE_INSTALL) => {
                let now = self.a11y_extension_page();
                if let (Some(was), Some(now)) = (&last.ext_page, now)
                    && was.key == now.key
                    && now.install.is_some()
                {
                    self.install_from_market(window, cx);
                }
            }
            _ if last.exts.is_none() || self.chrome.panel != Some(Panel::Extensions) => {}
            (Action::Click, EXT_TAB_INSTALLED) => self.set_extensions_tab(ExtTab::Installed, window, cx),
            (Action::Click, EXT_TAB_MARKET) => self.set_extensions_tab(ExtTab::Market, window, cx),
            (Action::Focus, EXT_SEARCH) => {
                let s = &self.cfg.exts;
                if let Some(f) = if s.tab == ExtTab::Market { s.market.search.clone() } else { s.filter.clone() } {
                    window.focus(&f.focus_handle(cx));
                }
            }
            (Action::Click | Action::Focus, id) if (EXT_ROW..EXT_ROW + BLOCK).contains(&id.0) => {
                let ix = (id.0 - EXT_ROW) as usize;
                let (Some(was), Some(now)) = (&last.exts, self.a11y_extensions(window, cx)) else { return true };
                // The row must still be the same extension, in the same list.
                if was.market != now.market || was.keys.get(ix).is_none() || was.keys.get(ix) != now.keys.get(ix) {
                    return true;
                }
                match (req.action, now.market) {
                    (Action::Focus, true) => {
                        self.cfg.exts.market.cursor = ix;
                        cx.notify();
                    }
                    (Action::Click, true) => self.open_market_row(ix, window, cx),
                    (Action::Click, false) => {
                        let ext_id = now.keys[ix].clone();
                        self.select_extension(&ext_id, cx);
                    }
                    _ => {}
                }
            }
            _ => {}
        }
        true
    }
}

#[cfg(test)]
#[path = "a11y_extensions_tests.rs"]
mod tests;
