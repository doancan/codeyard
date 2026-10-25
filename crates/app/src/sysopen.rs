//! "Open" requests from the system: Finder's "Open With", dropping a file or folder on the Dock icon, `open -a Codeyard x`.
//! macOS delivers them as `file://` URLs; they become paths and open in the right window.

use std::path::Path;

use gpui::{App, WindowHandle};

use crate::workbench::Workbench;

/// Paths from `file://` URLs (percent-escapes decoded, `localhost` host accepted, trailing `/` of a folder URL
/// dropped). Other schemes (https, mailto…) and undecodable URLs are skipped.
pub fn paths_from_urls(urls: &[String]) -> Vec<String> {
    urls.iter()
        .filter_map(|u| {
            let path = crate::lsp::uri_to_path(u)?;
            let trimmed = path.trim_end_matches('/');
            let drive_root = trimmed.len() == 2 && trimmed.as_bytes()[0].is_ascii_alphabetic() && trimmed.ends_with(':') && path.ends_with('/');
            let path = if trimmed.is_empty() {
                "/".to_string()
            } else if drive_root {
                format!("{trimmed}/")
            } else {
                trimmed.to_string()
            };
            let drive_absolute = path.len() >= 3 && path.as_bytes()[0].is_ascii_alphabetic() && path.as_bytes()[1..3] == *b":/";
            ((path.starts_with('/') || drive_absolute) && !path.contains('\0')).then_some(path)
        })
        .collect()
}

/// Apply the request. File: becomes a tab of the open window (project stays). Folder: if open in another window, raise it;
/// if the active window has no folder, open it there, else in a new window.
pub fn open_urls(urls: Vec<String>, cx: &mut App) {
    let windows = |cx: &App| -> Vec<WindowHandle<Workbench>> { cx.windows().into_iter().filter_map(|w| w.downcast::<Workbench>()).collect() };
    for path in paths_from_urls(&urls) {
        let abs = std::fs::canonicalize(&path).map_or(path.clone(), |p| crate::ui::local_path(&p));
        let is_folder = Path::new(&abs).is_dir();
        let all = windows(cx);
        let active = cx.active_window().and_then(|w| w.downcast::<Workbench>()).or_else(|| all.first().copied());
        if is_folder {
            if let Some(w) = all.iter().find(|w| w.read(cx).is_ok_and(|wb| wb.local_folder(cx).as_deref() == Some(abs.as_str()))) {
                w.update(cx, |_, window, _| window.activate_window()).ok();
                continue;
            }
            let target = match active.filter(|w| w.read(cx).is_ok_and(|wb| wb.local_folder(cx).is_none())) {
                Some(w) => Some(w),
                None => crate::open_workbench_window(cx),
            };
            if let Some(w) = target {
                w.update(cx, |wb, window, cx| {
                    wb.open_path(&abs, window, cx);
                    window.activate_window();
                })
                .ok();
            }
            continue;
        }
        let target = match active {
            Some(w) => Some(w),
            None => crate::open_workbench_window(cx),
        };
        if let Some(w) = target {
            w.update(cx, |wb, window, cx| {
                wb.open_external_file(&abs, window, cx);
                window.activate_window();
            })
            .ok();
        }
    }
    cx.activate(true);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(urls: &[&str]) -> Vec<String> {
        paths_from_urls(&urls.iter().map(|s| s.to_string()).collect::<Vec<_>>())
    }

    #[test]
    fn file_urls_become_paths() {
        assert_eq!(p(&["file:///Users/a/My%20Project/main.rs"]), ["/Users/a/My Project/main.rs"]);
        assert_eq!(p(&["file:///Users/a/dir/"]), ["/Users/a/dir"], "Finder gives folders a trailing slash");
        assert_eq!(p(&["file://localhost/tmp/x.txt"]), ["/tmp/x.txt"]);
        assert_eq!(p(&["file:///"]), ["/"]);
        assert_eq!(p(&["file:///caf%C3%A9/%C4%B1.txt"]), ["/café/ı.txt"], "UTF-8 sequences decode");
    }

    #[test]
    fn other_schemes_and_junk_are_ignored() {
        assert!(p(&["https://example.com/a", "mailto:a@b.c", "", "file:relative", "file://%FF%FE", "file:///a%00b"]).is_empty());
        assert_eq!(p(&["https://x/", "file:///ok"]), ["/ok"], "valid entries survive next to invalid ones");
    }

    #[test]
    fn windows_file_urls_keep_drive_roots_and_network_shares() {
        assert_eq!(p(&["file:///C:/work/a%20b.rs", "file://LOCALHOST/D:/work/", "file:///C:/"]), ["C:/work/a b.rs", "D:/work", "C:/"]);
        assert_eq!(p(&["file://server/share/dir/"]), ["//server/share/dir"]);
        assert!(p(&["file://C:relative", "file:///C:relative", "file:///C:/a%00b"]).is_empty());
    }
}
