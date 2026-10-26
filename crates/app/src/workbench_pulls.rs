//! Workbench: GitHub pull requests (read-only list). "GitHub: Pull Requests…" lists open requests; picking one
//! fetches its branch (`pull/N/head` → `pr-N`), opens it on GitHub, or copies the link or branch name.

use super::*;
use crate::git;
use crate::github::{self, Pull};

#[derive(Clone, Copy, Debug)]
pub(super) enum PullAction {
    Checkout,
    Open,
    CopyLink,
    CopyBranch,
}

fn token() -> Option<String> {
    ["GITHUB_TOKEN", "GH_TOKEN"].iter().find_map(|k| std::env::var(k).ok()).filter(|t| !t.trim().is_empty())
}

impl Workbench {
    pub(super) fn show_pull_requests(&mut self, _: &ShowPullRequests, window: &mut Window, cx: &mut Context<Self>) {
        let Some(root) = self.repo_root(cx) else { return };
        self.notice = Some(Notice::Info(crate::i18n::t("Loading pull requests…").into()));
        cx.notify();
        let host = self.remote.host.clone();
        let task = cx.background_executor().spawn(async move {
            let remotes = git::remotes(host.as_ref(), &root)?;
            // `origin` first, then the first GitHub remote.
            let found = remotes.iter().filter(|(n, _)| n == "origin").chain(remotes.iter()).find_map(|(n, u)| github::parse_remote(u).map(|r| (n.clone(), r)));
            let (remote, repo) = found.ok_or_else(|| crate::i18n::t("None of this repository's remotes is on github.com.").to_string())?;
            let (pulls, source) = github::list_pulls(&github::SystemRunner, &repo, token().as_deref())?;
            Ok::<_, String>((remote, repo, pulls, source))
        });
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            this.update_in(cx, |this, window, cx| match result {
                Ok((remote, repo, pulls, source)) => {
                    this.notice = None;
                    if pulls.is_empty() {
                        this.notice = Some(Notice::Info(crate::i18n::tf("{}: no open pull requests.", &[&repo.slug()]).into()));
                        cx.notify();
                        return;
                    }
                    let now = now_secs();
                    let items: Vec<PickItem> = pulls
                        .iter()
                        .map(|p| PickItem { label: format!("#{} {}", p.number, p.title).into(), detail: Some(github::detail(p, now).into()), keys: None })
                        .collect();
                    let via = match source {
                        github::Source::Cli => crate::i18n::t("via the gh CLI"),
                        github::Source::Api { authenticated: true } => crate::i18n::t("via the GitHub API"),
                        github::Source::Api { authenticated: false } => {
                            crate::i18n::t("via the public GitHub API (set GITHUB_TOKEN or use gh for private repositories and check results)")
                        }
                    };
                    let shown = if pulls.len() >= github::PAGE {
                        crate::i18n::tf("latest {} open", &[&pulls.len()])
                    } else {
                        crate::i18n::tf("{} open", &[&pulls.len()])
                    };
                    let title = format!("{} · {shown} · {via}", repo.slug());
                    let picker = cx
                        .new(|cx| Picker::new(vec![PickSource::List { prefix: "", items }], crate::i18n::t("Pull requests"), "", None, cx).with_message(title));
                    this.show_overlay(picker, OverlayKind::PullRequests { remote, pulls }, window, cx);
                }
                Err(e) => {
                    this.notice = Some(Notice::Error(e.lines().next().unwrap_or(crate::i18n::t("could not load pull requests")).to_string().into()));
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    pub(super) fn pick_pull_request(&mut self, remote: String, pull: Pull, window: &mut Window, cx: &mut Context<Self>) {
        let choices = vec![PullAction::Checkout, PullAction::Open, PullAction::CopyLink, PullAction::CopyBranch];
        let items = vec![
            PickItem {
                label: crate::i18n::tf("Check out #{} as pr-{}", &[&pull.number, &pull.number]).into(),
                detail: Some(crate::i18n::tf("fetches {} from {}", &[&pull.head, &remote]).into()),
                keys: None,
            },
            PickItem { label: crate::i18n::t("Open on GitHub").into(), detail: None, keys: None },
            PickItem { label: crate::i18n::t("Copy link").into(), detail: None, keys: None },
            PickItem { label: crate::i18n::t("Copy branch name").into(), detail: Some(pull.head.clone().into()), keys: None },
        ];
        let title = format!("#{} {}", pull.number, pull.title);
        let picker =
            cx.new(|cx| Picker::new(vec![PickSource::List { prefix: "", items }], crate::i18n::t("Choose an action"), "", None, cx).with_message(title));
        self.show_overlay(picker, OverlayKind::PullRequestActions { remote, pull, choices }, window, cx);
    }

    pub(super) fn run_pull_action(&mut self, remote: String, pull: Pull, action: PullAction, cx: &mut Context<Self>) {
        match action {
            PullAction::Open => cx.open_url(&pull.url),
            PullAction::CopyLink => {
                cx.write_to_clipboard(gpui::ClipboardItem::new_string(pull.url.clone()));
                self.notice = Some(Notice::Info(crate::i18n::t("Link copied.").into()));
                cx.notify();
            }
            PullAction::CopyBranch => {
                cx.write_to_clipboard(gpui::ClipboardItem::new_string(pull.head.clone()));
                self.notice = Some(Notice::Info(crate::i18n::tf("Copied {}.", &[&pull.head]).into()));
                cx.notify();
            }
            PullAction::Checkout => {
                let Some(root) = self.repo_root(cx) else { return };
                let n = pull.number;
                self.git_task(&crate::i18n::tf("Checking out pull request #{}…", &[&n]), cx, move |h| {
                    // `-B`: if the branch exists it is reset to the PR's current tip (in case the PR was updated); git refuses if a dirty tree conflicts.
                    git::fetch_pull(h, &root, &remote, n).map_err(|e| github::explain_checkout(&e, n, &remote))?;
                    git::git_ok(h, &root, &["checkout", "-q", "-B", &format!("pr-{n}"), "FETCH_HEAD"]).map_err(|e| github::explain_checkout(&e, n, &remote))?;
                    Ok(crate::i18n::tf("Checked out #{} as pr-{}", &[&n, &n]))
                });
            }
        }
    }

    #[cfg(test)]
    pub fn checkout_pull_for_test(&mut self, remote: String, number: u64, cx: &mut Context<Self>) {
        let pull = Pull {
            number,
            title: String::new(),
            author: String::new(),
            head: "topic".into(),
            base: "main".into(),
            draft: false,
            updated: String::new(),
            url: String::new(),
            review: None,
            checks: None,
        };
        self.run_pull_action(remote, pull, PullAction::Checkout, cx);
    }
}
