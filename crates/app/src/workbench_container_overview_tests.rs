use super::*;

#[gpui::test]
fn container_overview_selects_details_and_preserves_live_scroll(cx: &mut gpui::TestAppContext) {
    let (wb, cx) = crate::qa_workbench_tests::setup(cx);
    let source: Arc<dyn host::Host> = crate::container_monitor::tests::FixtureHost::new();
    cx.update(|_, cx| cx.global_mut::<Settings>().docker_enabled = true);
    wb.update(cx, |wb, cx| {
        wb.remote.host = source.clone();
        wb.remote.conn = Conn::Connected { alias: "fixture".into() };
        let m = &mut wb.remote.docker.monitor;
        m.host = Some("fixture".into());
        m.source = Some(source.clone());
        m.apply(Ok(crate::container_monitor::read(source.as_ref(), "fixture", true, &std::sync::atomic::AtomicBool::new(false)).unwrap()));
        m.paused = true;
        wb.open_container_dashboard(cx);
    });
    cx.run_until_parked();
    assert!(!wb.read_with(cx, |wb, _| wb.remote.docker.monitor.browser.overview.details));
    assert_eq!(wb.read_with(cx, |wb, _| wb.panel_for_test()), None);
    let row = cx.debug_bounds("overview-row-web-id").unwrap();
    cx.simulate_click(row.origin + gpui::point(px(30.), px(20.)), gpui::Modifiers::none());
    crate::qa_integration_tests::wait_ticking(cx, "overview selection", |cx| wb.read_with(cx, |wb, _| !wb.remote.docker.monitor.browser.loading));
    wb.read_with(cx, |wb, _| {
        let b = &wb.remote.docker.monitor.browser;
        assert_eq!(b.selected.as_deref(), Some("web-id"));
        assert!(b.overview.details);
        assert!(b.tab == Tab::Resources);
    });
    // Reopening an already-selected resource must leave overview even on the fast path.
    wb.update(cx, |wb, cx| {
        wb.remote.docker.monitor.browser.overview.details = false;
        wb.select_container("web-id".into(), Tab::Resources, cx);
    });
    assert!(wb.read_with(cx, |wb, _| wb.remote.docker.monitor.browser.overview.details));
    wb.update(cx, |wb, cx| {
        let m = &mut wb.remote.docker.monitor;
        m.browser.overview.details = false;
        let template = m.containers[0].clone();
        for n in 0..60 {
            let mut row = template.clone();
            row.id = format!("extra-{n:02}");
            row.name = row.id.clone();
            m.containers.push(row);
        }
        cx.notify();
    });
    cx.run_until_parked();
    let search = wb.read_with(cx, |wb, _| wb.remote.docker.monitor.browser.field.clone()).unwrap();
    search.update(cx, |field, cx| field.set_text("nginx", cx));
    cx.run_until_parked();
    wb.update(cx, |wb, cx| {
        wb.remote.docker.monitor.browser.overview.scroll.set_offset(gpui::point(px(-40.), px(-100.)));
        cx.notify();
    });
    cx.run_until_parked();
    let offset = wb.read_with(cx, |wb, _| wb.remote.docker.monitor.browser.overview.scroll.offset());
    assert!(offset.y < px(0.));
    let order = wb.read_with(cx, |wb, _| wb.container_overview_rows().iter().map(|c| c.id.clone()).collect::<Vec<_>>());
    wb.update(cx, |wb, cx| {
        let m = &mut wb.remote.docker.monitor;
        let mut snapshot = crate::container_monitor::read(source.as_ref(), "fixture", true, &std::sync::atomic::AtomicBool::new(false)).unwrap();
        snapshot.containers = m.containers.iter().rev().cloned().collect();
        snapshot.usage[0].cpu = "99%".into();
        m.apply(Ok(snapshot));
        cx.notify();
    });
    cx.run_until_parked();
    wb.read_with(cx, |wb, _| {
        let b = &wb.remote.docker.monitor.browser;
        assert_eq!(b.overview.scroll.offset(), offset);
        assert_eq!(b.selected.as_deref(), Some("web-id"));
        assert_eq!(wb.container_overview_rows().iter().map(|c| c.id.clone()).collect::<Vec<_>>(), order);
    });
    wb.update(cx, |wb, cx| wb.select_container("web-id".into(), Tab::Resources, cx));
    cx.run_until_parked();
    let back = cx.debug_bounds("container-button-← Containers").unwrap().center();
    cx.simulate_click(back, gpui::Modifiers::none());
    cx.run_until_parked();
    wb.read_with(cx, |wb, _| {
        let b = &wb.remote.docker.monitor.browser;
        assert!(!b.overview.details);
        assert_eq!(b.query, "nginx");
        assert_eq!(b.overview.scroll.offset(), offset);
        assert_eq!(wb.panel_for_test(), None);
    });
    let columns = cx.debug_bounds("container-button-Columns").unwrap().center();
    cx.simulate_click(columns, gpui::Modifiers::none());
    cx.run_until_parked();
    assert!(wb.read_with(cx, |wb, _| wb.remote.docker.monitor.browser.overview.columns_open));
    let ports = cx.debug_bounds("container-button-Ports").unwrap().center();
    cx.simulate_click(ports, gpui::Modifiers::none());
    cx.run_until_parked();
    assert!(wb.read_with(cx, |wb, _| wb.remote.docker.monitor.browser.overview.columns[5]));
    cx.update(|window, cx| {
        window.focus(wb.read(cx).remote.docker.monitor.browser.overview.focus.as_ref().unwrap());
    });
    cx.simulate_keystrokes("up");
    cx.run_until_parked();
    let cursor = wb.read_with(cx, |wb, _| wb.remote.docker.monitor.browser.overview.cursor.clone()).unwrap();
    assert_ne!(cursor, "web-id");
    assert_eq!(wb.read_with(cx, |wb, _| wb.remote.docker.monitor.browser.selected.clone()).as_deref(), Some("web-id"));
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    wb.read_with(cx, |wb, _| {
        assert!(wb.remote.docker.monitor.browser.overview.details);
        assert_eq!(wb.remote.docker.monitor.browser.selected.as_deref(), Some(cursor.as_str()));
    });
    wb.update(cx, |wb, cx| {
        wb.remote.docker.monitor.clear();
        cx.notify();
    });
}

