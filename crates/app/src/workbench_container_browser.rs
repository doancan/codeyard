//! Container browser: compact navigation, sortable inventory, and selected-container diagnostics.
use super::container_monitor_glue::button;
use super::*;
use crate::container_history::{self as history, Sort};

impl Workbench {
    pub(super) fn ensure_container_search(&mut self, cx: &mut Context<Self>) {
        if self.remote.docker.monitor.browser.overview.focus.is_none() {
            self.remote.docker.monitor.browser.overview.focus = Some(cx.focus_handle().tab_stop(true));
        }
        if self.remote.docker.monitor.browser.field.is_some() {
            return;
        }
        let field = cx.new(|cx| crate::input::TextField::new(crate::i18n::t("Search containers, images or projects…"), cx));
        let subscription = cx.subscribe(&field, |this, field, event, cx| {
            if matches!(event, crate::input::FieldEvent::Changed) {
                this.remote.docker.monitor.browser.query = field.read(cx).text().to_string();
                cx.notify();
            }
        });
        self.remote.docker.monitor.browser.field = Some(field);
        self.remote.docker.monitor.browser.subscription = Some(subscription);
    }

    pub(super) fn container_rows(&self) -> Vec<&crate::docker::Container> {
        let m = &self.remote.docker.monitor;
        let q = m.browser.query.trim().to_lowercase();
        let mut rows: Vec<_> = m
            .containers
            .iter()
            .filter(|c| {
                m.browser.status_filter.matches(&c.state)
                    && (q.is_empty()
                        || q.split_whitespace()
                            .all(|term| format!("{} {} {} {}", c.name, c.image, c.id, history::project(c).unwrap_or_default()).to_lowercase().contains(term)))
            })
            .collect();
        let percent = |id: &str, memory: bool| {
            m.browser
                .sort_usage
                .iter()
                .find(|u| u.id == id)
                .and_then(|u| crate::container_monitor::Usage::percent(if memory { &u.memory_percent } else { &u.cpu }))
                .unwrap_or(-1.)
        };
        rows.sort_by(|a, b| {
            history::project(a)
                .cmp(&history::project(b))
                .then_with(|| match m.browser.sort {
                    Sort::Name => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
                    Sort::Cpu => percent(&b.id, false).total_cmp(&percent(&a.id, false)),
                    Sort::Memory => percent(&b.id, true).total_cmp(&percent(&a.id, true)),
                })
                .then_with(|| a.name.cmp(&b.name))
        });
        rows
    }

    fn container_update_status(&self, p: &Palette) -> gpui::Stateful<gpui::Div> {
        let m = &self.remote.docker.monitor;
        let (status, color) = if m.error.is_some() && m.updated.is_some() {
            (crate::i18n::t("Showing the last successful snapshot."), p.warning)
        } else if m.paused {
            (crate::i18n::t("Updates paused"), p.warning)
        } else if m.updated.is_some() {
            (crate::i18n::t("Live updates"), if m.warning.is_some() { p.warning } else { p.success })
        } else {
            (crate::i18n::t("Discovering containers…"), p.info)
        };
        div()
            .id("container-update-status")
            .debug_selector(|| "container-update-status".into())
            .flex_none()
            .max_w_full()
            .flex()
            .items_center()
            .gap(px(theme::SPACE[2]))
            .text_size(px(text::XS.0))
            .text_color(c(p.text_muted))
            .child(div().size(px(6.)).rounded_full().flex_none().bg(c(color)))
            .child(div().min_w_0().truncate().child(status))
    }

