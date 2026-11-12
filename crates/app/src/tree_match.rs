//! Matching names against the explorer filter: case-insensitive substring search that treats the Turkish dotted and
//! dotless i like the plain one (`ISTANBUL`, `İzmir` and `ırmak` all match `i`). No UI and no I/O.

use std::ops::Range;

/// One character in its comparison form. It is always one character, so positions in the folded text are positions in the
/// original.
fn fold(c: char) -> char {
    match c {
        'İ' | 'I' | 'ı' => 'i',
        c => {
            let mut lower = c.to_lowercase();
            match (lower.next(), lower.next()) {
                (Some(one), None) => one,
                _ => c,
            }
        }
    }
}

/// `text` in comparison form (what [`Query::matches_folded`] expects).
pub fn folded(text: &str) -> String {
    text.chars().map(fold).collect()
}

/// A non-empty filter text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Query {
    folded: String,
    chars: Vec<char>,
}

impl Query {
    /// `None` for a text with nothing but white space: that means "no filter".
    pub fn new(text: &str) -> Option<Query> {
        let text = text.trim();
        if text.is_empty() {
            return None;
        }
        let folded = folded(text);
        Some(Query { chars: folded.chars().collect(), folded })
    }

    /// Whether a name already folded with [`folded`] contains the query.
    pub fn matches_folded(&self, folded_name: &str) -> bool {
        folded_name.contains(self.folded.as_str())
    }

    /// Byte range of the first match in `name` (as written, not folded), for highlighting.
    pub fn find(&self, name: &str) -> Option<Range<usize>> {
        let marks: Vec<(usize, char)> = name.char_indices().map(|(at, c)| (at, fold(c))).collect();
        let n = self.chars.len();
        let start = (0..=marks.len().checked_sub(n)?).find(|&i| marks[i..i + n].iter().map(|m| m.1).eq(self.chars.iter().copied()))?;
        let end = marks.get(start + n).map_or(name.len(), |m| m.0);
        Some(marks[start].0..end)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn q(text: &str) -> Query {
        Query::new(text).unwrap()
    }

    #[test]
    fn blank_text_is_no_filter() {
        assert!(Query::new("").is_none());
        assert!(Query::new("  \t").is_none());
    }

    #[test]
    fn matching_ignores_case() {
        assert!(q("readme").matches_folded(&folded("README.md")));
        assert!(q("README").matches_folded(&folded("readme.md")));
        assert!(!q("readme").matches_folded(&folded("Cargo.toml")));
    }

    #[test]
    fn dotted_and_dotless_i_match_the_plain_one() {
        for name in ["ISTANBUL.txt", "İzmir.txt", "ırmak.txt", "istanbul.txt"] {
            assert!(q("i").matches_folded(&folded(name)), "{name}");
        }
        assert!(q("İZMİR").matches_folded(&folded("izmir.txt")));
        assert!(q("ırmak").matches_folded(&folded("IRMAK.TXT")));
        assert!(!q("i").matches_folded(&folded("Cargo.toml")));
    }

    #[test]
    fn other_unicode_names_fold_by_character() {
        assert!(q("şirket").matches_folded(&folded("ŞİRKET notları.md")));
        assert!(q("ÖĞRENCİ").matches_folded(&folded("öğrenci.csv")));
        assert!(q("日本").matches_folded(&folded("資料-日本語.txt")));
        assert!(q("straße").matches_folded(&folded("STRAßE.txt")));
    }

    #[test]
    fn the_match_is_found_at_byte_offsets_of_the_original() {
        assert_eq!(q("main").find("src/main.rs"), Some(4..8));
        assert_eq!(q("MAIN").find("main.rs"), Some(0..4));
        assert_eq!(q("rs").find("main.rs"), Some(5..7));
        assert_eq!(q("zz").find("main.rs"), None);
    }

    #[test]
    fn offsets_are_right_after_wider_characters() {
        // `İ` is two bytes but folds to the one-byte `i`.
        let name = "İİzmir.txt";
        let range = q("zmir").find(name).unwrap();
        assert_eq!(&name[range], "zmir");
        let range = q("İz").find("aİzmir").unwrap();
        assert_eq!(&"aİzmir"[range], "İz");
    }

    #[test]
    fn a_query_longer_than_the_name_does_not_match() {
        assert_eq!(q("longer than the name").find("a.rs"), None);
        assert_eq!(q("a").find(""), None);
    }

    #[test]
    fn the_whole_name_can_match() {
        assert_eq!(q("a.rs").find("a.rs"), Some(0..4));
    }

    #[test]
    fn surrounding_white_space_is_ignored() {
        assert_eq!(q("  main ").find("main.rs"), Some(0..4));
    }
}
