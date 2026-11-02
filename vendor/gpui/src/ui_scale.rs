//! Logical interface scale (Codeyard patch, see PATCHES.md).
//!
//! A window has two scales. The display scale factor comes from the platform (2.0 on Retina) and turns platform points
//! into device pixels. The interface scale is chosen by the application ([`App::set_ui_scale`](crate::App::set_ui_scale))
//! and makes one logical pixel cover `ui_scale` platform points. GPUI applies it at a single boundary:
//!
//! - rendering uses `display scale × ui_scale` as the window's [`scale_factor`](crate::Window::scale_factor), so every
//!   quad, glyph, image and path is rasterised once at the final size;
//! - the logical viewport is the platform content size divided by `ui_scale`, so layout reflows instead of overflowing;
//! - positions arriving from the platform (mouse, scroll, file drops, IME hit tests) are divided by it, and positions
//!   handed back to the platform (IME candidate bounds, window content size, the window menu, client insets) are
//!   multiplied by it.
//!
//! Everything inside the window, hit testing included, therefore works in one logical coordinate space, and a scale of
//! 1.0 leaves every value bit-for-bit unchanged. Window bounds in screen coordinates (`Window::bounds`,
//! `Window::window_bounds`, `WindowOptions::window_bounds`, minimum window sizes) are platform values and are not scaled.

use crate::{Bounds, FileDropEvent, Pixels, PlatformInput, Point, ScrollDelta, Size};

/// Smallest interface scale GPUI accepts; applications usually allow a narrower range.
pub const MIN_UI_SCALE: f32 = 0.25;
/// Largest interface scale GPUI accepts.
pub const MAX_UI_SCALE: f32 = 4.0;

/// A usable interface scale: non-finite or non-positive values become 1.0, the rest is clamped to
/// [`MIN_UI_SCALE`]..=[`MAX_UI_SCALE`].
pub fn normalize_ui_scale(scale: f32) -> f32 {
    if !scale.is_finite() || scale <= 0. {
        return 1.;
    }
    scale.clamp(MIN_UI_SCALE, MAX_UI_SCALE)
}

/// Platform points → logical pixels.
pub fn ui_to_logical(value: Pixels, ui_scale: f32) -> Pixels {
    if ui_scale == 1. { value } else { value / ui_scale }
}

/// Logical pixels → platform points.
pub fn ui_to_platform(value: Pixels, ui_scale: f32) -> Pixels {
    if ui_scale == 1. { value } else { value * ui_scale }
}

/// A platform position in the window as a logical position.
pub fn ui_point_to_logical(point: Point<Pixels>, ui_scale: f32) -> Point<Pixels> {
    point.map(|v| ui_to_logical(v, ui_scale))
}

/// A logical position as a platform position in the window.
pub fn ui_point_to_platform(point: Point<Pixels>, ui_scale: f32) -> Point<Pixels> {
    point.map(|v| ui_to_platform(v, ui_scale))
}

/// A platform size (the window's content size) as a logical size.
pub fn ui_size_to_logical(size: Size<Pixels>, ui_scale: f32) -> Size<Pixels> {
    size.map(|v| ui_to_logical(v, ui_scale))
}

/// A logical size as a platform size.
pub fn ui_size_to_platform(size: Size<Pixels>, ui_scale: f32) -> Size<Pixels> {
    size.map(|v| ui_to_platform(v, ui_scale))
}

/// Logical bounds (an IME caret, a text range) as platform bounds in the window.
pub fn ui_bounds_to_platform(bounds: Bounds<Pixels>, ui_scale: f32) -> Bounds<Pixels> {
    bounds.map(|v| ui_to_platform(v, ui_scale))
}

impl PlatformInput {
    /// The event with every platform position converted to logical pixels. Pixel scroll deltas are converted too, so
    /// content follows the finger at any scale; line deltas and keyboard events are unchanged.
    pub fn to_logical(self, ui_scale: f32) -> Self {
        if ui_scale == 1. {
            return self;
        }
        let pt = |p: Point<Pixels>| ui_point_to_logical(p, ui_scale);
        match self {
            PlatformInput::MouseDown(mut e) => {
                e.position = pt(e.position);
                PlatformInput::MouseDown(e)
            }
            PlatformInput::MouseUp(mut e) => {
                e.position = pt(e.position);
                PlatformInput::MouseUp(e)
            }
            PlatformInput::MouseMove(mut e) => {
                e.position = pt(e.position);
                PlatformInput::MouseMove(e)
            }
            PlatformInput::MouseExited(mut e) => {
                e.position = pt(e.position);
                PlatformInput::MouseExited(e)
            }
            PlatformInput::ScrollWheel(mut e) => {
                e.position = pt(e.position);
                if let ScrollDelta::Pixels(delta) = e.delta {
                    e.delta = ScrollDelta::Pixels(pt(delta));
                }
                PlatformInput::ScrollWheel(e)
            }
            PlatformInput::FileDrop(drop) => PlatformInput::FileDrop(match drop {
                FileDropEvent::Entered { position, paths } => FileDropEvent::Entered { position: pt(position), paths },
                FileDropEvent::Pending { position } => FileDropEvent::Pending { position: pt(position) },
                FileDropEvent::Submit { position } => FileDropEvent::Submit { position: pt(position) },
                FileDropEvent::Exited => FileDropEvent::Exited,
            }),
            event @ (PlatformInput::KeyDown(_) | PlatformInput::KeyUp(_) | PlatformInput::ModifiersChanged(_)) => event,
        }
    }
}
