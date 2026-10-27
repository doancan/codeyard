//! Selected-container diagnostics and time-based CPU/memory charts.
use super::container_monitor_glue::button;
use super::*;
use crate::container_history::{self as history, Tab};

impl Workbench {
    pub(super) fn select_container(&mut self, id: String, tab: Tab, cx: &mut Context<Self>) {
        if tab == Tab::Logs {
            self.ensure_container_log_search(cx);
        }
        let m = &mut self.remote.docker.monitor;
        if m.browser.selected.as_ref() == Some(&id) && m.browser.tab == tab && tab != Tab::Logs && (tab == Tab::Resources || m.browser.detail.is_some()) {
            return;
        }
        let changed = m.browser.selected.as_ref() != Some(&id);
        m.browser.selected = Some(id.clone());
        m.browser.tab = tab;
        if changed {
            m.browser.log_view.follow = true;
            m.browser.detail = None;
            m.browser.logs.clear();
            m.browser.error = None;
        }
        if m.browser.minutes == 0 {
            m.browser.minutes = 5;
        }
        let (Some(source), Some(context)) = (m.source.clone(), m.context.clone()) else {
            cx.notify();
            return;
        };
        if !changed && tab != Tab::Logs && m.browser.detail.is_some() {
            cx.notify();
            return;
        }
        m.browser.request += 1;
        if context.container_alias(&id).is_err() {
            return;
        }
        if let Some(cancel) = m.browser.cancel.take() {
            cancel.store(true, Ordering::Relaxed);
        }
        let cancel = Arc::new(std::sync::atomic::AtomicBool::new(false));
        m.browser.cancel = Some(cancel.clone());
        let request = m.browser.request;
        m.browser.loading = true;
        let args = if tab == Tab::Logs { vec!["logs", "--tail", "300", "--timestamps", &id] } else { vec!["inspect", &id] };
        let command = crate::docker::Command::docker(Some(&context.name), &args);
        let task = cx.background_executor().spawn(async move { crate::container_monitor::execute(source.as_ref(), &command, &cancel) });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| {
                let b = &mut this.remote.docker.monitor.browser;
                if b.request != request || b.selected.as_ref() != Some(&id) {
                    return;
                }
                b.loading = false;
                b.cancel = None;
                match result {
                    Ok(text) => {
                        b.error = None;
                        if tab == Tab::Logs {
                            b.logs = text;
                            if b.log_view.follow {
                                b.log_view.scroll.scroll_to_bottom();
                            }
                        } else {
                            match serde_json::from_str::<serde_json::Value>(&text) {
                                Ok(value) => b.detail = value.as_array().and_then(|a| a.first()).cloned(),
                                Err(e) => b.error = Some(e.to_string()),
                            }
                        }
                    }
                    Err(e) => b.error = Some(e),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    pub(super) fn container_detail_view(&self, container: &crate::docker::Container, p: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let m = &self.remote.docker.monitor;
        let b = &m.browser;
        let context = m.context.clone();
        let target = container.clone();
        let terminal_context = context.clone();
        let terminal_target = target.clone();
        let mut view = div()
            .id("container-detail")
            .debug_selector(|| "container-detail".into())
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .border_t_1()
            .border_color(c(p.border))
            .child(
                div()
                    .px(px(theme::SPACE[5]))
                    .py(px(theme::SPACE[3]))
                    .flex()
                    .items_center()
                    .gap(px(theme::SPACE[3]))
                    .child(div().flex_1().min_w_0().truncate().text_size(px(text::SM.0)).font_weight(gpui::FontWeight::SEMIBOLD).child(container.name.clone()))
                    .child(button("detail-files", crate::i18n::t("Open files"), p).on_click(cx.listener(move |this, _, window, cx| {
                        if let Some(context) = context.clone() {
                            this.docker_container_picked(context, target.clone(), false, window, cx);
                        }
                    })))
                    .child(button("detail-terminal", crate::i18n::t("Terminal"), p).on_click(cx.listener(move |this, _, window, cx| {
                        if this.refuse_if_dirty(cx) {
                            return;
                        }
                        if let Some(context) = terminal_context.clone() {
                            if let Ok(alias) = context.container_alias(&terminal_target.id) {
                                if matches!(&this.remote.conn,Conn::Connected{alias:current} if current==&alias) {
                                    this.open_terminal(window, cx);
                                    return;
                                }
                                this.remote.docker.monitor.browser.terminal_target = Some(alias);
                            }
                            this.docker_container_picked(context, terminal_target.clone(), false, window, cx);
                        }
                    }))),
            );
        let mut tabs = div().px(px(theme::SPACE[5])).pb(px(theme::SPACE[3])).flex().gap(px(theme::SPACE[3]));
        for (label, tab) in
            [(crate::i18n::t("Resources"), Tab::Resources), (crate::i18n::t("Ports and storage"), Tab::Details), (crate::i18n::t("Logs"), Tab::Logs)]
        {
            let id = container.id.clone();
            tabs = tabs.child(
                button(SharedString::from(format!("container-detail-{label}")), label, p)
                    .when(b.tab == tab, |s| s.bg(c(p.surface_active)))
                    .on_click(cx.listener(move |this, _, _, cx| this.select_container(id.clone(), tab, cx))),
            );
        }
        view = view.child(tabs);
        let mut body = div()
            .id("container-detail-scroll")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .px(px(theme::SPACE[5]))
            .pb(px(theme::SPACE[5]))
            .flex()
            .flex_col()
            .gap(px(theme::SPACE[4]));
        if b.loading && b.detail.is_none() {
            body = body.child(div().text_size(px(text::XS.0)).text_color(c(p.text_muted)).child(crate::i18n::t("Loading container details…")));
        }
        if let Some(error) = &b.error {
            body = body.child(div().text_size(px(text::SM.0)).child(error.clone()));
        }
        match b.tab {
            Tab::Resources => {
                let usage = m.usage.iter().find(|u| u.id == container.id);
                body = body.child(
                    div()
                        .id(SharedString::from(format!("selected-container-info-{}", container.id)))
                        .debug_selector({
                            let id = container.id.clone();
                            move || format!("selected-container-info-{id}")
                        })
                        .flex()
                        .flex_col()
                        .gap(px(theme::SPACE[2]))
                        .text_size(px(text::SM.0))
                        .child(div().text_color(c(p.text_muted)).child(format!("{} · {}", container.image, container.status)))
                        .child(value_row(crate::i18n::t("Processes"), usage.map(|u| u.pids.as_str()).unwrap_or("—"), p)),
                );
                let mut toolbar = div().flex().flex_wrap().items_center().gap(px(theme::SPACE[3]));
                for (label, minutes) in [(crate::i18n::t("1 minute"), 1), (crate::i18n::t("5 minutes"), 5), (crate::i18n::t("15 minutes"), 15)] {
                    toolbar = toolbar.child(
                        button(SharedString::from(format!("container-period-{minutes}")), label, p)
                            .when(b.minutes == minutes, |s| s.bg(c(p.surface_active)))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.remote.docker.monitor.browser.minutes = minutes;
                                cx.notify();
                            })),
                    );
                }
                body = body.child(toolbar);
                let samples = m.history.get(&container.id);
                let now = std::time::Instant::now();
                let seconds = (b.minutes.max(1) * 60) as f32;
                let points = |memory: bool| {
                    samples
                        .into_iter()
                        .flatten()
                        .filter_map(|s| {
                            let age = now.duration_since(s.at).as_secs_f32();
                            let value = if memory { s.memory } else { s.cpu }?;
                            (age <= seconds).then_some((seconds - age, value))
                        })
                        .collect::<Vec<_>>()
                };
                body = body.child(chart(crate::i18n::t("CPU usage"), usage.map(|u| u.cpu.as_str()).unwrap_or("—"), points(false), seconds, p.accent, p)).child(
                    chart(
                        crate::i18n::t("Memory usage (%)"),
                        &usage.map(|u| format!("{} · {}", u.memory_percent, u.memory)).unwrap_or_else(|| "—".into()),
                        points(true),
                        seconds,
                        p.info,
                        p,
                    ),
                );
                let last = samples.and_then(|s| s.back()).filter(|s| container.state == "running" && now.duration_since(s.at).as_secs() < 15);
                let disk = m.disks.iter().find(|d| d.id == container.id);
                body = body.child(section(crate::i18n::t("Disk and network activity"), p));
                for (label, value) in [
                    (crate::i18n::t("Disk read / write rate"), history::speed(last.and_then(|s| s.disk))),
                    (crate::i18n::t("Network RX / TX rate"), history::speed(last.and_then(|s| s.network))),
                    (crate::i18n::t("Disk I/O total"), usage.map(|u| u.disk_io.clone()).unwrap_or_else(|| "—".into())),
                    (crate::i18n::t("Network I/O total"), usage.map(|u| u.network.clone()).unwrap_or_else(|| "—".into())),
                    (crate::i18n::t("Disk size"), disk.map(|d| d.size.clone()).unwrap_or_else(|| "—".into())),
                ] {
                    body = body.child(value_row(label, &value, p));
                }
                body=body.child(note(crate::i18n::t("History is collected while monitoring is active. CPU can exceed 100% across cores. Rates need two samples; counter resets show no rate."), p))
                    .child(note(crate::i18n::t("Disk size includes the writable layer and virtual image size; volumes are excluded. Disk and network I/O are cumulative totals."), p));
            }
            Tab::Details => {
                let id = container.id.clone();
                body = body.child(
                    div()
                        .flex()
                        .flex_wrap()
                        .items_center()
                        .justify_between()
                        .gap(px(theme::SPACE[3]))
                        .child(section(crate::i18n::t("Container identity"), p))
                        .child(button("inspect-refresh", crate::i18n::t("Refresh details"), p).on_click(cx.listener(move |this, _, _, cx| {
                            this.remote.docker.monitor.browser.detail = None;
                            this.select_container(id.clone(), Tab::Details, cx);
                        }))),
                );
                body = body
                    .child(copy_row("container-id", crate::i18n::t("Container ID"), &container.id, p, cx))
                    .child(copy_row("container-image", crate::i18n::t("Image"), &container.image, p, cx))
                    .child(value_row(crate::i18n::t("Status"), &container.status, p));
                if let Some(detail) = &b.detail {
                    if let Some(health) = detail.pointer("/State/Health/Status").and_then(|v| v.as_str()) {
                        body = body.child(value_row(crate::i18n::t("Health"), health, p));
                    }
                    body = body
                        .child(section(crate::i18n::t("Port mappings"), p))
                        .child(note(crate::i18n::t("Published addresses belong to the SSH server. Use port forwarding to access remote services locally."), p));
                    let ports = port_mappings(detail);
                    if ports.is_empty() {
                        body = body.child(note(crate::i18n::t("No published ports"), p));
                    }
                    for (index, (port, bindings)) in ports.iter().enumerate() {
                        let mut row = div()
                            .id(("container-port", index))
                            .py(px(theme::SPACE[3]))
                            .border_b_1()
                            .border_color(c(p.border))
                            .flex()
                            .flex_col()
                            .gap(px(theme::SPACE[2]))
                            .child(value_row(crate::i18n::t("Container port / protocol"), port, p));
                        if bindings.is_empty() {
                            row = row.child(note(crate::i18n::t("Exposed only; not published on the server."), p));
                        }
                        for (binding_index, binding) in bindings.iter().enumerate() {
                            row = row.child(copy_row(
                                SharedString::from(format!("port-{index}-{binding_index}")),
                                crate::i18n::t("Server binding"),
                                binding,
                                p,
                                cx,
                            ));
                        }
                        body = body.child(row);
                    }
                    body = body.child(section(crate::i18n::t("Mounted storage"), p));
                    if let Some(mounts) = detail["Mounts"].as_array() {
                        if mounts.is_empty() {
                            body = body.child(note(crate::i18n::t("No mounted storage"), p));
                        }
                        for (index, mount) in mounts.iter().enumerate() {
                            let s = |key: &str| mount[key].as_str().unwrap_or("—");
                            let access = match mount["RW"].as_bool() {
                                Some(true) => crate::i18n::t("Read and write"),
                                Some(false) => crate::i18n::t("Read only"),
                                None => "—",
                            };
                            let mut row = div()
                                .id(("container-mount", index))
                                .py(px(theme::SPACE[3]))
                                .border_b_1()
                                .border_color(c(p.border))
                                .flex()
                                .flex_col()
                                .gap(px(theme::SPACE[2]))
                                .child(value_row(crate::i18n::t("Storage type"), s("Type"), p))
                                .child(copy_row(SharedString::from(format!("mount-source-{index}")), crate::i18n::t("Source on server"), s("Source"), p, cx))
                                .child(copy_row(
                                    SharedString::from(format!("mount-target-{index}")),
                                    crate::i18n::t("Path in container"),
                                    s("Destination"),
                                    p,
                                    cx,
                                ))
                                .child(value_row(crate::i18n::t("Access"), access, p));
                            if let Some(name) = mount["Name"].as_str().filter(|s| !s.is_empty()) {
                                row = row.child(copy_row(SharedString::from(format!("mount-name-{index}")), crate::i18n::t("Volume name"), name, p, cx));
                            }
                            body = body.child(row);
                        }
                    } else {
                        body = body.child(note(crate::i18n::t("Storage information is unavailable."), p));
                    }
                    body = body.child(note(
                        crate::i18n::t(
                            "Mount paths are shown as reported by the engine. Volume and bind-mount sizes are not included in container disk usage.",
                        ),
                        p,
                    ));
                } else if !b.loading {
                    body = body.child(note(crate::i18n::t("Container details are unavailable. Refresh details to try again."), p));
                }
            }
            Tab::Logs => {
                return view.child(self.container_logs_view(p, cx)).into_any_element();
            }
        }
        view.child(body).into_any_element()
    }
}

