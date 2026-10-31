//! Workbench: the Pull Requests view. "Pull Requests: Show…" lists GitHub's open requests (read-only) and those of
//! code extensions' `[[pull-requests]]` providers for the repository's remotes (`workbench_ext_pulls.rs`). Picking a
//! GitHub request fetches its branch (`pull/N/head` → `pr-N`), opens it on GitHub, or copies the link or branch name.

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

/// GitHub's answer: the remote, the repository, its open requests and where they came from.
type GitHubListing = Result<(String, github::Repo, Vec<Pull>, github::Source), String>;

/// What the view says above its rows: one line per list shown, then the lists with nothing open, then failures.
#[derive(Default)]
pub(super) struct Notes {
    pub lines: Vec<String>,
    pub empty: Vec<String>,
    pub failed: Vec<String>,
}

fn token() -> Option<String> {
    ["GITHUB_TOKEN", "GH_TOKEN"].iter().find_map(|k| std::env::var(k).ok()).filter(|t| !t.trim().is_empty())
}

/// One row of the Pull Requests view: GitHub's, or a code extension's (`workbench_ext_pulls.rs`).
#[derive(Clone)]
pub(super) enum PullRow {
    GitHub { remote: String, pull: Pull },
    Extension(ext_pulls_glue::ExtPull),
}

impl Workbench {
    pub(super) fn show_pull_requests(&mut self, _: &ShowPullRequests, window: &mut Window, cx: &mut Context<Self>) {
        let Some(root) = self.repo_root(cx) else { return };
        self.notice = Some(Notice::Info(crate::i18n::t("Loading pull requests…").into()));
        cx.notify();
        let host = self.remote.host.clone();
        let remotes = cx.background_executor().spawn(async move { git::remotes(host.as_ref(), &root) });
        cx.spawn_in(window, async move |this, cx| {
            let remotes = remotes.await;
            this.update_in(cx, |this, window, cx| this.list_pull_requests(remotes, window, cx)).ok();
        })
        .detach();
    }

    /// GitHub's open requests for the first GitHub remote (`origin` first), and those of every extension provider that
    /// claims one of `remotes`, in one list.
    fn list_pull_requests(&mut self, remotes: Result<Vec<(String, String)>, String>, window: &mut Window, cx: &mut Context<Self>) {
        let remotes = match remotes {
            Ok(r) => r,
            Err(e) => {
                self.notice = Some(Notice::Error(e.lines().next().unwrap_or(crate::i18n::t("could not load pull requests")).to_string().into()));
                cx.notify();
                return;
            }
        };
        // `origin` first, then the first GitHub remote.
        let found = remotes.iter().filter(|(n, _)| n == "origin").chain(remotes.iter()).find_map(|(n, u)| github::parse_remote(u).map(|r| (n.clone(), r)));
        let asked = self.ask_pull_providers(&remotes, window, cx);
        if found.is_none() && asked.is_empty() {
            // Name every provider there is, so a GitLab or Gitea user learns what to install or which remote is off.
            let others = self.pull_provider_names();
            let msg = if others.is_empty() {
                crate::i18n::t("None of this repository's remotes is on github.com, and no installed extension provides pull requests for their hosts.")
                    .to_string()
            } else {
                crate::i18n::tf("None of this repository's remotes is on github.com or on a host of {}.", &[&others.join(", ")])
            };
            self.notice = Some(Notice::Error(msg.into()));
            cx.notify();
            return;
        }
        let github = found.map(|(remote, repo)| {
            cx.background_executor().spawn(async move {
                let (pulls, source) = github::list_pulls(&github::SystemRunner, &repo, token().as_deref())?;
                Ok::<_, String>((remote, repo, pulls, source))
            })
        });
        cx.spawn_in(window, async move |this, cx| {
            let github = match github {
                Some(task) => Some(task.await),
                None => None,
            };
            let listed = ext_pulls_glue::listed(asked).await;
            this.update_in(cx, |this, window, cx| this.show_pull_list(github, listed, window, cx)).ok();
        })
        .detach();
    }

    fn show_pull_list(&mut self, github: Option<GitHubListing>, listed: Vec<ext_pulls_glue::Listed>, window: &mut Window, cx: &mut Context<Self>) {
        let now = now_secs();
        let mut rows = Vec::new();
        let mut items = Vec::new();
        let mut notes = Notes::default();
        match github {
            Some(Ok((remote, repo, pulls, source))) => {
                items.extend(pulls.iter().map(|p| PickItem {
                    label: format!("#{} {}", p.number, p.title).into(),
                    detail: Some(github::detail(p, now).into()),
                    keys: None,
                }));
                if pulls.is_empty() {
                    notes.empty.push(crate::i18n::tf("{}: no open pull requests.", &[&repo.slug()]));
                } else {
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
                    notes.lines.push(format!("{} · {shown} · {via}", repo.slug()));
                }
                rows.extend(pulls.into_iter().map(|pull| PullRow::GitHub { remote: remote.clone(), pull }));
            }
            Some(Err(e)) => notes.failed.push(e.lines().next().unwrap_or(crate::i18n::t("could not load pull requests")).to_string()),
            None => {}
        }
        self.add_extension_pulls(listed, &mut rows, &mut items, &mut notes, now);
        if rows.is_empty() {
            // Nothing open anywhere is news, not an error; a failure is.
            let said = notes.failed.iter().chain(&notes.empty).cloned().collect::<Vec<_>>().join(" · ");
            self.notice = Some(match (notes.failed.is_empty(), said.is_empty()) {
                (true, true) => Notice::Info(crate::i18n::t("No open pull requests.").into()),
                (true, false) => Notice::Info(said.into()),
                (false, _) => Notice::Error(said.into()),
            });
            cx.notify();
            return;
        }
        self.notice = None;
        let title = notes.lines.into_iter().chain(notes.empty).chain(notes.failed).collect::<Vec<_>>().join(" — ");
        let picker = cx.new(|cx| Picker::new(vec![PickSource::List { prefix: "", items }], crate::i18n::t("Pull requests"), "", None, cx).with_message(title));
        self.show_overlay(picker, OverlayKind::PullRequests(rows), window, cx);
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