#[gpui::test]
fn container_overview_attention_combines_search_and_health(cx: &mut gpui::TestAppContext) {
    let (wb, cx) = crate::qa_workbench_tests::setup(cx);
    let source = crate::container_monitor::tests::FixtureHost::new();
    wb.update(cx, |wb, _| {
        let m = &mut wb.remote.docker.monitor;
        m.apply(Ok(crate::container_monitor::read(source.as_ref(), "fixture", true, &std::sync::atomic::AtomicBool::new(false)).unwrap()));
        m.containers[0].status = "Up 2 days (unhealthy)".into();
        m.browser.overview.attention = true;
        assert_eq!(wb.container_overview_rows().len(), 1);
        wb.remote.docker.monitor.browser.query = "no match".into();
        assert!(wb.container_overview_rows().is_empty());
        wb.remote.docker.monitor.browser.query.clear();
        wb.remote.docker.monitor.containers[0].status = "Up 2 days (healthy)".into();
        assert!(wb.container_overview_rows().is_empty());
        wb.remote.docker.monitor.containers[0].state = "restarting".into();
        assert_eq!(wb.container_overview_rows().len(), 1);
    });
}

fn layout_fixture(cx: &mut gpui::TestAppContext) -> (Entity<Workbench>, &mut gpui::VisualTestContext) {
    let (wb, cx) = crate::qa_workbench_tests::setup(cx);
    let source: Arc<dyn host::Host> = crate::container_monitor::tests::FixtureHost::new();
    cx.update(|_, cx| cx.global_mut::<Settings>().docker_enabled = true);
    wb.update(cx, |wb, cx| {
        wb.remote.host = source.clone();
        wb.remote.conn = Conn::Connected { alias: "fixture".into() };
        let m = &mut wb.remote.docker.monitor;
        m.host = Some("fixture".into());
        m.source = Some(source.clone());
        m.apply(Ok(crate::container_monitor::read(source.as_ref(), "fixture", true, &std::sync::atomic::AtomicBool::new(false)).unwrap()));
        m.paused = true;
        wb.open_container_dashboard(cx);
    });
    cx.run_until_parked();
    (wb, cx)
}

