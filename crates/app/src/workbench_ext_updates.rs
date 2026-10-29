//! Workbench: updates and version status of installed registry extensions (#69). The check (`ext_updates::check`)
//! runs in the background whenever settings load (which includes a change of the lock file) and once an hour; it
//! is cheap within the day, when each registry's cached index is used. What it did is said in one notice, what needs
//! the user is shown on the extension's page: a card with only the new permissions, **Update Now** (no 48 h wait),
//! and the full consent sheet for another publisher or a list older than 3 days.

use std::collections::{BTreeMap, BTreeSet};

use ext_manifest::{ExtId, Permissions, Version};
use gpui::{Context, Window};

use super::*;
use crate::ext_updates::{Offer, Status};

/// How often a long session checks again (each registry is still asked at most once a day).
const POLL_SECS: u64 = 3600;

#[derive(Default)]
pub(super) struct UpdateState {
    /// Each installed registry extension after the last check.
    pub(super) status: BTreeMap<ExtId, Status>,
    checking: bool,
    /// Asked for again while a check ran.
    again: bool,
    /// Versions already announced in a notice, so a card is not announced on every check.
    told: BTreeSet<(ExtId, Version)>,
    /// The extension an **Update Now** is downloading.
    pub(super) busy: Option<ExtId>,
}

impl Workbench {
    /// Check the installed registry extensions in the background; at most one check at a time.
    pub(super) fn check_extension_updates(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let settings = cx.global::<Settings>();
        let Some(dir) = self.cfg.dir.clone().filter(|_| settings.extensions_enabled && !crate::ext::turned_off()) else { return };
        let registries = crate::ext_registry::registries(settings);
        if registries.is_empty() {
            return;
        }
        let auto = settings.extension_auto_update;
        let u = &mut self.cfg.exts.updates;
        if u.checking {
            u.again = true;
            return;
        }
        u.checking = true;
        let task = cx.background_executor().spawn(async move { crate::ext_updates::check(&dir, &registries, auto, crate::ext_registry::now()) });
        cx.spawn_in(window, async move |this, cx| {
            let report = task.await;
            this.update_in(cx, |this, window, cx| {
                let u = &mut this.cfg.exts.updates;
                u.checking = false;
                u.status = report.status;
                let mut ask = Vec::new();
                for (id, s) in &u.status {
                    if let Some(o) = s.offer.as_ref().filter(|o| matches!(o, Offer::MorePermissions { .. } | Offer::NewPublisher { .. }))
                        && u.told.insert((id.clone(), o.version().clone()))
                    {
                        ask.push(crate::i18n::tf("{0} {1} waits for your OK in the Extensions view.", &[id, o.version()]));
                    }
                }
                let again = std::mem::take(&mut u.again);
                if report.changed {
                    this.reload_config(window, cx);
                }
                this.refresh_extension_list();
                if !report.stopped.is_empty() {
                    this.notice = Some(Notice::Error(report.stopped.join(" ").into()));
                } else if !report.done.is_empty() || !ask.is_empty() {
                    this.notice = Some(Notice::Info(report.done.into_iter().chain(ask).collect::<Vec<_>>().join(" ").into()));
                }
                if again {
                    this.check_extension_updates(window, cx);
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Check again every hour while the window lives.
    pub(super) fn poll_extension_updates(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        cx.spawn_in(window, async move |this, cx| {
            loop {
                cx.background_executor().timer(std::time::Duration::from_secs(POLL_SECS)).await;
                if this.update_in(cx, |this, window, cx| this.check_extension_updates(window, cx)).is_err() {
                    break;
                }
            }
        })
        .detach();
    }

    /// **Update Now** (and **Try Again**): the newest version without the 48 h wait. It installs when it asks for
    /// nothing new; else the card or the full sheet asks first.
    pub(super) fn update_extension_now(&mut self, id: ExtId, window: &mut Window, cx: &mut Context<Self>) {
        self.update_extension(id, None, window, cx);
    }

    /// **Update** on the card: the user agreed to the new permissions it listed (and no more).
    pub(super) fn accept_extension_update(&mut self, id: ExtId, window: &mut Window, cx: &mut Context<Self>) {
        let added = match self.cfg.exts.updates.status.get(&id).and_then(|s| s.offer.as_ref()) {
            Some(Offer::MorePermissions { added, .. }) => added.clone(),
            _ => return,
        };
        self.update_extension(id, Some(added), window, cx);
    }

    fn update_extension(&mut self, id: ExtId, accepted: Option<Permissions>, window: &mut Window, cx: &mut Context<Self>) {
        let Some(dir) = self.cfg.dir.clone().filter(|_| !crate::ext::turned_off()) else { return };
        if self.cfg.exts.updates.busy.is_some() {
            return;
        }
        let registries = crate::ext_registry::registries(cx.global::<Settings>());
        self.cfg.exts.updates.busy = Some(id.clone());
        self.notice = Some(Notice::Info(crate::i18n::tf("Updating {}…", &[&id]).into()));
        cx.notify();
        let task = cx.background_executor().spawn({
            let id = id.clone();
            async move { crate::ext_updates::update_now(&dir, &registries, &id, accepted.as_ref(), crate::ext_registry::now()) }
        });
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            this.update_in(cx, |this, window, cx| {
                this.cfg.exts.updates.busy = None;
                this.notice = None;
                let registry = this.cfg.exts.updates.status.get(&id).map(|s| s.registry.clone());
                match result {
                    Ok(None) => {
                        if let Some(s) = this.cfg.exts.updates.status.get_mut(&id) {
                            s.offer = None;
                        }
                        let version = this.installed_version_after_reload(&id);
                        this.after_extension_change(crate::i18n::tf("{0} was updated to version {1}.", &[&id, &version]), window, cx);
                    }
                    Ok(Some(offer)) if offer.needs_sheet() => {
                        this.review_registry_extension(format!("{id}@{}", offer.version()), registry, window, cx);
                    }
                    Ok(Some(offer)) => {
                        if let Some(s) = this.cfg.exts.updates.status.get_mut(&id) {
                            s.offer = Some(offer);
                        }
                        cx.notify();
                    }
                    Err(e) => {
                        this.notice = Some(Notice::Error(crate::i18n::tf("Cannot update {0}: {1}", &[&id, &e]).into()));
                        cx.notify();
                    }
                }
            })
            .ok();
        })
        .detach();
    }

    /// The version in the lock now (after an update, before the list is read again).
    fn installed_version_after_reload(&self, id: &ExtId) -> String {
        let Some(dir) = self.cfg.dir.as_deref() else { return String::new() };
        crate::ext::store(dir).locked().ok().and_then(|l| l.get(id).map(|l| l.version.to_string())).unwrap_or_default()
    }

    /// Get the registry's pre-releases of one extension, or stop getting them; the next check follows.
    pub(super) fn set_extension_pre_release(&mut self, id: ExtId, on: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(store) = self.cfg.dir.as_deref().filter(|_| !crate::ext::turned_off()).map(crate::ext::store) else { return };
        match store.set_pre_release(&id, on) {
            Ok(()) => {
                let done = if on {
                    crate::i18n::tf("{} gets pre-releases from now on.", &[&id])
                } else {
                    crate::i18n::tf("{} gets releases only; the pre-release you have stays until a release passes it.", &[&id])
                };
                self.after_extension_change(done, window, cx);
            }
            Err(e) => {
                self.notice = Some(Notice::Error(e.to_string().into()));
                cx.notify();
            }
        }
    }
}
