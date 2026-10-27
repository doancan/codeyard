//! Container browser: compact navigation, sortable inventory, and selected-container diagnostics.
use super::container_monitor_glue::button;
use super::*;
use crate::container_history::{self as history, Sort, StatusFilter, Tab};

impl Workbench {
    pub(super) fn ensure_container_search(&mut self, cx: &mut Context<Self>) {
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

    fn container_rows(&self) -> Vec<&crate::docker::Container> {
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

    fn container_status_view(&self, p: &Palette) -> AnyElement {
        let m = &self.remote.docker.monitor;
        let status = if m.host.is_none() {
            crate::i18n::t("No SSH host connected").to_string()
        } else if m.paused {
            crate::i18n::t("Updates paused").to_string()
        } else if m.updated.is_some() {
            crate::i18n::t("Live updates").into()
        } else {
            crate::i18n::t("Discovering containers…").into()
        };
        let mut view = div()
            .flex()
            .flex_col()
            .gap(px(theme::SPACE[2]))
            .text_size(px(text::XS.0))
            .text_color(c(p.text_muted))
            .child(div().h(px(18.)).truncate().child(status));
        if let Some(error) = m.error.as_ref().or(m.warning.as_ref()) {
            view = view.child(
                div()
                    .text_color(c(p.text))
                    .child(crate::i18n::tf("Could not refresh container data: {}. Check Docker access on the SSH host and retry.", &[&error])),
            );
            if m.error.is_some() && m.updated.is_some() {
                view = view.child(crate::i18n::t("Showing the last successful snapshot."));
            }
        }
        view.into_any_element()
    }

    pub(super) fn container_inventory_view(&self, p: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let m = &self.remote.docker.monitor;
        let Some(host) = &m.host else {
            return div()
                .p(px(theme::SPACE[3]))
                .text_size(px(text::SM.0))
                .text_color(c(p.text_muted))
                .child(crate::i18n::t("Connect to an SSH host to discover its containers automatically, or choose a local engine below."))
                .into_any_element();
        };
        let mut view = div()
            .flex()
            .flex_col()
            .gap(px(theme::SPACE[3]))
            .child(div().px(px(theme::SPACE[3])).text_size(px(text::SM.0)).text_color(c(p.text)).child(format!("SSH · {host}"))) // i18n-keep: SSH protocol name and the user-provided host identity.
            .child(div().px(px(theme::SPACE[3])).child(self.container_status_view(p)))
            .when_some(m.browser.field.clone(), |d, field| d.child(div().px(px(theme::SPACE[3])).child(field)))
            .child(
                div().flex().flex_wrap().gap(px(theme::SPACE[2])).children(
                    [
                        (crate::i18n::t("All"), StatusFilter::All),
                        (crate::i18n::t("Running"), StatusFilter::Running),
                        (crate::i18n::t("Stopped"), StatusFilter::Stopped),
                    ]
                    .into_iter()
                    .map(|(label, filter)| {
                        button(SharedString::from(format!("container-status-{label}")), label, p)
                            .when(m.browser.status_filter == filter, |s| s.bg(c(p.surface_active)).text_color(c(p.accent)))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.remote.docker.monitor.browser.status_filter = filter;
                                cx.notify();
                            }))
                    }),
                ),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(px(theme::SPACE[2]))
                    .child(div().text_size(px(text::XS.0)).text_color(c(p.text_muted)).child(crate::i18n::t("Sort by")))
                    .children(
                        [(crate::i18n::t("Name"), Sort::Name), (crate::i18n::t("CPU"), Sort::Cpu), (crate::i18n::t("Memory"), Sort::Memory)].into_iter().map(
                            |(label, sort)| {
                                button(SharedString::from(format!("sidebar-sort-{label}")), label, p)
                                    .when(m.browser.sort == sort, |s| s.bg(c(p.surface_active)).text_color(c(p.accent)))
                                    .tooltip(|_, cx| {
                                        cx.new(|_| Tooltip(crate::i18n::t("Click to sort using current values. Order stays fixed during live updates.").into()))
                                            .into()
                                    })
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        let m = &mut this.remote.docker.monitor;
                                        m.browser.sort = sort;
                                        m.browser.sort_usage = m.usage.clone();
                                        cx.notify();
                                    }))
                            },
                        ),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .justify_between()
                    .gap(px(theme::SPACE[2]))
                    .child(
                        div()
                            .text_size(px(text::XS.0))
                            .text_color(c(p.text_muted))
                            .child(crate::i18n::tf("{0} / {1} containers", &[&self.container_rows().len(), &m.containers.len()])),
                    )
                    .child(button("container-reset-filters", crate::i18n::t("Clear filters"), p).on_click(cx.listener(|this, _, _, cx| {
                        let b = &mut this.remote.docker.monitor.browser;
                        b.status_filter = StatusFilter::All;
                        b.query.clear();
                        let field = b.field.clone();
                        if let Some(field) = field {
                            field.update(cx, |field, cx| field.set_text("", cx));
                        }
                        cx.notify();
                    }))),
            );
        let mut group = None;
        for row in self.container_rows() {
            let project = history::project(row).unwrap_or(crate::i18n::t("Standalone containers"));
            if group != Some(project) {
                view = view.child(
                    div().px(px(theme::SPACE[3])).pt(px(theme::SPACE[3])).text_size(px(text::XS.0)).text_color(c(p.text_muted)).child(project.to_string()),
                );
                group = Some(project);
            }
            let id = row.id.clone();
            view = view.child(
                div()
                    .id(SharedString::from(format!("container-resource-{}", row.id)))
                    .debug_selector({
                        let id = id.clone();
                        move || format!("container-resource-{id}")
                    })
                    .px(px(theme::SPACE[3]))
                    .py(px(theme::SPACE[2]))
                    .rounded(px(theme::radius::SM))
                    .cursor_pointer()
                    .hover(|s| s.bg(c(p.surface_hover)))
                    .when(m.browser.selected.as_ref() == Some(&id), |s| s.bg(c(p.surface_active)))
                    .flex()
                    .justify_between()
                    .gap(px(theme::SPACE[2]))
                    .child(div().flex_1().min_w_0().truncate().text_size(px(text::SM.0)).text_color(c(p.text)).child(row.name.clone()))
                    .child(div().text_size(px(text::XS.0)).text_color(c(p.text_muted)).child(state_label(&row.state)))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.open_container_dashboard(cx);
                        this.select_container(id.clone(), Tab::Resources, cx);
                    })),
            )
        }
        if m.updated.is_some() && m.containers.is_empty() {
            view = view.child(div().text_size(px(text::SM.0)).text_color(c(p.text_muted)).child(crate::i18n::t("No containers found on this host.")));
        } else if m.updated.is_some() && self.container_rows().is_empty() {
            view = view.child(div().text_size(px(text::SM.0)).text_color(c(p.text_muted)).child(crate::i18n::t("No containers match the current filters.")));
        }
        view.into_any_element()
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
        let mut view = div()
            .id("container-dashboard")
            .debug_selector(|| "container-dashboard".into())
            .size_full()
            .flex()
            .flex_col()
            .bg(c(p.bg))
            .text_color(c(p.text))
            .child(
                div()
                    .p(px(theme::SPACE[5]))
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(px(theme::SPACE[3]))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap(px(theme::SPACE[2]))
                            .child(div().text_size(px(text::XL.0)).font_weight(gpui::FontWeight::SEMIBOLD).child(crate::i18n::t("Containers")))
                            .child(
                                div().truncate().text_size(px(text::SM.0)).text_color(c(p.text_muted)).child(
                                    m.host
                                        .as_ref()
                                        .map(|h| format!("SSH · {h} · {}", m.context.as_ref().map(|c| c.name.as_str()).unwrap_or("—")))
                                        .unwrap_or_else(|| crate::i18n::t("No SSH host connected").into()),
                                ),
                            ),
                    )
                    .child(button("dashboard-refresh", crate::i18n::t("Refresh"), p).on_click(cx.listener(|this, _, _, cx| {
                        this.remote.docker.monitor.paused = false;
                        this.refresh_container_monitor(cx);
                    })))
                    .child(button("dashboard-pause", if m.paused { crate::i18n::t("Resume updates") } else { crate::i18n::t("Pause updates") }, p).on_click(
                        cx.listener(|this, _, _, cx| {
                            this.remote.docker.monitor.paused = !this.remote.docker.monitor.paused;
                            if !this.remote.docker.monitor.paused {
                                this.refresh_container_monitor(cx);
                            }
                            cx.notify();
                        }),
                    )),
            )
            .child(div().px(px(theme::SPACE[5])).pb(px(theme::SPACE[3])).child(self.container_status_view(p)));
        if let Some(container) = m.browser.selected.as_ref().and_then(|id| m.containers.iter().find(|c| &c.id == id)) {
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
            wb.chrome.panel = Some(Panel::Containers);
            cx.notify();
        });
        cx.run_until_parked();
        let sort = cx.debug_bounds("container-button-CPU").unwrap().center();
        cx.simulate_click(sort, gpui::Modifiers::none());
        cx.run_until_parked();
        assert_eq!(wb.read_with(cx, |wb, _| wb.container_rows()[0].id.clone()), "web-id");
        let before = cx.debug_bounds("container-resource-web-id").unwrap();
        wb.update(cx, |wb, cx| {
            let m = &mut wb.remote.docker.monitor;
            m.paused = false;
            m.cancel = Some(Arc::new(std::sync::atomic::AtomicBool::new(false)));
            m.usage[0].cpu = "0%".into();
            m.usage[1].cpu = "99%".into();
            cx.notify();
        });
        cx.run_until_parked();
        assert_eq!(cx.debug_bounds("container-resource-web-id").unwrap(), before);
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
