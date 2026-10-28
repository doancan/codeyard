//! Every size and count limit of the extension platform, in one table. The author docs (`reference/limits.md`) and the
//! performance rows are generated from here, so a limit is changed in one place.

/// `codeyard-extension.toml` larger than this is refused before it is parsed.
pub const MANIFEST_BYTES: usize = 64 * 1024;
/// `publisher.name` as a whole.
pub const ID_LEN: usize = 64;
/// One part of an id (publisher, name, or a contribution's local id).
pub const ID_PART_LEN: usize = 40;
pub const NAME_LEN: usize = 64;
pub const DESCRIPTION_LEN: usize = 300;
/// The author's `why`, quoted on the consent sheet.
pub const WHY_LEN: usize = 200;
pub const CATEGORIES: usize = 5;
/// Entries of one contribution kind (`[[commands]]`, `[[themes]]`, …).
pub const CONTRIBUTIONS_PER_KIND: usize = 256;
/// Entries of one permission list (`read`, `net`, `run`, …).
pub const PERMISSION_ENTRIES: usize = 64;
pub const DEPENDENCIES: usize = 32;
/// Length of a glob, host, program, path or title.
pub const ITEM_LEN: usize = 256;
/// Length of a `when` / `enablement` expression.
pub const WHEN_LEN: usize = 512;
/// Nesting depth of a `when` expression (parentheses and `!`).
pub const WHEN_DEPTH: usize = 16;

/// A `.cyx` package, compressed.
pub const PACKAGE_BYTES: u64 = 64 * 1024 * 1024;
/// A `.cyx` package, unpacked.
pub const PACKAGE_UNPACKED_BYTES: u64 = 256 * 1024 * 1024;
pub const PACKAGE_FILES: usize = 10_000;
/// One file inside a package.
pub const PACKAGE_FILE_BYTES: u64 = 64 * 1024 * 1024;
/// An extension's own data directory (`storage`).
pub const DATA_DIR_BYTES: u64 = 50 * 1024 * 1024;