fn chart(label: &'static str, current: &str, points: Vec<(f32, f32)>, seconds: f32, color: u32, p: &Palette) -> AnyElement {
    let peak = points.iter().map(|(_, v)| *v).fold(100., f32::max).ceil();
    let grid = p.border;
    let mut view = div()
        .flex()
        .flex_col()
        .gap(px(theme::SPACE[3]))
        .py(px(theme::SPACE[3]))
        .child(
            div()
                .flex()
                .flex_wrap()
                .justify_between()
                .gap(px(theme::SPACE[3]))
                .text_size(px(text::SM.0))
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .child(label)
                .child(div().font_family(ui::MONO_FONT).text_color(c(color)).child(current.to_string())),
        )
        .child(div().text_size(px(text::XS.0)).text_color(c(p.text_muted)).child(format!("0–{peak:.0}%")));
    if points.len() < 2 {
        return view
            .child(
                div()
                    .h(px(60.))
                    .flex()
                    .items_center()
                    .text_size(px(text::XS.0))
                    .text_color(c(p.text_muted))
                    .child(crate::i18n::t("Waiting for enough samples to draw history…")),
            )
            .into_any_element();
    }
    view = view
        .child(
            gpui::canvas(
                |_, _, _| {},
                move |bounds, _, window, _| {
                    let mut guides = gpui::PathBuilder::stroke(px(1.));
                    for fraction in [0., 0.5, 1.] {
                        let y = bounds.top() + bounds.size.height * fraction;
                        guides.move_to(gpui::point(bounds.left(), y));
                        guides.line_to(gpui::point(bounds.right(), y));
                    }
                    if let Ok(path) = guides.build() {
                        window.paint_path(path, c(grid));
                    }
                    let mut path = gpui::PathBuilder::stroke(px(2.));
                    for (i, (time, value)) in points.iter().enumerate() {
                        let point = gpui::point(bounds.left() + bounds.size.width * (*time / seconds), bounds.bottom() - bounds.size.height * (*value / peak));
                        if i == 0 || time - points[i - 1].0 > 15. {
                            path.move_to(point);
                        } else {
                            path.line_to(point);
                        }
                    }
                    if let Ok(path) = path.build() {
                        window.paint_path(path, c(color));
                    }
                },
            )
            .w_full()
            .h(px(112.)),
        )
        .child(
            div()
                .flex()
                .justify_between()
                .text_size(px(text::XS.0))
                .text_color(c(p.text_muted))
                .child(crate::i18n::tf("{} minutes ago", &[&(seconds as u64 / 60)]))
                .child(crate::i18n::t("Now")),
        );
    view.into_any_element()
}

