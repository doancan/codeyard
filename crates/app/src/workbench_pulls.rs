//! Workbench: the Pull Requests view. "Pull Requests: Show…" lists GitHub's open requests (read-only) and those of
//! code extensions' `[[pull-requests]]` providers for the repository's remotes (`workbench_ext_pulls.rs`). Picking a
//! GitHub request fetches its branch (`pull/N/head` → `pr-N`), opens it on GitHub, or copies the link or branch name.

use super::*;
use crate::git;
use crate::github::{self, Pull};

#[cfg(test)]
#[path = "workbench_pull_race_tests.rs"]
mod race_tests;

#[derive(Clone, Copy, Debug)]
pub(super) enum PullAction {
    Checkout,
    Open,
    CopyLink,
    CopyBranch,
}

/// The remote a GitHub list was read from, with its URL as Git resolved it then (`insteadOf` applied): checkout
/// refuses a remote repointed since, so an old pull request number cannot fetch from another repository.
#[derive(Clone)]
pub(super) struct PullRemote {
    pub name: String,
    pub url: String,
}

/// GitHub's answer: the remote, the repository, its open requests and where they came from.
type GitHubListing = Result<(PullRemote, github::Repo, Vec<Pull>, github::Source), String>;

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
    GitHub { remote: PullRemote, pull: Pull },
    Extension(ext_pulls_glue::ExtPull),
}

impl Workbench {
    pub(super) fn show_pull_requests(&mut self, _: &ShowPullRequests, window: &mut Window, cx: &mut Context<Self>) {
        let Some(root) = self.repo_root(cx) else { return };
        self.notify(NoticeSource::App, Notice::Info(crate::i18n::t("Loading pull requests…").into()));
        cx.notify();
        let host = self.remote.host.clone();
        let source_host = host.clone();
        let source_root = root.clone();
        let remotes = cx.background_executor().spawn(async move { git::remotes(host.as_ref(), &root) });
        cx.spawn_in(window, async move |this, cx| {
            let remotes = remotes.await;
            this.update_in(cx, |this, window, cx| {
                // Another host or repository was opened while the remotes were read: the list belongs to neither.
                if this.git_context_matches(&source_host, &source_root) {
                    this.list_pull_requests(source_host, source_root, remotes, window, cx);
                }
            })
            .ok();
        })
        .detach();
    }

    /// GitHub's open requests for the first remote that resolves to github.com (`origin` first), and those of every
    /// extension provider that claims one of `remotes`, in one list.
    fn list_pull_requests(
        &mut self,
        host: Arc<dyn Host>,
        root: String,
        remotes: Result<Vec<(String, String)>, String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let remotes = match remotes {
            Ok(r) => r,
            Err(e) => {
                self.notify(NoticeSource::App, Notice::Error(e.lines().next().unwrap_or(crate::i18n::t("could not load pull requests")).to_string().into()));
                cx.notify();
                return;
            }
        };
        let asked = self.ask_pull_providers(&remotes, window, cx);
        let (task_host, task_root) = (host.clone(), root.clone());
        // The URL each remote resolves to (with `insteadOf`), read off the UI thread; `None` when no remote is on GitHub.
        let github = cx.background_executor().spawn(async move {
            let mut found = None;
            for (name, stored_url) in remotes.iter().filter(|(name, _)| name == "origin").chain(remotes.iter().filter(|(name, _)| name != "origin")) {
                if stored_url.is_empty() {
                    continue;
                }
                let url = match git::resolved_remote_url(task_host.as_ref(), &task_root, name) {
                    Ok(url) => url,
                    Err(e) => return Some(Err(e)),
                };
                if let Some(repo) = github::parse_remote(&url) {
                    found = Some((PullRemote { name: name.clone(), url }, repo));
                    break;
                }
            }
            let (remote, repo) = found?;
            Some(github::list_pulls(&github::SystemRunner, &repo, token().as_deref()).map(|(pulls, source)| (remote, repo, pulls, source)))
        });
        cx.spawn_in(window, async move |this, cx| {
            let github = github.await;
            let listed = ext_pulls_glue::listed(asked).await;
            this.update_in(cx, |this, window, cx| {
                if this.git_context_matches(&host, &root) {
                    this.show_pull_list(host, root, github, listed, window, cx);
                }
            })
            .ok();
        })
        .detach();
    }

