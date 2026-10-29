//! Workbench: the Market tab of the Extensions view (#68). Lists every extension of the user's registries from their
//! signed indexes and searches them locally, off the UI thread, on every keystroke; a registry is asked for its index
//! at most once a day (the cached copy is used within the day) and never while searching. An extension's page shows
//! its README (no image is loaded), versions with their status, permissions in plain words, the publisher and the
//! download count; Install goes through the consent sheet of `workbench_ext.rs`. Rendering is in
//! `workbench_market_view.rs`.

use std::sync::Arc;

use ext_core::market::Catalog;
use ext_core::registry::Registry;
use ext_core::yard::{Entry, Release, Verified};
use ext_manifest::{ExtId, Manifest};
use gpui::{Context, Entity, ScrollStrategy, UniformListScrollHandle, Window};

use super::*;
use crate::input::{FieldEvent, TextField};

gpui::actions!(workbench, [ShowMarket, MarketPrevious, MarketNext]);

/// The Extensions view's two lists.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum ExtTab {
    #[default]
    Installed,
    Market,
}

/// One registry's verified index, as read for the Market.
pub(super) struct Listing {
    pub(super) registry: Registry,
    pub(super) verified: Verified,
}

#[derive(Default)]
pub(super) struct MarketState {
    pub(super) search: Option<Entity<TextField>>,
    pub(super) catalog: Option<Arc<Catalog>>,
    pub(super) listings: Arc<Vec<Listing>>,
    /// When the indexes were read (Unix seconds); read again a day later.
    loaded_at: Option<u64>,
    /// The registries it was read for: a change in the settings reads them again.
    loaded_for: Vec<Registry>,
    pub(super) loading: bool,
    /// Registries that could not be read, and why; the others are listed.
    pub(super) errors: Vec<String>,
    /// Item indexes into the catalogue matching `answered`, best first.
    pub(super) results: Arc<Vec<usize>>,
    /// The query `results` answer (the field may be a keystroke ahead).
    pub(super) answered: Option<String>,
    search_generation: u64,
    /// The row ↑ and ↓ move to and ↵ opens (an index into `results`).
    pub(super) cursor: usize,
    pub(super) scroll: UniformListScrollHandle,
    /// The extension on the page.
    pub(super) page: Option<MarketPage>,
}

/// An extension of the Market on the page in the editor area.
pub(super) struct MarketPage {
    pub(super) registry: String,
    pub(super) id: ExtId,
    /// The page data of the newest installable version (or the newest one when none can be installed).
    pub(super) loaded: Option<Result<LoadedPage, String>>,
}

pub(super) struct LoadedPage {
    pub(super) manifest: Arc<Manifest>,
    pub(super) readme: Option<Vec<crate::markdown::Block>>,
    pub(super) readme_cut: bool,
}

impl MarketState {
    pub(super) fn listing(&self, registry: &str) -> Option<&Listing> {
        self.listings.iter().find(|l| l.registry.name == registry)
    }

    /// The index entry of the extension on the page.
    pub(super) fn page_entry(&self) -> Option<(&Listing, &Entry)> {
        let page = self.page.as_ref()?;
        let l = self.listing(&page.registry)?;
        Some((l, l.verified.index.extensions.get(&page.id)?))
    }
}

/// The version a page describes: the newest installable one, else the newest.
pub(super) fn page_release<'a>(v: &'a Verified, id: &ExtId, entry: &'a Entry) -> &'a Release {
    v.index.latest(id, false).unwrap_or_else(|| entry.versions.last().expect("the index check refuses an entry without versions"))
}

impl Workbench {
    pub(super) fn show_market(&mut self, _: &ShowMarket, window: &mut Window, cx: &mut Context<Self>) {
        self.chrome.panel = Some(Panel::Extensions);
        self.extensions_opened(window, cx);
        self.set_extensions_tab(ExtTab::Market, window, cx);
    }

    /// Switch the Extensions view's list; the search field of the list takes the keyboard.
    pub(super) fn set_extensions_tab(&mut self, tab: ExtTab, window: &mut Window, cx: &mut Context<Self>) {
        self.cfg.exts.tab = tab;
        if tab == ExtTab::Market {
            self.open_market(window, cx);
        }
        let field = match tab {
            ExtTab::Installed => self.cfg.exts.filter.clone(),
            ExtTab::Market => self.cfg.exts.market.search.clone(),
        };
        if let Some(f) = field {
            window.focus(&f.focus_handle(cx));
        }
        cx.notify();
    }