fn section(label: &'static str, p: &Palette) -> gpui::Div {
    div().pt(px(theme::SPACE[4])).text_size(px(text::SM.0)).font_weight(gpui::FontWeight::SEMIBOLD).text_color(c(p.text)).child(label)
}

fn note(label: &'static str, p: &Palette) -> gpui::Div {
    div().text_size(px(text::XS.0)).text_color(c(p.text_muted)).child(label)
}

fn value_row(label: &'static str, value: &str, p: &Palette) -> gpui::Div {
    div()
        .flex()
        .flex_wrap()
        .items_start()
        .gap(px(theme::SPACE[3]))
        .text_size(px(text::SM.0))
        .child(div().w(px(170.)).flex_shrink_0().text_color(c(p.text_muted)).child(label))
        .child(
            div()
                .id(SharedString::from(format!("value-{label}-{value}")))
                .flex_1()
                .min_w(px(100.))
                .font_family(ui::MONO_FONT)
                .whitespace_nowrap()
                .overflow_x_scroll()
                .child(value.to_string()),
        )
}

fn copy_row(id: impl Into<gpui::ElementId>, label: &'static str, value: &str, p: &Palette, cx: &mut Context<Workbench>) -> gpui::Div {
    let copy = value.to_string();
    div().flex().items_start().gap(px(theme::SPACE[3])).child(value_row(label, value, p).flex_1().min_w_0()).child(
        button(id, crate::i18n::t("Copy"), p).debug_selector(move || format!("container-copy-{label}")).on_click(cx.listener(move |_, _, _, cx| {
            cx.write_to_clipboard(gpui::ClipboardItem::new_string(copy.clone()));
        })),
    )
}

