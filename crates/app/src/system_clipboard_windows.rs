//! Files on the Windows clipboard: the `CF_HDROP` list that Explorer's Ctrl+C leaves there.

use std::ffi::OsString;
use std::os::windows::ffi::OsStringExt;
use std::path::PathBuf;

use windows::Win32::System::DataExchange::{CloseClipboard, GetClipboardData, GetClipboardSequenceNumber, IsClipboardFormatAvailable, OpenClipboard};
use windows::Win32::UI::Shell::{DragQueryFileW, HDROP};

/// `CF_HDROP`: a list of files.
const CF_HDROP: u32 = 15;
/// More than this is not a paste somebody meant.
const MAX_FILES: u32 = 10_000;

/// Closes the clipboard when it goes out of scope: it is open for one program at a time.
struct Open;

impl Open {
    /// Another program may hold the clipboard for a moment (a clipboard manager looking at a copy): a few tries.
    fn new() -> Option<Open> {
        for _ in 0..5 {
            // SAFETY: no window owns the clipboard here; the call has no other preconditions.
            if unsafe { OpenClipboard(None) }.is_ok() {
                return Some(Open);
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        None
    }
}

impl Drop for Open {
    fn drop(&mut self) {
        // SAFETY: this value exists only while this thread has the clipboard open.
        let _ = unsafe { CloseClipboard() };
    }
}

/// A number that changes with every copy anywhere on the system.
pub fn stamp() -> isize {
    // SAFETY: no arguments, no preconditions.
    unsafe { GetClipboardSequenceNumber() as isize }
}

pub fn files() -> Vec<PathBuf> {
    let mut out = Vec::new();
    // SAFETY: no arguments, no preconditions.
    if unsafe { IsClipboardFormatAvailable(CF_HDROP) }.is_err() {
        return out;
    }
    let Some(_open) = Open::new() else { return out };
    // SAFETY: the clipboard is open; the handle stays valid (and is only read) until it is closed at the end of this function.
    unsafe {
        let Ok(handle) = GetClipboardData(CF_HDROP) else { return out };
        let drop = HDROP(handle.0);
        let count = DragQueryFileW(drop, u32::MAX, None).min(MAX_FILES);
        for index in 0..count {
            // Without a buffer the length (in UTF-16 units, without the terminator) is returned.
            let length = DragQueryFileW(drop, index, None) as usize;
            if length == 0 {
                continue;
            }
            let mut buffer = vec![0u16; length + 1];
            let got = DragQueryFileW(drop, index, Some(&mut buffer)) as usize;
            let path = PathBuf::from(OsString::from_wide(&buffer[..got.min(length)]));
            if !out.contains(&path) {
                out.push(path);
            }
        }
    }
    out
}

pub fn read() -> (isize, Vec<PathBuf>) {
    (stamp(), files())
}