    fn show_pull_list(
        &mut self,
        host: Arc<dyn Host>,
        root: String,
        github: Option<GitHubListing>,
        listed: Vec<ext_pulls_glue::Listed>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if github.is_none() && listed.is_empty() {
            // Name every provider there is, so a GitLab or Gitea user learns what to install or which remote is off.
            let others = self.pull_provider_names();
            let msg = if others.is_empty() {
                crate::i18n::t("None of this repository's remotes is on github.com, and no installed extension provides pull requests for their hosts.")
                    .to_string()
            } else {
                crate::i18n::tf("None of this repository's remotes is on github.com or on a host of {}.", &[&others.join(", ")])
            };
            self.notify(NoticeSource::App, Notice::Error(msg.into()));
            cx.notify();
            return;
        }
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
            self.notify(
                NoticeSource::App,
                match (notes.failed.is_empty(), said.is_empty()) {
                    (true, true) => Notice::Info(crate::i18n::t("No open pull requests.").into()),
                    (true, false) => Notice::Info(said.into()),
                    (false, _) => Notice::Error(said.into()),
                },
            );
            cx.notify();
            return;
        }
        self.dismiss_notice();
        let title = notes.lines.into_iter().chain(notes.empty).chain(notes.failed).collect::<Vec<_>>().join(" — ");
        let picker = cx.new(|cx| Picker::new(vec![PickSource::List { prefix: "", items }], crate::i18n::t("Pull requests"), "", None, cx).with_message(title));
        self.show_overlay(picker, OverlayKind::PullRequests { host, root, rows }, window, cx);
    }

    pub(super) fn pick_pull_request(&mut self, host: Arc<dyn Host>, root: String, remote: PullRemote, pull: Pull, window: &mut Window, cx: &mut Context<Self>) {
        if !self.git_context_matches(&host, &root) {
            return;
        }
        let choices = vec![PullAction::Checkout, PullAction::Open, PullAction::CopyLink, PullAction::CopyBranch];
        let items = vec![
            PickItem {
                label: crate::i18n::tf("Check out #{} as pr-{}", &[&pull.number, &pull.number]).into(),
                detail: Some(crate::i18n::tf("fetches {} from {}", &[&pull.head, &remote.name]).into()),
                keys: None,
            },
            PickItem { label: crate::i18n::t("Open on GitHub").into(), detail: None, keys: None },
            PickItem { label: crate::i18n::t("Copy link").into(), detail: None, keys: None },
            PickItem { label: crate::i18n::t("Copy branch name").into(), detail: Some(pull.head.clone().into()), keys: None },
        ];
        let title = format!("#{} {}", pull.number, pull.title);
        let picker =
            cx.new(|cx| Picker::new(vec![PickSource::List { prefix: "", items }], crate::i18n::t("Choose an action"), "", None, cx).with_message(title));
        self.show_overlay(picker, OverlayKind::PullRequestActions { host, root, remote, pull, choices }, window, cx);
    }

    pub(super) fn run_pull_action(&mut self, remote: PullRemote, pull: Pull, action: PullAction, cx: &mut Context<Self>) {
        match action {
            PullAction::Open => cx.open_url(&pull.url),
            PullAction::CopyLink => {
                cx.write_to_clipboard(gpui::ClipboardItem::new_string(pull.url.clone()));
                self.notify(NoticeSource::App, Notice::Info(crate::i18n::t("Link copied.").into()));
                cx.notify();
            }
            PullAction::CopyBranch => {
                cx.write_to_clipboard(gpui::ClipboardItem::new_string(pull.head.clone()));
                self.notify(NoticeSource::App, Notice::Info(crate::i18n::tf("Copied {}.", &[&pull.head]).into()));
                cx.notify();
            }
            PullAction::Checkout => {
                let Some(root) = self.repo_root(cx) else { return };
                let n = pull.number;
                self.git_task(&crate::i18n::tf("Checking out pull request #{}…", &[&n]), cx, move |h| {
                    if git::resolved_remote_url(h, &root, &remote.name)? != remote.url {
                        return Err(crate::i18n::tf(
                            "Remote {} changed since this pull request list was loaded. Reopen the list and try again.",
                            &[&remote.name],
                        ));
                    }
                    git::checkout_pull(h, &root, &remote.url, n).map_err(|e| github::explain_checkout(&e, n, &remote.name))?;
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
        let remote = self.pull_remote_for_test(remote);
        self.run_pull_action(remote, pull, PullAction::Checkout, cx);
    }

    #[cfg(test)]
    pub(super) fn pull_remote_for_test(&self, name: String) -> PullRemote {
        let url = self.git_status().and_then(|status| git::resolved_remote_url(self.remote.host.as_ref(), &status.root, &name).ok()).unwrap_or_default();
        PullRemote { name, url }
    }
}
