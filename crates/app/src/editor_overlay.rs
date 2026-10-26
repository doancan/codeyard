//! What is drawn over the syntax colours of one line: the IME underline, diagnostic squiggles and bracket colours. Pure
//! functions on `TextRun`s, so the cost on a very long line (a minified file is one line of a megabyte) can be tested.

use std::ops::Range;

use gpui::{Hsla, TextRun, UnderlineStyle, px};

/// Splits the segments at every position in `cuts` (sorted, no duplicates) that falls inside a segment, in one pass.
fn split_runs_at(runs: Vec<TextRun>, cuts: &[usize]) -> Vec<TextRun> {
    let mut out = Vec::with_capacity(runs.len() + cuts.len());
    let (mut pos, mut next) = (0, 0);
    for r in runs {
        let end = pos + r.len;
        while next < cuts.len() && cuts[next] <= pos {
            next += 1;
        }
        let mut start = pos;
        while next < cuts.len() && cuts[next] < end {
            out.push(TextRun { len: cuts[next] - start, ..r.clone() });
            start = cuts[next];
            next += 1;
        }
        out.push(TextRun { len: end - start, ..r });
        pos = end;
    }
    out
}

/// Indices of the segments that lie inside `range`; `starts[i]` is where segment `i` begins and `range` was cut at both ends.
fn within(starts: &[usize], runs: &[TextRun], range: &Range<usize>) -> Range<usize> {
    let first = starts.partition_point(|&s| s < range.start);
    let mut last = first;
    while last < runs.len() && starts[last] + runs[last].len <= range.end {
        last += 1;
    }
    first..last
}

