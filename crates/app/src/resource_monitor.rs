//! Status-bar resources: a strip that opens leftwards inside the status bar, closes when the pointer leaves, can be pinned.

use std::path::PathBuf;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::{Duration, Instant};

use gpui::{Animation, AnimationExt as _, Context, FocusHandle, Render, Task, Window, div, prelude::*, px};

use crate::i18n::t;
use crate::resources::{self, CpuTicks, ProjectSize, SystemSample};
use crate::ui::{self, c, icon, palette, text};

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

    /// The figures, laid out in one row of the status bar. The most important ones sit next to the icon, so a narrow window
    /// cuts the least important ones on the left first.
    fn strip(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let p = *palette(cx);
        let keyboard = ui::keyboard_focus(cx);
        let unavailable = || t("Unavailable").to_string();
        let bytes = |n: Option<u64>| n.map(crate::compare_logic::human_size).unwrap_or_else(unavailable);
        let stat = |label: &str, value: String, color: u32| {
            let read = format!("{label}: {value}");
            div()
                .relative()
                .child(crate::a11y_controls::control(crate::a11y_controls::Control::text(
                    t("Status bar"),
                    format!("resources-{label}"),
                    accesskit::Role::Label,
                    read,
                )))
                .flex_none()
                .flex()
                .items_center()
                .gap(px(5.))
                .child(div().text_color(c(p.text_muted)).child(label.to_string()))
                .child(div().text_color(c(color)).child(value))
        };
        let meter = |fraction: Option<f32>| {
            div().flex_none().w(px(28.)).h(px(3.)).rounded_full().bg(c(p.surface_active)).when_some(fraction, |d, value| {
                d.child(div().h_full().w(gpui::relative(value.clamp(0., 1.))).rounded_full().bg(c(if value > 0.9 { p.warning } else { p.accent })))
            })
        };
        let divider = || div().flex_none().w(px(1.)).h(px(12.)).bg(c(p.border));
        let sample = self.sample.as_ref();
        let cpu = self.cpu.map(|n| format!("{n:.0}%")).unwrap_or_else(|| t("Measuring…").to_string());
        let memory = sample
            .and_then(|s| s.memory)
            .map(|(used, total)| format!("{} / {}", crate::compare_logic::human_size(used), crate::compare_logic::human_size(total)))
            .unwrap_or_else(unavailable);
        let project = (!self.remote && self.project.is_some()).then(|| match &self.size {
            Some(size) => {
                let at_least = if size.partial { "≥ " } else { "" };
                let value = format!("{at_least}{} · {at_least}{} {}", crate::compare_logic::human_size(size.bytes), size.files, t("files"));
                (value, if size.partial { p.warning } else { p.text })
            }
            None => (t("Measuring…").to_string(), p.text_muted),
        });
        let weak = cx.entity().downgrade();
        let button = |id: &'static str, glyph: &'static str, tip: &'static str, on: bool, focus: &FocusHandle| {
            let control = crate::a11y_controls::Control::button(t("Status bar"), id, tip, |_, _| {}).focus(focus.clone());
            let control = match id {
                "resources-pin" => control.toggled(on).on_entity(weak.clone(), |this: &mut Self, _, cx| {
                    this.pinned = !this.pinned;
                    this.dismiss = None;
                    cx.notify();
                }),
                _ => control.on_entity(weak.clone(), |this: &mut Self, _, cx| this.close(cx)),
            };
            div()
                .id(id)
                .relative()
                .child(crate::a11y_controls::control(control))
                .debug_selector(move || id.into())
                .flex_none()
                .size(px(20.))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(theme::radius::SM))
                .cursor_pointer()
                .border_1()
                .border_color(ui::ca(p.focus_ring, 0.))
                .when(keyboard, |d| d.focus(|d| d.border_color(c(p.focus_ring))))
                .when(on, |d| d.bg(c(p.surface_active)))
                .hover(|d| d.bg(c(p.surface_hover)))
                .active(|d| d.bg(c(p.surface_active)))
                .track_focus(focus)
                .tab_index(0)
                .child(icon(glyph, 13., if on { p.accent } else { p.text_muted }))
                .tooltip(move |_, cx| cx.new(|_| crate::workbench::Tooltip(tip.into())).into())
        };
        div()
            .flex_none()
            .h_full()
            .flex()
            .items_center()
            .gap(px(10.))
            .pl(px(10.))
            .pr(px(4.))
            .text_size(px(text::XS.0))
            .whitespace_nowrap()
            .when_some(project, |d, (value, color)| d.child(stat(t("Project"), value, color)).child(divider()))
            .child(stat(t("Disk available"), bytes(sample.and_then(|s| s.disk_free)), p.text))
            .child(divider())
            .child(stat(t("Codeyard memory"), bytes(sample.and_then(|s| s.app_memory)), p.text))
            .when_some(sample.and_then(|s| s.load), |d, load| {
                d.child(divider()).child(stat(t("Load"), format!("{:.2} / {:.2} / {:.2}", load[0], load[1], load[2]), p.text))
            })
            .child(divider())
            .child(stat(t("Memory"), memory, p.text))
            .child(meter(sample.and_then(|s| s.memory).map(|(used, total)| used as f32 / total.max(1) as f32)))
            .child(divider())
            .child(stat(t("CPU"), cpu, p.text))
            .child(meter(self.cpu.map(|n| n / 100.)))
            .child(button("resources-pin", "pin", if self.pinned { t("Unpin") } else { t("Pin") }, self.pinned, &self.pin_focus).ml(px(2.)).on_click(
                cx.listener(|this, _, _, cx| {
                    this.pinned = !this.pinned;
                    this.dismiss = None;
                    cx.notify();
                }),
            ))
            .child(button("resources-close", "x", t("Close"), false, &self.close_focus).on_click(cx.listener(|this, _, _, cx| this.close(cx))))
    }
}

