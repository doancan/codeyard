//! Painted, clipped bounds of the rows of views that draw themselves (the Settings and Keyboard Shortcuts pages, context
//! menus), for the accessibility tree: each row carries a probe that records where it was painted, cut to what is
//! visible. A row scrolled out of view is not recorded, so it gets no bounds. The view clears its record when it draws
//! again, so only rows of the last frame have bounds. The workbench's own lists use `a11y::row_frame` the same way.

use std::cell::RefCell;
use std::collections::HashMap;
use std::hash::Hash;
use std::rc::Rc;

use gpui::{Bounds, IntoElement, Pixels, Window, WindowId, canvas, prelude::*, px};

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

/// As `probe`, but recorded when the row is laid out rather than painted: a control that is hidden until the pointer
/// is over its row (and so is not painted) has the place it appears at.
pub fn layout_probe<K: Eq + Hash + Clone + 'static>(sink: &RowBounds<K>, key: K) -> impl IntoElement + use<K> {
    let sink = sink.clone();
    canvas(
        move |bounds, window, _| {
            let bounds = bounds.intersect(&window.content_mask().bounds);
            if bounds.size.width > px(0.) && bounds.size.height > px(0.) {
                sink.borrow_mut().insert(key.clone(), bounds);
            }
        },
        |_, _, _, _| {},
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

/// As `RowBounds`, for a view that can be drawn in any window (the picker, a page): it also remembers the window it was
/// last drawn in, so another window's tree gets no bounds from it.
pub struct WindowBounds<K> {
    window: Rc<std::cell::Cell<Option<WindowId>>>,
    rows: RowBounds<K>,
}

impl<K> Default for WindowBounds<K> {
    fn default() -> Self {
        Self { window: Default::default(), rows: Default::default() }
    }
}

impl<K: Eq + Hash + Clone + 'static> WindowBounds<K> {
    /// The view is drawn again in `window`: forget the last frame.
    pub fn begin(&self, window: &Window) {
        self.rows.borrow_mut().clear();
        self.window.set(Some(window.window_handle().window_id()));
    }

    /// Records where `key` is painted (see `probe`).
    pub fn probe(&self, key: K) -> impl IntoElement + use<K> {
        probe(&self.rows, key)
    }

    /// `key`'s last painted bounds in physical pixels, if the view was last drawn in `window`.
    pub fn physical(&self, key: &K, window: &Window) -> Option<accesskit::Rect> {
        (self.window.get() == Some(window.window_handle().window_id())).then(|| physical(&self.rows, key, window)).flatten()
    }
}

/// A line of text as last painted, for the accessibility tree's character positions: the document index of its first
/// character, the left and right edge of each character in reading (logical) order as it was drawn (a right-to-left
/// character's right edge is where it starts), where the line ends on the right (a selected line break is drawn from
/// there), its top and bottom, and the width the line break is drawn with (in logical pixels, window coordinates).
#[derive(Clone, Debug, PartialEq)]
pub struct TextLine {
    pub start: usize,
    pub cells: Vec<(Pixels, Pixels)>,
    pub end: Pixels,
    pub top: Pixels,
    pub bottom: Pixels,
    pub line_break: Pixels,
}

/// A stretch of a painted line whose characters follow each other on screen in one direction: characters
/// `start..end` of the line, and whether they read right to left.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Stretch {
    pub start: usize,
    pub end: usize,
    pub rtl: bool,
}

/// How far apart two edges may be and still touch (shaped positions are fractional).
const TOUCH: f32 = 0.5;

impl TextLine {
    /// Characters on the line (the line break not included).
    pub fn chars(&self) -> usize {
        self.cells.len()
    }

