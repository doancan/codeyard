//! Running components built against a 0.x API during the move to 1.0 (docs/plugin-architecture.md §5.2). API 0.1.0
//! and 1.0.0 have the same WIT shape, so such a component differs only in the names of its `codeyard:api` imports
//! and exports (`codeyard:api/net@0.1.0`). The exthost renames them to this build's version before it compiles the
//! component; wasmtime then type-checks every import and export as usual, so a shape that did not match would fail
//! there rather than run.

use crate::{PACKAGE, TRANSITION, VERSION, Version};

/// `wasm` with every `codeyard:api/<interface>@<0.x>` import and export of the outer component renamed to
/// `@<VERSION>`; none when it has no such name (built against 1.x, or not a component at all). Only the outer
/// component's import and export sections are touched: nested components and modules are satisfied inside it.
pub fn upgrade(wasm: &[u8]) -> Option<Vec<u8>> {
    const COMPONENT_HEADER: [u8; 8] = [0x00, 0x61, 0x73, 0x6d, 0x0d, 0x00, 0x01, 0x00];
    const IMPORT_SECTION: u8 = 10;
    const EXPORT_SECTION: u8 = 11;
    if !wasm.starts_with(&COMPONENT_HEADER) {
        return None;
    }
    let to = VERSION.to_string();
    let mut out = wasm.to_vec();
    let mut changed = false;
    let mut at = COMPONENT_HEADER.len();
    while at < wasm.len() {
        let id = wasm[at];
        let (size, len) = leb128(&wasm[at + 1..])?;
        let start = at + 1 + len;
        let end = start.checked_add(size)?;
        if end > wasm.len() {
            return None;
        }
        if id == IMPORT_SECTION || id == EXPORT_SECTION {
            changed |= rename(&mut out[start..end], &to);
        }
        at = end;
    }
    changed.then_some(out)
}

/// Rename the transition versions in one section, in place: `0.1.0` and `1.0.0` have the same length, so no size in
/// the binary changes. A name is the package, a slash, an interface name and `@version`.
fn rename(section: &mut [u8], to: &str) -> bool {
    let prefix = format!("{PACKAGE}/");
    let mut changed = false;
    let mut i = 0;
    while let Some(p) = find(&section[i..], prefix.as_bytes()) {
        let start = i + p + prefix.len();
        let name_end = start + section[start..].iter().take_while(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || **b == b'-').count();
        i = name_end;
        if section.get(name_end) != Some(&b'@') {
            continue;
        }
        for from in TRANSITION {
            let from = from.to_string();
            let v = name_end + 1;
            if from.len() == to.len() && section.get(v..v + from.len()) == Some(from.as_bytes()) && !continues(section.get(v + from.len())) {
                section[v..v + to.len()].copy_from_slice(to.as_bytes());
                changed = true;
            }
        }
    }
    changed
}

/// Whether a version string goes on after this byte (`0.1.0` must not match the start of `0.1.01`).
fn continues(b: Option<&u8>) -> bool {
    b.is_some_and(|b| b.is_ascii_digit() || *b == b'.' || *b == b'-' || *b == b'+')
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

/// An unsigned LEB128 number and its length in bytes.
fn leb128(b: &[u8]) -> Option<(usize, usize)> {
    let mut v: u64 = 0;
    for (i, byte) in b.iter().enumerate().take(5) {
        v |= u64::from(byte & 0x7f) << (7 * i);
        if byte & 0x80 == 0 {
            return Some((usize::try_from(v).ok()?, i + 1));
        }
    }
    None
}

/// Whether components built against `v` run here only through `upgrade`.
pub fn needs_upgrade(v: Version) -> bool {
    v.in_transition() && !v.runs_on(VERSION)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A component header, an import section naming `names` (each as a plain import name with a placeholder type),
    /// and an export section with `export`. Only the names matter to `upgrade`.
    fn component(names: &[&str], export: &str) -> Vec<u8> {
        let mut out = vec![0x00, 0x61, 0x73, 0x6d, 0x0d, 0x00, 0x01, 0x00];
        let mut imports = vec![names.len() as u8];
        for n in names {
            imports.extend([0x00, n.len() as u8]);
            imports.extend(n.as_bytes());
            imports.extend([0x05, 0x00]);
        }
        out.push(10);
        out.push(imports.len() as u8);
        out.extend(imports);
        let mut exports = vec![1, 0x00, export.len() as u8];
        exports.extend(export.as_bytes());
        exports.extend([0x05, 0x00, 0x00]);
        out.push(11);
        out.push(exports.len() as u8);
        out.extend(exports);
        out
    }

    #[test]
    fn a_0x_component_is_renamed_in_place() {
        let old = component(&["codeyard:api/net@0.1.0", "wasi:io/streams@0.2.6", "codeyard:api/types@0.1.0"], "codeyard:api/guest@0.1.0");
        let new = upgrade(&old).expect("renamed");
        assert_eq!(new, component(&["codeyard:api/net@1.0.0", "wasi:io/streams@0.2.6", "codeyard:api/types@1.0.0"], "codeyard:api/guest@1.0.0"));
        assert_eq!(old.len(), new.len());
    }

    #[test]
    fn current_and_unknown_versions_are_left_alone() {
        assert_eq!(upgrade(&component(&["codeyard:api/net@1.0.0"], "codeyard:api/guest@1.0.0")), None);
        assert_eq!(upgrade(&component(&["codeyard:api/net@0.2.0"], "codeyard:api/guest@0.2.0")), None);
        assert_eq!(upgrade(&component(&["codeyard:api/net@0.1.01"], "x")), None);
        assert_eq!(upgrade(b"\0asm\x01\0\0\0"), None);
        assert_eq!(upgrade(b"not wasm"), None);
        // A truncated section is not walked past its end.
        let mut cut = component(&["codeyard:api/net@0.1.0"], "codeyard:api/guest@0.1.0");
        cut.truncate(20);
        assert_eq!(upgrade(&cut), None);
    }

    #[test]
    fn the_transition_is_0_1() {
        assert!(needs_upgrade(Version(0, 1, 0)));
        assert!(!needs_upgrade(Version(1, 0, 0)));
        assert!(!needs_upgrade(Version(0, 2, 0)));
        assert!(!Version(0, 2, 0).in_transition());
    }
}
