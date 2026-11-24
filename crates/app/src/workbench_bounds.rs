//! Saved window bounds: kept here so `workbench.rs` stays within its size budget.

use gpui::px;
use state::State;

use super::state_path;

pub(crate) const WINDOW_BOUNDS_KEY: &str = "window.bounds";

/// `"windowed x y w h"`, `"maximized …"`, `"fullscreen …"` (the bounds are the size to restore to).
pub(crate) fn format_bounds(b: gpui::WindowBounds) -> String {
    let (kind, r) = match b {
        gpui::WindowBounds::Windowed(r) => ("windowed", r),
        gpui::WindowBounds::Maximized(r) => ("maximized", r),
        gpui::WindowBounds::Fullscreen(r) => ("fullscreen", r),
    };
    let f = |p: gpui::Pixels| f32::from(p).round();
    format!("{kind} {} {} {} {}", f(r.origin.x), f(r.origin.y), f(r.size.width), f(r.size.height))
}

pub(crate) fn parse_bounds(s: &str) -> Option<gpui::WindowBounds> {
    let mut it = s.split_whitespace();
    let kind = it.next()?;
    let n: Vec<f32> = it.map(|x| x.parse().ok()).collect::<Option<_>>()?;
    let [x, y, w, h] = n[..] else { return None };
    if w < 200. || h < 150. {
        return None;
    }
    let r = gpui::Bounds::new(gpui::point(px(x), px(y)), gpui::size(px(w), px(h)));
    Some(match kind {
        "windowed" => gpui::WindowBounds::Windowed(r),
        "maximized" => gpui::WindowBounds::Maximized(r),
        "fullscreen" => gpui::WindowBounds::Fullscreen(r),
        _ => return None,
    })
}

/// Whether the saved window is usable on this screen: if the display was unplugged or the resolution
/// changed, the window must not open as a sliver at the screen edge. A grabbable part
/// of the title bar (200×100 px) has to be visible.
pub(crate) fn visible_enough(display: gpui::Bounds<gpui::Pixels>, window: gpui::Bounds<gpui::Pixels>) -> bool {
    let top = gpui::Bounds::new(window.origin, gpui::size(window.size.width, px(100.).min(window.size.height)));
    let seen = display.intersect(&top);
    seen.size.width >= px(200.) && seen.size.height >= px(30.) && display.intersect(&window).size.height >= px(100.)
}

/// Last saved window bounds (the first window opens with these).
pub fn saved_window_bounds() -> Option<gpui::WindowBounds> {
    State::open(&state_path()?).ok()?.get(WINDOW_BOUNDS_KEY).as_deref().and_then(parse_bounds)
}

/// Where a new window opens. On macOS it fills the screen (zoomed, the menu bar and Dock stay visible; `default` is the size it
/// returns to), unless the last window was in full screen. Elsewhere the last window's bounds, or `default`, as before.
pub fn initial_bounds(saved: Option<gpui::WindowBounds>, default: gpui::Bounds<gpui::Pixels>, fill_screen: bool) -> gpui::WindowBounds {
    match (saved, fill_screen) {
        (Some(gpui::WindowBounds::Fullscreen(b)), _) => gpui::WindowBounds::Fullscreen(b),
        (Some(gpui::WindowBounds::Maximized(b)), _) => gpui::WindowBounds::Maximized(b),
        (Some(gpui::WindowBounds::Windowed(b)), false) => gpui::WindowBounds::Windowed(b),
        // The size to return to is the one the user last chose, when there is one.
        (Some(gpui::WindowBounds::Windowed(b)), true) => gpui::WindowBounds::Maximized(b),
        (None, true) => gpui::WindowBounds::Maximized(default),
        (None, false) => gpui::WindowBounds::Windowed(default),
    }
}

#[cfg(test)]
mod initial_bounds_tests {
    use super::*;
    use gpui::{Bounds, WindowBounds, point, size};

    fn b(w: f32) -> Bounds<gpui::Pixels> {
        Bounds::new(point(px(10.), px(10.)), size(px(w), px(800.)))
    }

    #[test]
    fn on_a_mac_new_windows_fill_the_screen_unless_the_last_one_was_full_screen() {
        assert_eq!(initial_bounds(None, b(1280.), true), WindowBounds::Maximized(b(1280.)));
        assert_eq!(
            initial_bounds(Some(WindowBounds::Windowed(b(900.))), b(1280.), true),
            WindowBounds::Maximized(b(900.)),
            "it returns to the size the user chose"
        );
        assert_eq!(initial_bounds(Some(WindowBounds::Fullscreen(b(900.))), b(1280.), true), WindowBounds::Fullscreen(b(900.)));
        assert_eq!(initial_bounds(Some(WindowBounds::Maximized(b(900.))), b(1280.), true), WindowBounds::Maximized(b(900.)));
    }

    #[test]
    fn elsewhere_the_saved_bounds_or_the_default_are_used_as_they_are() {
        assert_eq!(initial_bounds(None, b(1280.), false), WindowBounds::Windowed(b(1280.)));
        assert_eq!(initial_bounds(Some(WindowBounds::Windowed(b(900.))), b(1280.), false), WindowBounds::Windowed(b(900.)));
    }
}
