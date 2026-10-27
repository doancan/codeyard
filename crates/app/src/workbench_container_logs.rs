//! Searchable, severity-aware native log console.
use super::container_monitor_glue::button;
use super::*;
use crate::container_logs::{self as logs, Level};

impl Workbench {
    pub(super) fn ensure_container_log_search(&mut self, cx: &mut Context<Self>) {
        if self.remote.docker.monitor.browser.log_view.field.is_some() {
            return;
        }
        let field = cx.new(|cx| crate::input::TextField::new(crate::i18n::t("Search log messages or timestamps…"), cx));
        let subscription = cx.subscribe(&field, |this, field, event, cx| {
            if matches!(event, crate::input::FieldEvent::Changed) {
                let view = &mut this.remote.docker.monitor.browser.log_view;
                view.query = field.read(cx).text().to_string();
                if view.follow {
                    view.scroll.scroll_to_bottom();
                }
                cx.notify();
            }
        });
        self.remote.docker.monitor.browser.log_view.field = Some(field);
        self.remote.docker.monitor.browser.log_view.subscription = Some(subscription);
    }

    pub(super) fn container_logs_view(&self, p: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let browser = &self.remote.docker.monitor.browser;
        let state = &browser.log_view;
        let lines = logs::parse(&browser.logs);
        let visible = logs::visible(&lines, &state.query, state.problems_only);
        let errors = lines.iter().filter(|l| l.level == Level::Error).count();
        let warnings = lines.iter().filter(|l| l.level == Level::Warning).count();
        let copy = visible.iter().map(|l| l.full.as_str()).collect::<Vec<_>>().join("\n");
        let mut view = div()
            .id("container-log-console")
            .debug_selector(|| "container-log-console".into())
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .gap(px(theme::SPACE[3]))
            .child(
                div()
                    .px(px(theme::SPACE[5]))
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(px(theme::SPACE[3]))
                    .when_some(state.field.clone(), |d, field| d.child(div().min_w(px(180.)).flex_1().child(field)))
                    .child(
                        button("logs-problems", if state.problems_only { crate::i18n::t("Errors and warnings") } else { crate::i18n::t("All levels") }, p)
                            .on_click(cx.listener(|this, _, _, cx| {
                                let s = &mut this.remote.docker.monitor.browser.log_view;
                                s.problems_only = !s.problems_only;
                                if s.follow {
                                    s.scroll.scroll_to_bottom();
                                }
                                cx.notify();
                            })),
                    )
                    .child(button("logs-wrap", if state.wrap { crate::i18n::t("Wrapped lines") } else { crate::i18n::t("Unwrapped lines") }, p).on_click(
                        cx.listener(|this, _, _, cx| {
                            let s = &mut this.remote.docker.monitor.browser.log_view;
                            s.wrap = !s.wrap;
                            if s.follow {
                                s.scroll.scroll_to_bottom();
                            }
                            cx.notify();
                        }),
                    ))
                    .child(button("logs-copy", crate::i18n::t("Copy visible logs"), p).on_click(cx.listener(move |_, _, _, cx| {
                        if !copy.is_empty() {
                            cx.write_to_clipboard(gpui::ClipboardItem::new_string(copy.clone()));
                        }
                    }))),
            )
            .child(
                div()
                    .px(px(theme::SPACE[5]))
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .justify_between()
                    .gap(px(theme::SPACE[3]))
                    .child(
                        div()
                            .text_size(px(text::XS.0))
                            .text_color(c(p.text_muted))
                            .child(crate::i18n::tf("{0} / {1} lines · {2} errors · {3} warnings", &[&visible.len(), &lines.len(), &errors, &warnings])),
                    )
                    .child(
                        button(
                            "logs-follow",
                            if state.follow { crate::i18n::t("Following latest logs") } else { crate::i18n::t("Jump to latest and follow") },
                            p,
                        )
                        .when(state.follow, |s| s.bg(c(p.surface_active)))
                        .on_click(cx.listener(|this, _, _, cx| {
                            let s = &mut this.remote.docker.monitor.browser.log_view;
                            s.follow = !s.follow;
                            if s.follow {
                                s.scroll.scroll_to_bottom();
                            }
                            cx.notify();
                        })),
                    ),
            )
            .child(
                div()
                    .px(px(theme::SPACE[5]))
                    .text_size(px(text::XS.0))
                    .text_color(c(p.text_muted))
                    .child(crate::i18n::t("Latest 300 lines; refreshed with resource snapshots.")),
            );
        if browser.loading && browser.logs.is_empty() {
            view = view
                .child(div().px(px(theme::SPACE[5])).text_size(px(text::XS.0)).text_color(c(p.text_muted)).child(crate::i18n::t("Loading container details…")));
        }
        if let Some(error) = &browser.error {
            view = view.child(div().px(px(theme::SPACE[5])).text_size(px(text::SM.0)).text_color(c(p.danger)).child(error.clone()));
        }
        let regex = logs::matcher(&state.query);
        let mut rows = div().min_w_full().flex().flex_col().font_family(ui::MONO_FONT).text_size(px(text::XS.0)).text_color(c(p.text));
        for line in &visible {
            let (label, color) = match line.level {
                Level::Error => (crate::i18n::t("Error"), p.danger),
                Level::Warning => (crate::i18n::t("Warning"), p.warning),
                Level::Info => (crate::i18n::t("Info"), p.info),
                Level::Debug => (crate::i18n::t("Debug"), p.text_muted),
                Level::Plain => ("—", p.text_muted),
            };
            let highlights = regex
                .as_ref()
                .map(|r| {
                    r.find_iter(&line.message)
                        .map(|m| {
                            (
                                m.range(),
                                gpui::HighlightStyle { color: Some(c(p.accent).into()), font_weight: Some(gpui::FontWeight::BOLD), ..Default::default() },
                            )
                        })
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            let timestamp = line.timestamp.clone();
            let time = timestamp.as_ref().map(|t| format!("{}Z", &t[11..19])).unwrap_or_else(|| "—".into());
            rows =
                rows.child(
                    div()
                        .id(("container-log-line", line.number))
                        .debug_selector({
                            let n = line.number;
                            move || format!("container-log-line-{n}")
                        })
                        .min_w_full()
                        .flex_none()
                        .when(state.wrap, |d| d.w_full())
                        .flex()
                        .items_start()
                        .gap(px(theme::SPACE[3]))
                        .px(px(theme::SPACE[4]))
                        .py(px(3.))
                        .when(line.level == Level::Error, |d| d.bg(c(p.diff_removed)))
                        .hover(|d| d.bg(c(p.surface_hover)))
                        .child(div().w(px(30.)).flex_none().text_right().text_color(c(p.text_muted)).child(line.number.to_string()))
                        .child(div().id("container-log-time").w(px(90.)).flex_none().text_color(c(p.text_muted)).child(time).tooltip(move |_, cx| {
                            cx.new(|_| Tooltip(timestamp.clone().unwrap_or_else(|| crate::i18n::t("No timestamp").into()).into())).into()
                        }))
                        .child(div().w(px(62.)).flex_none().text_color(c(color)).child(label))
                        .child(
                            div()
                                .when(state.wrap, |d| d.flex_1().min_w_0())
                                .when(!state.wrap, |d| d.flex_none().whitespace_nowrap())
                                .child(gpui::StyledText::new(line.message.clone()).with_highlights(highlights)),
                        ),
                );
        }
        if visible.is_empty() && !browser.loading {
            rows = rows.child(div().p(px(theme::SPACE[5])).text_color(c(p.text_muted)).child(if lines.is_empty() {
                crate::i18n::t("No log output")
            } else {
                crate::i18n::t("No log lines match the current filters.")
            }));
        }
        view.child(
            div()
                .id("container-log-scroll")
                .debug_selector(|| "container-log-scroll".into())
                .flex_1()
                .min_h_0()
                .overflow_scroll()
                .track_scroll(&state.scroll)
                .bg(c(p.surface))
                .border_t_1()
                .border_color(c(p.border))
                .child(rows)
                .on_scroll_wheel(cx.listener(|this, event: &gpui::ScrollWheelEvent, _, cx| {
                    if event.delta.pixel_delta(px(16.)).y > px(0.) {
                        this.remote.docker.monitor.browser.log_view.follow = false;
                        cx.notify();
                    }
                })),
        )
        .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[gpui::test]
    fn container_logs_console_search_copy_wrap_and_follow_controls_work(cx: &mut gpui::TestAppContext) {
        let (wb, cx) = crate::qa_workbench_tests::setup(cx);
        let source: Arc<dyn host::Host> = crate::container_monitor::tests::FixtureHost::new();
        let snapshot = crate::container_monitor::read(source.as_ref(), "fixture", true, &std::sync::atomic::AtomicBool::new(false)).unwrap();
        cx.update(|_, cx| cx.global_mut::<Settings>().docker_enabled = true);
        wb.update(cx, |wb, cx| {
            wb.remote.host = source.clone();
            wb.remote.conn = Conn::Connected { alias: "fixture".into() };
            let m = &mut wb.remote.docker.monitor;
            m.source = Some(source);
            m.host = Some("fixture".into());
            m.apply(Ok(snapshot));
            m.paused = true;
            m.browser.selected = Some("web-id".into());
            m.browser.tab = crate::container_history::Tab::Logs;
            m.browser.logs = "2026-10-27T12:34:55Z INFO ready\n2026-10-27T12:34:56Z ERROR disk full\n2026-10-27T12:34:57Z WARN slow".into();
            wb.ensure_container_log_search(cx);
            wb.open_container_dashboard(cx);
        });
        cx.run_until_parked();
        assert!(cx.debug_bounds("container-log-console").is_some());
        let field = wb.read_with(cx, |wb, _| wb.remote.docker.monitor.browser.log_view.field.clone()).unwrap();
        field.update(cx, |field, cx| field.set_text("disk", cx));
        cx.run_until_parked();
        let copy = cx.debug_bounds("container-button-Copy visible logs").unwrap().center();
        cx.simulate_click(copy, gpui::Modifiers::none());
        cx.run_until_parked();
        assert_eq!(cx.read_from_clipboard().and_then(|c| c.text()).as_deref(), Some("2026-10-27T12:34:56Z ERROR disk full"));
        let wrap = cx.debug_bounds("container-button-Wrapped lines").unwrap().center();
        cx.simulate_click(wrap, gpui::Modifiers::none());
        cx.run_until_parked();
        assert!(!wb.read_with(cx, |wb, _| wb.remote.docker.monitor.browser.log_view.wrap));
        let follow = cx.debug_bounds("container-button-Following latest logs").unwrap().center();
        cx.simulate_click(follow, gpui::Modifiers::none());
        cx.run_until_parked();
        assert!(!wb.read_with(cx, |wb, _| wb.remote.docker.monitor.browser.log_view.follow));
        wb.update(cx, |wb, cx| {
            wb.remote.docker.monitor.clear();
            cx.notify();
        });
    }
}
