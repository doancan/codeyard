//! Paths in tests spelled the way the app spells a local path.
//!
//! The app keeps a canonical local path in one form on every system (`crate::ui::local_path`): on Windows `C:/dir/file`,
//! without the `\\?\` prefix `std::fs::canonicalize` adds and with `/` separators. A test that compares a path from the
//! app with one it built itself builds it with these.

use std::path::Path;

pub trait AppPath {
    /// The path as the app shows and keys it.
    fn app(&self) -> String;
}

impl AppPath for Path {
    fn app(&self) -> String {
        crate::ui::local_path(self)
    }
}
