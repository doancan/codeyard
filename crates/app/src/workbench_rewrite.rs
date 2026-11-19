//! Workbench: squashing and reordering unpushed commits from the graph (#270). The menu offers *Squash N Commits…* for the range
//! picked with Shift-click and *Move Commit Up / Down* for one commit; what is allowed, and the refusals with their reasons,
//! are in `git::rewrite`. Both are pinned to the commit the branch was at in the graph and to the repository the menu was
//! opened on.

use super::*;
use crate::git;

impl Workbench {
    /// *Squash N Commits…*: the messages of the commits, in order, in a card to edit as one text; then rewrite.
    pub(super) fn squash_commits(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let run = self.scm.read(cx).range_run();
        let Some(repo) = self.vcs.menu_repo.clone() else { return };
        let (Some(tip), true) = (self.scm.read(cx).head_hash(), run.len() >= 2) else {
            return self.rewrite_unavailable(cx);
        };
        let (host, root, hashes) = (self.remote.host.clone(), repo.root.clone(), run.clone());
        let task = cx.background_executor().spawn(async move { git::default_squash_message(host.as_ref(), &root, &hashes) });
        cx.spawn_in(window, async move |this, cx| {
            let message = task.await;
            this.update_in(cx, |this, window, cx| match message {
                Ok(message) => this.show_squash_message(&message, run, tip, Some(repo), window, cx),
                Err(e) => this.notify_rewrite_failed(e, cx),
            })
            .ok();
        })
        .detach();
    }

    /// Squash `run` into one commit with `message` (all of it: subject and body as edited).
    pub(super) fn finish_squash(&mut self, run: Vec<String>, tip: String, repo: Option<crate::scm::Target>, message: String, cx: &mut Context<Self>) {
        // No comparison with the repository Source Control shows now: the rewrite runs in `repo`, the one the menu was opened on,
        // and `run_in` checks that it is still the same repository (identity, not a snapshot of Source Control's state).
        let message = message.trim();
        if message.is_empty() {
            self.notify_rewrite_failed(crate::i18n::t("The message of the squashed commit cannot be empty.").to_string(), cx);
            return;
        }
        self.scm.update(cx, |s, cx| s.rewrite_in(repo, tip, git::Rewrite::Squash { run, message: message.to_string() }, cx));
    }

    /// *Move Commit Up / Down*.
    pub(super) fn move_commit(&mut self, hash: String, down: bool, cx: &mut Context<Self>) {
        let Some(repo) = self.vcs.menu_repo.clone() else { return };
        let Some(tip) = self.scm.read(cx).head_hash() else { return self.rewrite_unavailable(cx) };
        self.scm.update(cx, |s, cx| s.rewrite_in(Some(repo), tip, git::Rewrite::Move { hash, down }, cx));
    }

    fn rewrite_unavailable(&mut self, cx: &mut Context<Self>) {
        self.notify_rewrite_failed(crate::i18n::t("The graph does not show the current branch's latest commit. Reload it and try again.").to_string(), cx);
    }

    pub(super) fn notify_rewrite_failed(&mut self, message: String, cx: &mut Context<Self>) {
        self.notify(NoticeSource::Git, Notice::Error(message.into()));
        cx.notify();
    }
}

#[cfg(test)]
#[path = "workbench_rewrite_tests.rs"]
mod tests;
