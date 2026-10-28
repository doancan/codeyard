//! Bounded preview reads through directory handles. Path checks alone cannot prevent rename races.
use cap_fs_ext::{DirExt, FollowSymlinks, OpenOptionsFollowExt, OpenOptionsMaybeDirExt, OpenOptionsSyncExt};
use cap_std::fs::{Dir, File, Metadata, OpenOptions};
use proto::{Error, Response};
use std::io::{self, Read};
use std::path::Path;

pub(super) fn read(root: &Path, relative: &str) -> Result<Response, Error> {
    let file = open(root, relative)?;
    read_opened(file, &root.join(relative))
}

fn permitted(metadata: &Metadata, path: &Path) -> Result<(), Error> {
    let kind = metadata.file_type();
    #[cfg(windows)]
    {
        use cap_std::fs::MetadataExt;
        use windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT;
        if metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return Err(Error::PermissionDenied(path.display().to_string()));
        }
    }
    if kind.is_symlink() || !(kind.is_file() || kind.is_dir()) {
        return Err(Error::PermissionDenied(path.display().to_string()));
    }
    Ok(())
}

fn open(root: &Path, relative: &str) -> Result<File, Error> {
    // Validate at the agent boundary too, including Windows alternate streams and device paths.
    if !root.is_absolute()
        || relative.is_empty()
        || relative.len() > 16 * 1024
        || relative.chars().any(|c| c.is_control() || matches!(c, '\\' | ':'))
        || relative.split('/').any(invalid_component)
    {
        return Err(Error::PermissionDenied(relative.into()));
    }
    let map = |e| super::map_err(&root.join(relative), e);
    // The configured root's ancestors may contain OS aliases (e.g. /var on macOS).
    // Refuse a symlink at the root itself and resolve children through its directory handle.
    let mut dir = if let (Some(parent), Some(name)) = (root.parent(), root.file_name()) {
        let parent = Dir::open_ambient_dir(parent, cap_std::ambient_authority()).map_err(map)?;
        permitted(&parent.symlink_metadata(name).map_err(map)?, root)?;
        parent.open_dir_nofollow(name).map_err(map)?
    } else {
        Dir::open_ambient_dir(root, cap_std::ambient_authority()).map_err(map)?
    };
    permitted(&dir.dir_metadata().map_err(map)?, root)?;
    let mut parts = relative.split('/').peekable();
    while let Some(part) = parts.next() {
        // This check provides consistent errors and avoids opening special devices. The nofollow
        // open and handle metadata below enforce the boundary if the entry changes after this check.
        permitted(&dir.symlink_metadata(part).map_err(map)?, &root.join(relative))?;
        if parts.peek().is_some() {
            dir = dir.open_dir_nofollow(part).map_err(map)?;
            permitted(&dir.dir_metadata().map_err(map)?, &root.join(relative))?;
        } else {
            let mut options = OpenOptions::new();
            options.read(true).follow(FollowSymlinks::No).maybe_dir(true).nonblock(true);
            return dir.open_with(part, &options).map_err(map);
        }
    }
    Err(Error::PermissionDenied(relative.into()))
}

fn invalid_component(part: &str) -> bool {
    if part.is_empty() || part.starts_with('.') || part.ends_with('.') || part.ends_with(' ') {
        return true;
    }
    // Windows recognizes DOS device names even with an extension. Reject them on every host.
    let stem = part.split('.').next().unwrap_or("").trim_end_matches(' ').to_ascii_uppercase();
    matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$")
        || stem
            .strip_prefix("COM")
            .or_else(|| stem.strip_prefix("LPT"))
            .is_some_and(|n| matches!(n, "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "¹" | "²" | "³"))
}

fn read_opened(mut file: File, path: &Path) -> Result<Response, Error> {
    let metadata = file.metadata().map_err(|e| super::map_err(path, e))?;
    permitted(&metadata, path)?;
    if metadata.is_dir() {
        return Err(Error::IsDirectory(path.display().to_string()));
    }
    let too_large = |size| Error::TooLarge { path: path.display().to_string(), size, limit: proto::MAX_FILE };
    if metadata.len() > proto::MAX_FILE {
        return Err(too_large(metadata.len()));
    }
    let mut data = Vec::with_capacity(metadata.len() as usize);
    io::Read::take(&mut file, proto::MAX_FILE + 1).read_to_end(&mut data).map_err(|e| super::map_err(path, e))?;
    if data.len() as u64 > proto::MAX_FILE {
        return Err(too_large(data.len() as u64));
    }
    let hash = proto::content_hash(&data);
    Ok(Response::File { data, hash })
}

#[cfg(test)]
#[path = "preview_tests.rs"]
mod tests;