#[gpui::test]
fn container_overview_fills_available_width_with_default_and_hidden_columns(cx: &mut gpui::TestAppContext) {
    let (wb, cx) = layout_fixture(cx);
    for columns in [history::Overview::default().columns, [false; 7]] {
        wb.update(cx, |wb, cx| {
            wb.remote.docker.monitor.browser.overview.columns = columns;
            cx.notify();
        });
        for width in [1280., 1920., 2560.] {
            cx.simulate_resize(gpui::size(px(width), px(900.)));
            cx.run_until_parked();
            let viewport = cx.debug_bounds("container-overview-scroll").unwrap();
            let row = cx.debug_bounds("overview-row-web-id").unwrap();
            assert!((viewport.right() - row.right()).abs() <= px(2.), "unused space at width {width}: viewport {viewport:?}, row {row:?}");
        }
    }
}

#[gpui::test]
fn container_overview_controls_fit_narrow_and_translated_windows(cx: &mut gpui::TestAppContext) {
    let (wb, cx) = layout_fixture(cx);
    wb.update(cx, |wb, cx| {
        let row = &mut wb.remote.docker.monitor.containers[0];
        row.name = "checkout-api-production-eu-".repeat(12);
        row.image = "registry.example.com/platform/commerce/checkout:".repeat(8);
        cx.notify();
    });
    for language in ["en", "tr"] {
        let _language = crate::i18n::LangGuard::set(language);
        for width in [480., 800., 1280.] {
            cx.simulate_resize(gpui::size(px(width), px(800.)));
            cx.run_until_parked();
            let page = cx.debug_bounds("container-dashboard").unwrap();
            let scroll = cx.debug_bounds("container-overview-scroll").unwrap();
            assert!(page.right() <= px(width) && scroll.right() <= page.right(), "page overflow at {width} ({language}): {page:?}");
            for (english, turkish) in [
                ("container-button-Refresh", "container-button-Yenile"),
                ("container-button-Resume updates", "container-button-Güncellemeleri sürdür"),
                ("container-button-Container actions…", "container-button-Konteyner işlemleri…"),
                ("container-button-All", "container-button-Tümü"),
                ("container-button-Running", "container-button-Çalışıyor"),
                ("container-button-Stopped", "container-button-Durmuş"),
                ("container-button-Needs attention", "container-button-Sorunlu"),
                ("container-button-Columns", "container-button-Sütunlar"),
            ] {
                let id = if language == "en" { english } else { turkish };
                let control = cx.debug_bounds(id).unwrap_or_else(|| panic!("missing {id} at {width} ({language})"));
                assert!(control.left() >= page.left() && control.right() <= page.right(), "{id} overflow at {width} ({language}): {control:?} inside {page:?}");
            }
            assert!(scroll.size.height >= px(100.), "table is crowded out by controls at {width} ({language}): {scroll:?}");
        }
    }
}

#[gpui::test]
fn container_overview_header_sort_and_clear_filters_are_actionable(cx: &mut gpui::TestAppContext) {
    let (wb, cx) = layout_fixture(cx);
    cx.simulate_resize(gpui::size(px(1280.), px(900.)));
    cx.run_until_parked();
    assert!(cx.debug_bounds("container-button-Clear filters").is_none(), "no clear action for an unfiltered inventory");
    let cpu = cx.debug_bounds("overview-sort-CPU").unwrap();
    cx.simulate_click(cpu.center(), gpui::Modifiers::none());
    cx.run_until_parked();
    wb.read_with(cx, |wb, _| {
        assert!(wb.remote.docker.monitor.browser.sort == Sort::Cpu);
        assert_eq!(wb.remote.docker.monitor.browser.sort_usage[0].cpu, wb.remote.docker.monitor.usage[0].cpu);
        assert!(!wb.remote.docker.monitor.browser.overview.details, "sorting must keep the inventory open");
    });
    cx.simulate_keystrokes("m");
    cx.run_until_parked();
    assert!(wb.read_with(cx, |wb, _| wb.remote.docker.monitor.browser.sort == Sort::Memory));
    let search = wb.read_with(cx, |wb, _| wb.remote.docker.monitor.browser.field.clone()).unwrap();
    assert!(search.read_with(cx, |field, _| field.text().is_empty()), "table shortcuts must not type into search");
    search.update(cx, |field, cx| field.set_text("no matching image", cx));
    cx.run_until_parked();
    let clear = cx.debug_bounds("container-button-Clear filters").unwrap();
    cx.simulate_click(clear.center(), gpui::Modifiers::none());
    cx.run_until_parked();
    assert!(search.read_with(cx, |field, _| field.text().is_empty()));
    assert!(!wb.read_with(cx, |wb, _| wb.container_overview_rows().is_empty()));
}

