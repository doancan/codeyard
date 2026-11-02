//! Where characters are drawn, from the shaped line: left-to-right, right-to-left and mixed text, ligatures and
//! characters without a glyph, and the stretches a painted line is read in.

use super::*;

/// What a shaped line gives: glyphs (x, byte index) in the order the platform returns them, and the line's width.
struct Shaped(Vec<(Pixels, usize)>, Pixels);

fn layout(glyphs: &[(f32, usize)], width: f32) -> Shaped {
    Shaped(glyphs.iter().map(|&(x, index)| (px(x), index)).collect(), px(width))
}

fn cells(text: &str, l: &Shaped) -> Vec<(f32, f32)> {
    cells_of(text, l.0.clone(), l.1).into_iter().map(|(a, b)| (a / px(1.), b / px(1.))).collect()
}

fn line(cells: &[(f32, f32)], end: f32) -> TextLine {
    TextLine { start: 0, cells: cells.iter().map(|&(a, b)| (px(a), px(b))).collect(), end: px(end), top: px(0.), bottom: px(10.), line_break: px(4.) }
}

#[test]
fn left_to_right_text_takes_each_glyph_up_to_the_next() {
    let l = layout(&[(0., 0), (7., 1), (14., 2)], 21.);
    assert_eq!(cells("abc", &l), vec![(0., 7.), (7., 14.), (14., 21.)]);
    assert_eq!(line(&cells("abc", &l), 21.).stretches(), vec![Stretch { start: 0, end: 3, rtl: false }]);
}

/// Hebrew after Latin: the platform draws "abc " then the Hebrew letters in screen order (the last one leftmost), so
/// the Hebrew characters, in reading order, step leftwards; that part is one right-to-left stretch.
#[test]
fn right_to_left_text_keeps_the_place_it_is_drawn_at() {
    // "ab אבג": a=0, b=1, space=2, א=3 (2 bytes), ב=5, ג=7; on screen: a b ␠ ג ב א.
    let text = "ab אבג";
    let l = layout(&[(0., 0), (7., 1), (14., 2), (21., 7), (28., 5), (35., 3)], 42.);
    let c = cells(text, &l);
    assert_eq!(c, vec![(0., 7.), (7., 14.), (14., 21.), (35., 42.), (28., 35.), (21., 28.)]);
    let tl = line(&c, 42.);
    assert_eq!(tl.stretches(), vec![Stretch { start: 0, end: 3, rtl: false }, Stretch { start: 3, end: 6, rtl: true }]);
    // The line break follows the Hebrew on screen only through its leftmost letter: not right after the last one read.
    assert!(!tl.break_follows(6));
    assert!(line(&cells("abc", &layout(&[(0., 0), (7., 1), (14., 2)], 21.)), 21.).break_follows(3));
}

/// Glyphs given in reading order with right-to-left positions (as some shapers return a right-to-left run) are put in
/// screen order first.
#[test]
fn glyph_order_does_not_matter() {
    let l = layout(&[(14., 0), (7., 2), (0., 4)], 21.);
    assert_eq!(cells("אבג", &l), vec![(14., 21.), (7., 14.), (0., 7.)]);
    assert_eq!(line(&cells("אבג", &l), 21.).stretches(), vec![Stretch { start: 0, end: 3, rtl: true }]);
}

/// "Latin, Arabic, Latin" in one line: three stretches, the middle one right to left.
#[test]
fn mixed_text_is_cut_where_the_direction_changes() {
    // "a سل b": a=0, ␠=1, س=2, ل=4, ␠=6, b=7; on screen: a ␠ ل س ␠ b.
    let l = layout(&[(0., 0), (5., 1), (10., 4), (15., 2), (20., 6), (25., 7)], 30.);
    let tl = line(&cells("a سل b", &l), 30.);
    assert_eq!(
        tl.stretches(),
        vec![Stretch { start: 0, end: 2, rtl: false }, Stretch { start: 2, end: 4, rtl: true }, Stretch { start: 4, end: 6, rtl: false }]
    );
    assert!(tl.break_follows(6));
}

/// A ligature ("fi") is one glyph for two characters: they share its width in reading order. A right-to-left
/// cluster (lam-alef) shares it from the right.
#[test]
fn a_cluster_is_shared_by_its_characters() {
    assert_eq!(cells("fix", &layout(&[(0., 0), (10., 2)], 15.)), vec![(0., 5.), (5., 10.), (10., 15.)]);
    // "لاب": ل=0, ا=2 (no glyph of its own), ب=4; on screen: ب then the lam-alef.
    assert_eq!(cells("لاب", &layout(&[(0., 4), (6., 0)], 16.)), vec![(11., 16.), (6., 11.), (0., 6.)]);
}

/// Characters before the first glyph (nothing drawn) have no width.
#[test]
fn characters_without_glyphs_have_no_width() {
    assert_eq!(cells("\u{200c}a", &layout(&[(0., 3)], 7.)), vec![(0., 0.), (0., 7.)]);
    assert!(cells("", &layout(&[], 0.)).is_empty());
}
