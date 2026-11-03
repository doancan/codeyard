//! Moving and deleting by word part: the pieces of `camelCase`, `PascalCase`, `snake_case`, `kebab-case` and
//! `HTTPServer` names, and of digits (part of `impl Buffer`; reads the private fields of `lib.rs`).

use super::*;

/// What a character is made of for word parts. Anything not a letter, digit or separator is punctuation.
#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Separator,
    Lower,
    Upper,
    Punct,
}

fn kind(c: char) -> Kind {
    match c {
        c if c.is_whitespace() || c == '_' || c == '-' => Kind::Separator,
        c if c.is_uppercase() => Kind::Upper,
        c if c.is_alphanumeric() => Kind::Lower,
        _ => Kind::Punct,
    }
}

impl Buffer {
    fn part_kind(&self, i: usize) -> Kind {
        kind(self.rope.char(i))
    }

    /// The start of the word part before `idx`: separators are skipped, then a lowercase or digit run takes the capital that
    /// starts it (`fooBar|` → `foo|Bar`), a capital run is one part (`HTTP|`), and punctuation goes as a run.
    fn word_part_start_before(&self, idx: usize) -> usize {
        let mut i = idx;
        while i > 0 && self.part_kind(i - 1) == Kind::Separator {
            i -= 1;
        }
        if i == 0 {
            return 0;
        }
        match self.part_kind(i - 1) {
            Kind::Lower => {
                while i > 0 && self.part_kind(i - 1) == Kind::Lower {
                    i -= 1;
                }
                if i > 0 && self.part_kind(i - 1) == Kind::Upper {
                    // A run that starts with a digit belongs to the capitals before it (`MD5`), a lowercase one takes one (`Hash`).
                    let digits_first = !self.rope.char(i).is_alphabetic();
                    i -= 1;
                    while digits_first && i > 0 && self.part_kind(i - 1) == Kind::Upper {
                        i -= 1;
                    }
                }
            }
            class => {
                while i > 0 && self.part_kind(i - 1) == class {
                    i -= 1;
                }
            }
        }
        i
    }

    /// The end of the word part after `idx`: the mirror of `word_part_start_before`. In a capital run followed by lowercase
    /// the last capital belongs to the next part (`|HTTPServer` → `HTTP|Server`).
    fn word_part_end_after(&self, idx: usize) -> usize {
        let n = self.len_chars();
        let mut i = idx;
        while i < n && self.part_kind(i) == Kind::Separator {
            i += 1;
        }
        if i == n {
            return n;
        }
        match self.part_kind(i) {
            Kind::Upper => {
                let start = i;
                while i < n && self.part_kind(i) == Kind::Upper {
                    i += 1;
                }
                if i < n && self.part_kind(i) == Kind::Lower {
                    if i - start > 1 && self.rope.char(i).is_alphabetic() {
                        i -= 1;
                    } else {
                        while i < n && self.part_kind(i) == Kind::Lower {
                            i += 1;
                        }
                    }
                }
            }
            class => {
                while i < n && self.part_kind(i) == class {
                    i += 1;
                }
            }
        }
        i
    }

    /// Move every cursor to the previous or next word part (extending the selection when `select`).
    pub fn move_word_part(&mut self, forward: bool, select: bool) {
        self.move_each(select, |b, s| (if forward { b.word_part_end_after(s.cursor) } else { b.word_part_start_before(s.cursor) }, None));
    }

    /// Delete the selection if any, otherwise from the cursor to the word part boundary.
    pub fn delete_word_part(&mut self, forward: bool) {
        self.edit_each(
            |b, _, s| match s.selection() {
                Some(r) => (r, String::new()),
                None if forward => (s.cursor..b.word_part_end_after(s.cursor), String::new()),
                None => (b.word_part_start_before(s.cursor)..s.cursor, String::new()),
            },
            false,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parts(text: &str) -> Vec<usize> {
        let b = Buffer::from_str(text);
        let (mut at, mut stops) = (0, Vec::new());
        while at < b.len_chars() {
            at = b.word_part_end_after(at);
            stops.push(at);
        }
        stops
    }

    #[test]
    fn names_split_into_their_parts_going_right() {
        assert_eq!(parts("fooBar"), [3, 6]);
        assert_eq!(parts("FooBar"), [3, 6]);
        assert_eq!(parts("HTTPServer"), [4, 10]);
        assert_eq!(parts("snake_case_name"), [5, 10, 15]);
        assert_eq!(parts("kebab-case"), [5, 10]);
        assert_eq!(parts("a1b2"), [4], "digits stay with their letters");
        assert_eq!(parts("MD5Hash"), [3, 7], "digits stay with the capitals before them");
        assert_eq!(parts("HTTP2Server"), [5, 11]);
        assert_eq!(parts("x.y(z)"), [1, 2, 3, 4, 5, 6]);
        assert_eq!(parts("  foo  bar"), [5, 10]);
        assert_eq!(parts("ÇalışanAdı"), [7, 10], "Unicode letters are letters");
    }

    #[test]
    fn capitals_followed_by_digits_split_the_same_way_going_left() {
        let b = Buffer::from_str("MD5Hash");
        let (mut at, mut stops) = (b.len_chars(), Vec::new());
        while at > 0 {
            at = b.word_part_start_before(at);
            stops.push(at);
        }
        assert_eq!(stops, [3, 0]);
    }

    #[test]
    fn going_left_stops_at_the_same_places() {
        let b = Buffer::from_str("HTTPServer fooBar_baz");
        let (mut at, mut stops) = (b.len_chars(), Vec::new());
        while at > 0 {
            at = b.word_part_start_before(at);
            stops.push(at);
        }
        assert_eq!(stops, [18, 14, 11, 4, 0]);
    }

    #[test]
    fn deleting_a_part_is_one_undo_step() {
        let mut b = Buffer::from_str("fooBarBaz");
        b.set_cursor(9, false);
        b.delete_word_part(false);
        assert_eq!(String::from_utf8(b.to_bytes()).unwrap(), "fooBar");
        b.move_word_part(false, true);
        assert_eq!(b.selection(), Some(3..6), "selecting extends over the previous part");
        b.undo();
        assert_eq!(String::from_utf8(b.to_bytes()).unwrap(), "fooBarBaz");
    }
}
