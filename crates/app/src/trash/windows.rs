//! The Shell requires an STA; GPUI's worker threads may already be initialized as MTA.
//! Keep the operation and all COM objects on a short-lived, dedicated thread.

use std::io;
use std::os::windows::ffi::OsStrExt;
use std::path::Path;

use ::windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx, CoUninitialize};
use ::windows::Win32::UI::Shell::{
    FOF_NO_UI, FOF_WANTNUKEWARNING, FOFX_ADDUNDORECORD, FOFX_EARLYFAILURE, FOFX_RECYCLEONDELETE, FileOperation, IFileOperation, IShellItem,
    SHCreateItemFromParsingName,
};
use ::windows::core::PCWSTR;

struct Apartment;

impl Apartment {
    fn new() -> ::windows::core::Result<Self> {
        // SAFETY: this thread owns its COM initialization and balances every successful call in Drop.
        unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED).ok()? };
        Ok(Self)
    }
}

impl Drop for Apartment {
    fn drop(&mut self) {
        // SAFETY: constructed only after a successful CoInitializeEx, on this same thread.
        unsafe { CoUninitialize() };
    }
}

pub(super) fn recycle(path: &Path) -> io::Result<()> {
    // Do not canonicalize the leaf: deleting a symlink must recycle the link, not its target.
    let path = std::path::absolute(path)?;
    std::fs::symlink_metadata(&path)?;
    std::thread::Builder::new()
        .name("codeyard-recycle".into())
        .spawn(move || recycle_on_thread(&path))?
        .join()
        .map_err(|_| io::Error::other("Recycle Bin worker failed"))?
}

fn shell_path(path: &Path) -> Vec<u16> {
    // The Shell does not accept Rust's verbatim prefixes. Keep the original UTF-16 filename intact.
    use std::ffi::OsString;
    let wide: Vec<u16> = path.as_os_str().encode_wide().collect();
    let unc: Vec<u16> = r"\\?\UNC\".encode_utf16().collect();
    let verbatim: Vec<u16> = r"\\?\".encode_utf16().collect();
    let path = if wide.starts_with(&unc) {
        use std::os::windows::ffi::OsStringExt;
        OsString::from_wide(&[&[b'\\' as u16, b'\\' as u16][..], &wide[unc.len()..]].concat())
    } else if wide.starts_with(&verbatim) {
        use std::os::windows::ffi::OsStringExt;
        OsString::from_wide(&wide[verbatim.len()..])
    } else {
        path.as_os_str().to_owned()
    };
    path.encode_wide().chain(Some(0)).collect()
}

fn recycle_on_thread(path: &Path) -> io::Result<()> {
    let run = || -> ::windows::core::Result<()> {
        let _apartment = Apartment::new()?;
        let name = shell_path(path);
        // SAFETY: name is NUL-terminated and alive throughout the call; COM objects stay in this apartment.
        unsafe {
            let item: IShellItem = SHCreateItemFromParsingName(PCWSTR(name.as_ptr()), None)?;
            let operation: IFileOperation = CoCreateInstance(&FileOperation, None, CLSCTX_INPROC_SERVER)?;
            // WANTNUKEWARNING overrides NO_UI's automatic confirmation if recycling is impossible.
            // NOERRORUI + EARLYFAILURE returns errors to our own permanent-delete confirmation.
            operation.SetOperationFlags(FOF_NO_UI | FOF_WANTNUKEWARNING | FOFX_RECYCLEONDELETE | FOFX_ADDUNDORECORD | FOFX_EARLYFAILURE)?;
            operation.DeleteItem(&item, None)?;
            operation.PerformOperations()?;
            if operation.GetAnyOperationsAborted()?.as_bool() {
                return Err(::windows::core::Error::from_hresult(::windows::core::HRESULT(0x800704C7_u32 as i32)));
            }
        }
        Ok(())
    };
    run().map_err(io::Error::other)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ::windows::Win32::System::Com::CoTaskMemFree;
    use ::windows::Win32::UI::Shell::{
        BHID_EnumItems, FOLDERID_RecycleBinFolder, IEnumShellItems, KF_FLAG_DEFAULT, SHGetKnownFolderItem, SIGDN_PARENTRELATIVE,
    };

    #[test]
    fn windows_recycle_bin_preserves_and_restores_files_and_folders() {
        let dir = std::env::temp_dir().join(format!(
            "codeyard-recycle-{}-{}",
            std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let name = format!("codeyard-şablon boşluk-{}", dir.file_name().unwrap().to_string_lossy());
        let file = dir.join(format!("{name}.txt"));
        std::fs::write(&file, "recoverable çığ").unwrap();
        let folder = dir.join(&name);
        std::fs::create_dir_all(folder.join("iç")).unwrap();
        std::fs::write(folder.join("iç/a.txt"), "nested").unwrap();
        super::super::move_to_trash(&file).unwrap();
        super::super::move_to_trash(&folder).unwrap();
        assert!(!file.exists() && !folder.exists());

        // Restore only these uniquely named fixtures, through the same Shell API Explorer uses.
        let restore_dir = dir.clone();
        std::thread::spawn(move || unsafe {
            let _apartment = Apartment::new().unwrap();
            let bin: IShellItem = SHGetKnownFolderItem(&FOLDERID_RecycleBinFolder, KF_FLAG_DEFAULT, None).unwrap();
            let items: IEnumShellItems = bin.BindToHandler(None, &BHID_EnumItems).unwrap();
            let target_name = shell_path(&restore_dir);
            let target: IShellItem = SHCreateItemFromParsingName(PCWSTR(target_name.as_ptr()), None).unwrap();
            let operation: IFileOperation = CoCreateInstance(&FileOperation, None, CLSCTX_INPROC_SERVER).unwrap();
            operation.SetOperationFlags(FOF_NO_UI | FOFX_EARLYFAILURE).unwrap();
            let mut found = 0;
            loop {
                let mut next = [None];
                let mut fetched = 0;
                items.Next(&mut next, Some(&mut fetched)).unwrap();
                if fetched == 0 {
                    break;
                }
                let item = next[0].take().unwrap();
                let display = item.GetDisplayName(SIGDN_PARENTRELATIVE).unwrap();
                let label = display.to_string().unwrap();
                CoTaskMemFree(Some(display.0.cast()));
                if label == name || label == format!("{name}.txt") {
                    operation.MoveItem(&item, &target, PCWSTR::null(), None).unwrap();
                    found += 1;
                }
            }
            assert_eq!(found, 2);
            operation.PerformOperations().unwrap();
            assert!(!operation.GetAnyOperationsAborted().unwrap().as_bool());
        })
        .join()
        .unwrap();
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "recoverable çığ");
        assert_eq!(std::fs::read_to_string(folder.join("iç/a.txt")).unwrap(), "nested");
        assert!(super::super::move_to_trash(&dir.join("missing")).is_err());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn windows_shell_paths_strip_verbatim_prefixes_without_losing_unicode() {
        let decode = |path| String::from_utf16(&shell_path(Path::new(path))).unwrap().trim_end_matches('\0').to_string();
        assert_eq!(decode(r"\\?\C:\şablon boşluk\a.txt"), r"C:\şablon boşluk\a.txt");
        assert_eq!(decode(r"\\?\UNC\server\share\şablon.txt"), r"\\server\share\şablon.txt");
    }
}
