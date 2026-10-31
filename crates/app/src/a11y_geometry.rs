//! Painted landmark, row and control bounds, converted from GPUI logical pixels to AccessKit physical pixels.

use std::collections::HashMap;

use accesskit::{Node, NodeId, Rect};
use gpui::{Bounds, Context, IntoElement, Pixels, Window, WindowId, canvas, point, prelude::*, px};

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

/// A control of the workbench's own chrome with a frame of its own in the accessibility tree.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub(in crate::workbench) enum Part {
    /// An item of the status bar.
    Status(StatusPart),
    /// A tab of the focused editor group, by its index among the group's editor tabs.
    EditorTab(usize),
    /// A row of the Problems panel.
    Problem(usize),
    /// A button inside a row of a side view list: the row, and which of the row's buttons (in the tree's order).
    RowButton(RowFrame, usize, u8),
}

/// The status bar's items that the tree lists.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub(in crate::workbench) enum StatusPart {
    Remote,
    Branch,
    Problems,
    Position,
    Indent,
    LineEnding,
    Language,
    Encoding,
    Update,
}

/// Logical bounds of one window's most recent frame; absent landmarks, rows and controls were not laid out (a row
/// scrolled out of its list is not drawn at all).
#[derive(Debug, Default)]
struct Frames {
    explorer: Option<Bounds<Pixels>>,
    editor: Option<Bounds<Pixels>>,
    status: Option<Bounds<Pixels>>,
    rows: HashMap<(RowFrame, usize), Bounds<Pixels>>,
    parts: HashMap<Part, Bounds<Pixels>>,
}

/// What the workbench drew, per window: a side view in a window of its own is drawn by the workbench too, in that
/// window's coordinates, so each window keeps its own record. A window's record is cleared when that window draws the
/// workbench's part again.
#[derive(Debug, Default)]
pub(in crate::workbench) struct PaintedBounds {
    windows: HashMap<WindowId, Frames>,
}

impl PaintedBounds {
    /// Forget what `window` drew (it is about to draw again).
    pub(in crate::workbench) fn clear(&mut self, window: WindowId) {
        self.windows.remove(&window);
    }

    /// Keep only the records of these windows (the others are closed).
    pub(in crate::workbench) fn retain(&mut self, open: &[WindowId]) {
        self.windows.retain(|id, _| open.contains(id));
    }

    /// The visible part of a row as last painted in `window`.
    pub(in crate::workbench) fn row(&self, window: WindowId, which: RowFrame, index: usize) -> Option<Bounds<Pixels>> {
        self.windows.get(&window)?.rows.get(&(which, index)).copied()
    }

    fn frames(&mut self, window: WindowId) -> &mut Frames {
        self.windows.entry(window).or_default()
    }
}

#[cfg(test)]
impl PaintedBounds {
    /// The logical bounds the landmark was painted at in the last frame of `window` (tests).
    pub(in crate::workbench) fn get(&self, window: WindowId, which: Landmark) -> Option<Bounds<Pixels>> {
        let frames = self.windows.get(&window)?;
        match which {
            Landmark::Explorer => frames.explorer,
            Landmark::Editor => frames.editor,
            Landmark::Status => frames.status,
        }
    }

    /// The visible part of a control as last laid out in `window` (tests).
    pub(in crate::workbench) fn part(&self, window: WindowId, which: Part) -> Option<Bounds<Pixels>> {
        self.windows.get(&window)?.parts.get(&which).copied()
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
    parts: HashMap<Part, Rect>,
    /// The status bar's items in the tree's order (`None`: not drawn at this width).
    status_items: Vec<Option<Rect>>,
}

impl Geometry {
    /// What the workbench last drew in `window`, and the window itself.
    pub(super) fn new(painted: &PaintedBounds, window: &Window) -> Self {
        let scale = window.scale_factor();
        let whole = physical_bounds(Bounds::new(point(px(0.), px(0.)), window.viewport_size()), scale);
        let Some(frames) = painted.windows.get(&window.window_handle().window_id()) else {
            return Self { window: whole, ..Default::default() };
        };
        Self {
            window: whole,
            explorer: frames.explorer.and_then(|bounds| physical_bounds(bounds, scale)),
            editor: frames.editor.and_then(|bounds| physical_bounds(bounds, scale)),
            status: frames.status.and_then(|bounds| physical_bounds(bounds, scale)),
            rows: frames.rows.iter().filter_map(|(key, bounds)| Some((*key, physical_bounds(*bounds, scale)?))).collect(),
            parts: frames.parts.iter().filter_map(|(key, bounds)| Some((*key, physical_bounds(*bounds, scale)?))).collect(),
            status_items: Vec::new(),
        }
    }

    /// The bounds of a row, while it is painted.
    pub(super) fn apply_row(&self, which: RowFrame, index: usize, node: &mut Node) {
        if let Some(bounds) = self.rows.get(&(which, index)) {
            node.set_bounds(*bounds);
        }
    }

