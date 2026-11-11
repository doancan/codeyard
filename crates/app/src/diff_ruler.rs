//! The diff's overview ruler: a narrow lane at the right edge with a marker for every change, in proportion to the
//! list, and a thumb for the part on screen. This file is the geometry; the view draws it (`diff_chrome`).

use std::ops::Range;

use crate::git::{DiffRow, InlineRow};

/// What a marker says about its rows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Added,
    Removed,
    /// Old and new text on the same row: drawn half red, half green.
    Modified,
}

/// A run of list items of one kind.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Span {
    pub items: Range<usize>,
    pub kind: Kind,
}

pub fn kind_of_row(row: &DiffRow) -> Option<Kind> {
    match row {
        DiffRow::Same(..) => None,
        DiffRow::Changed(Some(_), Some(_)) => Some(Kind::Modified),
        DiffRow::Changed(Some(_), None) => Some(Kind::Removed),
        DiffRow::Changed(None, _) => Some(Kind::Added),
    }
}

pub fn kind_of_inline(row: &InlineRow) -> Option<Kind> {
    match row {
        InlineRow::Same(..) => None,
        InlineRow::Removed(_) => Some(Kind::Removed),
        InlineRow::Added(_) => Some(Kind::Added),
    }
}

/// Runs of equal kinds in `kinds` (one per list item, `None` for an unchanged item).
pub fn spans(kinds: impl IntoIterator<Item = Option<Kind>>) -> Vec<Span> {
    let mut out: Vec<Span> = Vec::new();
    for (ix, kind) in kinds.into_iter().enumerate() {
        let Some(kind) = kind else { continue };
        match out.last_mut() {
            Some(last) if last.kind == kind && last.items.end == ix => last.items.end = ix + 1,
            _ => out.push(Span { items: ix..ix + 1, kind }),
        }
    }
    out
}

/// A range of `total` items as a fraction of the lane: `(top, height)`, both in 0..=1.
pub fn fraction(items: &Range<usize>, total: usize) -> (f32, f32) {
    if total == 0 {
        return (0., 0.);
    }
    let top = (items.start.min(total) as f32 / total as f32).clamp(0., 1.);
    let height = (items.len() as f32 / total as f32).clamp(0., 1. - top);
    (top, height)
}

/// The thumb for a list scrolled by `offset` (0 at the top, negative below it, as the scroll handle says) in a viewport
/// of `viewport` px over `content` px: `(top, height)` as fractions of the lane. None when everything fits or nothing
/// has been laid out yet.
pub fn thumb(offset: f32, viewport: f32, content: f32) -> Option<(f32, f32)> {
    if viewport <= 0. || content <= viewport {
        return None;
    }
    let height = (viewport / content).clamp(0., 1.);
    let top = (-offset / content).clamp(0., 1. - height);
    Some((top, height))
}

/// The scroll offset that centers the point `fraction` (0..=1 down the lane) in the viewport, within the list.
pub fn offset_for(fraction: f32, viewport: f32, content: f32) -> f32 {
    let max = (content - viewport).max(0.);
    -(fraction.clamp(0., 1.) * content - viewport / 2.).clamp(0., max)
}

/// How far the list is scrolled: 0 at the top, 1 at the end (the slider's value).
pub fn progress(offset: f32, viewport: f32, content: f32) -> f32 {
    let room = content - viewport;
    if room <= 0. { 0. } else { (-offset / room).clamp(0., 1.) }
}

/// The scroll offset for a `progress` between the top (0) and the end (1).
pub fn offset_at(progress: f32, viewport: f32, content: f32) -> f32 {
    -(progress.clamp(0., 1.) * (content - viewport).max(0.))
}

/// How far down the lane (0..=1) the pointer at `y` is, for a lane from `top` of height `height`.
pub fn fraction_at(y: f32, top: f32, height: f32) -> f32 {
    if height <= 0. { 0. } else { ((y - top) / height).clamp(0., 1.) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rows_say_what_changed() {
        assert_eq!(kind_of_row(&DiffRow::Same(0, 0)), None);
        assert_eq!(kind_of_row(&DiffRow::Changed(Some(1), None)), Some(Kind::Removed));
        assert_eq!(kind_of_row(&DiffRow::Changed(None, Some(1))), Some(Kind::Added));
        assert_eq!(kind_of_row(&DiffRow::Changed(Some(1), Some(1))), Some(Kind::Modified));
        assert_eq!(kind_of_inline(&InlineRow::Removed(0)), Some(Kind::Removed));
        assert_eq!(kind_of_inline(&InlineRow::Added(0)), Some(Kind::Added));
        assert_eq!(kind_of_inline(&InlineRow::Same(0, 0)), None);
    }

    #[test]
    fn neighbours_of_one_kind_make_one_marker() {
        use Kind::*;
        let list = [None, Some(Added), Some(Added), None, Some(Added), Some(Removed), Some(Removed)];
        assert_eq!(spans(list), vec![Span { items: 1..3, kind: Added }, Span { items: 4..5, kind: Added }, Span { items: 5..7, kind: Removed }]);
        assert!(spans([None, None]).is_empty());
    }

    #[test]
    fn markers_are_proportional_to_the_list() {
        assert_eq!(fraction(&(25..50), 100), (0.25, 0.25));
        assert_eq!(fraction(&(0..1), 4), (0., 0.25));
        // Never past the end, never a division by zero.
        assert_eq!(fraction(&(3..9), 4), (0.75, 0.25));
        assert_eq!(fraction(&(0..1), 0), (0., 0.));
    }

    #[test]
    fn the_thumb_follows_the_scroll_offset() {
        // 1000 px of content in a 250 px viewport.
        assert_eq!(thumb(0., 250., 1000.), Some((0., 0.25)));
        assert_eq!(thumb(-500., 250., 1000.), Some((0.5, 0.25)));
        // Scrolled to the very end.
        assert_eq!(thumb(-750., 250., 1000.), Some((0.75, 0.25)));
        // Over-scroll stays in the lane.
        assert_eq!(thumb(-900., 250., 1000.), Some((0.75, 0.25)));
        // Everything fits, or no layout yet: no thumb.
        assert_eq!(thumb(0., 250., 200.), None);
        assert_eq!(thumb(0., 0., 1000.), None);
    }

    #[test]
    fn progress_runs_from_the_top_to_the_end_and_back() {
        assert_eq!(progress(0., 250., 1000.), 0.);
        assert_eq!(progress(-750., 250., 1000.), 1.);
        assert_eq!(progress(-375., 250., 1000.), 0.5);
        assert_eq!(progress(-900., 250., 1000.), 1.);
        assert_eq!(progress(0., 250., 100.), 0.);
        for p in [0., 0.3, 1.] {
            assert!((progress(offset_at(p, 250., 1000.), 250., 1000.) - p).abs() < 1e-6);
        }
        assert_eq!(offset_at(0.5, 250., 100.), 0.);
    }

    #[test]
    fn a_click_centers_the_viewport_and_stays_inside_the_list() {
        assert_eq!(offset_for(0.5, 250., 1000.), -375.);
        assert_eq!(offset_for(0., 250., 1000.), 0.);
        assert_eq!(offset_for(1., 250., 1000.), -750.);
        assert_eq!(offset_for(0.5, 250., 100.), 0.);
        assert_eq!(fraction_at(150., 100., 200.), 0.25);
        assert_eq!(fraction_at(10., 100., 200.), 0.);
        assert_eq!(fraction_at(900., 100., 200.), 1.);
        assert_eq!(fraction_at(5., 0., 0.), 0.);
    }
}