/// Keep exposed-only ports and every IPv4/IPv6 binding; never invent a local URL.
fn port_mappings(detail: &serde_json::Value) -> std::collections::BTreeMap<String, Vec<String>> {
    let mut ports = std::collections::BTreeMap::new();
    if let Some(exposed) = detail.pointer("/Config/ExposedPorts").and_then(|v| v.as_object()) {
        ports.extend(exposed.keys().map(|key| (key.clone(), Vec::new())));
    }
    if let Some(bindings) = detail.pointer("/NetworkSettings/Ports").and_then(|v| v.as_object()) {
        for (port, values) in bindings {
            let entries = ports.entry(port.clone()).or_default();
            for binding in values.as_array().into_iter().flatten() {
                let Some(number) = binding["HostPort"].as_str().filter(|s| !s.is_empty()) else { continue };
                let address = binding["HostIp"].as_str().filter(|s| !s.is_empty()).unwrap_or("*");
                entries.push(if address.contains(':') { format!("[{address}]:{number}") } else { format!("{address}:{number}") });
            }
        }
    }
    ports
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn container_ports_preserve_protocol_exposed_only_and_ipv6_bindings() {
        let detail = serde_json::json!({"Config":{"ExposedPorts":{"53/udp":{},"80/tcp":{}}},
            "NetworkSettings":{"Ports":{"80/tcp":[{"HostIp":"127.0.0.1","HostPort":"8080"},{"HostIp":"::","HostPort":"8080"}],"443/tcp":null}}});
        let ports = port_mappings(&detail);
        assert_eq!(ports["80/tcp"], ["127.0.0.1:8080", "[::]:8080"]);
        assert!(ports["53/udp"].is_empty());
        assert!(ports["443/tcp"].is_empty());
        assert!(port_mappings(&serde_json::Value::Null).is_empty());
    }
}
