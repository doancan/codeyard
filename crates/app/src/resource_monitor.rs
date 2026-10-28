//! Status-bar resource popover: explicit open, pointer dismissal, optional pinning.

use std::path::PathBuf;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::{Duration, Instant};

use gpui::{Animation, AnimationExt as _, Context, FocusHandle, Render, Task, Window, deferred, div, prelude::*, px};

use crate::i18n::t;
use crate::resources::{self, CpuTicks, ProjectSize, SystemSample};
use crate::ui::{self, c, icon, layout, palette, text};

gpui::actions!(resources, [ToggleResources]);

pub struct ResourceMonitor {
    shown: bool,
    closing: bool,
    pinned: bool,
    over_icon: bool,
    over_panel: bool,
    focus: FocusHandle,
    pin_focus: FocusHandle,
    close_focus: FocusHandle,
    project: Option<PathBuf>,
    remote: bool,
    sample: Option<SystemSample>,
    cpu: Option<f32>,
    previous_cpu: Option<CpuTicks>,
    size: Option<ProjectSize>,
    size_at: Option<Instant>,
    poll: Option<Task<()>>,
    scan: Option<Task<()>>,
    cancel_scan: Arc<AtomicBool>,
    dismiss: Option<Task<()>>,
}

impl ResourceMonitor {
    pub fn new(cx: &mut Context<Self>) -> Self {
        Self {
            shown: false,
            closing: false,
            pinned: false,
            over_icon: false,
            over_panel: false,
            focus: cx.focus_handle(),
            pin_focus: cx.focus_handle(),
            close_focus: cx.focus_handle(),
            project: None,
            remote: false,
            sample: None,
            cpu: None,
            previous_cpu: None,
            size: None,
            size_at: None,
            poll: None,
            scan: None,
            cancel_scan: Arc::new(AtomicBool::new(false)),
            dismiss: None,
        }
    }

    pub fn set_project(&mut self, project: Option<PathBuf>, remote: bool, cx: &mut Context<Self>) {
        if self.project == project && self.remote == remote {
            return;
        }
        self.cancel_scan.store(true, Ordering::Relaxed);
        self.scan = None;
        self.project = project;
        self.remote = remote;
        self.size = None;
        self.size_at = None;
        if self.shown && !self.closing {
            self.scan_project(cx);
        }
        cx.notify();
    }

