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
