//! Editing a file that is not open: bytes in, bytes out.

use std::ops::Range;

use super::{BOM, Buffer, LineEnding, NotUtf8};

/// Inserted text holds `\n` only, whatever line breaks it came with.
pub(crate) fn unify_breaks(text: &str) -> String {
    text.replace("\r\n", "\n").replace('\r', "\n")
}

impl Buffer {
    /// Edit a file that is not open in an editor, from its bytes to its new bytes (`None`: no edit asked for). `edits_for` sees the
    /// buffer the file reads as and answers with char ranges in it. A file with one kind of line ending goes through the buffer;
    /// a mixed one is edited in place, so the lines nobody touched keep the endings they had.
    pub fn edit_bytes(bytes: &[u8], edits_for: impl FnOnce(&Buffer) -> Vec<(Range<usize>, String)>) -> Result<Option<Vec<u8>>, NotUtf8> {
        let mut b = Self::from_bytes(bytes)?;
        let mut edits = edits_for(&b);
        if edits.is_empty() {
            return Ok(None);
        }
        if !b.mixed_endings {
            b.apply_edits(edits);
            return Ok(Some(b.to_bytes()));
        }
        edits.sort_by_key(|(r, _)| (r.start, r.end));
        let text = std::str::from_utf8(bytes).map_err(|_| NotUtf8)?;
        let body_at = if b.bom { BOM.len() } else { 0 };
        let raw = &text[body_at..];
        let sep = match b.line_ending {
            LineEnding::Lf => "\n",
            LineEnding::Crlf => "\r\n",
            LineEnding::Cr => "\r",
        };
        // (chars as the buffer counts them, byte offset in the raw text): a `\r\n` is one char there, unless the file is CR-only.
        let (mut chars, mut byte) = (0usize, 0usize);
        let mut at = |target: usize| {
            while chars < target && byte < raw.len() {
                let rest = &raw[byte..];
                byte += if b.line_ending != LineEnding::Cr && rest.starts_with("\r\n") { 2 } else { rest.chars().next().map_or(1, char::len_utf8) };
                chars += 1;
            }
            byte
        };
        let mut out = String::with_capacity(text.len());
        out.push_str(&text[..body_at]);
        let mut done = 0;
        let mut floor = 0;
        for (range, new) in edits {
            let (start, end) = (range.start.max(floor), range.end.max(floor));
            let (from, to) = (at(start.min(b.len_chars())), at(end.min(b.len_chars())));
            out.push_str(&raw[done..from]);
            out.push_str(&unify_breaks(&new).replace('\n', sep));
            done = to.max(from);
            floor = end;
        }
        out.push_str(&raw[done..]);
        Ok(Some(out.into_bytes()))
    }
}
