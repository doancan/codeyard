//! Identity of multiply linked files; regular files keep stable path-based save locks across atomic replacements.

use std::io;
use std::path::Path;

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
pub(super) struct Identity {
    volume: u64,
    index: u128,
}

pub(super) fn hard_link(path: &Path) -> io::Result<Option<Identity>> {
    match linked_identity(path) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        result => result,
    }
}

#[cfg(unix)]
fn linked_identity(path: &Path) -> io::Result<Option<Identity>> {
    use std::os::unix::fs::MetadataExt;
    let metadata = std::fs::metadata(path)?;
    Ok((metadata.nlink() > 1).then_some(Identity { volume: metadata.dev(), index: u128::from(metadata.ino()) }))
}

#[cfg(windows)]
fn linked_identity(path: &Path) -> io::Result<Option<Identity>> {
    use std::os::windows::fs::OpenOptionsExt;
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{
        BY_HANDLE_FILE_INFORMATION, FILE_FLAG_BACKUP_SEMANTICS, FILE_ID_INFO, FILE_READ_ATTRIBUTES, FileIdInfo, GetFileInformationByHandle,
        GetFileInformationByHandleEx,
    };
    // Attribute access is sufficient; a write-only document need not be opened for reading its contents.
    let file = std::fs::OpenOptions::new().access_mode(FILE_READ_ATTRIBUTES).custom_flags(FILE_FLAG_BACKUP_SEMANTICS).open(path)?;
    let mut info = BY_HANDLE_FILE_INFORMATION::default();
    // SAFETY: the handle stays live throughout the call; info is a correctly sized writable output buffer.
    if unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut info) } == 0 {
        return Err(io::Error::last_os_error());
    }
    if info.nNumberOfLinks <= 1 {
        return Ok(None);
    }
    let mut extended = FILE_ID_INFO::default();
    // SAFETY: the handle and output buffer are live; the class and buffer size match FILE_ID_INFO.
    let extended_ok = unsafe {
        GetFileInformationByHandleEx(file.as_raw_handle(), FileIdInfo, (&mut extended as *mut FILE_ID_INFO).cast(), std::mem::size_of::<FILE_ID_INFO>() as u32)
    };
    Ok(Some(if extended_ok != 0 {
        Identity { volume: extended.VolumeSerialNumber, index: u128::from_le_bytes(extended.FileId.Identifier) }
    } else {
        // Older filesystems expose only the legacy 64-bit file index.
        Identity { volume: u64::from(info.dwVolumeSerialNumber), index: (u128::from(info.nFileIndexHigh) << 32) | u128::from(info.nFileIndexLow) }
    }))
}

#[cfg(not(any(unix, windows)))]
fn linked_identity(_: &Path) -> io::Result<Option<Identity>> {
    Ok(None)
}