    pub fn toggle(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus);
        if self.shown && !self.closing {
            self.close(cx);
        } else {
            self.open(cx);
        }
    }

    fn open(&mut self, cx: &mut Context<Self>) {
        self.dismiss = None;
        self.shown = true;
        self.closing = false;
        self.previous_cpu = None;
        self.cpu = None;
        self.start_poll(cx);
        self.scan_project(cx);
        cx.notify();
    }

    fn close(&mut self, cx: &mut Context<Self>) {
        if !self.shown || self.closing {
            return;
        }
        self.pinned = false;
        self.closing = true;
        self.poll = None;
        self.cancel_scan.store(true, Ordering::Relaxed);
        self.scan = None;
        self.dismiss = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(Duration::from_millis(120)).await;
            this.update(cx, |this, cx| {
                this.shown = false;
                this.closing = false;
                this.over_panel = false;
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    fn hover(&mut self, panel: bool, hovered: bool, cx: &mut Context<Self>) {
        let was_hovered = if panel { self.over_panel } else { self.over_icon };
        if panel {
            self.over_panel = hovered;
        } else {
            self.over_icon = hovered;
        }
        if hovered {
            // Leave enough time to cross the small gap between the icon and its panel.
            if !self.closing {
                self.dismiss = None;
            }
        } else if was_hovered && self.shown && !self.pinned && !self.over_icon && !self.over_panel && !self.closing {
            self.dismiss = Some(cx.spawn(async move |this, cx| {
                cx.background_executor().timer(Duration::from_millis(160)).await;
                this.update(cx, |this, cx| {
                    if !this.pinned && !this.over_icon && !this.over_panel {
                        this.close(cx);
                    }
                })
                .ok();
            }));
        }
    }

    fn start_poll(&mut self, cx: &mut Context<Self>) {
        self.poll = Some(cx.spawn(async move |this, cx| {
            loop {
                let Ok(disk) = this.read_with(cx, |this, _| this.project.clone().filter(|_| !this.remote).unwrap_or_else(std::env::temp_dir)) else {
                    break;
                };
                let sample = cx.background_executor().spawn(async move { resources::sample(&disk) }).await;
                if this
                    .update(cx, |this, cx| {
                        if !this.shown || this.closing {
                            return;
                        }
                        this.cpu = sample.cpu.zip(this.previous_cpu).and_then(|(now, old)| now.usage_since(old));
                        this.previous_cpu = sample.cpu;
                        this.sample = Some(sample);
                        this.scan_project(cx);
                        cx.notify();
                    })
                    .is_err()
                {
                    break;
                }
                cx.background_executor().timer(Duration::from_secs(2)).await;
            }
        }));
    }

    fn scan_project(&mut self, cx: &mut Context<Self>) {
        if self.remote || self.scan.is_some() || self.size_at.is_some_and(|at| at.elapsed() < Duration::from_secs(30)) {
            return;
        }
        let Some(root) = self.project.clone() else {
            return;
        };
        let cancel = Arc::new(AtomicBool::new(false));
        self.cancel_scan = cancel.clone();
        self.scan = Some(cx.spawn(async move |this, cx| {
            let scan_root = root.clone();
            let result = cx.background_executor().spawn(async move { resources::project_size(scan_root, &cancel) }).await;
            this.update(cx, |this, cx| {
                if this.project.as_ref() == Some(&root) && this.shown && !this.closing {
                    this.size = Some(result);
                    this.size_at = Some(Instant::now());
                    this.scan = None;
                    cx.notify();
                }
            })
            .ok();
        }));
    }

    fn card(&self, window: &Window, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let p = *palette(cx);
        let unavailable = || t("Unavailable").to_string();
        let bytes = |n: Option<u64>| n.map(crate::compare_logic::human_size).unwrap_or_else(unavailable);
        let row = |label: &str, value: String| {
            div()
                .flex()
                .justify_between()
                .items_center()
                .gap(px(16.))
                .py(px(5.))
                .child(div().text_color(c(p.text_muted)).child(label.to_string()))
                .child(div().text_color(c(p.text)).child(value))
        };
        let sample = self.sample.as_ref();
        let cpu = self.cpu.map(|n| format!("{n:.0}%")).unwrap_or_else(|| t("Measuring…").to_string());
        let memory = sample
            .and_then(|s| s.memory)
            .map(|(used, total)| format!("{} / {}", crate::compare_logic::human_size(used), crate::compare_logic::human_size(total)))
            .unwrap_or_else(unavailable);
        let meter = |fraction: Option<f32>| {
            div().h(px(3.)).w_full().rounded_full().bg(c(p.surface_active)).when_some(fraction, |d, value| {
                d.child(div().h_full().w(gpui::relative(value.clamp(0., 1.))).rounded_full().bg(c(if value > 0.9 { p.warning } else { p.accent })))
            })
        };
        div()
            .id("resource-card")
            .debug_selector(|| "resource-card".into())
            .occlude()
            .w(px(336.))
            .max_w((window.viewport_size().width - px(24.)).max(px(160.)))
            .max_h((window.viewport_size().height - px(layout::STATUSBAR_H + layout::TITLEBAR_H + 16.)).max(px(100.)))
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .p(px(14.))
            .rounded(px(theme::radius::LG))
            .bg(c(p.surface_raised))
            .border_1()
            .border_color(c(p.border))
            .shadow_lg()
            .text_size(px(text::XS.0))
            .on_hover(cx.listener(|this, hovered, _, cx| this.hover(true, *hovered, cx)))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .mb(px(2.))
                    .child(icon("activity", 16., p.accent))
                    .child(div().flex_1().text_size(px(text::SM.0)).font_weight(gpui::FontWeight::SEMIBOLD).child(t("Resources")))
                    .child(
                        ui::icon_button("resources-pin", "pin", if self.pinned { t("Unpin") } else { t("Pin") }, &p)
                            .size(px(26.))
                            .border_1()
                            .border_color(ui::ca(p.focus_ring, 0.))
                            .focus(|d| d.border_color(c(p.focus_ring)))
                            .track_focus(&self.pin_focus)
                            .tab_index(0)
                            .debug_selector(|| "resources-pin".into())
                            .when(self.pinned, |d| d.bg(c(p.surface_active)))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.pinned = !this.pinned;
                                this.dismiss = None;
                                cx.notify();
                            })),
                    )
                    .child(
                        ui::icon_button("resources-close", "x", t("Close"), &p)
                            .size(px(26.))
                            .border_1()
                            .border_color(ui::ca(p.focus_ring, 0.))
                            .focus(|d| d.border_color(c(p.focus_ring)))
                            .track_focus(&self.close_focus)
                            .tab_index(0)
                            .on_click(cx.listener(|this, _, _, cx| this.close(cx))),
                    ),
            )
            .child(row(t("CPU"), cpu))
            .child(meter(self.cpu.map(|n| n / 100.)))
            .child(row(t("Memory"), memory))
            .child(meter(sample.and_then(|s| s.memory).map(|(used, total)| used as f32 / total.max(1) as f32)))
            .when_some(sample.and_then(|s| s.load), |d, load| {
                d.child(row(t("Load average (1 / 5 / 15 min)"), format!("{:.2} / {:.2} / {:.2}", load[0], load[1], load[2])))
            })
            .when_some(sample, |d, sample| d.child(row(t("Logical processors"), sample.cores.to_string())))
            .child(row(t("Codeyard memory"), bytes(sample.and_then(|s| s.app_memory))))
            .child(row(t("Disk available"), bytes(sample.and_then(|s| s.disk_free))))
            .child(
                div()
                    .border_t_1()
                    .border_color(c(p.border))
                    .mt(px(8.))
                    .pt(px(10.))
                    .child(div().font_weight(gpui::FontWeight::SEMIBOLD).child(t("Project")))
                    .when_some(self.project.as_ref(), |d, root| {
                        d.child(div().text_color(c(p.text_muted)).truncate().child(root.to_string_lossy().into_owned()))
                    })
                    .when(self.remote, |d| d.child(div().py(px(8.)).text_color(c(p.text_muted)).child(t("Project size is available for local folders."))))
                    .when(!self.remote && self.project.is_none(), |d| {
                        d.child(div().py(px(8.)).text_color(c(p.text_muted)).child(t("Open a folder to measure its size.")))
                    })
                    .when(!self.remote && self.project.is_some(), |d| {
                        d.child(row(
                            t("Project size"),
                            self.size
                                .as_ref()
                                .map(|s| format!("{}{}", if s.partial { "≥ " } else { "" }, crate::compare_logic::human_size(s.bytes)))
                                .unwrap_or_else(|| t("Measuring…").to_string()),
                        ))
                        .child(row(
                            t("Files"),
                            self.size.as_ref().map(|s| format!("{}{}", if s.partial { "≥ " } else { "" }, s.files)).unwrap_or_else(|| "—".into()),
                        ))
                        .child(div().text_color(c(p.text_muted)).child(t("Includes hidden and build files; symlinks are not followed.")))
                        .when(self.size.as_ref().is_some_and(|s| s.partial), |d| {
                            d.child(div().mt(px(4.)).text_color(c(p.warning)).child(t("Partial scan: a time, file or access limit was reached.")))
                        })
                    }),
            )
            .child(div().mt(px(12.)).text_color(c(p.text_muted)).child(if self.pinned {
                t("Pinned · click the pin to restore auto-close")
            } else {
                t("Closes when the pointer leaves · pin to keep open")
            }))
    }
}