    /// The buttons among `nodes` (a row's nodes, from its first): a button with id `base + k` gets where button `k` is
    /// laid out (a button shown while the pointer is over its row is laid out while hidden), another (the row's "More
    /// Actions…") where `MENU_BUTTON` is. A button with no control of its own on screen (a row whose menu opens only
    /// with a right click) has no frame: the row's would hide the row from hit testing.
    pub(super) fn apply_row_buttons(&self, which: RowFrame, index: usize, base: u64, nodes: &mut [(NodeId, Node)]) {
        for (id, node) in nodes.iter_mut().filter(|(_, n)| n.role() == accesskit::Role::Button && n.bounds().is_none()) {
            let k = id.0.checked_sub(base).filter(|k| (1..=u64::from(u8::MAX)).contains(k)).map_or(MENU_BUTTON, |k| k as u8);
            if let Some(bounds) = self.parts.get(&Part::RowButton(which, index, k)) {
                node.set_bounds(*bounds);
            }
        }
    }

    /// Whether the focused editor was drawn in this window's last frame.
    pub(super) fn editor_painted(&self) -> bool {
        self.editor.is_some()
    }

    /// The bounds of a control, while it is drawn.
    pub(super) fn apply_part(&self, which: Part, node: &mut Node) {
        if let Some(bounds) = self.parts.get(&which) {
            node.set_bounds(*bounds);
        }
    }

    /// The physical frame of a control, while it is drawn.
    pub(super) fn part(&self, which: Part) -> Option<Rect> {
        self.parts.get(&which).copied()
    }

    /// The status bar's items, in the tree's order.
    pub(super) fn set_status_items(&mut self, items: &[StatusPart]) {
        self.status_items = items.iter().map(|item| self.parts.get(&Part::Status(*item)).copied()).collect();
    }

    /// The frame of status item `index` (in the tree's order), while it is drawn.
    pub(super) fn status_item(&self, index: usize) -> Option<Rect> {
        self.status_items.get(index).copied().flatten()
    }

    /// A window that shows one editor and nothing else: the editor fills it.
    pub(super) fn whole_window(window: &Window) -> Self {
        let whole = physical_bounds(Bounds::new(point(px(0.), px(0.)), window.viewport_size()), window.scale_factor());
        Self { window: whole, editor: whole, ..Default::default() }
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
    crate::painted::physical_scaled(bounds, scale)
}

/// Capture layout without drawing, taking focus, intercepting input or requesting another frame.
pub(in crate::workbench) fn landmark(which: Landmark, cx: &Context<Workbench>) -> impl IntoElement + use<> {
    let entity = cx.entity().downgrade();
    canvas(
        |bounds, window, _| bounds.intersect(&window.content_mask().bounds),
        move |_, bounds, window, cx| {
            let id = window.window_handle().window_id();
            entity
                .update(cx, |wb, _| {
                    let frames = wb.chrome.a11y_bounds.frames(id);
                    let target = match which {
                        Landmark::Explorer => &mut frames.explorer,
                        Landmark::Editor => &mut frames.editor,
                        Landmark::Status => &mut frames.status,
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
        move |_, bounds, window, cx| {
            if bounds.size.width <= px(0.) || bounds.size.height <= px(0.) {
                return;
            }
            let id = window.window_handle().window_id();
            entity
                .update(cx, |wb, _| {
                    wb.chrome.a11y_bounds.frames(id).rows.insert((which, index), bounds);
                })
                .ok();
        },
    )
    .absolute()
    .top_0()
    .left_0()
    .size_full()
}

/// A control (its parent must be `relative`): capture its visible bounds when it is laid out. The record is made
/// before painting, so a control that is laid out but hidden until the pointer hovers its row (a row's buttons) has
/// the place it appears at.
pub(in crate::workbench) fn part_frame(which: Part, cx: &Context<Workbench>) -> impl IntoElement + use<> {
    let entity = cx.entity().downgrade();
    canvas(
        move |bounds, window, cx| {
            let bounds = bounds.intersect(&window.content_mask().bounds);
            if bounds.size.width > px(0.) && bounds.size.height > px(0.) {
                let id = window.window_handle().window_id();
                entity
                    .update(cx, |wb, _| {
                        wb.chrome.a11y_bounds.frames(id).parts.insert(which, bounds);
                    })
                    .ok();
            }
        },
        |_, _, _, _| {},
    )
    .absolute()
    .top_0()
    .left_0()
    .size_full()
}

/// Button `k` of a row (the offset of its node id from the row's, or `MENU_BUTTON` for its "More Actions…" button),
/// wrapped so where it is laid out is recorded.
pub(in crate::workbench) fn row_button(el: impl IntoElement, which: RowFrame, index: usize, k: u8, cx: &Context<Workbench>) -> gpui::Div {
    gpui::div().relative().flex_none().child(el).child(part_frame(Part::RowButton(which, index, k), cx))
}

/// The row button that opens the row's menu.
pub(in crate::workbench) const MENU_BUTTON: u8 = 0;

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
