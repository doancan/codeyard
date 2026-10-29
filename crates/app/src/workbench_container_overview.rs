//! Fleet inventory over the existing SSH snapshots; refreshing never changes ranking.
use super::container_monitor_glue::button;
use super::*;
use crate::container_history::{self as history, Sort, StatusFilter, Tab};

#[derive(Clone, Copy)]
pub(super) enum ContainerAction {
    Files,
    Terminal,
    CopyId,
}

pub(super) fn unhealthy(container: &crate::docker::Container) -> bool {
    container.status.contains("(unhealthy)") || matches!(container.state.as_str(), "dead" | "restarting")
}

// Minimum cell widths keep the default inventory readable in a 1280-point window.
const NAME_WIDTH: f32 = 200.;
const STATUS_WIDTH: f32 = 124.;
const ACTIONS_WIDTH: f32 = 128.;

fn columns() -> [(&'static str, f32); 7] {
    [
        (crate::i18n::t("Image"), 180.),
        (crate::i18n::t("CPU"), 84.),
        (crate::i18n::t("Memory"), 176.),
        (crate::i18n::t("Network RX / TX rate"), 176.),
        (crate::i18n::t("Disk read / write rate"), 176.),
        (crate::i18n::t("Ports"), 200.),
        (crate::i18n::t("Compose project"), 128.),
    ]
}

fn text_cell(value: impl Into<String>, width: f32, p: &Palette) -> gpui::Stateful<gpui::Div> {
    div()
        .id(SharedString::from(format!("cell-{width}")))
        .w(px(width))
        .flex_shrink_0()
        .px(px(theme::SPACE[3]))
        .truncate()
        .text_color(c(p.text_muted))
        .child(value.into())
}

fn cell(value: impl Into<String>, width: f32, p: &Palette) -> gpui::Stateful<gpui::Div> {
    let value = value.into();
    text_cell(value.clone(), width, p).tooltip(move |_, cx| cx.new(|_| Tooltip(value.clone().into())).into())
}

fn rate_cell(index: usize, rates: Option<(f64, f64)>, p: &Palette) -> gpui::Stateful<gpui::Div> {
    let labels = if index == 3 {
        [("↓ RX", crate::i18n::t("Network received"), p.success), ("↑ TX", crate::i18n::t("Network sent"), p.info)]
    } else {
        [("R", crate::i18n::t("Disk read"), p.success), ("W", crate::i18n::t("Disk write"), p.info)]
    };
    div().id(("column", index)).w(px(columns()[index].1)).flex_shrink_0().px(px(theme::SPACE[3])).flex().flex_col().children(
        labels.into_iter().enumerate().map(|(direction, (symbol, label, color))| {
            let rate = rates.map(|(a, b)| if direction == 0 { a } else { b });
            let value = rate.map(history::speed_value).unwrap_or_else(|| "—".into());
            let description = format!("{label}: {value}");
            div()
                .id(("direction", direction))
                .h(px(18.))
                .line_height(px(18.))
                .flex()
                .items_center()
                .gap(px(theme::SPACE[3]))
                .font_family(ui::MONO_FONT)
                .tooltip(move |_, cx| cx.new(|_| Tooltip(description.clone().into())).into())
                .child(div().w(px(38.)).flex_shrink_0().text_color(c(if rate.is_some() { color } else { p.text_muted })).child(symbol))
                .child(div().flex_1().min_w_0().text_right().text_color(c(p.text)).child(value))
        }),
    )
}

fn usage_cell(index: usize, value: String, usage: Option<&crate::container_monitor::Usage>, p: &Palette) -> gpui::Stateful<gpui::Div> {
    let percent = usage.and_then(|u| crate::container_monitor::Usage::percent(if index == 1 { &u.cpu } else { &u.memory_percent }));
    let description = if index == 1 {
        crate::i18n::tf("CPU: {}. 100% equals one CPU core.", &[&value])
    } else {
        crate::i18n::tf("Memory: {0} ({1})", &[&value, &percent.map(|n| format!("{n:.1}%")).unwrap_or_else(|| "—".into())])
    };
    div()
        .id(("column", index))
        .w(px(columns()[index].1))
        .flex_shrink_0()
        .px(px(theme::SPACE[3]))
        .flex()
        .flex_col()
        .gap(px(4.))
        .tooltip(move |_, cx| cx.new(|_| Tooltip(description.clone().into())).into())
        .child(div().truncate().text_right().font_family(ui::MONO_FONT).text_color(c(p.text)).child(value))
        .child(div().h(px(3.)).w_full().rounded_full().bg(c(p.surface_active)).when_some(percent, |d, percent| {
            d.child(div().h_full().w(gpui::relative((percent / 100.).clamp(0., 1.))).rounded_full().bg(c(if index == 2 && percent >= 90. {
                p.warning
            } else {
                p.accent
            })))
        }))
}

fn status_cell(row: &crate::docker::Container, p: &Palette) -> gpui::Stateful<gpui::Div> {
    let (label, color) = if unhealthy(row) {
        (
            if row.status.contains("(unhealthy)") { crate::i18n::t("Unhealthy").to_string() } else { super::container_browser_glue::state_label(&row.state) },
            p.danger,
        )
    } else if row.state == "running" {
        (crate::i18n::t("Running").to_string(), p.success)
    } else {
        (super::container_browser_glue::state_label(&row.state), p.text_muted)
    };
    let description = row.status.clone();
    div()
        .id("status")
        .w(px(STATUS_WIDTH))
        .flex_shrink_0()
        .px(px(theme::SPACE[3]))
        .flex()
        .items_center()
        .gap(px(theme::SPACE[3]))
        .tooltip(move |_, cx| cx.new(|_| Tooltip(description.clone().into())).into())
        .child(div().size(px(6.)).flex_shrink_0().rounded_full().bg(c(color)))
        .child(div().min_w_0().truncate().text_color(c(color)).child(label))
}

impl Workbench {
    fn sort_container_overview(&mut self, sort: Sort, cx: &mut Context<Self>) {
        let m = &mut self.remote.docker.monitor;
        m.browser.sort = sort;
        m.browser.sort_usage = m.usage.clone();
        cx.notify();
    }

    fn container_sort_header(&self, label: &'static str, width: f32, sort: Sort, p: &Palette, cx: &mut Context<Self>) -> gpui::Stateful<gpui::Div> {
        let selected = self.remote.docker.monitor.browser.sort == sort;
        let key = match sort {
            Sort::Name => "N",
            Sort::Cpu => "C",
            Sort::Memory => "M",
        };
        text_cell(label, width, p)
            .debug_selector(move || format!("overview-sort-{label}"))
            .flex()
            .items_center()
            .gap(px(theme::SPACE[2]))
            .when(sort != Sort::Name, |d| d.justify_end())
            .cursor_pointer()
            .hover(|s| s.bg(c(p.surface_active)))
            .when(selected, |d| d.text_color(c(p.accent)).child(icon(if sort == Sort::Name { "arrow-up" } else { "arrow-down" }, 12., p.accent)))
            .tooltip(move |_, cx| {
                cx.new(|_| Tooltip(format!("{} ({key})", crate::i18n::t("Click to sort using current values. Order stays fixed during live updates.")).into()))
                    .into()
            })
            .on_click(cx.listener(move |this, _, window, cx| {
                if let Some(focus) = &this.remote.docker.monitor.browser.overview.focus {
                    window.focus(focus);
                }
                this.sort_container_overview(sort, cx);
            }))
    }

    fn container_overview_rows(&self) -> Vec<&crate::docker::Container> {
        self.container_rows().into_iter().filter(|row| !self.remote.docker.monitor.browser.overview.attention || unhealthy(row)).collect()
    }
    pub(super) fn run_container_action(&mut self, action: ContainerAction, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let m = &self.remote.docker.monitor;
        let (Some(context), Some(target)) = (m.context.clone(), m.containers.iter().find(|c| c.id == id).cloned()) else { return };
        match action {
            ContainerAction::CopyId => cx.write_to_clipboard(gpui::ClipboardItem::new_string(id.to_string())),
            ContainerAction::Files | ContainerAction::Terminal => {
                if self.refuse_if_dirty(cx) {
                    return;
                }
                self.select_container(id.to_string(), Tab::Resources, cx);
                if matches!(action, ContainerAction::Terminal) {
                    let Ok(alias) = context.container_alias(id) else { return };
                    if matches!(&self.remote.conn, Conn::Connected { alias: current } if current == &alias) {
                        self.open_terminal(window, cx);
                        return;
                    }
                    self.remote.docker.monitor.browser.terminal_target = Some(alias);
                }
                self.docker_container_picked(context, target, false, window, cx);
            }
        }
    }
    pub(super) fn container_overview_navigation(&self, p: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let b = &self.remote.docker.monitor.browser;
        div()
            .px(px(theme::SPACE[5]))
            .pb(px(theme::SPACE[3]))
            .flex()
            .gap(px(theme::SPACE[2]))
            .child(
                button("overview-tab", if b.overview.details { crate::i18n::t("← Containers") } else { crate::i18n::t("Overview") }, p)
                    .when(!b.overview.details, |s| s.bg(c(p.surface_active)).text_color(c(p.accent)))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.remote.docker.monitor.browser.overview.details = false;
                        cx.notify();
                    })),
            )
            .when_some(b.selected.clone().filter(|_| !b.overview.details), |d, id| {
                d.child(
                    button("selected-tab", crate::i18n::t("Selected container"), p)
                        .when(b.overview.details, |s| s.bg(c(p.surface_active)).text_color(c(p.accent)))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            let tab = this.remote.docker.monitor.browser.tab;
                            this.select_container(id.clone(), tab, cx);
                        })),
                )
            })
            .into_any_element()
    }

    pub(super) fn container_overview_view(&self, p: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let m = &self.remote.docker.monitor;
        let b = &m.browser;
        let rows = self.container_overview_rows();
        let mut status_filters = div()
            .id("overview-status-filters")
            .flex_none()
            .flex()
            .p(px(theme::SPACE[1]))
            .rounded(px(theme::radius::SM))
            .border_1()
            .border_color(c(p.border))
            .bg(c(p.surface_hover));
        for (label, filter) in
            [(crate::i18n::t("All"), StatusFilter::All), (crate::i18n::t("Running"), StatusFilter::Running), (crate::i18n::t("Stopped"), StatusFilter::Stopped)]
        {
            status_filters = status_filters.child(
                button(SharedString::from(format!("overview-filter-{label}")), label, p)
                    .bg(c(p.surface_hover))
                    .when(b.status_filter == filter && !b.overview.attention, |s| s.bg(c(p.surface_active)).text_color(c(p.accent)))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        let b = &mut this.remote.docker.monitor.browser;
                        b.status_filter = filter;
                        b.overview.attention = false;
                        cx.notify();
                    })),
            );
        }
        let mut toolbar = div().flex_1().min_w(px(320.)).flex().flex_wrap().items_center().justify_end().gap(px(theme::SPACE[2])).child(status_filters);
        if b.status_filter != StatusFilter::All || b.overview.attention || !b.query.trim().is_empty() {
            toolbar = toolbar.child(button("overview-clear", crate::i18n::t("Clear filters"), p).on_click(cx.listener(|this, _, _, cx| {
                let b = &mut this.remote.docker.monitor.browser;
                b.status_filter = StatusFilter::All;
                b.overview.attention = false;
                b.query.clear();
                if let Some(field) = b.field.clone() {
                    field.update(cx, |field, cx| field.set_text("", cx));
                }
                cx.notify();
            })));
        }
        toolbar = toolbar.child(
            button("overview-attention", crate::i18n::t("Needs attention"), p)
                .when(b.overview.attention, |s| s.bg(c(p.surface_active)).text_color(c(p.accent)))
                .on_click(cx.listener(|this, _, _, cx| {
                    let b = &mut this.remote.docker.monitor.browser;
                    b.overview.attention = !b.overview.attention;
                    b.status_filter = StatusFilter::All;
                    cx.notify();
                })),
        );
        toolbar =
            toolbar.child(button("overview-columns", crate::i18n::t("Columns"), p).when(b.overview.columns_open, |s| s.bg(c(p.surface_active))).on_click(
                cx.listener(|this, _, _, cx| {
                    let o = &mut this.remote.docker.monitor.browser.overview;
                    o.columns_open = !o.columns_open;
                    cx.notify();
                }),
            ));
        let mut view = div().flex_1().min_w_0().min_h_0().flex().flex_col().pt(px(theme::SPACE[4])).gap(px(theme::SPACE[3])).child(
            div()
                .px(px(theme::SPACE[5]))
                .flex()
                .flex_wrap()
                .items_center()
                .gap(px(theme::SPACE[3]))
                .when_some(b.field.clone(), |d, field| {
                    d.child(div().flex_1().min_w(px(240.)).max_w(px(480.)).child(super::container_monitor_glue::search_field("overview-search", field, p, cx)))
                })
                .child(toolbar),
        );
        if b.overview.columns_open {
            view = view.child(div().px(px(theme::SPACE[5])).flex().flex_wrap().gap(px(theme::SPACE[2])).children(columns().into_iter().enumerate().map(
                |(index, (label, _))| {
                    button(SharedString::from(format!("overview-column-{index}")), label, p)
                        .when(b.overview.columns[index], |s| s.bg(c(p.surface_active)).text_color(c(p.accent)))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            let value = &mut this.remote.docker.monitor.browser.overview.columns[index];
                            *value = !*value;
                            cx.notify();
                        }))
                },
            )));
        }
        view = view.child(
            div()
                .id("overview-hint")
                .debug_selector(|| "overview-hint".into())
                .px(px(theme::SPACE[5]))
                .flex()
                .flex_wrap()
                .items_center()
                .gap(px(theme::SPACE[3]))
                .text_size(px(text::XS.0))
                .text_color(c(p.text_muted))
                .when(rows.len() != m.containers.len(), |d| d.child(crate::i18n::tf("{0} / {1} containers", &[&rows.len(), &m.containers.len()])))
                .child(crate::i18n::t("Select a row to open its details.")),
        );
        if rows.is_empty() {
            let message = if m.host.is_none() {
                crate::i18n::t("No SSH host connected")
            } else if m.updated.is_none() {
                crate::i18n::t("Discovering containers…")
            } else if m.containers.is_empty() {
                crate::i18n::t("No containers found on this host.")
            } else {
                crate::i18n::t("No containers match the current filters.")
            };
            return view.child(div().p(px(theme::SPACE[6])).text_color(c(p.text_muted)).child(message)).into_any_element();
        }
        let width =
            NAME_WIDTH + STATUS_WIDTH + ACTIONS_WIDTH + columns().iter().enumerate().filter(|(i, _)| b.overview.columns[*i]).map(|(_, (_, w))| w).sum::<f32>();
        let mut header = div()
            .id("overview-header")
            .debug_selector(|| "overview-header".into())
            .w_full()
            .flex()
            .items_center()
            .h(px(34.))
            .bg(c(p.surface_hover))
            .child(self.container_sort_header(crate::i18n::t("Name"), NAME_WIDTH, Sort::Name, p, cx).flex_grow())
            .child(cell(crate::i18n::t("Status"), STATUS_WIDTH, p));
        for (index, (label, width)) in columns().into_iter().enumerate() {
            if b.overview.columns[index] {
                let column = match index {
                    1 => self.container_sort_header(label, width, Sort::Cpu, p, cx),
                    2 => self.container_sort_header(label, width, Sort::Memory, p, cx),
                    _ => cell(label, width, p),
                };
                header = header.child(column.id(("column", index)).when(index == 0, |d| d.flex_grow()));
            }
        }
        header = header.child(cell(crate::i18n::t("Actions"), ACTIONS_WIDTH, p).text_right());
        let mut table = div().id("overview-table").debug_selector(|| "overview-table".into()).w_full().min_w(px(width)).text_size(px(text::SM.0)).child(header);
        for row in rows {
            let id = row.id.clone();
            let logs_id = id.clone();
            let action_id = id.clone();
            let usage = (row.state == "running").then(|| m.usage.iter().find(|u| u.id == row.id)).flatten();
            let last = (row.state == "running").then(|| m.history.get(&row.id).and_then(|h| h.back())).flatten();
            let value = |s: Option<&str>| s.filter(|s| !s.is_empty()).unwrap_or("—").to_string();
            let values = [
                row.image.clone(),
                value(usage.map(|u| u.cpu.as_str())),
                value(usage.map(|u| u.memory.as_str())),
                history::speed(last.and_then(|s| s.network)),
                history::speed(last.and_then(|s| s.disk)),
                value(Some(&row.ports)),
                value(history::project(row)),
            ];
            let mut line = div()
                .id(SharedString::from(format!("overview-row-{id}")))
                .debug_selector({
                    let id = id.clone();
                    move || format!("overview-row-{id}")
                })
                .w_full()
                .flex()
                .items_center()
                .h(px(42.))
                .border_b_1()
                .border_color(c(p.border))
                .cursor_pointer()
                .hover(|s| s.bg(c(p.surface_hover)))
                .when(b.overview.cursor.as_ref().or(b.selected.as_ref()) == Some(&id), |s| s.bg(c(p.surface_active)))
                .child(cell(&row.name, NAME_WIDTH, p).flex_grow().text_color(c(p.text)).font_weight(gpui::FontWeight::MEDIUM))
                .child(status_cell(row, p))
                .on_click(cx.listener(move |this, _, _, cx| this.select_container(id.clone(), Tab::Resources, cx)));
            for (index, value) in values.into_iter().enumerate() {
                if b.overview.columns[index] {
                    line = line.child(match index {
                        1 | 2 => usage_cell(index, value, usage, p).into_any_element(),
                        3 => rate_cell(index, last.and_then(|s| s.network), p).into_any_element(),
                        4 => rate_cell(index, last.and_then(|s| s.disk), p).into_any_element(),
                        _ => cell(value, columns()[index].1, p).id(("column", index)).when(index == 0, |d| d.flex_grow()).into_any_element(),
                    });
                }
            }
            line = line.child(
                div()
                    .id("row-actions")
                    .w(px(ACTIONS_WIDTH))
                    .flex_shrink_0()
                    .px(px(theme::SPACE[3]))
                    .flex()
                    .items_center()
                    .justify_end()
                    .gap(px(theme::SPACE[1]))
                    .child(button(SharedString::from(format!("overview-logs-{logs_id}")), crate::i18n::t("Logs"), p).flex_none().on_click(cx.listener(
                        move |this, _, _, cx| {
                            cx.stop_propagation();
                            this.select_container(logs_id.clone(), Tab::Logs, cx);
                        },
                    )))
                    .child(
                        button(SharedString::from(format!("overview-actions-{action_id}")), "…", p)
                            .w(px(28.))
                            .flex_none()
                            .tooltip(|_, cx| cx.new(|_| Tooltip(crate::i18n::t("Actions").into())).into())
                            .on_click(cx.listener(move |this, _, window, cx| {
                                cx.stop_propagation();
                                this.show_menu(
                                    vec![
                                        MenuItem::new(crate::i18n::t("Open files")),
                                        MenuItem::new(crate::i18n::t("Terminal")),
                                        MenuItem::new(crate::i18n::t("Copy container ID")),
                                    ],
                                    vec![
                                        MenuCmd::Container(ContainerAction::Files),
                                        MenuCmd::Container(ContainerAction::Terminal),
                                        MenuCmd::Container(ContainerAction::CopyId),
                                    ],
                                    Vec::new(),
                                    MenuTarget { path: action_id.clone(), is_dir: false, tab: None },
                                    window.mouse_position(),
                                    window,
                                    cx,
                                );
                            })),
                    ),
            );
            table = table.child(line);
        }
        view.child(
            div()
                .id("container-overview-scroll")
                .debug_selector(|| "container-overview-scroll".into())
                .flex_1()
                .min_w_0()
                .min_h_0()
                .mx(px(theme::SPACE[5]))
                .mb(px(theme::SPACE[5]))
                .overflow_scroll()
                .track_scroll(&b.overview.scroll)
                .when_some(b.overview.focus.as_ref(), |d, focus| {
                    let keyboard = ui::keyboard_focus(cx);
                    d.track_focus(focus).border_1().border_color(c(p.border)).when(keyboard, |d| d.focus(|s| s.border_color(c(p.focus_ring))))
                })
                .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, _, cx| {
                    let key = event.keystroke.key.as_str();
                    if event.keystroke.modifiers == gpui::Modifiers::none()
                        && let Some(sort) = match key {
                            "n" => Some(Sort::Name),
                            "c" => Some(Sort::Cpu),
                            "m" => Some(Sort::Memory),
                            _ => None,
                        }
                    {
                        cx.stop_propagation();
                        this.sort_container_overview(sort, cx);
                        return;
                    }
                    if !matches!(key, "up" | "down" | "enter") {
                        return;
                    }
                    let rows = this.container_overview_rows();
                    if rows.is_empty() {
                        return;
                    }
                    let b = &this.remote.docker.monitor.browser;
                    let selected = b.overview.cursor.as_ref().or(b.selected.as_ref());
                    let current = rows.iter().position(|c| Some(&c.id) == selected);
                    let index = match (key, current) {
                        ("up", Some(i)) => i.saturating_sub(1),
                        ("down", Some(i)) => (i + 1).min(rows.len() - 1),
                        (_, Some(i)) => i,
                        _ => 0,
                    };
                    let id = rows[index].id.clone();
                    cx.stop_propagation();
                    if key == "enter" {
                        this.select_container(id, Tab::Resources, cx);
                    } else {
                        let b = &mut this.remote.docker.monitor.browser;
                        b.overview.cursor = Some(id);
                        let offset = b.overview.scroll.offset();
                        b.overview.scroll.set_offset(gpui::point(offset.x, px(-(index as f32 * 42.))));
                        cx.notify();
                    }
                }))
                .child(table),
        )
        .into_any_element()
    }
}

#[cfg(test)]
#[path = "workbench_container_overview_tests.rs"]
mod tests;
