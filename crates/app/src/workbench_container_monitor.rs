//! Live inventory uses the connected agent. Rendering never launches processes.
use super::*;

pub(super) fn button(id: impl Into<gpui::ElementId>, label: &'static str, p: &Palette) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .min_h(px(28.))
        .px(px(theme::SPACE[3]))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(theme::radius::SM))
        .cursor_pointer()
        .bg(c(p.surface_hover))
        .text_color(c(p.text))
        .text_size(px(text::XS.0))
        .debug_selector({
            let label = label.to_string();
            move || format!("container-button-{label}")
        })
        .hover(|s| s.bg(c(p.surface_active)))
        .child(label)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[gpui::test]
    fn container_files_can_return_to_original_ssh_folder_without_reconnecting(cx: &mut gpui::TestAppContext) {
        let (wb, cx) = crate::qa_workbench_tests::setup(cx);
        let source: Arc<dyn host::Host> = crate::container_monitor::tests::FixtureHost::new();
        let container: Arc<dyn host::Host> = crate::container_monitor::tests::FixtureHost::new();
        let root = crate::qa_workbench_tests::fresh("container-return-source").to_string_lossy().into_owned();
        let inside = crate::qa_workbench_tests::fresh("container-return-inside").to_string_lossy().into_owned();
        let alias = host::ssh::ssh_docker_target("linux-fixture", "default", "web-id").unwrap();
        cx.update(|_, cx| cx.global_mut::<Settings>().docker_enabled = true);
        wb.update_in(cx, |wb, window, cx| {
            wb.remote.host = source.clone();
            wb.remote.conn = Conn::Connected { alias: "linux-fixture".into() };
            wb.remote.docker.monitor.source = Some(source.clone());
            wb.remote.docker.monitor.host = Some("linux-fixture".into());
            wb.remote.docker.monitor.paused = true;
            wb.remote.docker.monitor.browser.selected = Some("web-id".into());
            wb.set_root(root.clone(), cx);
            wb.remote.pending_path = Some(inside);
            wb.attach_host(container, alias, window, cx);
            assert_eq!(wb.remote.docker.monitor.return_workspace.as_ref().unwrap().root.as_deref(), Some(root.as_str()));
            wb.tabs.page_active = false;
        });
        cx.run_until_parked();
        let back = cx.debug_bounds("container-return-workspace").expect("file view exposes a persistent return action").center();
        cx.simulate_click(back, gpui::Modifiers::none());
        cx.run_until_parked();
        wb.read_with(cx, |wb, cx| {
            assert!(Arc::ptr_eq(&wb.remote.host, &source));
            assert!(matches!(&wb.remote.conn,Conn::Connected{alias} if alias=="linux-fixture"));
            assert_eq!(wb.root(cx).as_deref(), Some(root.as_str()));
            assert_eq!(wb.remote.docker.monitor.browser.selected.as_deref(), Some("web-id"));
            assert_eq!(wb.page_for_test(), Some(("containers", true)));
            assert!(wb.remote.docker.monitor.return_workspace.is_none());
        });
    }
    #[gpui::test]
    fn container_monitor_autodiscovers_and_renders_live_resources_and_dashboard(cx: &mut gpui::TestAppContext) {
        let (wb, cx) = crate::qa_workbench_tests::setup(cx);
        let source = crate::container_monitor::tests::FixtureHost::new();
        cx.update(|_, cx| cx.global_mut::<Settings>().docker_enabled = true);
        wb.update(cx, |wb, cx| {
            wb.remote.host = source.clone();
            wb.remote.conn = Conn::Connected { alias: "linux-fixture".into() };
            wb.sync_container_monitor(cx);
        });
        crate::qa_integration_tests::wait_ticking(cx, "automatic container inventory", |cx| {
            wb.read_with(cx, |wb, _| wb.remote.docker.monitor.updated.is_some())
        });
        assert_eq!(source.calls.load(Ordering::Relaxed), 4);
        wb.update(cx, |wb, cx| {
            assert_eq!(wb.remote.docker.monitor.containers.len(), 2);
            assert_eq!(wb.remote.docker.monitor.usage[0].cpu, "145.6%");
            wb.remote.docker.monitor.paused = true;
            wb.show_containers(cx);
        });
        cx.run_until_parked();
        assert!(cx.debug_bounds("container-resource-web-id").is_some());
        assert!(cx.debug_bounds("container-resource-db-id").is_some());
        let open = cx.debug_bounds("container-resource-web-id").unwrap().center();
        cx.simulate_click(open, gpui::Modifiers::none());
        cx.run_until_parked();
        assert_eq!(wb.read_with(cx, |wb, _| wb.page_for_test()), Some(("containers", true)));
        assert!(cx.debug_bounds("container-dashboard").is_some());
        assert!(cx.debug_bounds("dashboard-row-web-id").is_none(), "the main view must not repeat the sidebar inventory");
        wb.update(cx, |wb, cx| {
            wb.remote.docker.monitor.clear();
            assert!(wb.remote.docker.monitor.containers.is_empty());
            cx.notify();
        });
    }
}

