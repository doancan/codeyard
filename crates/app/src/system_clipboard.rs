//! Files on the system clipboard: what Finder's ⌘C leaves there. gpui's clipboard carries text and images only, so the
//! Explorer's own ⌘C / ⌘V (`file_batch::Clipboard`) never saw them. Read on macOS; Windows and Linux report none yet.
//! Tests never touch the real clipboard: `set_for_test` stands in for it.

use std::path::PathBuf;

/// A number that changes whenever anything is copied anywhere. A paste compares it with the one the Explorer noted at
/// its own copy to tell which of the two clipboards is newer.
#[cfg(not(test))]
pub fn stamp() -> isize {
    imp::stamp()
}

/// The files on the clipboard that still exist, in order.
#[cfg(not(test))]
pub fn files() -> Vec<PathBuf> {
    imp::files().into_iter().filter(|p| p.exists()).collect()
}

#[cfg(test)]
thread_local! {
    static FAKE: std::cell::RefCell<(Vec<PathBuf>, isize)> = const { std::cell::RefCell::new((Vec::new(), 0)) };
}

#[cfg(test)]
pub fn stamp() -> isize {
    FAKE.with(|f| f.borrow().1)
}

#[cfg(test)]
pub fn files() -> Vec<PathBuf> {
    FAKE.with(|f| f.borrow().0.iter().filter(|p| p.exists()).cloned().collect())
}

/// The clipboard as a test wants it: these files, and this change number.
#[cfg(test)]
pub fn set_for_test(files: Vec<PathBuf>, stamp: isize) {
    FAKE.with(|f| *f.borrow_mut() = (files, stamp));
}

// In test builds `stamp` and `files` answer from `FAKE`, so the real ones go unused there.
#[cfg_attr(test, allow(dead_code))]
#[cfg(target_os = "macos")]
mod imp {
    use std::ffi::CStr;
    use std::os::raw::c_char;
    use std::path::PathBuf;

    use objc2::runtime::AnyObject;
    use objc2::{class, msg_send};

    pub type Pasteboard = *mut AnyObject;

    pub fn general() -> Pasteboard {
        // SAFETY: `+[NSPasteboard generalPasteboard]` takes no arguments and returns the shared pasteboard.
        unsafe { msg_send![class!(NSPasteboard), generalPasteboard] }
    }

    pub fn stamp() -> isize {
        stamp_of(general())
    }

    pub fn files() -> Vec<PathBuf> {
        files_in(general())
    }

    pub fn stamp_of(pasteboard: Pasteboard) -> isize {
        // SAFETY: `pasteboard` is a live NSPasteboard; `changeCount` returns an NSInteger.
        unsafe { msg_send![pasteboard, changeCount] }
    }

    fn ns_string(text: &CStr) -> *mut AnyObject {
        // SAFETY: `text` is NUL-terminated; the result is an autoreleased NSString.
        unsafe { msg_send![class!(NSString), stringWithUTF8String: text.as_ptr()] }
    }

    fn to_string(string: *mut AnyObject) -> Option<String> {
        if string.is_null() {
            return None;
        }
        // SAFETY: `string` is a live NSString; `UTF8String` is NUL-terminated and valid for the autorelease scope.
        unsafe {
            let utf8: *const c_char = msg_send![string, UTF8String];
            (!utf8.is_null()).then(|| CStr::from_ptr(utf8).to_string_lossy().into_owned())
        }
    }

