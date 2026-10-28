//! Web Preview lifecycle and native editor controls. No server exists while disabled.
use super::*;

#[derive(Default)]
pub(super) struct State {
    pub server: Option<crate::web_preview::Server>,
    events: Option<gpui::Task<()>>,
    generation: u64,
    starting: bool,
    config: Option<crate::web_preview_config::Config>,
}

impl Workbench {
    pub(super) fn copy_web_preview_url(&mut self, cx: &mut Context<Self>) {
        let Some(server) = self.chrome.web_preview.server.as_ref() else {
            self.notice = Some(Notice::Info(crate::i18n::t("Start Web Preview before copying its URL.").into()));
            cx.notify();
            return;
        };
        let relative = self.active_editor().and_then(|editor| {
            let editor = editor.read(cx);
            (editor.uses_host(&self.remote.host) && crate::web_preview::is_html(&editor.path))
                .then(|| editor.path.strip_prefix(&format!("{}/", server.root().trim_end_matches('/'))).map(str::to_owned))
                .flatten()
        });
        cx.write_to_clipboard(gpui::ClipboardItem::new_string(server.url(relative.as_deref().unwrap_or(""))));
        self.notice = Some(Notice::Info(crate::i18n::t("Preview URL copied.").into()));
        cx.notify();
    }

    pub(super) fn web_preview_needs_restart(&self, config: &crate::web_preview_config::Config) -> bool {
        self.chrome.web_preview.config.as_ref().is_some_and(|current| current != config)
    }

    pub(super) fn stop_web_preview(&mut self, cx: &mut Context<Self>) {
        let state = &mut self.chrome.web_preview;
        state.generation += 1;
        state.starting = false;
        state.config = None;
        state.events = None;
        if state.server.take().is_some() {
            self.log_output(crate::i18n::t("Web Preview stopped.").to_string());
        }
        self.sync_watches(cx);
        cx.notify();
    }

    pub(super) fn open_web_preview(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !cx.global::<Settings>().web_preview_enabled {
            self.notice = Some(Notice::Info(crate::i18n::t("Enable Web Preview in Settings > Modules to preview HTML files.").into()));
            cx.notify();
            return;
        }
        let path = self
            .active_editor()
            .filter(|ed| ed.read(cx).uses_host(&self.remote.host))
            .map(|ed| ed.read(cx).path.clone())
            .filter(|p| crate::web_preview::is_html(p));
        let base = path
            .as_ref()
            .and_then(|path| self.all_roots(cx).into_iter().filter(|r| path.starts_with(&format!("{}/", r.trim_end_matches('/')))).max_by_key(String::len))
            .or_else(|| path.as_ref().map(|p| parent_of(p)))
            .or_else(|| self.root(cx));
        let config = cx.global::<Settings>().web_preview.clone();
        let info = self.remote.host.info();
        let root = config.resolve_root(base.as_deref(), &info.home, &info.os);
        let Some(root) = root else {
            self.notice = Some(Notice::Info(crate::i18n::t("Open an HTML file or a project folder to start Web Preview.").into()));
            cx.notify();
            return;
        };
        let relative = path.and_then(|p| p.strip_prefix(&format!("{}/", root.trim_end_matches('/'))).map(str::to_string)).unwrap_or_default();
        let host = self.remote.host.clone();
        let generation = self.chrome.web_preview.generation;
        self.require_trust_for(Some(root.clone()), window, cx, move |this, _, cx| {
            if !cx.global::<Settings>().web_preview_enabled
                || cx.global::<Settings>().web_preview != config
                || generation != this.chrome.web_preview.generation
                || !Arc::ptr_eq(&host, &this.remote.host)
            {
                return;
            }
            this.start_web_preview(root, relative, cx);
        });
    }

    fn start_web_preview(&mut self, root: String, relative: String, cx: &mut Context<Self>) {
        let config = cx.global::<Settings>().web_preview.clone();
        if let Some(server) = &self.chrome.web_preview.server
            && server.root().trim_end_matches('/') == root.trim_end_matches('/')
            && server.uses_host(&self.remote.host)
            && server.config() == &config
        {
            cx.open_url(&server.url(&relative));
            return;
        }
        if self.chrome.web_preview.starting {
            return;
        }
        self.stop_web_preview(cx);
        self.chrome.web_preview.starting = true;
        self.chrome.web_preview.config = Some(config.clone());
        let generation = self.chrome.web_preview.generation;
        let host = self.remote.host.clone();
        let task = cx.background_executor().spawn(async move {
            // Check the source before binding a listener; remote failures never block the UI.
            host.read_dir(&root).map_err(|e| e.to_string())?;
            crate::web_preview::Server::start(host, root, config)
        });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| {
                if generation != this.chrome.web_preview.generation || !cx.global::<Settings>().web_preview_enabled {
                    return;
                }
                this.chrome.web_preview.starting = false;
                match result {
                    Ok((server, rx)) => {
                        let url = server.url(&relative);
                        this.log_output(crate::i18n::tf("Web Preview listening on {}", &[&server.listen_addr()]));
                        this.chrome.web_preview.server = Some(server);
                        this.chrome.web_preview.events = Some(cx.spawn(async move |this, cx| {
                            use futures::StreamExt;
                            let mut events = ui::bridge(rx);
                            while let Some(event) = events.next().await {
                                if this
                                    .update(cx, |this, cx| {
                                        if generation != this.chrome.web_preview.generation {
                                            return;
                                        }
                                        match event {
                                            crate::web_preview::Event::Watch => this.sync_watches(cx),
                                            crate::web_preview::Event::Error(error) => {
                                                this.log_output(crate::i18n::tf("Web Preview: {}", &[&error.chars().take(512).collect::<String>()]));
                                                cx.notify();
                                            }
                                        }
                                    })
                                    .is_err()
                                {
                                    break;
                                }
                            }
                        }));
                        cx.open_url(&url);
                    }
                    Err(error) => {
                        this.chrome.web_preview.config = None;
                        let message = crate::i18n::tf("Could not start Web Preview: {}", &[&error]);
                        this.log_output(message.clone());
                        this.notice = Some(Notice::Error(message.into()));
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    pub(super) fn web_preview_status(&self, p: &Palette, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !cx.global::<Settings>().web_preview_enabled {
            return None;
        }
        let state = &self.chrome.web_preview;
        let label = if let Some(server) = &state.server {
            format!("{} :{}", crate::i18n::t("Preview"), server.port())
        } else if state.starting {
            crate::i18n::t("Starting preview…").to_string()
        } else {
            crate::i18n::t("Web Preview").to_string()
        };
        Some(
            div()
                .flex()
                .flex_none()
                .items_center()
                .h_full()
                .gap(px(4.))
                .px(px(8.))
                .child(
                    div()
                        .id("web-preview-open")
                        .debug_selector(|| "web-preview-open".into())
                        .cursor_pointer()
                        .text_color(c(if state.server.is_some() { p.accent } else { p.text_muted }))
                        .child(label)
                        .tooltip(|_, cx| cx.new(|_| Tooltip(crate::i18n::t("Preview HTML in your browser").into())).into())
                        .on_click(cx.listener(|this, _, window, cx| this.open_web_preview(window, cx))),
                )
                .when(state.server.is_some() || state.starting, |d| {
                    d.child(
                        ui::icon_button("web-preview-stop", "square", crate::i18n::t("Stop Web Preview"), p)
                            .on_click(cx.listener(|this, _, _, cx| this.stop_web_preview(cx))),
                    )
                })
                .into_any_element(),
        )
    }
}

#[cfg(test)]
#[path = "workbench_web_preview_tests.rs"]
mod tests;
