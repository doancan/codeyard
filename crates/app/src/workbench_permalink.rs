//! Workbench: links to the active file on the remote's hosting service: copy a permanent link (to the commit the branch is
//! at) or open the file at the branch. The selected lines are marked in the link.

use super::*;

gpui::actions!(workbench, [CopyGitPermalink, OpenOnGitRemote]);

/// What the link is for.
#[derive(Clone, Copy, PartialEq)]
enum Use {
    Copy,
    Open,
}

/// The remote's name: `origin`, or the first one there is.
fn pick_remote(remotes: &[(String, String)]) -> Option<&str> {
    remotes.iter().find(|(name, _)| name == "origin").or_else(|| remotes.first()).map(|(name, _)| name.as_str())
}

/// `path` below `root`, `/` separated; `None` outside it.
fn relative_to(root: &str, path: &str) -> Option<String> {
    let root = root.trim_end_matches(['/', '\\']);
    let rest = path.strip_prefix(root)?;
    let rest = rest.strip_prefix(['/', '\\'])?;
    (!rest.is_empty()).then(|| rest.replace('\\', "/"))
}

impl Workbench {
    pub(super) fn copy_git_permalink(&mut self, _: &CopyGitPermalink, window: &mut Window, cx: &mut Context<Self>) {
        self.git_file_link(Use::Copy, window, cx);
    }

    pub(super) fn open_on_git_remote(&mut self, _: &OpenOnGitRemote, window: &mut Window, cx: &mut Context<Self>) {
        self.git_file_link(Use::Open, window, cx);
    }

    fn git_file_link(&mut self, how: Use, window: &mut Window, cx: &mut Context<Self>) {
        let tell = |this: &mut Self, text: String, cx: &mut Context<Self>| {
            this.notice = Some(Notice::Info(text.into()));
            cx.notify();
        };
        let (Some(status), Some(ed)) = (self.git_status(), self.active_editor().cloned()) else {
            return tell(self, crate::i18n::t("Open a file in a Git repository first.").to_string(), cx);
        };
        let (path, lines) = {
            let ed = ed.read(cx);
            (ed.path.clone(), ed.selected_lines())
        };
        let Some(rel) = relative_to(&status.root, &path) else {
            return tell(self, crate::i18n::t("The active file is not in the repository.").to_string(), cx);
        };
        let (host, root, branch) = (self.remote.host.clone(), status.root.clone(), status.branch.clone());
        let changed = status.changes.iter().any(|c| c.path == rel);
        let task = cx.background_executor().spawn(async move {
            let remotes = crate::git::remotes(host.as_ref(), &root)?;
            let name = pick_remote(&remotes).ok_or_else(|| crate::i18n::t("This repository has no remote.").to_string())?.to_string();
            let url = crate::git::resolved_remote_url(host.as_ref(), &root, &name)?;
            crate::git::git_ok(host.as_ref(), &root, &["ls-files", "--error-unmatch", "--", &rel])
                .map_err(|_| crate::i18n::t("The file is not tracked by Git.").to_string())?;
            let sha = crate::git::git_ok(host.as_ref(), &root, &["rev-parse", "HEAD"])?.trim().to_string();
            Ok::<_, String>((url, sha, rel))
        });
        let (source_host, source_root) = (self.remote.host.clone(), status.root.clone());
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| {
                if !this.git_context_matches(&source_host, &source_root) {
                    return;
                }
                let linked = result.and_then(|(url, sha, rel)| {
                    // Opening follows the branch; a permanent link names the commit.
                    let rev = match (how, &branch) {
                        (Use::Open, Some(branch)) => branch.to_string(),
                        _ => sha,
                    };
                    crate::git::web_file_url(&url, &rev, &rel, lines).ok_or_else(|| {
                        crate::i18n::t("The remote is not on a hosting service Codeyard knows (GitHub, GitLab, Bitbucket, Codeberg).").to_string()
                    })
                });
                match (linked, how) {
                    (Ok(link), Use::Copy) => {
                        cx.write_to_clipboard(gpui::ClipboardItem::new_string(link));
                        let note = if changed {
                            crate::i18n::t("Permalink copied. The file has changes that are not committed, so the link shows the committed version.")
                        } else {
                            crate::i18n::t("Permalink copied.")
                        };
                        this.notice = Some(Notice::Info(note.into()));
                    }
                    (Ok(link), Use::Open) => cx.open_url(&link),
                    (Err(e), _) => this.notice = Some(Notice::Info(e.into())),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_origin_remote_is_preferred_and_paths_are_relative_to_the_repository() {
        let remotes = vec![("fork".to_string(), "a".to_string()), ("origin".to_string(), "b".to_string())];
        assert_eq!(pick_remote(&remotes), Some("origin"));
        assert_eq!(pick_remote(&remotes[..1]), Some("fork"));
        assert_eq!(pick_remote(&[]), None);
        assert_eq!(relative_to("/r/app", "/r/app/src/a.rs"), Some("src/a.rs".into()));
        assert_eq!(relative_to("/r/app/", "/r/app/a.rs"), Some("a.rs".into()));
        assert_eq!(relative_to(r"C:\r\app", r"C:\r\app\src\a.rs"), Some("src/a.rs".into()));
        assert_eq!(relative_to("/r/app", "/r/application/a.rs"), None, "a longer folder name is not inside");
        assert_eq!(relative_to("/r/app", "/r/app"), None);
    }
}
