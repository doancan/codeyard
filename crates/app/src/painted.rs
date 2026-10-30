//! Painted, clipped bounds of the rows of views that draw themselves (the Settings and Keyboard Shortcuts pages, context
//! menus), for the accessibility tree: each row carries a probe that records where it was painted, cut to what is
//! visible. A row scrolled out of view is not recorded, so it gets no bounds. The view clears its record when it draws
//! again, so only rows of the last frame have bounds. The workbench's own lists use `a11y::row_frame` the same way.

use std::cell::RefCell;
use std::collections::HashMap;
use std::hash::Hash;
use std::rc::Rc;

use gpui::{Bounds, IntoElement, Pixels, Window, canvas, prelude::*, px};

/// Where each row (by key) was painted in the last frame, in logical pixels.
pub type RowBounds<K> = Rc<RefCell<HashMap<K, Bounds<Pixels>>>>;

/// Records where the row `key` is painted (its parent must be `relative`). Takes no space, input or focus.
pub fn probe<K: Eq + Hash + Clone + 'static>(sink: &RowBounds<K>, key: K) -> impl IntoElement + use<K> {
    let sink = sink.clone();
    canvas(
        |bounds, window, _| bounds.intersect(&window.content_mask().bounds),
        move |_, bounds, _, _| {
            if bounds.size.width > px(0.) && bounds.size.height > px(0.) {
                sink.borrow_mut().insert(key.clone(), bounds);
            }
        },
    )
    .absolute()
    .top_0()
    .left_0()
    .size_full()
}

/// Logical bounds in AccessKit's physical pixels for a window drawn at `scale`; `None` for an empty or broken rectangle.
pub fn physical_scaled(bounds: Bounds<Pixels>, scale: f32) -> Option<accesskit::Rect> {
    if !scale.is_finite() || scale <= 0. {
        return None;
    }
    let scale = f64::from(scale);
    let rect = accesskit::Rect::new(
        f64::from(bounds.left() / px(1.)) * scale,
        f64::from(bounds.top() / px(1.)) * scale,
        f64::from(bounds.right() / px(1.)) * scale,
        f64::from(bounds.bottom() / px(1.)) * scale,
    );
    (rect.width() > 0. && rect.height() > 0. && [rect.x0, rect.y0, rect.x1, rect.y1].into_iter().all(f64::is_finite)).then_some(rect)
}

/// The row `key`'s last painted bounds in physical pixels of `window`.
pub fn physical<K: Eq + Hash>(sink: &RowBounds<K>, key: &K, window: &Window) -> Option<accesskit::Rect> {
    sink.borrow().get(key).and_then(|b| physical_scaled(*b, window.scale_factor()))
}
