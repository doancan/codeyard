//! Workbench: servers the user allowed for an extension's `net-from-setting` (docs/plugin-architecture.md §6.1), such
//! as a company GitHub Enterprise named in the extension's settings. Only the extension's page adds one: **Allow
//! Host…** asks for the host (the setting's value filled in) and a system dialog names the extension, its publisher and
//! the host before anything is stored, as for an instance (§6.4). The hosts live with the instances in
//! `<data>/extensions/instances.json` (`ext_core::instances`); no extension, workspace or setting writes them. They join
//! the extension's `net` while its `net-from-setting` switch is on and the folder is trusted (`extension_scope`).

use std::collections::BTreeMap;

use super::ext_view_glue::{action_button, section_title};
use super::*;

#[derive(Default)]
pub(super) struct HostsState {
    /// The allowed hosts by extension id: read from the store on first use and after every change.
    hosts: std::cell::RefCell<Option<BTreeMap<String, Vec<String>>>>,
}

impl Workbench {
    /// The hosts the user allowed for `id`'s `net-from-setting`.
    pub(super) fn extension_user_hosts(&self, id: &str) -> Vec<String> {
        let mut cache = self.cfg.exts.hosts.hosts.borrow_mut();
        let all = cache.get_or_insert_with(|| match &self.cfg.dir {
            Some(dir) => crate::ext::store(dir).all_user_hosts(),
            None => BTreeMap::new(),
        });
        all.get(id).cloned().unwrap_or_default()
    }

    /// The value of the setting `net-from-setting` names, as the user set it.
    fn net_setting_value(m: &ext_manifest::Manifest, cx: &App) -> Option<String> {
        let key = m.permissions.net_from_setting.as_ref()?;
        let value = cx.global::<Settings>().ext_settings.get(&format!("ext.{}.{key}", m.id))?;
        value.as_str().map(str::to_string).filter(|s| !s.trim().is_empty())
    }

    /// The Servers section of an extension's page, for an extension with `net-from-setting`: each host the user allowed,
    /// with **Remove**, and **Allow Host…**.
    pub(super) fn extension_hosts_section(&self, m: &ext_manifest::Manifest, p: &Palette, cx: &mut Context<Self>) -> Option<gpui::Div> {
        let key = m.permissions.net_from_setting.clone()?;
        let ext = m.id.to_string();
        let hosts = self.extension_user_hosts(&ext);
        let mut section = div().flex().flex_col().gap(px(theme::SPACE[1])).child(section_title(crate::i18n::t("Servers"), p)).child(
            div()
                .text_color(c(p.text_muted))
                .child(crate::i18n::tf("It connects only to the servers you allow here, such as the one in its “{}” setting.", &[&key])),
        );
        for (i, host) in hosts.into_iter().enumerate() {
            let (e, h) = (ext.clone(), host.clone());
            section = section.child(
                div().id(("ext-user-host", i)).flex().items_center().gap(px(theme::SPACE[2])).child(div().child(host)).child(
                    action_button("ext-remove-host", crate::i18n::t("Remove"), p)
                        .on_click(cx.listener(move |this, _, _, cx| this.remove_extension_user_host(e.clone(), h.clone(), cx))),
                ),
            );
        }
        let typed = Self::net_setting_value(m, cx).unwrap_or_default();
        Some(
            section.child(
                div().child(
                    action_button("ext-allow-host", crate::i18n::t("Allow Host…"), p)
                        .on_click(cx.listener(move |this, _, window, cx| this.start_allow_extension_host(ext.clone(), typed.clone(), window, cx))),
                ),
            ),
        )
    }

    /// **Allow Host…**: ask for the host, with the setting's value filled in.
    pub(super) fn start_allow_extension_host(&mut self, extension: String, typed: String, window: &mut Window, cx: &mut Context<Self>) {
        let placeholder = crate::i18n::tf("Host name of the server {} may connect to, such as github.example.com", &[&extension]);
        self.show_input(InputPurpose::AllowHost { extension }, crate::i18n::t("Allow"), &placeholder, &typed, window, cx);
    }

    /// The typed host: checked, then the user agrees in a system dialog that names the extension, its publisher and the
    /// host, as for a permission (docs/plugin-architecture.md §6.3). Nothing is stored before that.
    pub(super) fn finish_allow_extension_host(&mut self, extension: String, typed: String, window: &mut Window, cx: &mut Context<Self>) {
        let reg = crate::ext::current();
        let Some(active) = reg.active.iter().find(|e| e.id() == extension).cloned() else { return };
        let checked = match self.extension_source(&active) {
            None => Err(crate::i18n::t("Install the extension to allow a server; one in development has none.").to_string()),
            Some(source) => ext_core::instances::user_host(&active.manifest, &typed).map(|h| (source, h)),
        };
        let (source, host) = match checked {
            Ok(x) => x,
            Err(e) => {
                self.notice = Some(Notice::Error(e.into()));
                cx.notify();
                return;
            }
        };
        let publisher = self.extension_publisher(&active);
        let name = active.manifest.name.clone();
        let who = crate::i18n::tf("{0} ({1}) is published by {2}.", &[&name, &extension, &publisher.shown]);
        let what = crate::i18n::tf(
            "It may send requests to {0} over HTTPS, with whatever it can read or store, such as a token you give it. Remove it on the extension's page in Extensions.",
            &[&host],
        );
        let answer = crate::questions::allow_host(&name, &host, &format!("{who}\n\n{what}")).ask(window, cx);
        cx.spawn_in(window, async move |this, cx| {
            if answer.await != Some(0) {
                return;
            }
            let _ = this.update(cx, |this, cx| {
                let (Some(dir), Ok(id)) = (this.cfg.dir.clone(), ext_manifest::ExtId::parse(&extension)) else { return };
                this.notice = Some(match crate::ext::store(&dir).add_user_host(&id, &source, &host) {
                    Ok(()) => {
                        *this.cfg.exts.hosts.hosts.borrow_mut() = None;
                        Notice::Info(crate::i18n::tf("{0} may now connect to {1}.", &[&extension, &host]).into())
                    }
                    Err(e) => Notice::Error(crate::i18n::tf("Could not allow the server: {}", &[&e]).into()),
                });
                cx.notify();
            });
        })
        .detach();
    }

    /// **Remove** on an allowed host: the extension's next request to it is refused.
    pub(super) fn remove_extension_user_host(&mut self, extension: String, host: String, cx: &mut Context<Self>) {
        let (Some(dir), Ok(id)) = (self.cfg.dir.clone(), ext_manifest::ExtId::parse(&extension)) else { return };
        if let Err(e) = crate::ext::store(&dir).remove_user_host(&id, &host) {
            self.notice = Some(Notice::Error(crate::i18n::tf("Could not remove the server: {}", &[&e]).into()));
        }
        *self.cfg.exts.hosts.hosts.borrow_mut() = None;
        cx.notify();
    }
}

#[cfg(test)]
impl Workbench {
    /// **Allow Host…** on the extension page of `extension`, with `typed` in its input.
    pub(crate) fn allow_host_for_test(&mut self, extension: &str, typed: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.start_allow_extension_host(extension.to_string(), typed.to_string(), window, cx);
        assert!(matches!(&self.overlay, Some(o) if matches!(&o.kind, OverlayKind::Input(InputPurpose::AllowHost { .. }))), "the input");
        self.close_overlay(window, cx);
        self.finish_allow_extension_host(extension.to_string(), typed.to_string(), window, cx);
    }

    pub(crate) fn extension_user_hosts_for_test(&self, extension: &str) -> Vec<String> {
        self.extension_user_hosts(extension)
    }
}
