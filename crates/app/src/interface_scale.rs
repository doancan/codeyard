//! Interface scale (`ui.scale`): one logical zoom for everything in a window, applied by the patched GPUI window
//! (`Window::set_interface_scale`) on top of the display's own scale factor. Elements never scale themselves, so
//! the editor and terminal font sizes, hit regions and accessibility bounds all go through the same factor once.

use gpui::{App, Window};

use crate::settings::{Settings, UI_SCALE_MAX, UI_SCALE_MIN};

/// Bring the window to the current `ui.scale`.
pub fn apply(window: &mut Window, cx: &App) {
    window.set_interface_scale(cx.global::<Settings>().ui_scale);
}

/// The scale after one Zoom In (`direction` > 0) or Zoom Out (`direction` < 0) from `current`, within
/// `UI_SCALE_MIN..=UI_SCALE_MAX`. `current` may be any value from the settings file, not only one this returns.
pub fn step(current: f32, direction: i8) -> f32 {
    // Fine steps near 100%, coarse ones at the ends; 100% is always one of them.
    const STEPS: [f32; 9] = [0.75, 0.8, 0.9, 1., 1.1, 1.25, 1.5, 1.75, 2.];
    // Values a little apart from a step (f32 from the settings file) count as that step.
    const NEAR: f32 = 0.005;
    let (min, max) = (UI_SCALE_MIN as f32, UI_SCALE_MAX as f32);
    let current = if current.is_finite() { current.clamp(min, max) } else { 1. };
    let next = match direction.signum() {
        1 => STEPS.iter().copied().find(|s| *s > current + NEAR),
        -1 => STEPS.iter().rev().copied().find(|s| *s < current - NEAR),
        _ => Some(current),
    };
    next.unwrap_or(current).clamp(min, max)
}

/// "150%".
pub fn percent(scale: f32) -> String {
    format!("{}%", (scale * 100.).round() as i32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zoom_steps_stay_in_range_and_return_to_where_they_started() {
        let (min, max) = (UI_SCALE_MIN as f32, UI_SCALE_MAX as f32);
        let mut s = 1.;
        for _ in 0..50 {
            s = step(s, 1);
        }
        assert_eq!(s, max);
        for _ in 0..50 {
            s = step(s, -1);
        }
        assert_eq!(s, min);
        assert!(step(1., 1) > 1. && step(1., -1) < 1.);
        assert_eq!(step(step(1., 1), -1), 1.);
        // A value typed into the settings file moves to a neighbour, never away from it.
        assert!(step(1.33, 1) > 1.33 && step(1.33, -1) < 1.33);
    }

    type Scrolled = std::rc::Rc<std::cell::Cell<Option<(gpui::Point<gpui::Pixels>, gpui::ScrollDelta)>>>;

    /// Fills the window and records the last scroll it receives.
    struct Probe(Scrolled);
    impl gpui::Render for Probe {
        fn render(&mut self, _: &mut Window, _: &mut gpui::Context<Self>) -> impl gpui::IntoElement {
            use gpui::{InteractiveElement, Styled};
            let sink = self.0.clone();
            gpui::div().id("probe").size_full().on_scroll_wheel(move |e, _, _| sink.set(Some((e.position, e.delta))))
        }
    }

    /// The patched window divides platform positions by the scale and multiplies the device factor by it, so a point
    /// on screen and the element drawn there agree at any zoom.
    #[gpui::test]
    fn platform_positions_and_device_scale_go_through_the_interface_scale(cx: &mut gpui::TestAppContext) {
        use gpui::{Modifiers, ScrollDelta, ScrollWheelEvent, point, px};
        let seen = Scrolled::default();
        let sink = seen.clone();
        let (_, cx) = cx.add_window_view(move |_, _| Probe(sink));
        let (device, viewport) = cx.update(|window, _| (window.scale_factor(), window.viewport_size()));
        cx.update(|window, _| window.set_interface_scale(2.));
        cx.simulate_mouse_move(point(px(200.), px(100.)), None, Modifiers::default());
        cx.update(|window, _| {
            assert_eq!(window.mouse_position(), point(px(100.), px(50.)));
            assert_eq!(window.scale_factor(), device * 2.);
            assert_eq!(window.viewport_size(), viewport.map(|v| v * 0.5));
        });
        // Scrolling by pixels keeps the content under the fingers; lines stay lines.
        cx.run_until_parked();
        cx.simulate_event(ScrollWheelEvent { position: point(px(40.), px(60.)), delta: ScrollDelta::Pixels(point(px(0.), px(-20.))), ..Default::default() });
        let (at, delta) = seen.take().expect("the scroll reaches the element under the pointer");
        assert_eq!(at, point(px(20.), px(30.)));
        assert!(matches!(delta, ScrollDelta::Pixels(d) if d == point(px(0.), px(-10.))), "{delta:?}");
        cx.simulate_event(ScrollWheelEvent { position: point(px(40.), px(60.)), delta: ScrollDelta::Lines(point(0., -3.)), ..Default::default() });
        let (_, delta) = seen.take().expect("the scroll reaches the element under the pointer");
        assert!(matches!(delta, ScrollDelta::Lines(d) if d == point(0., -3.)), "{delta:?}");
        // Invalid scales fall back to 100%.
        for bad in [f32::NAN, f32::INFINITY, 0., -1.] {
            cx.update(|window, _| window.set_interface_scale(bad));
            assert_eq!(cx.update(|window, _| (window.interface_scale(), window.scale_factor())), (1., device));
        }
    }

    #[test]
    fn percent_rounds_to_whole_numbers() {
        assert_eq!(percent(1.), "100%");
        assert_eq!(percent(1.25), "125%");
        assert_eq!(percent(0.75), "75%");
    }
}