impl Render for ResourceMonitor {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = *palette(cx);
        let shown = self.shown;
        let closing = self.closing;
        if closing && (self.pin_focus.is_focused(window) || self.close_focus.is_focused(window)) {
            window.focus(&self.focus);
        }
        div()
            .relative()
            .h_full()
            .flex_none()
            .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, window, cx| {
                if event.keystroke.key == "escape" && this.shown {
                    this.close(cx);
                    cx.stop_propagation();
                } else if event.keystroke.key == "tab" && this.shown && !this.closing {
                    let modifiers = event.keystroke.modifiers;
                    if modifiers.control || modifiers.alt || modifiers.platform || modifiers.function {
                        return;
                    }
                    let handles = [&this.focus, &this.pin_focus, &this.close_focus];
                    let current = handles.iter().position(|h| h.is_focused(window)).unwrap_or(0);
                    let next = (current + if modifiers.shift { 2 } else { 1 }) % handles.len();
                    window.focus(handles[next]);
                    window.prevent_default();
                    cx.stop_propagation();
                }
            }))
            .child(
                div()
                    .id("sb-resources")
                    .debug_selector(|| "sb-resources".into())
                    .track_focus(&self.focus)
                    .tab_index(0)
                    .h_full()
                    .w(px(32.))
                    .border_1()
                    .border_color(ui::ca(p.focus_ring, 0.))
                    .focus(|d| d.border_color(c(p.focus_ring)))
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .when(shown && !closing, |d| d.bg(c(p.surface_active)))
                    .hover(|d| d.bg(c(p.surface_hover)))
                    .child(icon("activity", 16., if shown { p.accent } else { p.text_muted }))
                    .on_hover(cx.listener(|this, hovered, _, cx| this.hover(false, *hovered, cx)))
                    .on_click(cx.listener(|this, _, window, cx| this.toggle(window, cx))),
            )
            .when(shown, |d| {
                let card = div().absolute().bottom(px(layout::STATUSBAR_H + 6.)).right_0().child(self.card(window, cx));
                if crate::system_prefs::reduce_motion(cx) {
                    return d.child(deferred(card));
                }
                d.child(deferred(card.with_animation(
                    if closing { "resources-exit" } else { "resources-enter" },
                    Animation::new(Duration::from_millis(if closing { 120 } else { 160 })),
                    move |d, delta| d.opacity(if closing { 1. - delta } else { delta }),
                )))
            })
    }
}