    /// The line cut where reading order and screen order part: each stretch reads left to right with each
    /// character starting where the one before it ends, or right to left with each ending where the one before it
    /// starts. Mixed text ("abc אבג def") is three stretches; a single character, or a run of characters that do not
    /// touch, reads left to right.
    pub fn stretches(&self) -> Vec<Stretch> {
        let touch = |a: Pixels, b: Pixels| ((a - b) / px(1.)).abs() <= TOUCH;
        let mut out: Vec<Stretch> = Vec::new();
        let mut dir: Option<bool> = None;
        for k in 0..self.cells.len() {
            if k > 0 {
                let ((l0, r0), (l1, r1)) = (self.cells[k - 1], self.cells[k]);
                let (ltr, rtl) = (touch(l1, r0), touch(r1, l0));
                let goes_on = match dir {
                    Some(false) => ltr,
                    Some(true) => rtl,
                    None => ltr || rtl,
                };
                if goes_on {
                    if dir.is_none() {
                        dir = Some(!ltr && rtl);
                    }
                    if let Some(last) = out.last_mut() {
                        last.end = k + 1;
                        last.rtl = dir == Some(true);
                    }
                    continue;
                }
            }
            dir = None;
            out.push(Stretch { start: k, end: k + 1, rtl: false });
        }
        out
    }

    /// Whether the line break is drawn right after characters `..end` (they end the line on the right, read left to
    /// right).
    pub fn break_follows(&self, end: usize) -> bool {
        if end == 0 || self.cells.is_empty() {
            return true;
        }
        let ltr = self.stretches().iter().find(|s| s.start < end && end <= s.end).is_some_and(|s| !s.rtl && s.end == end);
        ltr && ((self.cells[end - 1].1 - self.end) / px(1.)).abs() <= TOUCH
    }
}

/// The left and right edge of each character of `text` as `layout` (its shaped line) draws it, in reading order and
/// relative to the line's origin. Each glyph cluster spans from its glyph to the next cluster on screen; characters
/// without a glyph of their own (a ligature, a cluster of several characters) share the cluster's span in reading
/// order. Right-to-left text, which the platform shapes in screen order, keeps the place it is drawn at.
pub fn char_cells(text: &str, layout: &gpui::LineLayout) -> Vec<(Pixels, Pixels)> {
    let glyphs = layout.runs.iter().flat_map(|r| r.glyphs.iter().map(|g| (g.position.x, g.index))).collect();
    cells_of(text, glyphs, layout.width)
}

/// `char_cells` from the glyphs' positions and byte indices (in any order) and the line's width.
fn cells_of(text: &str, mut glyphs: Vec<(Pixels, usize)>, width: Pixels) -> Vec<(Pixels, Pixels)> {
    glyphs.sort_by(|a, b| (a.0 / px(1.)).total_cmp(&(b.0 / px(1.))));
    // Clusters on screen, left to right: (byte index, left, right).
    let mut clusters: HashMap<usize, (Pixels, Pixels)> = HashMap::new();
    for (k, &(x, index)) in glyphs.iter().enumerate() {
        if clusters.contains_key(&index) {
            continue;
        }
        let right = glyphs[k + 1..].iter().find(|g| g.1 != index).map_or(width, |g| g.0).max(x);
        clusters.insert(index, (x, right));
    }
    let starts: Vec<usize> = text.char_indices().map(|(b, _)| b).collect();
    let mut cells = Vec::with_capacity(starts.len());
    let mut i = 0;
    while i < starts.len() {
        let mut j = i + 1;
        while j < starts.len() && !clusters.contains_key(&starts[j]) {
            j += 1;
        }
        let n = (j - i) as f32;
        match clusters.get(&starts[i]) {
            Some(&(left, right)) => {
                // Shared in reading order: right to left when the next cluster in reading order is drawn to the left.
                let next = starts.get(j).and_then(|b| clusters.get(b));
                let rtl = next.is_some_and(|&(l, _)| l < left);
                let w = (right - left) / n;
                for k in 0..j - i {
                    let a = if rtl { right - w * (k + 1) as f32 } else { left + w * k as f32 };
                    cells.push((a, a + w));
                }
            }
            None => {
                // Characters before any glyph (nothing drawn): no width, where the next cluster starts.
                let x = starts.get(j).and_then(|b| clusters.get(b)).map_or(width, |c| c.0);
                cells.extend(std::iter::repeat_n((x, x), j - i));
            }
        }
        i = j;
    }
    cells
}

#[cfg(test)]
#[path = "painted_tests.rs"]
mod tests;
