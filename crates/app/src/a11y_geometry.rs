//! Painted landmark and row bounds, converted from GPUI logical pixels to AccessKit physical pixels.

use std::collections::HashMap;

use accesskit::{Node, NodeId, Rect};
use gpui::{Bounds, Context, IntoElement, Pixels, Window, canvas, point, prelude::*, px};

use super::{EDITOR, EXPLORER, ROOT, STATUS, Workbench};

#[derive(Clone, Copy, Debug)]
pub(in crate::workbench) enum Landmark {
    Explorer,
    Editor,
    Status,
}

/// A row of a virtualized side view or panel list, by its index in the list it is drawn in (the same index as in the
/// accessibility tree).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub(in crate::workbench) enum RowFrame {
    DebugVariable,
    DebugWatch,
    DebugStack,
    DebugBreakpoint,
    Test,
    Remote,
    Port,
}

/// Logical bounds from the most recent workbench frame; absent landmarks and rows were not painted (a row scrolled out
/// of its list is not drawn at all).
#[derive(Debug, Default)]
pub(in crate::workbench) struct PaintedBounds {
    explorer: Option<Bounds<Pixels>>,
    editor: Option<Bounds<Pixels>>,
    status: Option<Bounds<Pixels>>,
    rows: HashMap<(RowFrame, usize), Bounds<Pixels>>,
}

impl PaintedBounds {
    /// The visible part of a row as last painted.
    pub(in crate::workbench) fn row(&self, which: RowFrame, index: usize) -> Option<Bounds<Pixels>> {
        self.rows.get(&(which, index)).copied()
    }
}

#[cfg(test)]
impl PaintedBounds {
    /// The logical bounds the landmark was painted at in the last frame (tests).
    pub(in crate::workbench) fn get(&self, which: Landmark) -> Option<Bounds<Pixels>> {
        match which {
            Landmark::Explorer => self.explorer,
            Landmark::Editor => self.editor,
            Landmark::Status => self.status,
        }
    }
}

/// Physical coordinates relative to the native content view, with y increasing downwards.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Geometry {
    window: Option<Rect>,
    explorer: Option<Rect>,
    editor: Option<Rect>,
    status: Option<Rect>,
    rows: HashMap<(RowFrame, usize), Rect>,
}

impl Geometry {
    pub(super) fn new(painted: &PaintedBounds, window: &Window) -> Self {
        let scale = window.scale_factor();
        Self {
            window: physical_bounds(Bounds::new(point(px(0.), px(0.)), window.viewport_size()), scale),
            explorer: painted.explorer.and_then(|bounds| physical_bounds(bounds, scale)),
            editor: painted.editor.and_then(|bounds| physical_bounds(bounds, scale)),
            status: painted.status.and_then(|bounds| physical_bounds(bounds, scale)),
            rows: painted.rows.iter().filter_map(|(key, bounds)| Some((*key, physical_bounds(*bounds, scale)?))).collect(),
        }
    }

    /// The bounds of a row, while it is painted.
    pub(super) fn apply_row(&self, which: RowFrame, index: usize, node: &mut Node) {
        if let Some(bounds) = self.rows.get(&(which, index)) {
            node.set_bounds(*bounds);
        }
    }

    pub(super) fn apply(&self, id: NodeId, node: &mut Node) {
        let bounds = match id {
            ROOT => self.window,
            EXPLORER => self.explorer,
            EDITOR => self.editor,
            STATUS => self.status,
            _ => None,
        };
        if let Some(bounds) = bounds {
            node.set_bounds(bounds);
        }
    }
}

fn physical_bounds(bounds: Bounds<Pixels>, scale: f32) -> Option<Rect> {
    if !scale.is_finite() || scale <= 0. {
        return None;
    }
    let scale = f64::from(scale);
    let rect = Rect::new(
        f64::from(bounds.left() / px(1.)) * scale,
        f64::from(bounds.top() / px(1.)) * scale,
        f64::from(bounds.right() / px(1.)) * scale,
        f64::from(bounds.bottom() / px(1.)) * scale,
    );
    (rect.width() > 0. && rect.height() > 0. && [rect.x0, rect.y0, rect.x1, rect.y1].into_iter().all(f64::is_finite)).then_some(rect)
}

/// Capture layout without drawing, taking focus, intercepting input or requesting another frame.
pub(in crate::workbench) fn landmark(which: Landmark, cx: &Context<Workbench>) -> impl IntoElement + use<> {
    let entity = cx.entity().downgrade();
    canvas(
        |bounds, window, _| bounds.intersect(&window.content_mask().bounds),
        move |_, bounds, _, cx| {
            entity
                .update(cx, |wb, _| {
                    let target = match which {
                        Landmark::Explorer => &mut wb.chrome.a11y_bounds.explorer,
                        Landmark::Editor => &mut wb.chrome.a11y_bounds.editor,
                        Landmark::Status => &mut wb.chrome.a11y_bounds.status,
                    };
                    *target = Some(bounds);
                })
                .ok();
        },
    )
    .absolute()
    .top_0()
    .left_0()
    .size_full()
}

/// A row drawn in a list: capture its visible bounds the same way (an empty intersection, a row clipped away, is not
/// recorded).
pub(in crate::workbench) fn row_frame(which: RowFrame, index: usize, cx: &Context<Workbench>) -> impl IntoElement + use<> {
    let entity = cx.entity().downgrade();
    canvas(
        |bounds, window, _| bounds.intersect(&window.content_mask().bounds),
        move |_, bounds, _, cx| {
            if bounds.size.width <= px(0.) || bounds.size.height <= px(0.) {
                return;
            }
            entity
                .update(cx, |wb, _| {
                    wb.chrome.a11y_bounds.rows.insert((which, index), bounds);
                })
                .ok();
        },
    )
    .absolute()
    .top_0()
    .left_0()
    .size_full()
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::size;

    #[test]
    fn accessibility_bounds_follow_display_scale() {
        let logical = Bounds::new(point(px(12.5), px(30.)), size(px(100.), px(80.)));
        for scale in [1., 1.5, 2.] {
            let rect = physical_bounds(logical, scale).unwrap();
            let scale = f64::from(scale);
            assert_eq!(rect, Rect::new(12.5 * scale, 30. * scale, 112.5 * scale, 110. * scale));
        }
    }

    #[test]
    fn accessibility_bounds_reject_empty_and_non_finite_layout() {
        for bounds in [Bounds::default(), Bounds::new(point(px(0.), px(0.)), size(px(f32::INFINITY), px(20.)))] {
            assert!(physical_bounds(bounds, 2.).is_none(), "bounds {bounds:?}");
        }
        let logical = Bounds::new(point(px(0.), px(0.)), size(px(100.), px(80.)));
        for scale in [0., -1., f32::NAN, f32::INFINITY] {
            assert!(physical_bounds(logical, scale).is_none(), "scale {scale}");
        }
    }
}
