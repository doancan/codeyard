//! The subset of ustar that `.cyx` packages use: regular files only, names in the 100-byte `name` and 155-byte `prefix`
//! fields, no extensions. Written canonically (sorted, mtime 0, uid/gid 0, mode 0644 or 0755) and read strictly, so
//! the reader never meets a format feature the writer would not produce.

use crate::Error;

pub const BLOCK: usize = 512;

/// One file of a package.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub path: String,
    pub data: Vec<u8>,
    pub executable: bool,
}

fn octal(field: &mut [u8], value: u64) {
    // Width minus one digit, then the NUL terminator, as the classic tar writers do.
    let s = format!("{value:0width$o}", width = field.len() - 1);
    field[..s.len()].copy_from_slice(s.as_bytes());
    field[field.len() - 1] = 0;
}

/// Split a path over `prefix` (155) and `name` (100) at a `/`; `None` when it does not fit.
fn split_name(path: &str) -> Option<(&str, &str)> {
    if path.len() <= 100 {
        return Some(("", path));
    }
    let bytes = path.as_bytes();
    (0..path.len()).rev().filter(|&i| bytes[i] == b'/').find(|&i| i <= 155 && path.len() - i - 1 <= 100 && i > 0).map(|i| (&path[..i], &path[i + 1..]))
}

/// The 512-byte header of `e`.
pub fn header(e: &Entry) -> Result<[u8; BLOCK], Error> {
    let mut h = [0u8; BLOCK];
    let (prefix, name) = split_name(&e.path).ok_or_else(|| Error::Path { path: e.path.clone(), reason: "is too long for a package (255 bytes)".into() })?;
    h[..name.len()].copy_from_slice(name.as_bytes());
    octal(&mut h[100..108], if e.executable { 0o755 } else { 0o644 });
    octal(&mut h[108..116], 0);
    octal(&mut h[116..124], 0);
    octal(&mut h[124..136], e.data.len() as u64);
    octal(&mut h[136..148], 0);
    h[156] = b'0';
    h[257..263].copy_from_slice(b"ustar\0");
    h[263..265].copy_from_slice(b"00");
    h[345..345 + prefix.len()].copy_from_slice(prefix.as_bytes());
    h[148..156].copy_from_slice(b"        ");
    let sum: u32 = h.iter().map(|&b| u32::from(b)).sum();
    octal(&mut h[148..155], u64::from(sum));
    h[155] = b' ';
    Ok(h)
}

/// The canonical tar of `entries`, which must already be sorted by path.
pub fn write(entries: &[Entry], out: &mut impl std::io::Write) -> Result<(), Error> {
    for e in entries {
        out.write_all(&header(e)?).map_err(Error::Io)?;
        out.write_all(&e.data).map_err(Error::Io)?;
        let pad = (BLOCK - e.data.len() % BLOCK) % BLOCK;
        out.write_all(&[0u8; BLOCK][..pad]).map_err(Error::Io)?;
    }
    out.write_all(&[0u8; BLOCK * 2]).map_err(Error::Io)
}

fn parse_octal(field: &[u8], what: &str) -> Result<u64, Error> {
    let end = field.iter().position(|&b| b == 0 || b == b' ').unwrap_or(field.len());
    let digits = &field[..end];
    if field[end..].iter().any(|&b| b != 0 && b != b' ') || (digits.is_empty() && what != "mtime") {
        return Err(Error::Format(format!("the {what} field of a header is not an octal number")));
    }
    digits.iter().try_fold(0u64, |acc, &b| match b {
        b'0'..=b'7' => acc.checked_mul(8).map(|v| v + u64::from(b - b'0')).ok_or_else(|| Error::Format(format!("the {what} field is too large"))),
        _ => Err(Error::Format(format!("the {what} field of a header is not an octal number"))),
    })
}

fn text(field: &[u8], what: &str) -> Result<String, Error> {
    let end = field.iter().position(|&b| b == 0).unwrap_or(field.len());
    if field[end..].iter().any(|&b| b != 0) {
        return Err(Error::Format(format!("the {what} field of a header has bytes after its end")));
    }
    String::from_utf8(field[..end].to_vec()).map_err(|_| Error::Format(format!("the {what} field of a header is not UTF-8")))
}

/// Parse a tar produced by `write`, or any tar that uses only what `write` uses. Checks every header field it does not
/// use too, so two readers can never disagree about what a package contains.
pub fn read(tar: &[u8], mut budget: impl FnMut(&str, u64) -> Result<(), Error>) -> Result<Vec<Entry>, Error> {
    let mut out = Vec::new();
    let mut at = 0;
    loop {
        let h = tar.get(at..at + BLOCK).ok_or_else(|| Error::Format("the archive ends inside a header".into()))?;
        if h.iter().all(|&b| b == 0) {
            let next = tar.get(at + BLOCK..at + 2 * BLOCK).ok_or_else(|| Error::Format("the archive ends after one end-of-archive block".into()))?;
            if next.iter().any(|&b| b != 0) || tar[at + 2 * BLOCK..].iter().any(|&b| b != 0) {
                return Err(Error::Format("there is data after the end of the archive".into()));
            }
            return Ok(out);
        }
        let stored = parse_octal(&h[148..156], "checksum")?;
        let sum: u64 = h.iter().enumerate().map(|(i, &b)| if (148..156).contains(&i) { 32 } else { u64::from(b) }).sum();
        if stored != sum {
            return Err(Error::Format("a header checksum does not match".into()));
        }
        if &h[257..263] != b"ustar\0" || &h[263..265] != b"00" {
            return Err(Error::Format("a header is not POSIX ustar".into()));
        }
        match h[156] {
            b'0' | 0 => {}
            b'1' | b'2' => return Err(Error::Path { path: text(&h[..100], "name")?, reason: "is a link; packages contain regular files only".into() }),
            b'5' => return Err(Error::Path { path: text(&h[..100], "name")?, reason: "is a folder entry; folders are implied by file paths".into() }),
            b'x' | b'g' | b'L' | b'K' => return Err(Error::Format("the archive uses a tar extension (pax or GNU); packages use plain ustar".into())),
            t => {
                return Err(Error::Path {
                    path: text(&h[..100], "name")?,
                    reason: format!("has entry type '{}'; packages contain regular files only", t.escape_ascii()),
                });
            }
        }
        let name = text(&h[..100], "name")?;
        let prefix = text(&h[345..500], "prefix")?;
        if h[500..].iter().any(|&b| b != 0) || !text(&h[157..257], "link name")?.is_empty() {
            return Err(Error::Format("a header has data in fields packages do not use".into()));
        }
        let path = if prefix.is_empty() { name } else { format!("{prefix}/{name}") };
        let mode = parse_octal(&h[100..108], "mode")?;
        let executable = match mode & 0o7777 {
            0o644 => false,
            0o755 => true,
            m => return Err(Error::Path { path, reason: format!("has mode {m:o}; packages use 644 or 755") }),
        };
        let size = parse_octal(&h[124..136], "size")?;
        budget(&path, size)?;
        let size = usize::try_from(size).map_err(|_| Error::Format("an entry is too large".into()))?;
        let start = at + BLOCK;
        let data = tar.get(start..start + size).ok_or_else(|| Error::Format(format!("the archive ends inside {path}")))?.to_vec();
        out.push(Entry { path, data, executable });
        at = start + size.div_ceil(BLOCK) * BLOCK;
    }
}
