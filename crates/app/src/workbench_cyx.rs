//! The two palette commands around `cyx`: "Extensions: Install cyx Command in PATH" puts the `cyx` that ships beside
//! Codeyard on the user's PATH, so `cyx new` and `cyx dev` work in any terminal; "Extensions: New Extension…" runs
//! `cyx new` and loads the result in this window as an extension in development (docs/extensions/developer-guide.md).

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

/// The kinds `cyx new` scaffolds, in picker order.
pub(super) const KINDS: &[&str] = &["theme", "snippets", "keymap", "language", "language-server"];

fn kind_label(kind: &str) -> &'static str {
    match kind {
        "theme" => crate::i18n::t("Color theme"),
        "snippets" => crate::i18n::t("Snippets"),
        "keymap" => crate::i18n::t("Keymap preset"),
        "language" => crate::i18n::t("Language"),
        _ => crate::i18n::t("Language server"),
    }
}

impl Workbench {
    pub(super) fn new_extension(&mut self, _: &NewExtension, window: &mut Window, cx: &mut Context<Self>) {
        if bundled_cyx().is_none() {
            self.notice =
                Some(Notice::Error(crate::i18n::t("cyx is not next to Codeyard in this build; install it with cargo install --path crates/cyx").into()));
            return cx.notify();
        }
        let items = KINDS.iter().map(|k| PickItem { label: kind_label(k).into(), detail: None, keys: None }).collect();
        let picker = cx.new(|cx| Picker::new(vec![PickSource::List { prefix: "", items }], crate::i18n::t("What kind of extension?"), "", None, cx));
        self.show_overlay(picker, OverlayKind::NewExtensionKind, window, cx);
    }

    /// Ask for the folder to create it in, run `cyx new`, then load it as the extension in development and open its manifest.
    pub(super) fn create_extension(&mut self, kind: &'static str, name: String, window: &mut Window, cx: &mut Context<Self>) {
        let name = name.trim().to_string();
        let Some(cyx) = bundled_cyx() else { return };
        let rx = cx.prompt_for_paths(gpui::PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some(crate::i18n::t("Create Here").to_string().into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(paths))) = rx.await else { return };
            let Some(parent) = paths.into_iter().next() else { return };
            let (dir, n, cwd) = (parent.join(&name), name.clone(), parent.clone());
            let run = cx
                .background_executor()
                .spawn(async move { std::process::Command::new(&cyx).args(["new", kind, n.as_str()]).current_dir(&cwd).output() })
                .await;
            this.update_in(cx, |this, window, cx| this.finish_create_extension(run, dir, window, cx)).ok();
        })
        .detach();
    }

    pub(super) fn finish_create_extension(
        &mut self,
        run: std::io::Result<std::process::Output>,
        dir: std::path::PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match run {
            Ok(out) if out.status.success() => {
                crate::ext::set_dev(Some(dir.clone()));
                self.reload_config(window, cx);
                self.open_path(&dir.join(ext_manifest::FILE_NAME).to_string_lossy(), window, cx);
                self.notice =
                    Some(Notice::Info(crate::i18n::tf("{} is loaded as an extension in development: save a file to reload it", &[&dir.display()]).into()));
            }
            Ok(out) => {
                let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
                self.notice = Some(Notice::Error(crate::i18n::tf("Could not create the extension: {}", &[&err]).into()));
            }
            Err(e) => self.notice = Some(Notice::Error(crate::i18n::tf("Could not create the extension: {}", &[&e]).into())),
        }
        cx.notify();
    }

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
