//! Fuzzy matching for short lists (commands, themes). Rule: letters must appear in order;
//! word start +10, adjacent +5, first letter +3, each skipped character -1 (at most -20).
//! Turkish folding: i/ı/İ/I are equivalent, ş/ç/ğ/ö/ü → s/c/g/o/u.
//! `nucleo` handles thousands of file paths; this greedy version is for short labels.

/// Turkish folding + lowercase (one char per char: indices are preserved).
pub fn fold(ch: char) -> char {
    match ch {
        'İ' | 'I' | 'ı' => 'i',
        'Ş' | 'ş' => 's',
        'Ç' | 'ç' => 'c',
        'Ğ' | 'ğ' => 'g',
        'Ö' | 'ö' => 'o',
        'Ü' | 'ü' => 'u',
        c => c.to_lowercase().next().unwrap_or(c),
    }
}

fn is_word_start(chars: &[char], i: usize) -> bool {
    i == 0 || matches!(chars[i - 1], ' ' | ':' | '/' | '.' | '_' | '-' | '(')
}

/// (score, matched char indices). `None` if there is no match; an empty query matches everything.
/// The greedy match takes each letter as early as it can, so "reverse lines" would start at the `r` of "Editor" in
/// "Editor: Reverse Lines"; when the query appears as it is typed, that match is scored too and the better one wins.
pub fn score(query: &str, label: &str) -> Option<(i32, Vec<usize>)> {
    let chars: Vec<char> = label.chars().collect();
    let mut hits = Vec::new();
    let mut from = 0;
    for q in query.chars().filter(|c| !c.is_whitespace()).map(fold) {
        let i = (from..chars.len()).find(|&i| fold(chars[i]) == q)?;
        hits.push(i);
        from = i + 1;
    }
    let greedy = (rate(&chars, &hits), hits);
    Some(match whole(query, &chars) {
        Some(at) if rate(&chars, &at) > greedy.0 => (rate(&chars, &at), at),
        _ => greedy,
    })
}

/// The score of matching `hits` (char indices, ascending) in `chars`.
fn rate(chars: &[char], hits: &[usize]) -> i32 {
    let mut score = 0;
    let mut from = 0;
    for (k, &i) in hits.iter().enumerate() {
        if is_word_start(chars, i) {
            score += 10;
        }
        if k > 0 && hits[k - 1] + 1 == i {
            score += 5;
        }
        if i == 0 {
            score += 3;
        }
        score -= (i - from).min(20) as i32;
        from = i + 1;
    }
    score
}

/// Where the query appears as typed (letters folded, spaces as typed), at its first word start if it can: the indices
/// of its letters.
fn whole(query: &str, chars: &[char]) -> Option<Vec<usize>> {
    let q: Vec<char> = query.trim().chars().map(fold).collect();
    if q.len() < 2 || q.len() > chars.len() {
        return None;
    }
    let at = |p: usize| (0..q.len()).all(|k| fold(chars[p + k]) == q[k]);
    let starts: Vec<usize> = (0..=chars.len() - q.len()).filter(|&p| at(p)).collect();
    let p = starts.iter().copied().find(|&p| is_word_start(chars, p)).or(starts.first().copied())?;
    Some((p..p + q.len()).filter(|&i| !q[i - p].is_whitespace()).collect())
}

/// Converts char indices to byte ranges (for text highlighting).
pub fn hit_ranges(label: &str, hits: &[usize]) -> Vec<std::ops::Range<usize>> {
    let mut out = Vec::new();
    for (ci, (bi, ch)) in label.char_indices().enumerate() {
        if hits.contains(&ci) {
            out.push(bi..bi + ch.len_utf8());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_cases_as_js_preview() {
        assert_eq!(score("gpk", "Görünüm: Paneli Aç/Kapat").unwrap().1, [0, 9, 19]);
        assert_eq!(score("gorunum", "Görünüm: Kenar").unwrap().1, [0, 1, 2, 3, 4, 5, 6]);
        assert_eq!(score("ışık", "IŞIK modu").unwrap().1, [0, 1, 2, 3]);
        assert!(score("xyz", "Dosya: Kaydet").is_none());
        let short = score("kay", "Dosya: Kaydet").unwrap().0;
        let long = score("kay", "Dosya: Tümünü Kaydet").unwrap().0;
        assert!(short > long);
    }

    #[test]
    fn theme_names() {
        assert!(score("hcd", "High Contrast Dark").is_some());
        let fj = score("fj", "Fjord").unwrap().0;
        let other = score("fj", "Midnight (fj)").map_or(i32::MIN, |s| s.0);
        assert!(fj > other);
    }

    #[test]
    fn the_query_as_typed_beats_an_early_scattered_match() {
        // Greedy, "reverse lines" starts at the r of "Editor"; as typed it is the label's last two words.
        let exact = score("reverse lines", "Editor: Reverse Lines").unwrap();
        assert_eq!(exact.1, [8, 9, 10, 11, 12, 13, 14, 16, 17, 18, 19, 20]);
        assert!(exact.0 > score("reverse lines", "Git: Revert Selected Lines").unwrap().0);
        // Folding applies; a scattered query still matches greedily.
        assert!(score("REVERSE", "Editor: Reverse Lines").unwrap().1.starts_with(&[8]));
        assert_eq!(score("gpk", "Görünüm: Paneli Aç/Kapat").unwrap().1, [0, 9, 19]);
    }

    #[test]
    fn hit_ranges_are_byte_ranges() {
        assert_eq!(hit_ranges("ağ", &[1]), [1..3]);
    }
}