    pub(super) fn container_dashboard_view(&self, p: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let m = &self.remote.docker.monitor;
        if !cx.global::<Settings>().docker_enabled {
            return div()
                .p(px(theme::SPACE[6]))
                .text_color(c(p.text))
                .child(crate::i18n::t("Container module is off. Enable modules.containers.enabled in Settings."))
                .into_any_element();
        }
        let context = m
            .host
            .as_ref()
            .map(|h| format!("SSH · {h} · {}", m.context.as_ref().map(|c| c.name.as_str()).unwrap_or("—")))
            .unwrap_or_else(|| crate::i18n::t("No SSH host connected").into());
        let selected = m.browser.overview.details.then(|| m.browser.selected.as_ref().and_then(|id| m.containers.iter().find(|c| &c.id == id))).flatten();
        let title = selected.map(|container| container.name.clone()).unwrap_or_else(|| crate::i18n::t("Containers").to_string());
        let running = m.containers.iter().filter(|c| c.state == "running").count();
        let issues = m.containers.iter().filter(|c| super::container_overview_glue::unhealthy(c)).count();
        let mut view = div()
            .id("container-dashboard")
            .debug_selector(|| "container-dashboard".into())
            .size_full()
            .min_w_0()
            .min_h_0()
            .flex()
            .flex_col()
            .bg(c(p.bg))
            .text_color(c(p.text))
            .child(
                div()
                    .id("container-dashboard-header")
                    .debug_selector(|| "container-dashboard-header".into())
                    .p(px(theme::SPACE[5]))
                    .border_b_1()
                    .border_color(c(p.border))
                    .bg(c(p.surface))
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(px(theme::SPACE[4]))
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(220.))
                            .flex()
                            .flex_col()
                            .gap(px(theme::SPACE[3]))
                            .child(
                                div()
                                    .flex()
                                    .flex_wrap()
                                    .items_center()
                                    .gap(px(theme::SPACE[4]))
                                    .child(
                                        div()
                                            .id("container-dashboard-title")
                                            .debug_selector(|| "container-dashboard-title".into())
                                            .flex_none()
                                            .min_w_0()
                                            .max_w_full()
                                            .truncate()
                                            .text_size(px(text::XL.0))
                                            .font_weight(gpui::FontWeight::SEMIBOLD)
                                            .tooltip({
                                                let title = title.clone();
                                                move |_, cx| cx.new(|_| Tooltip(title.clone().into())).into()
                                            })
                                            .child(title),
                                    )
                                    .when(m.host.is_some(), |d| d.child(self.container_update_status(p))),
                            )
                            .child(
                                div()
                                    .flex()
                                    .flex_wrap()
                                    .items_center()
                                    .gap(px(theme::SPACE[4]))
                                    .text_size(px(text::XS.0))
                                    .text_color(c(p.text_muted))
                                    .child(
                                        div()
                                            .id("container-dashboard-context")
                                            .min_w_0()
                                            .max_w_full()
                                            .truncate()
                                            .tooltip({
                                                let context = context.clone();
                                                move |_, cx| cx.new(|_| Tooltip(context.clone().into())).into()
                                            })
                                            .child(context),
                                    )
                                    .when(!m.browser.overview.details && m.updated.is_some(), |d| {
                                        d.child(div().id("container-fleet-summary").debug_selector(|| "container-fleet-summary".into()).child(crate::i18n::tf(
                                            "{0} containers · {1} running · {2} need attention",
                                            &[&m.containers.len(), &running, &issues],
                                        )))
                                    }),
                            ),
                    )
                    .child(
                        div()
                            .min_w_0()
                            .max_w_full()
                            .flex()
                            .flex_wrap()
                            .items_center()
                            .gap(px(theme::SPACE[2]))
                            .child(button("dashboard-refresh", crate::i18n::t("Refresh"), p).flex_none().on_click(cx.listener(|this, _, _, cx| {
                                this.remote.docker.monitor.paused = false;
                                this.refresh_container_monitor(cx);
                            })))
                            .child(
                                button("dashboard-pause", if m.paused { crate::i18n::t("Resume updates") } else { crate::i18n::t("Pause updates") }, p)
                                    .flex_none()
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.remote.docker.monitor.paused = !this.remote.docker.monitor.paused;
                                        if !this.remote.docker.monitor.paused {
                                            this.refresh_container_monitor(cx);
                                        }
                                        cx.notify();
                                    })),
                            )
                            .when_some(selected, |d, container| d.child(self.container_detail_actions(container, p, cx)))
                            .when(selected.is_none(), |d| {
                                d.child(
                                    button("container-tools", crate::i18n::t("Container actions…"), p)
                                        .flex_none()
                                        .on_click(cx.listener(|this, _, window, cx| this.open_container_tools(window, cx))),
                                )
                            }),
                    ),
            );
        if let Some(error) = m.error.as_ref().or(m.warning.as_ref()) {
            view = view.child(
                div()
                    .id("container-dashboard-error")
                    .debug_selector(|| "container-dashboard-error".into())
                    .min_w_0()
                    .mx(px(theme::SPACE[5]))
                    .mt(px(theme::SPACE[3]))
                    .p(px(theme::SPACE[3]))
                    .rounded(px(theme::radius::SM))
                    .border_1()
                    .border_color(c(p.border))
                    .bg(ui::ca(p.warning, 0.08))
                    .text_size(px(text::SM.0))
                    .text_color(c(p.text))
                    .child(crate::i18n::tf("Could not refresh container data: {}. Check Docker access on the SSH host and retry.", &[error])),
            );
        }
        if m.browser.overview.details || m.browser.selected.is_some() {
            view = view.child(div().pt(px(theme::SPACE[3])).child(self.container_overview_navigation(p, cx)));
        }
        if !m.browser.overview.details {
            view = view.child(self.container_overview_view(p, cx));
        } else if let Some(container) = m.browser.selected.as_ref().and_then(|id| m.containers.iter().find(|c| &c.id == id)) {
            view = view.child(self.container_detail_view(container, p, cx));
        } else {
            view = view.child(
                div()
                    .flex_1()
                    .p(px(theme::SPACE[6]))
                    .text_size(px(text::SM.0))
                    .text_color(c(p.text_muted))
                    .child(crate::i18n::t("Select a container to inspect resource history, logs, ports and storage.")),
            );
        }
        view.into_any_element()
    }
}

