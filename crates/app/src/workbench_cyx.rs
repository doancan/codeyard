//! "Extensions: Install cyx Command in PATH": put the `cyx` that ships beside Codeyard on the user's PATH, so
//! `cyx new` and `cyx dev` work in any terminal (docs/extensions/developer-guide.md, Quick start).

use super::*;

/// Where the link goes: the first of these that is writable without asking for a password.
fn link_dirs() -> Vec<std::path::PathBuf> {
    let mut v = vec![std::path::PathBuf::from("/usr/local/bin")];
    if let Some(home) = dirs::home_dir() {
        v.push(home.join(".local").join("bin"));
    }
    v
}

/// The bundled `cyx`: beside the running Codeyard (the app bundle's MacOS folder, the Windows install folder).
pub(super) fn bundled_cyx() -> Option<std::path::PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let p = exe.parent()?.join(if cfg!(windows) { "cyx.exe" } else { "cyx" });
    p.is_file().then_some(p)
}

/// Link `target` as `<dir>/cyx`, replacing an older link but never a real file. Returns the link's folder.
#[cfg(unix)]
pub(super) fn install_link(target: &std::path::Path, dirs: &[std::path::PathBuf]) -> Result<std::path::PathBuf, String> {
    let mut last = String::new();
    for dir in dirs {
        if std::fs::create_dir_all(dir).is_err() {
            continue;
        }
        let link = dir.join("cyx");
        match std::fs::symlink_metadata(&link) {
            Ok(m) if !m.file_type().is_symlink() => return Err(crate::i18n::tf("{} exists and is not a link; remove it first", &[&link.display()])),
            Ok(_) => {
                if let Err(e) = std::fs::remove_file(&link) {
                    last = e.to_string();
                    continue;
                }
            }
            Err(_) => {}
        }
        match std::os::unix::fs::symlink(target, &link) {
            Ok(()) => return Ok(dir.clone()),
            Err(e) => last = e.to_string(),
        }
    }
    Err(last)
}

impl Workbench {
    pub(super) fn install_cyx(&mut self, _: &InstallCyx, _: &mut Window, cx: &mut Context<Self>) {
        let notice = match bundled_cyx() {
            None => Notice::Error(crate::i18n::t("cyx is not next to Codeyard in this build; install it with cargo install --path crates/cyx").into()),
            Some(cyx) => Self::link_cyx(&cyx),
        };
        self.notice = Some(notice);
        cx.notify();
    }

    #[cfg(unix)]
    fn link_cyx(cyx: &std::path::Path) -> Notice {
        match install_link(cyx, &link_dirs()) {
            Ok(dir) => {
                let on_path = std::env::var_os("PATH").is_some_and(|p| std::env::split_paths(&p).any(|d| d == dir));
                if on_path {
                    Notice::Info(crate::i18n::tf("cyx is installed in {}: run cyx new theme my-theme in a terminal", &[&dir.display()]).into())
                } else {
                    Notice::Info(crate::i18n::tf("cyx is installed in {}; add that folder to PATH in your shell profile", &[&dir.display()]).into())
                }
            }
            Err(e) => Notice::Error(crate::i18n::tf("Could not install cyx: {}", &[&e]).into()),
        }
    }

    /// Windows: the installer's folder holds `cyx.exe`; say where, rather than editing the user's PATH behind their back.
    #[cfg(not(unix))]
    fn link_cyx(cyx: &std::path::Path) -> Notice {
        let dir = cyx.parent().map(|d| d.display().to_string()).unwrap_or_default();
        Notice::Info(crate::i18n::tf("cyx is in {}; add that folder to PATH to use it in a terminal", &[&dir]).into())
    }
}
