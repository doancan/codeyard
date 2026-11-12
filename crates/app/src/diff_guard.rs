//! Texts the diff view does not try to line up: binary content and very large files get a message instead.

/// Either side above this many bytes is not diffed (the line diff is quadratic in the worst case, and a screen of
/// 4 MB of text is not something to read as a diff anyway).
pub const MAX_BYTES: usize = 4 * 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unsupported {
    Binary,
    /// The larger side's size in bytes.
    TooLarge(usize),
}

/// Why the texts cannot be shown as a diff, if they cannot.
pub fn check(old: &str, new: &str) -> Option<Unsupported> {
    if old.contains('\0') || new.contains('\0') {
        return Some(Unsupported::Binary);
    }
    let largest = old.len().max(new.len());
    (largest > MAX_BYTES).then_some(Unsupported::TooLarge(largest))
}

/// Whether a side holds the replacement character the lossy UTF-8 reading puts where bytes were not valid text.
pub fn has_invalid_bytes(old: &str, new: &str) -> bool {
    old.contains('\u{fffd}') || new.contains('\u{fffd}')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nul_bytes_mean_binary_and_size_is_capped() {
        assert_eq!(check("a\nb\n", "a\nc\n"), None);
        assert_eq!(check("PNG\0\u{1}", "x"), Some(Unsupported::Binary));
        assert_eq!(check("x", "a\0"), Some(Unsupported::Binary));
        let big = "x".repeat(MAX_BYTES + 1);
        assert_eq!(check(&big, "x"), Some(Unsupported::TooLarge(MAX_BYTES + 1)));
        assert_eq!(check("x", &"y".repeat(MAX_BYTES)), None);
    }

    #[test]
    fn replacement_characters_are_reported() {
        assert!(has_invalid_bytes(&String::from_utf8_lossy(b"caf\xe9"), ""));
        assert!(!has_invalid_bytes("café", "ğüş"));
    }
}
