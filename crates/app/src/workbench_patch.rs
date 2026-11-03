//! Workbench: the repository's changes as a patch on the clipboard, and a patch from the clipboard applied to the working tree.

use super::*;

gpui::actions!(workbench, [CopyChangesAsPatch, ApplyPatchFromClipboard]);

impl Workbench {
    fn patch_target(&mut self, cx: &mut Context<Self>) -> Option<(std::sync::Arc<dyn host::Host>, String)> {
        match self.git_status() {
            Some(status) => Some((self.remote.host.clone(), status.root.clone())),
            None => {
                self.notice = Some(Notice::Info(crate::i18n::t("Open a folder that is a Git repository first.").into()));
                cx.notify();
                None
            }
        }
    }

    pub(super) fn copy_changes_as_patch(&mut self, _: &CopyChangesAsPatch, window: &mut Window, cx: &mut Context<Self>) {
        let Some((host, root)) = self.patch_target(cx) else { return };
        let (source_host, source_root) = (host.clone(), root.clone());
        let task = cx.background_executor().spawn(async move { crate::git::changes_patch(host.as_ref(), &root) });
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| {
                if !this.git_context_matches(&source_host, &source_root) {
                    return;
                }
                this.notice = Some(match result {
                    Ok(patch) if patch.is_empty() => Notice::Info(crate::i18n::t("There are no changes to put in a patch.").into()),
                    Ok(patch) => {
                        cx.write_to_clipboard(gpui::ClipboardItem::new_string(patch));
                        Notice::Info(crate::i18n::t("Patch copied. Untracked files are not part of it.").into())
                    }
                    Err(e) => Notice::Error(e.into()),
                });
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub(super) fn apply_patch_from_clipboard(&mut self, _: &ApplyPatchFromClipboard, window: &mut Window, cx: &mut Context<Self>) {
        let Some((host, root)) = self.patch_target(cx) else { return };
        let Some(patch) = cx.read_from_clipboard().and_then(|c| c.text()) else {
            self.notice = Some(Notice::Info(crate::i18n::t("The clipboard has no text.").into()));
            return cx.notify();
        };
        let (source_host, source_root) = (host.clone(), root.clone());
        let task = cx.background_executor().spawn(async move { crate::git::apply_patch(host.as_ref(), &root, &patch) });
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| {
                if !this.git_context_matches(&source_host, &source_root) {
                    return;
                }
                this.notice = Some(match result {
                    Ok(()) => Notice::Info(crate::i18n::t("Patch applied to the working tree.").into()),
                    Err(e) => Notice::Error(crate::i18n::tf("The patch was not applied: {}", &[&e]).into()),
                });
                this.refresh_git(0, cx);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }
}