impl Workbench {
    pub(super) fn return_from_container(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.refuse_if_dirty(cx) {
            return;
        }
        let Some(previous) = self.remote.docker.monitor.return_workspace.take() else {
            return;
        };
        self.remote.pending_path = previous.root.or_else(|| Some(previous.host.info().home.clone()));
        self.persist.opening_recent = None;
        self.attach_host(previous.host, previous.alias, window, cx);
        self.chrome.panel = Some(Panel::Containers);
        self.open_container_dashboard(cx);
        cx.notify();
    }

    pub(super) fn open_container_dashboard(&mut self, cx: &mut Context<Self>) {
        if !cx.global::<Settings>().docker_enabled {
            return;
        }
        self.ensure_container_search(cx);
        self.tabs.page = Some(Page::Containers);
        self.tabs._page_sub = None;
        self.tabs.page_active = true;
        self.tabs.diff_active = false;
        self.tabs.large_active = false;
        self.sync_container_monitor(cx);
        cx.notify();
    }

    pub(super) fn sync_container_monitor(&mut self, cx: &mut Context<Self>) {
        if !cx.global::<Settings>().docker_enabled {
            self.remote.docker.monitor.clear();
            return;
        }
        let direct = match &self.remote.conn {
            Conn::Connected { alias } if self.remote.host.is_remote() && matches!(host::ssh::target_of(alias), host::ssh::Target::Ssh(_)) => {
                Some(alias.clone())
            }
            _ => None,
        };
        if let Some(host) = direct {
            if self.remote.docker.monitor.host.as_ref() != Some(&host)
                || self.remote.docker.monitor.source.as_ref().is_none_or(|source| !Arc::ptr_eq(source, &self.remote.host))
            {
                self.remote.docker.monitor.clear();
                self.remote.docker.monitor.host = Some(host);
                self.remote.docker.monitor.source = Some(self.remote.host.clone());
            }
            self.refresh_container_monitor(cx);
        } else if !self.docker_connected() {
            self.remote.docker.monitor.clear();
        }
        self.ensure_container_search(cx);
    }

    pub(super) fn refresh_container_monitor(&mut self, cx: &mut Context<Self>) {
        if !cx.global::<Settings>().docker_enabled {
            return;
        }
        let monitor = &mut self.remote.docker.monitor;
        if monitor.paused || monitor.cancel.is_some() {
            return;
        }
        let (Some(source), Some(host)) = (monitor.source.clone(), monitor.host.clone()) else {
            return;
        };
        let disk = monitor.disk_updated.is_none_or(|at| at.elapsed() >= Duration::from_secs(30));
        let cancel = Arc::new(std::sync::atomic::AtomicBool::new(false));
        monitor.cancel = Some(cancel.clone());
        monitor.generation += 1;
        let generation = monitor.generation;
        let task = cx.background_executor().spawn(async move { crate::container_monitor::read(source.as_ref(), &host, disk, &cancel) });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| {
                if this.remote.docker.monitor.generation != generation || !cx.global::<Settings>().docker_enabled {
                    return;
                }
                this.remote.docker.monitor.apply(result);
                if this.tabs.page_active
                    && matches!(this.tabs.page, Some(Page::Containers))
                    && this.remote.docker.monitor.browser.tab == crate::container_history::Tab::Logs
                    && !this.remote.docker.monitor.browser.loading
                    && let Some(id) = this.remote.docker.monitor.browser.selected.clone()
                {
                    this.select_container(id, crate::container_history::Tab::Logs, cx);
                }
                cx.notify();
            })
            .ok();
            cx.background_executor().timer(Duration::from_secs(3)).await;
            this.update(cx, |this, cx| {
                if this.remote.docker.monitor.generation == generation
                    && (this.chrome.panel == Some(Panel::Containers) || (this.tabs.page_active && matches!(this.tabs.page, Some(Page::Containers))))
                {
                    this.refresh_container_monitor(cx);
                }
            })
            .ok();
        })
        .detach();
        cx.notify();
    }
}