/// Width the strip may open to: wide enough for every figure; a narrower status bar cuts it from the left.
const STRIP_W: f32 = 900.;

impl Render for ResourceMonitor {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = *palette(cx);
        let shown = self.shown;
        let closing = self.closing;
        let keyboard = ui::keyboard_focus(cx);
        if closing && (self.pin_focus.is_focused(window) || self.close_focus.is_focused(window)) {
            window.focus(&self.focus);
        }
        div()
            .h_full()
            .min_w_0()
            .flex_shrink()
            .flex()
            .items_center()
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
            .when(shown, |d| {
                // Opens leftwards inside the status bar: the row is right-aligned in a box whose width grows from the icon.
                let strip = div()
                    .id("resource-strip")
                    .debug_selector(|| "resource-strip".into())
                    .h_full()
                    .min_w_0()
                    .overflow_hidden()
                    .flex()
                    .justify_end()
                    .items_center()
                    .bg(c(p.surface_raised))
                    .border_l_1()
                    .border_color(c(p.border))
                    .on_hover(cx.listener(|this, hovered, _, cx| this.hover(true, *hovered, cx)))
                    .child(self.strip(cx));
                if crate::system_prefs::reduce_motion(cx) {
                    return d.child(strip.when(closing, |d| d.opacity(0.)).max_w(px(STRIP_W)));
                }
                d.child(strip.with_animation(
                    if closing { "resources-exit" } else { "resources-enter" },
                    Animation::new(Duration::from_millis(if closing { 120 } else { 200 })).with_easing(gpui::ease_out_quint()),
                    move |d, delta| {
                        let open = if closing { 1. - delta } else { delta };
                        d.max_w(px(STRIP_W * open)).opacity(open.min(1.))
                    },
                ))
            })
            .child(
                div()
                    .id("sb-resources")
                    .debug_selector(|| "sb-resources".into())
                    .relative()
                    .child(crate::a11y_controls::control(
                        crate::a11y_controls::Control::button(t("Status bar"), "sb-resources", t("Resources"), |_, _| {})
                            .expanded(shown && !closing)
                            .focus(self.focus.clone())
                            .on_entity(cx.entity().downgrade(), |this: &mut Self, window, cx| this.toggle(window, cx)),
                    ))
                    .track_focus(&self.focus)
                    .tab_index(0)
                    .flex_none()
                    .h_full()
                    .w(px(32.))
                    .border_1()
                    .border_color(ui::ca(p.focus_ring, 0.))
                    .when(keyboard, |d| d.focus(|d| d.border_color(c(p.focus_ring))))
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .when(shown && !closing, |d| d.bg(c(p.surface_active)))
                    .hover(|d| d.bg(c(p.surface_hover)))
                    .active(|d| d.bg(c(p.surface_active)))
                    .child(icon("activity", 16., if shown && !closing { p.accent } else { p.text_muted }))
                    .on_hover(cx.listener(|this, hovered, _, cx| this.hover(false, *hovered, cx)))
                    .on_click(cx.listener(|this, _, window, cx| this.toggle(window, cx))),
            )
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
    use crate::ui::layout;
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
        let card = cx.debug_bounds("resource-strip").unwrap();
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
    fn the_strip_opens_leftwards_inside_the_status_bar(cx: &mut TestAppContext) {
        crate::qa_workbench_tests::globals(cx);
        // The final layout: with reduced motion the strip is drawn fully open at once.
        cx.update(|cx| cx.global_mut::<crate::settings::Settings>().reduce_motion = Some(true));
        let monitor = cx.new(ResourceMonitor::new);
        let (_fixture, cx) = cx.add_window_view(|_, _| BarFixture(monitor.clone()));
        cx.run_until_parked();
        let icon = cx.debug_bounds("sb-resources").unwrap();
        cx.simulate_click(icon.center(), gpui::Modifiers::none());
        cx.run_until_parked();
        let (strip, icon) = (cx.debug_bounds("resource-strip").unwrap(), cx.debug_bounds("sb-resources").unwrap());
        assert_eq!((strip.top(), strip.bottom()), (icon.top(), icon.bottom()), "the strip is a part of the status bar row, not a card above it");
        assert!((strip.right() - icon.left()).abs() < px(1.), "it grows from the icon: {strip:?} {icon:?}");
        assert!(strip.size.width > px(200.), "the figures are shown: {strip:?}");
        let (pin, close) = (cx.debug_bounds("resources-pin").unwrap(), cx.debug_bounds("resources-close").unwrap());
        assert!(strip.contains(&pin.center()) && strip.contains(&close.center()), "pin and close sit inside the strip");
    }

    #[gpui::test]
    fn a_narrow_status_bar_cuts_the_strip_on_the_left(cx: &mut TestAppContext) {
        crate::qa_workbench_tests::globals(cx);
        // The final layout: with reduced motion the strip is drawn fully open at once.
        cx.update(|cx| cx.global_mut::<crate::settings::Settings>().reduce_motion = Some(true));
        let monitor = cx.new(ResourceMonitor::new);
        let (_fixture, cx) = cx.add_window_view(|_, _| BarFixture(monitor.clone()));
        cx.simulate_resize(gpui::size(px(300.), px(200.)));
        cx.run_until_parked();
        let at = cx.debug_bounds("sb-resources").unwrap().center();
        cx.simulate_click(at, gpui::Modifiers::none());
        cx.run_until_parked();
        let (strip, icon) = (cx.debug_bounds("resource-strip").unwrap(), cx.debug_bounds("sb-resources").unwrap());
        assert!(strip.left() >= px(0.) && icon.right() <= px(300.), "nothing leaves the window: {strip:?} {icon:?}");
        let close = cx.debug_bounds("resources-close").unwrap();
        assert!(close.left() >= strip.left() && close.right() <= icon.left() + px(1.), "the controls next to the icon stay visible: {close:?}");
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
