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

/// The arguments or result of one API call (a file read, a fetch's request body, a view's tree).
pub const CALL_BYTES: usize = 8 * 1024 * 1024;
/// One frame between the app and an exthost: a call plus room for its envelope. A larger frame kills the exthost.
pub const FRAME_BYTES: usize = CALL_BYTES + 64 * 1024;
/// Sustained bytes per second an exthost may send the app; going over kills it.
pub const IPC_BYTES_PER_SECOND: u64 = 32 * 1024 * 1024;
/// Bytes an exthost may send in a burst above the sustained rate.
pub const IPC_BURST_BYTES: u64 = 64 * 1024 * 1024;
/// The shortest interval of `timers.every`.
pub const TIMER_MIN_SECONDS: u32 = 30;
/// Linear memory of one extension instance; growing past it traps.
pub const GUEST_MEMORY_BYTES: usize = 256 * 1024 * 1024;
/// The whole exthost process: the guest's linear memory plus the engine and compiled code. A Job Object commit limit
/// on Windows; elsewhere the app checks the process's resident size.
pub const EXTHOST_MEMORY_BYTES: u64 = 512 * 1024 * 1024;
/// CPU time one call into an extension may take (exports must return quickly; long work waits on tickets).
pub const GUEST_CALL_MILLIS: u64 = 2_000;
/// `activate` may take longer: language runtimes start there.
pub const ACTIVATE_MILLIS: u64 = 10_000;
/// How long the app waits for a provider (formatter, code actions, tasks) to answer a request. A later answer is dropped
/// and noted in the extension's log; the user's typing never waits for it.
pub const PROVIDER_DEADLINE_MILLIS: u64 = 1_000;
/// Text of a status bar item.
pub const STATUS_TEXT_LEN: usize = 64;
/// Diagnostics or decorations one extension may set on one file; the rest are dropped.
pub const MARKS_PER_FILE: usize = 1_000;
/// One `net` request, from the first connection to the last byte of the response, redirects included.
pub const NET_TIMEOUT_SECS: u64 = 60;
/// Redirects one `net` request follows; each is checked against `net` and the peer address rules again.
pub const NET_REDIRECTS: usize = 5;
/// The body one `net` request sends.
pub const NET_REQUEST_BYTES: u64 = 16 * 1024 * 1024;
/// The body one `net` request receives.
pub const NET_RESPONSE_BYTES: u64 = 64 * 1024 * 1024;

/// An encoded view (`ext-ui`), refused before it is decoded.
pub const UI_VIEW_BYTES: usize = 8 * 1024 * 1024;
/// Elements in one view, counting every row and cell.
pub const UI_NODES: usize = 50_000;
/// Nesting depth of a view; the root is depth 1.
pub const UI_DEPTH: usize = 32;
/// Rows of a list or table, and items in one page of a tree level.
pub const UI_ROWS_PER_PAGE: usize = 10_000;
/// A label, heading, badge, option or column header.
pub const UI_LABEL_LEN: usize = 256;
/// Body text: `text`, `banner`, a key-value value, an input's value or placeholder.
pub const UI_TEXT_LEN: usize = 4096;
/// The source of one `markdown` or `code` element, in bytes.
pub const UI_DOCUMENT_BYTES: usize = 64 * 1024;
/// An element key or a tree page token.
pub const UI_KEY_LEN: usize = 128;
/// An `image` given as bytes.
pub const UI_IMAGE_BYTES: usize = 1024 * 1024;
/// Options of a `select`, pairs of a `key-value`.
pub const UI_CHOICES: usize = 256;
/// Columns of a `table`.
pub const UI_COLUMNS: usize = 32;
/// An `https:` URL of an image or link.
pub const UI_URL_LEN: usize = 2048;