#[gpui::test]
fn container_overview_extra_columns_scroll_inside_the_page(cx: &mut gpui::TestAppContext) {
    let (wb, cx) = layout_fixture(cx);
    wb.update(cx, |wb, cx| {
        wb.remote.docker.monitor.browser.overview.columns = [true; 7];
        cx.notify();
    });
    cx.simulate_resize(gpui::size(px(640.), px(800.)));
    cx.run_until_parked();
    let viewport = cx.debug_bounds("container-overview-scroll").unwrap();
    let table = cx.debug_bounds("overview-table").unwrap();
    let header = cx.debug_bounds("overview-header").unwrap();
    let row = cx.debug_bounds("overview-row-web-id").unwrap();
    assert!(table.size.width > viewport.size.width, "extra columns must remain readable");
    assert_eq!(header.size.width, table.size.width, "header and row columns must align");
    assert_eq!(row.size.width, table.size.width);
    wb.update(cx, |wb, cx| {
        wb.remote.docker.monitor.browser.overview.scroll.set_offset(gpui::point(px(-5000.), px(0.)));
        cx.notify();
    });
    cx.run_until_parked();
    let logs = cx.debug_bounds("container-button-Logs").unwrap();
    let menu = cx.debug_bounds("container-button-…").unwrap();
    assert!(logs.left() >= viewport.left() && menu.right() <= viewport.right(), "horizontal scrolling must reveal the row actions: {logs:?}, {menu:?}");
}

#[gpui::test]
fn container_overview_header_keeps_status_readable_and_table_close(cx: &mut gpui::TestAppContext) {
    let (wb, cx) = layout_fixture(cx);
    cx.simulate_resize(gpui::size(px(1280.), px(900.)));
    cx.run_until_parked();
    let page = cx.debug_bounds("container-dashboard").unwrap();
    let header = cx.debug_bounds("container-dashboard-header").unwrap();
    let status = cx.debug_bounds("container-update-status").unwrap();
    let hint = cx.debug_bounds("overview-hint").unwrap();
    let search = cx.debug_bounds("overview-search").unwrap();
    let table = cx.debug_bounds("overview-table").unwrap();
    assert!(header.size.height <= px(100.), "the page header is too tall: {header:?}");
    assert!(status.size.width >= px(70.), "the update label collapsed into an ellipsis: {status:?}");
    assert!(hint.size.height <= px(24.), "the inventory hint must be one line: {hint:?}");
    assert!(table.top() - page.top() <= px(210.), "the header is crowding out the data: {table:?}");
    assert!(cx.debug_bounds("container-button-Overview").is_none(), "no overview tab is needed before selecting a container");
    cx.simulate_click(search.origin + gpui::point(px(10.), px(10.)), gpui::Modifiers::none());
    cx.run_until_parked();
    let field = wb.read_with(cx, |wb, _| wb.remote.docker.monitor.browser.field.clone()).unwrap();
    assert!(cx.update(|window, cx| field.focus_handle(cx).is_focused(window)), "the search icon must focus its input");
    cx.simulate_keystrokes("w e b");
    cx.run_until_parked();
    assert_eq!(wb.read_with(cx, |wb, _| wb.remote.docker.monitor.browser.query.clone()), "web", "the search icon must focus its input");
}

#[gpui::test]
fn container_overview_header_distinguishes_discovery_from_stale_data(cx: &mut gpui::TestAppContext) {
    let (wb, cx) = layout_fixture(cx);
    wb.update(cx, |wb, cx| {
        let m = &mut wb.remote.docker.monitor;
        m.updated = None;
        m.containers.clear();
        m.paused = false;
        cx.notify();
    });
    cx.run_until_parked();
    assert!(cx.debug_bounds("container-fleet-summary").is_none(), "discovery must not claim zero containers before its first snapshot");
    assert!(cx.debug_bounds("container-dashboard-error").is_none());
    wb.update(cx, |wb, cx| {
        let m = &mut wb.remote.docker.monitor;
        m.updated = Some(std::time::Instant::now());
        m.error = Some("permission denied for Docker socket".into());
        cx.notify();
    });
    cx.simulate_resize(gpui::size(px(640.), px(800.)));
    cx.run_until_parked();
    assert!(cx.debug_bounds("container-fleet-summary").is_some());
    let page = cx.debug_bounds("container-dashboard").unwrap();
    let error = cx.debug_bounds("container-dashboard-error").expect("stale data needs a visible explanation");
    assert!(error.size.height > px(20.) && error.right() <= page.right(), "error explanation must wrap within the page: {error:?}");
}