/// `marked`: the IME composition. `diags`: (range, colour, is an error). `brackets`: (byte, colour). Every position is cut once
/// and every decoration is then found by binary search, so the cost grows with the line and the decorations, not with their product.
pub fn overlay_runs(runs: Vec<TextRun>, marked: Option<&Range<usize>>, diags: &[(Range<usize>, Hsla, bool)], brackets: &[(usize, Hsla)]) -> Vec<TextRun> {
    let mut cuts: Vec<usize> = marked.iter().flat_map(|m| [m.start, m.end]).chain(diags.iter().flat_map(|d| [d.0.start, d.0.end])).collect();
    cuts.extend(brackets.iter().flat_map(|&(b, _)| [b, b + 1]));
    cuts.sort_unstable();
    cuts.dedup();
    let mut runs = split_runs_at(runs, &cuts);
    let starts: Vec<usize> = runs
        .iter()
        .scan(0, |at, r| {
            let start = *at;
            *at += r.len;
            Some(start)
        })
        .collect();
    if let Some(m) = marked {
        for i in within(&starts, &runs, m) {
            runs[i].underline = Some(UnderlineStyle { color: Some(runs[i].color), thickness: px(1.), wavy: false });
        }
    }
    for (r, color, error) in diags {
        for i in within(&starts, &runs, r) {
            if runs[i].underline.is_none() || *error {
                runs[i].underline = Some(UnderlineStyle { color: Some(*color), thickness: px(1.), wavy: true });
            }
        }
    }
    for &(b, color) in brackets {
        if let Ok(i) = starts.binary_search(&b) {
            runs[i].color = color;
        }
    }
    runs
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{Font, FontStyle, FontWeight, hsla};

    fn run(len: usize) -> TextRun {
        TextRun {
            len,
            font: Font { family: "Menlo".into(), features: Default::default(), fallbacks: None, weight: FontWeight::NORMAL, style: FontStyle::Normal },
            color: hsla(0., 0., 1., 1.),
            background_color: None,
            underline: None,
            strikethrough: None,
        }
    }

    /// The same text coloured by `n` equal segments, the way a highlighter hands a line over.
    fn segments(n: usize, len: usize) -> Vec<TextRun> {
        (0..n).map(|_| run(len)).collect()
    }

    /// The straightforward version this replaced: one pass over every segment per decoration.
    fn reference(mut runs: Vec<TextRun>, marked: Option<&Range<usize>>, diags: &[(Range<usize>, Hsla, bool)], brackets: &[(usize, Hsla)]) -> Vec<TextRun> {
        fn split(runs: Vec<TextRun>, at: usize) -> Vec<TextRun> {
            let mut out = Vec::with_capacity(runs.len() + 1);
            let mut pos = 0;
            for r in runs {
                let end = pos + r.len;
                if at > pos && at < end {
                    out.push(TextRun { len: at - pos, ..r.clone() });
                    out.push(TextRun { len: end - at, ..r });
                } else {
                    out.push(r);
                }
                pos = end;
            }
            out
        }
        if let Some(m) = marked {
            runs = split(split(runs, m.start), m.end);
            let mut at = 0;
            for r in &mut runs {
                if at >= m.start && at + r.len <= m.end {
                    r.underline = Some(UnderlineStyle { color: Some(r.color), thickness: px(1.), wavy: false });
                }
                at += r.len;
            }
        }
        for (r, color, error) in diags {
            runs = split(split(runs, r.start), r.end);
            let mut at = 0;
            for run in &mut runs {
                if at >= r.start && at + run.len <= r.end && (run.underline.is_none() || *error) {
                    run.underline = Some(UnderlineStyle { color: Some(*color), thickness: px(1.), wavy: true });
                }
                at += run.len;
            }
        }
        for &(b, color) in brackets {
            runs = split(split(runs, b), b + 1);
            let mut at = 0;
            for run in &mut runs {
                if at == b {
                    run.color = color;
                }
                at += run.len;
            }
        }
        runs
    }

    fn describe(runs: &[TextRun]) -> Vec<(usize, Hsla, Option<(Option<Hsla>, bool)>)> {
        runs.iter().map(|r| (r.len, r.color, r.underline.as_ref().map(|u| (u.color, u.wavy)))).collect()
    }

    /// Whatever the decorations are (overlapping, touching, empty, past the end), the segments come out exactly as before.
    #[test]
    fn the_overlays_come_out_exactly_as_the_straightforward_version_made_them() {
        let mut seed = 12345u64;
        let mut rnd = |n: usize| {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            ((seed >> 33) as usize) % n.max(1)
        };
        let palette = [hsla(0.1, 0.5, 0.5, 1.), hsla(0.4, 0.5, 0.5, 1.), hsla(0.8, 0.5, 0.5, 1.)];
        for _ in 0..300 {
            let total = 1 + rnd(60);
            let mut left = total;
            let mut runs = Vec::new();
            while left > 0 {
                let len = (1 + rnd(8)).min(left);
                let mut r = run(len);
                r.color = palette[rnd(3)];
                runs.push(r);
                left -= len;
            }
            let range = |rnd: &mut dyn FnMut(usize) -> usize| {
                let a = rnd(total + 3);
                a..a + rnd(12)
            };
            let marked = (rnd(2) == 0).then(|| range(&mut rnd));
            let diags: Vec<_> = (0..rnd(5)).map(|_| (range(&mut rnd), palette[rnd(3)], rnd(2) == 0)).collect();
            let brackets: Vec<_> = (0..rnd(10)).map(|_| (rnd(total + 2), palette[rnd(3)])).collect();
            let want = reference(runs.clone(), marked.as_ref(), &diags, &brackets);
            let got = overlay_runs(runs, marked.as_ref(), &diags, &brackets);
            assert_eq!(describe(&got), describe(&want), "marked {marked:?} diags {diags:?} brackets {brackets:?}");
        }
    }

    #[test]
    fn a_very_long_line_with_a_coloured_bracket_every_few_characters_is_laid_out_quickly() {
        let n = 15_000;
        let brackets: Vec<(usize, Hsla)> = (0..n).map(|i| (i * 2, hsla(0.3, 0.8, 0.6, 1.))).collect();
        let diags: Vec<(Range<usize>, Hsla, bool)> = (0..200).map(|i| (i * 50..i * 50 + 20, hsla(0., 0.8, 0.6, 1.), i % 3 == 0)).collect();
        let started = std::time::Instant::now();
        let out = overlay_runs(segments(n, 2), Some(&(10..30)), &diags, &brackets);
        let took = started.elapsed();
        assert_eq!(out.iter().map(|r| r.len).sum::<usize>(), n * 2, "no text is lost");
        assert!(out.iter().all(|r| r.len > 0), "no empty segment reaches the shaper");
        assert!(took < std::time::Duration::from_secs(2), "{n} brackets on one line took {took:?}: the work per bracket must not grow with the line");
    }
}