pub(super) fn state_label(state: &str) -> String {
    match state {
        "running" => crate::i18n::t("Running"),
        "exited" | "created" => crate::i18n::t("Stopped"),
        "paused" => crate::i18n::t("Paused"),
        _ => state,
    }
    .into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::container_history::Tab;
    #[gpui::test]
    fn container_browser_filters_and_background_refresh_keep_navigation_stable(cx: &mut gpui::TestAppContext) {
        let (wb, cx) = crate::qa_workbench_tests::setup(cx);
        let source: Arc<dyn host::Host> = crate::container_monitor::tests::FixtureHost::new();
        let snapshot = crate::container_monitor::read(source.as_ref(), "fixture", true, &std::sync::atomic::AtomicBool::new(false)).unwrap();
        cx.update(|_, cx| cx.global_mut::<Settings>().docker_enabled = true);
        wb.update(cx, |wb, cx| {
            let m = &mut wb.remote.docker.monitor;
            m.host = Some("fixture".into());
            m.source = Some(source);
            m.apply(Ok(snapshot));
            m.paused = true;
            for c in &mut m.containers {
                c.labels.clear();
            }
            m.usage.push(crate::container_monitor::Usage { id: "db-id".into(), cpu: "1%".into(), ..Default::default() });
            m.browser.selected = Some("web-id".into());
            wb.ensure_container_search(cx);
            wb.chrome.panel = None;
            wb.tabs.page = Some(Page::Containers);
            wb.tabs.page_active = true;
            cx.notify();
        });
        cx.run_until_parked();
        let sort = cx.debug_bounds("overview-sort-CPU").unwrap().center();
        cx.simulate_click(sort, gpui::Modifiers::none());
        cx.run_until_parked();
        assert_eq!(wb.read_with(cx, |wb, _| wb.container_rows()[0].id.clone()), "web-id");
        let before = cx.debug_bounds("overview-row-web-id").unwrap();
        wb.update(cx, |wb, cx| {
            let m = &mut wb.remote.docker.monitor;
            m.paused = false;
            m.cancel = Some(Arc::new(std::sync::atomic::AtomicBool::new(false)));
            m.usage[0].cpu = "0%".into();
            m.usage[1].cpu = "99%".into();
            cx.notify();
        });
        cx.run_until_parked();
        assert_eq!(cx.debug_bounds("overview-row-web-id").unwrap(), before);
        assert_eq!(wb.read_with(cx, |wb, _| wb.container_rows()[0].id.clone()), "web-id");
        cx.simulate_click(sort, gpui::Modifiers::none());
        cx.run_until_parked();
        assert_eq!(wb.read_with(cx, |wb, _| wb.container_rows()[0].id.clone()), "db-id");
        let stopped = cx.debug_bounds("container-button-Stopped").unwrap().center();
        cx.simulate_click(stopped, gpui::Modifiers::none());
        cx.run_until_parked();
        wb.read_with(cx, |wb, _| {
            assert_eq!(wb.container_rows().len(), 1);
            assert_eq!(wb.container_rows()[0].id, "db-id");
            assert_eq!(wb.remote.docker.monitor.browser.selected.as_deref(), Some("web-id"));
        });
        let field = wb.read_with(cx, |wb, _| wb.remote.docker.monitor.browser.field.clone()).unwrap();
        field.update(cx, |f, cx| f.set_text("no match", cx));
        cx.run_until_parked();
        assert!(wb.read_with(cx, |wb, _| wb.container_rows().is_empty()));
        let clear = cx.debug_bounds("container-button-Clear filters").unwrap().center();
        cx.simulate_click(clear, gpui::Modifiers::none());
        cx.run_until_parked();
        assert_eq!(wb.read_with(cx, |wb, _| wb.container_rows().len()), 2);
        assert!(field.read_with(cx, |field, _| field.text().is_empty()));
        field.update(cx, |f, cx| f.set_text("web nginx", cx));
        cx.run_until_parked();
        assert_eq!(wb.read_with(cx, |wb, _| wb.container_rows().len()), 1);
        wb.update(cx, |wb, cx| {
            wb.remote.docker.monitor.clear();
            cx.notify();
        });
    }
    #[gpui::test]
    fn container_browser_filters_selects_and_loads_real_detail_actions(cx: &mut gpui::TestAppContext) {
        let (wb, cx) = crate::qa_workbench_tests::setup(cx);
        let source = crate::container_monitor::tests::FixtureHost::new();
        cx.update(|_, cx| cx.global_mut::<Settings>().docker_enabled = true);
        wb.update(cx, |wb, cx| {
            wb.remote.host = source;
            wb.remote.conn = Conn::Connected { alias: "linux-fixture".into() };
            wb.sync_container_monitor(cx);
        });
        crate::qa_integration_tests::wait_ticking(cx, "inventory", |cx| wb.read_with(cx, |wb, _| wb.remote.docker.monitor.updated.is_some()));
        wb.update(cx, |wb, cx| {
            wb.remote.docker.monitor.paused = true;
            wb.open_container_dashboard(cx);
        });
        let field = wb.read_with(cx, |wb, _| wb.remote.docker.monitor.browser.field.clone()).unwrap();
        field.update(cx, |f, cx| f.set_text("shop", cx));
        cx.run_until_parked();
        wb.read_with(cx, |wb, _| {
            let rows = wb.container_rows();
            assert_eq!(rows.len(), 1);
            assert_eq!(rows[0].name, "web");
        });
        field.update(cx, |f, cx| f.set_text("missing", cx));
        cx.run_until_parked();
        assert!(wb.read_with(cx, |wb, _| wb.container_rows().is_empty()));
        field.update(cx, |f, cx| f.set_text("", cx));
        wb.update(cx, |wb, cx| {
            wb.select_container("web-id".into(), Tab::Resources, cx);
            wb.select_container("web-id".into(), Tab::Logs, cx);
        });
        crate::qa_integration_tests::wait_ticking(cx, "container logs", |cx| wb.read_with(cx, |wb, _| !wb.remote.docker.monitor.browser.loading));
        assert!(wb.read_with(cx, |wb, _| wb.remote.docker.monitor.browser.logs.contains("ready")));
        wb.update(cx, |wb, cx| wb.select_container("web-id".into(), Tab::Details, cx));
        crate::qa_integration_tests::wait_ticking(cx, "container inspect", |cx| wb.read_with(cx, |wb, _| !wb.remote.docker.monitor.browser.loading));
        assert_eq!(
            wb.read_with(cx, |wb, _| wb.remote.docker.monitor.browser.detail.as_ref().unwrap()["Mounts"][0]["Destination"].as_str().unwrap().to_string()),
            "/app/data"
        );
        cx.run_until_parked();
        let copy = cx.debug_bounds("container-copy-Container ID").unwrap().center();
        cx.simulate_click(copy, gpui::Modifiers::none());
        cx.run_until_parked();
        assert_eq!(cx.read_from_clipboard().and_then(|c| c.text()).as_deref(), Some("web-id"));
        wb.update(cx, |wb, _| wb.remote.docker.monitor.browser.detail = Some(serde_json::json!({"stale": true})));
        let refresh = cx.debug_bounds("container-button-Refresh details").unwrap().center();
        cx.simulate_click(refresh, gpui::Modifiers::none());
        crate::qa_integration_tests::wait_ticking(cx, "refreshed inspect", |cx| wb.read_with(cx, |wb, _| !wb.remote.docker.monitor.browser.loading));
        assert!(wb.read_with(cx, |wb, _| wb.remote.docker.monitor.browser.detail.as_ref().unwrap().get("Mounts").is_some()));
        wb.update(cx, |wb, cx| {
            let now = std::time::Instant::now();
            let usage = wb.remote.docker.monitor.usage[0].clone();
            let first = history::Sample::new(now - Duration::from_secs(4), &usage, None);
            let second = history::Sample::new(now, &usage, Some(&first));
            wb.remote.docker.monitor.history.insert("web-id".into(), [first, second].into());
            wb.select_container("web-id".into(), Tab::Resources, cx);
        });
        cx.run_until_parked();
        assert!(cx.debug_bounds("container-detail").is_some());
        assert!(cx.debug_bounds("selected-container-info-web-id").is_some());
        assert!(cx.debug_bounds("dashboard-row-db-id").is_none());
        wb.update(cx, |wb, cx| {
            wb.remote.docker.monitor.clear();
            cx.notify();
        });
    }
}