    /// The Market was opened: its search field exists, and the indexes are read when they were not read today.
    fn open_market(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.cfg.exts.market.search.is_none() {
            let field = cx.new(|cx| TextField::new(crate::i18n::tr("Search the Market"), cx));
            self._subs.push(cx.subscribe_in(&field, window, |this, _, ev: &FieldEvent, window, cx| match ev {
                FieldEvent::Changed => this.search_market(window, cx),
                FieldEvent::Submit => this.open_market_row(this.cfg.exts.market.cursor, window, cx),
                FieldEvent::Escape => {}
            }));
            self.cfg.exts.market.search = Some(field);
        }
        let registries = crate::ext_registry::registries(cx.global::<Settings>());
        let m = &self.cfg.exts.market;
        let now = crate::ext_registry::now();
        let fresh = m.loaded_at.is_some_and(|t| now < t + ext_core::registry::REFRESH_SECS) && m.loaded_for == registries;
        if m.loading || fresh || crate::ext::turned_off() {
            return;
        }
        let Some(dir) = self.cfg.dir.clone() else { return };
        let m = &mut self.cfg.exts.market;
        if registries.is_empty() {
            m.errors = vec![crate::i18n::t("No extension registry is set up yet. Add one to extensions.registries in your settings.").into()];
            m.catalog = None;
            m.listings = Arc::default();
            m.results = Arc::default();
            return;
        }
        m.loading = true;
        m.loaded_for = registries.clone();
        let task = cx.background_executor().spawn(async move {
            let (listed, errors) = crate::ext_registry::market(&dir, &registries, now);
            let listings: Vec<Listing> = listed.into_iter().map(|(registry, verified)| Listing { registry, verified }).collect();
            let catalog = Catalog::new(listings.iter().map(|l| (l.registry.name.as_str(), &l.verified.index)));
            (Arc::new(listings), Arc::new(catalog), errors)
        });
        cx.spawn_in(window, async move |this, cx| {
            let (listings, catalog, errors) = task.await;
            this.update_in(cx, |this, window, cx| {
                let m = &mut this.cfg.exts.market;
                m.loading = false;
                m.loaded_at = Some(now);
                m.listings = listings;
                m.catalog = Some(catalog);
                m.errors = errors;
                m.answered = None;
                this.search_market(window, cx);
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    /// Search the catalogue for the field's text in the background; an answer to an older keystroke is dropped.
    fn search_market(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let m = &mut self.cfg.exts.market;
        let Some(catalog) = m.catalog.clone() else { return };
        let query = m.search.as_ref().map(|f| f.read(cx).text().to_string()).unwrap_or_default();
        m.search_generation += 1;
        let generation = m.search_generation;
        let task = cx.background_executor().spawn({
            let query = query.clone();
            async move { catalog.search(&query) }
        });
        cx.spawn_in(window, async move |this, cx| {
            let results = task.await;
            this.update_in(cx, |this, window, cx| {
                let m = &mut this.cfg.exts.market;
                if m.search_generation != generation {
                    return;
                }
                m.results = Arc::new(results);
                m.answered = Some(query);
                m.cursor = 0;
                m.scroll.scroll_to_item(0, ScrollStrategy::Top);
                crate::perf::market_results_shown(window);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub(super) fn market_previous(&mut self, _: &MarketPrevious, _: &mut Window, cx: &mut Context<Self>) {
        self.move_market_cursor(-1, cx);
    }

    pub(super) fn market_next(&mut self, _: &MarketNext, _: &mut Window, cx: &mut Context<Self>) {
        self.move_market_cursor(1, cx);
    }

    fn move_market_cursor(&mut self, by: isize, cx: &mut Context<Self>) {
        let m = &mut self.cfg.exts.market;
        if m.results.is_empty() {
            return;
        }
        m.cursor = m.cursor.saturating_add_signed(by).min(m.results.len() - 1);
        // Scrolls only when the row is out of view, to the edge it went past.
        m.scroll.scroll_to_item(m.cursor, if by > 0 { ScrollStrategy::Bottom } else { ScrollStrategy::Top });
        cx.notify();
    }

    /// Open the page of the row at `ix` of the results (a click, or ↵ on the highlighted row).
    pub(super) fn open_market_row(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let m = &mut self.cfg.exts.market;
        let (Some(catalog), Some(&item)) = (m.catalog.clone(), m.results.get(ix)) else { return };
        m.cursor = ix;
        let item = catalog.item(item);
        self.open_market_page(item.registry.clone(), item.id.clone(), window, cx);
    }

    /// Show the page of `id` from `registry` and read its details in the background (from the cache when opened
    /// before).
    pub(super) fn open_market_page(&mut self, registry: String, id: ExtId, window: &mut Window, cx: &mut Context<Self>) {
        let Some(dir) = self.cfg.dir.clone() else { return };
        let exts = &mut self.cfg.exts;
        exts.pending = None;
        exts.selected = None;
        exts.market.page = Some(MarketPage { registry: registry.clone(), id: id.clone(), loaded: None });
        self.open_extension_page(cx);
        // Gone from the registry since the list was read: the page says so.
        let Some((l, entry)) = self.cfg.exts.market.page_entry() else { return };
        let (reg, release) = (l.registry.clone(), page_release(&l.verified, &id, entry).clone());
        let task = cx.background_executor().spawn({
            let id = id.clone();
            async move {
                let p = crate::ext_registry::page(&dir, &reg, &id, &release)?;
                let readme = p.readme.as_deref().map(crate::markdown::parse);
                Ok(LoadedPage { manifest: Arc::new(p.manifest), readme, readme_cut: p.readme_cut })
            }
        });
        cx.spawn_in(window, async move |this, cx| {
            let loaded = task.await;
            this.update(cx, |this, cx| {
                if let Some(page) = this.cfg.exts.market.page.as_mut().filter(|p| p.registry == registry && p.id == id) {
                    page.loaded = Some(loaded);
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    /// "Install" on a Market page: the same download, checks and consent sheet as Install from Registry, from this
    /// registry only.
    pub(super) fn install_from_market(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let m = &self.cfg.exts.market;
        let Some((page, l)) = m.page.as_ref().and_then(|p| Some((p, m.listing(&p.registry)?))) else { return };
        let Some(release) = l.verified.index.latest(&page.id, false) else { return };
        let spec = format!("{}@{}", page.id, release.version);
        let only = l.registry.name.clone();
        self.review_registry_extension(spec, Some(only), window, cx);
    }

    /// The Market's catalogue is read and its first results are on screen (`CODEYARD_PERF_MARKET`).
    pub fn market_ready_for_perf(&self) -> bool {
        let m = &self.cfg.exts.market;
        m.catalog.is_some() && m.answered.is_some()
    }

    /// The installed version of `id`, if any.
    pub(super) fn installed_version(&self, id: &ExtId) -> Option<String> {
        self.cfg.exts.list.iter().find(|e| &e.manifest.id == id).map(|e| e.manifest.version.to_string())
    }
}