    /// `public.file-url` of each pasteboard item (Finder writes one per file), as a plain path. `filePathURL` also turns the old
    /// `file:///.file/id=…` reference form into a path and gives nil for anything that is not a file.
    pub fn files_in(pasteboard: Pasteboard) -> Vec<PathBuf> {
        let mut out: Vec<PathBuf> = Vec::new();
        // SAFETY: every receiver below is either a live object returned by the call before it or checked for nil first;
        // `objectAtIndex:` is only called with an index below `count`.
        unsafe {
            let items: *mut AnyObject = msg_send![pasteboard, pasteboardItems];
            if items.is_null() {
                return out;
            }
            let count: usize = msg_send![items, count];
            let kind = ns_string(c"public.file-url");
            for i in 0..count.min(10_000) {
                let item: *mut AnyObject = msg_send![items, objectAtIndex: i];
                let text: *mut AnyObject = msg_send![item, stringForType: kind];
                if text.is_null() {
                    continue;
                }
                let url: *mut AnyObject = msg_send![class!(NSURL), URLWithString: text];
                if url.is_null() {
                    continue;
                }
                let file: *mut AnyObject = msg_send![url, filePathURL];
                if file.is_null() {
                    continue;
                }
                let path: *mut AnyObject = msg_send![file, path];
                if let Some(path) = to_string(path).filter(|p| p.starts_with('/')) {
                    let path = PathBuf::from(path);
                    if !out.contains(&path) {
                        out.push(path);
                    }
                }
            }
        }
        out
    }

    #[cfg(test)]
    pub mod tests {
        use super::*;

        /// Put one `public.file-url` item per path on `pasteboard`, as Finder does.
        pub fn write_files(pasteboard: Pasteboard, paths: &[&str]) {
            // SAFETY: plain Foundation/AppKit calls on objects created here; the leaked items live as long as the test.
            unsafe {
                let _: isize = msg_send![pasteboard, clearContents];
                let list: *mut AnyObject = msg_send![class!(NSMutableArray), array];
                for path in paths {
                    let item: *mut AnyObject = msg_send![class!(NSPasteboardItem), new];
                    let url = std::ffi::CString::new(schema::file_uri::from_path(path)).unwrap();
                    let _: bool = msg_send![item, setString: ns_string(&url), forType: ns_string(c"public.file-url")];
                    let _: () = msg_send![list, addObject: item];
                }
                let written: bool = msg_send![pasteboard, writeObjects: list];
                assert!(written, "the pasteboard refused the items");
            }
        }

        pub fn private_pasteboard() -> Pasteboard {
            // SAFETY: returns a pasteboard of its own, so the user's clipboard is not touched.
            unsafe { msg_send![class!(NSPasteboard), pasteboardWithUniqueName] }
        }
    }
}

#[cfg_attr(test, allow(dead_code))]
#[cfg(not(target_os = "macos"))]
mod imp {
    use std::path::PathBuf;

    pub fn stamp() -> isize {
        0
    }

    pub fn files() -> Vec<PathBuf> {
        Vec::new()
    }
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::imp::{files_in, stamp_of, tests::*};
    use std::path::PathBuf;

    #[test]
    fn finder_style_file_urls_come_back_as_paths_and_a_new_copy_changes_the_stamp() {
        let pasteboard = private_pasteboard();
        assert!(files_in(pasteboard).is_empty(), "an empty pasteboard has no files");
        let before = stamp_of(pasteboard);
        write_files(pasteboard, &["/tmp/Deneme klasörü/ğüş #1.txt", "/tmp/b.txt", "/tmp/b.txt"]);
        assert_eq!(files_in(pasteboard), [PathBuf::from("/tmp/Deneme klasörü/ğüş #1.txt"), PathBuf::from("/tmp/b.txt")], "order kept, duplicates dropped");
        assert_ne!(stamp_of(pasteboard), before, "copying changes the stamp");
    }

    #[test]
    fn text_on_the_pasteboard_is_not_a_file() {
        let pasteboard = private_pasteboard();
        // SAFETY: a plain string on a private pasteboard.
        unsafe {
            use objc2::runtime::AnyObject;
            use objc2::{class, msg_send};
            let _: isize = msg_send![pasteboard, clearContents];
            let text: *mut AnyObject = msg_send![class!(NSString), stringWithUTF8String: c"/tmp/not-a-file.txt".as_ptr()];
            let kind: *mut AnyObject = msg_send![class!(NSString), stringWithUTF8String: c"public.utf8-plain-text".as_ptr()];
            let _: bool = msg_send![pasteboard, setString: text, forType: kind];
        }
        assert!(files_in(pasteboard).is_empty());
    }
}
