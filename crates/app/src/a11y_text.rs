//! Text runs of the editor and the terminal: the exposed lines split into runs, with where each character was painted,
//! so assistive technology can ask for the bounds of the caret, the selection or any text range (a magnifier follows
//! the caret, a screen reader draws its cursor around the word it reads).
//!
//! A run never spans two painted lines: a wrapped line is split where it wraps. Each run of a painted line has its
//! physical bounds (that line's top and bottom, from its first character's left edge to its end), and the position and
//! advance of each character relative to the run, from the shaped line GPUI painted. A run that is not painted (the
//! line is scrolled out of view or folded away) has no bounds, so a range that reaches into it has none either.

use super::*;
use crate::painted::TextLine;
use accesskit::{Rect, TextDirection};

/// Where a run was painted, in physical pixels: its bounds, and per character (the line break included) its position
/// from the run's left edge and its advance width.
#[derive(Clone, PartialEq, Debug)]
pub struct RunFrame {
    pub bounds: Rect,
    pub positions: Vec<f32>,
    pub widths: Vec<f32>,
}

/// Split into runs: each line into runs of at most RUN_CHARS characters, the line ending in the last run.
#[cfg(test)]
pub(super) fn runs_for(lines: Vec<(usize, String)>, total_lines: usize, first_line: usize) -> Vec<RunSnap> {
    runs_painted(lines, total_lines, first_line, &[], 1.)
}

/// As `runs_for`, also split where a painted line starts (a wrapped line's later segments) and with each painted run's
/// frame from `painted` (drawn at `scale`).
pub(super) fn runs_painted(lines: Vec<(usize, String)>, total_lines: usize, first_line: usize, painted: &[TextLine], scale: f32) -> Vec<RunSnap> {
    let mut starts: Vec<usize> = painted.iter().map(|l| l.start).collect();
    starts.sort_unstable();
    let mut runs = Vec::new();
    for (i, (start, line)) in lines.into_iter().enumerate() {
        let last_in_doc = first_line + i + 1 >= total_lines;
        let chars: Vec<char> = line.chars().collect();
        let n = chars.len();
        let mut off = 0;
        loop {
            // The next painted line start inside this line, if it comes before the run would end.
            let wrap = starts.iter().map(|s| s.wrapping_sub(start)).find(|&s| s > off && s < n);
            let end = (off + RUN_CHARS).min(n).min(wrap.unwrap_or(n));
            let mut text: String = chars[off..end].iter().collect();
            let mut count = end - off;
            if end == n && !last_in_doc {
                text.push('\n');
                count += 1;
            }
            let frame = painted.iter().find_map(|l| run_frame(l, start + off, end - off, end == n && !last_in_doc, scale));
            runs.push(RunSnap { start: start + off, chars: count, text, frame });
            if end == n {
                break;
            }
            off = end;
        }
    }
    runs
}

/// The frame of the `chars` characters from document index `start` (followed by a line break when `line_break`) if
/// they lie on painted line `line`.
fn run_frame(line: &TextLine, start: usize, chars: usize, line_break: bool, scale: f32) -> Option<RunFrame> {
    let k = start.checked_sub(line.start)?;
    if k + chars > line.chars() || !scale.is_finite() || scale <= 0. {
        return None;
    }
    let px = |p: gpui::Pixels| f64::from(p / gpui::px(1.)) * f64::from(scale);
    let x0 = px(line.xs[k]);
    let mut positions = Vec::with_capacity(chars + 1);
    let mut widths = Vec::with_capacity(chars + 1);
    for i in k..k + chars {
        positions.push((px(line.xs[i]) - x0) as f32);
        widths.push((px(line.xs[i + 1]) - px(line.xs[i])).max(0.) as f32);
    }
    let mut x1 = px(line.xs[k + chars]);
    if line_break {
        positions.push((x1 - x0) as f32);
        widths.push(px(line.line_break).max(0.) as f32);
        x1 += px(line.line_break).max(0.);
    }
    // An empty run still has a place: the caret's, one logical pixel wide.
    if x1 <= x0 {
        x1 = x0 + f64::from(scale);
    }
    let bounds = Rect::new(x0, px(line.top), x1, px(line.bottom));
    let finite = [bounds.x0, bounds.y0, bounds.x1, bounds.y1].into_iter().all(f64::is_finite) && positions.iter().chain(&widths).all(|v| v.is_finite());
    (finite && bounds.height() > 0.).then_some(RunFrame { bounds, positions, widths })
}

fn word_starts(text: &str) -> Vec<u8> {
    #[derive(PartialEq, Clone, Copy)]
    enum K {
        Word,
        Space,
        Punct,
    }
    let kind = |c: char| {
        if c.is_alphanumeric() || c == '_' {
            K::Word
        } else if c.is_whitespace() {
            K::Space
        } else {
            K::Punct
        }
    };
    let mut out = Vec::new();
    let mut prev: Option<K> = None;
    for (i, c) in text.chars().enumerate() {
        let k = kind(c);
        // Each punctuation character is its own word; words and whitespace as clusters.
        if (prev != Some(k) || k == K::Punct) && i > 0 && i < 256 {
            out.push(i as u8);
        }
        prev = Some(k);
    }
    out
}

/// Convert a character index to a (run, char index within run) position; clamp to the edge if outside the window.
/// `base`: id of the first run.
pub(super) fn position_of(runs: &[RunSnap], base: u64, ix: usize) -> Option<TextPosition> {
    let first = runs.first()?;
    if ix <= first.start {
        return Some(TextPosition { node: NodeId(base), character_index: 0 });
    }
    for (i, r) in runs.iter().enumerate() {
        if ix < r.start + r.chars {
            return Some(TextPosition { node: NodeId(base + i as u64), character_index: ix - r.start });
        }
    }
    let i = runs.len() - 1;
    Some(TextPosition { node: NodeId(base + i as u64), character_index: runs[i].text.chars().count() })
}

pub(super) fn char_of(runs: &[RunSnap], p: TextPosition) -> Option<usize> {
    let i = p.node.0.checked_sub(RUN_BASE)? as usize;
    runs.get(i).map(|r| r.start + p.character_index.min(r.chars))
}

/// Text run nodes (one per run) with ids from `base`; a painted run has its bounds and character positions.
pub(super) fn run_nodes(runs: &[RunSnap], base: u64, nodes: &mut Vec<(NodeId, Node)>) -> Vec<NodeId> {
    let mut ids = Vec::new();
    for (i, r) in runs.iter().enumerate() {
        let id = NodeId(base + i as u64);
        let mut run = Node::new(Role::TextRun);
        run.set_value(r.text.clone());
        run.set_character_lengths(r.text.chars().map(|c| c.len_utf8() as u8).collect::<Vec<u8>>());
        run.set_word_starts(word_starts(&r.text));
        run.set_text_direction(TextDirection::LeftToRight);
        if let Some(frame) = &r.frame {
            run.set_bounds(frame.bounds);
            run.set_character_positions(frame.positions.clone());
            run.set_character_widths(frame.widths.clone());
        }
        ids.push(id);
        nodes.push((id, run));
    }
    ids
}