impl Drop for ResourceMonitor {
    fn drop(&mut self) {
        self.cancel_scan.store(true, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::TestAppContext;

    struct BarFixture(gpui::Entity<ResourceMonitor>);

    impl Render for BarFixture {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().size_full().flex().flex_col().child(div().flex_1()).child(div().h(px(layout::STATUSBAR_H)).flex().justify_end().child(self.0.clone()))
        }
    }

    #[gpui::test]
    fn rendered_pointer_can_cross_into_card_then_leave_to_dismiss(cx: &mut TestAppContext) {
        crate::qa_workbench_tests::globals(cx);
        let monitor = cx.new(ResourceMonitor::new);
        let (_fixture, cx) = cx.add_window_view(|_, _| BarFixture(monitor.clone()));
        cx.run_until_parked();
        let trigger = cx.debug_bounds("sb-resources").unwrap().center();
        cx.simulate_mouse_move(trigger, None, gpui::Modifiers::none());
        cx.simulate_click(trigger, gpui::Modifiers::none());
        cx.run_until_parked();
        let card = cx.debug_bounds("resource-card").unwrap();
        cx.simulate_mouse_move(card.center(), None, gpui::Modifiers::none());
        cx.run_until_parked();
        cx.executor().advance_clock(Duration::from_millis(300));
        cx.run_until_parked();
        assert!(monitor.read_with(cx, |m, _| m.shown && !m.closing && m.over_panel));
        cx.simulate_mouse_move(gpui::point(px(10.), px(10.)), None, gpui::Modifiers::none());
        cx.run_until_parked();
        cx.executor().advance_clock(Duration::from_millis(170));
        cx.run_until_parked();
        cx.executor().advance_clock(Duration::from_millis(130));
        cx.run_until_parked();
        assert!(monitor.read_with(cx, |m, _| !m.shown && m.poll.is_none()), "real hit testing must dismiss the panel and stop sampling");
    }

    #[gpui::test]
    fn keyboard_activation_opens_once_and_initial_nonhover_does_not_dismiss(cx: &mut TestAppContext) {
        crate::qa_workbench_tests::globals(cx);
        let (monitor, cx) = cx.add_window_view(|_, cx| ResourceMonitor::new(cx));
        monitor.update_in(cx, |m, window, cx| {
            window.focus(&m.focus);
            cx.notify();
        });
        cx.run_until_parked();
        cx.simulate_keystrokes("enter");
        // simulate_keystrokes emits only key-down; native button activation happens on key-up.
        cx.simulate_event(gpui::KeyUpEvent { keystroke: gpui::Keystroke::parse("enter").unwrap() });
        cx.run_until_parked();
        assert!(monitor.read_with(cx, |m, _| m.shown && !m.closing), "Enter must not toggle twice on key down/up");
        monitor.update(cx, |m, cx| m.hover(true, false, cx));
        cx.executor().advance_clock(Duration::from_millis(400));
        cx.run_until_parked();
        assert!(monitor.read_with(cx, |m, _| m.shown && !m.closing), "a newly rendered panel has not been left yet");
        cx.simulate_keystrokes("tab");
        assert!(monitor.update_in(cx, |m, window, _| m.pin_focus.is_focused(window)));
        cx.simulate_keystrokes("space");
        cx.simulate_event(gpui::KeyUpEvent { keystroke: gpui::Keystroke::parse("space").unwrap() });
        cx.run_until_parked();
        assert!(monitor.read_with(cx, |m, _| m.pinned), "pin is reachable and usable by keyboard");
        cx.simulate_keystrokes("tab");
        assert!(monitor.update_in(cx, |m, window, _| m.close_focus.is_focused(window)));
        cx.simulate_keystrokes("escape");
        cx.run_until_parked();
        cx.executor().advance_clock(Duration::from_millis(150));
        cx.run_until_parked();
        assert!(monitor.read_with(cx, |m, _| !m.shown));
        assert!(monitor.update_in(cx, |m, window, _| m.focus.is_focused(window)), "closing restores focus to a visible control");
    }

    #[gpui::test]
    fn pointer_exit_closes_unless_pinned_and_stops_sampling(cx: &mut TestAppContext) {
        crate::qa_workbench_tests::globals(cx);
        let (monitor, cx) = cx.add_window_view(|_, cx| ResourceMonitor::new(cx));
        assert!(monitor.read_with(cx, |m, _| m.poll.is_none() && !m.shown));
        monitor.update(cx, |m, cx| {
            m.open(cx);
            m.hover(false, true, cx);
            m.hover(false, false, cx);
            m.hover(true, true, cx);
        });
        cx.executor().advance_clock(Duration::from_millis(200));
        cx.run_until_parked();
        assert!(monitor.read_with(cx, |m, _| m.shown && !m.closing), "crossing into the panel keeps it open");
        monitor.update(cx, |m, cx| {
            m.pinned = true;
            m.hover(true, false, cx);
        });
        cx.executor().advance_clock(Duration::from_secs(1));
        cx.run_until_parked();
        assert!(monitor.read_with(cx, |m, _| m.shown && !m.closing));
        monitor.update(cx, |m, cx| {
            m.hover(true, true, cx);
            m.pinned = false;
            m.hover(true, false, cx);
        });
        cx.executor().advance_clock(Duration::from_millis(200));
        cx.run_until_parked();
        cx.executor().advance_clock(Duration::from_millis(150));
        cx.run_until_parked();
        assert!(monitor.read_with(cx, |m, _| !m.shown && m.poll.is_none() && m.scan.is_none()));
    }

    #[gpui::test]
    fn project_switch_drops_previous_sizes_and_closing_can_be_interrupted(cx: &mut TestAppContext) {
        crate::qa_workbench_tests::globals(cx);
        let (monitor, cx) = cx.add_window_view(|_, cx| ResourceMonitor::new(cx));
        monitor.update(cx, |m, cx| {
            m.size = Some(ProjectSize { bytes: 999, ..Default::default() });
            m.set_project(Some(PathBuf::from("/a/remote/project")), true, cx);
            assert!(m.size.is_none() && m.scan.is_none(), "a remote path must not be scanned locally");
            m.open(cx);
            m.close(cx);
            m.open(cx);
        });
        cx.executor().advance_clock(Duration::from_millis(500));
        cx.run_until_parked();
        assert!(monitor.read_with(cx, |m, _| m.shown && !m.closing && m.poll.is_some()));
    }
}