#[gpui::test]
fn container_detail_layout_adapts_charts_and_keeps_actions_visible(cx: &mut gpui::TestAppContext) {
    let (wb, cx) = layout_fixture(cx);
    wb.update(cx, |wb, cx| wb.select_container("web-id".into(), Tab::Resources, cx));
    crate::qa_integration_tests::wait_ticking(cx, "container details", |cx| wb.read_with(cx, |wb, _| !wb.remote.docker.monitor.browser.loading));
    for width in [1280., 480.] {
        cx.simulate_resize(gpui::size(px(width), px(900.)));
        cx.run_until_parked();
        let page = cx.debug_bounds("container-dashboard").unwrap();
        let cpu = cx.debug_bounds("container-chart-CPU usage").unwrap();
        let memory = cx.debug_bounds("container-chart-Memory usage (%)").unwrap();
        assert!(cpu.left() >= page.left() && memory.right() <= page.right(), "chart overflow at {width}");
        if width > 1000. {
            assert!((cpu.top() - memory.top()).abs() < px(2.));
            assert!(cpu.right() <= memory.left());
        } else {
            assert!(memory.top() >= cpu.bottom());
        }
        let actions = cx.debug_bounds("container-detail-actions").unwrap();
        assert!(actions.left() >= page.left() && actions.right() <= page.right());
    }
    wb.update(cx, |wb, cx| {
        wb.remote.docker.monitor.containers.iter_mut().find(|c| c.id == "web-id").unwrap().name = "storefront-production-".repeat(20);
        cx.notify();
    });
    cx.run_until_parked();
    let title = cx.debug_bounds("container-dashboard-title").unwrap();
    let page = cx.debug_bounds("container-dashboard").unwrap();
    assert!(title.right() <= page.right());
}

#[gpui::test]
fn container_ports_and_logs_fit_narrow_windows_with_long_content(cx: &mut gpui::TestAppContext) {
    let (wb, cx) = layout_fixture(cx);
    wb.update(cx, |wb, cx| wb.select_container("web-id".into(), Tab::Details, cx));
    crate::qa_integration_tests::wait_ticking(cx, "container inspection", |cx| wb.read_with(cx, |wb, _| !wb.remote.docker.monitor.browser.loading));
    wb.update(cx, |wb, cx| {
        wb.remote.docker.monitor.browser.detail = Some(serde_json::json!({
            "NetworkSettings":{"Ports":{"443/tcp":[{"HostIp":"::","HostPort":"8443"}],"53/udp":null}},
            "Mounts":[{"Type":"bind","Source":"/srv/very-long-path/".repeat(20),"Destination":"/app/config","RW":false}]
        }));
        cx.notify();
    });
    for width in [480., 1280.] {
        cx.simulate_resize(gpui::size(px(width), px(900.)));
        cx.run_until_parked();
        let page = cx.debug_bounds("container-dashboard").unwrap();
        let columns = cx.debug_bounds("container-inspection-columns").unwrap();
        assert!(columns.right() <= page.right());
        let copy = cx.debug_bounds("container-copy-Server binding").unwrap();
        assert!(copy.right() <= page.right());
        cx.simulate_click(copy.center(), gpui::Modifiers::none());
        cx.run_until_parked();
        assert_eq!(cx.read_from_clipboard().and_then(|c| c.text()).as_deref(), Some("[::]:8443"));
    }
    wb.update(cx, |wb, cx| {
        wb.select_container("web-id".into(), Tab::Logs, cx);
        wb.remote.docker.monitor.browser.logs = format!("2026-10-28T10:15:00Z ERROR {}", "long message ".repeat(40));
        cx.notify();
    });
    for width in [480., 1280.] {
        cx.simulate_resize(gpui::size(px(width), px(900.)));
        cx.run_until_parked();
        let page = cx.debug_bounds("container-dashboard").unwrap();
        let row = cx.debug_bounds("container-log-line-1").unwrap();
        assert!(row.right() <= page.right());
        let search = cx.debug_bounds("container-log-search").unwrap();
        assert!(search.right() <= page.right());
    }
}
