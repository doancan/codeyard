//! Workbench: pull (merge) requests from code extensions (`[[pull-requests]]`, #145) in the Pull Requests view, next
//! to GitHub's. Each active code extension whose provider claims a repository remote's host is asked for that
//! remote's open requests; picking one offers its details (a Markdown page with checks and comments), its changed
//! files as diffs, adding a comment, checking it out, and its link.
//!
//! Providers are asked like every provider (`ask_provider_within`): with a deadline
//! (`limits::PULL_REQUEST_DEADLINE_MILLIS`, since their answers come from the network), cancelled when it passes, and
//! with late answers and errors noted in the extension's log in the Inspector. A provider calls its forge with
//! `auth.fetch`, so it never sees the account's token. Checking out runs git on the workspace's host after the
//! provider's answer is checked (`ext_pulls::checkout_plan`).

use ext_api::value::{self as v, LogLevel, PullAnswer, PullQuery, Request, Response};
use ext_manifest::limits;
use futures::channel::oneshot;

use super::*;
use crate::ext::Active;
use crate::ext_pulls;

/// A request a provider listed, with what it takes to ask about it again.
#[derive(Clone, Debug)]
pub(super) struct ExtPull {
    pub extension: String,
    /// The contribution's label, such as `Gitea`.
    pub label: String,
    pub remote: v::PullRemote,
    pub pull: v::PullSummary,
}

impl ExtPull {
    fn title(&self) -> String {
        format!("{} {}", self.pull.reference, self.pull.title).trim().to_string()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ExtPullAction {
    Details,
    Files,
    Comment,
    Checkout,
    Open,
    CopyLink,
    CopyBranch,
}

/// One provider being asked for one remote's list.
pub(super) struct Asked {
    extension: String,
    label: String,
    remote: v::PullRemote,
    answer: oneshot::Receiver<Result<Response, v::Error>>,
}

/// What one provider listed for one remote.
pub(super) struct Listed {
    extension: String,
    label: String,
    remote: v::PullRemote,
    result: Result<Vec<v::PullSummary>, v::Error>,
}

/// Wait for every provider's list (each ends by its deadline).
pub(super) async fn listed(asked: Vec<Asked>) -> Vec<Listed> {
    let mut out = Vec::new();
    for a in asked {
        let result = match a.answer.await.unwrap_or(Err(v::Error::Cancelled)) {
            Ok(Response::PullRequests(PullAnswer::List(l))) => Ok(ext_pulls::clean(l)),
            Ok(other) => Err(unexpected(&other)),
            Err(e) => Err(e),
        };
        out.push(Listed { extension: a.extension, label: a.label, remote: a.remote, result });
    }
    out
}

fn unexpected(r: &Response) -> v::Error {
    let what = match r {
        Response::PullRequests(_) => "another kind of answer",
        _ => "something that is not a pull request answer",
    };
    v::Error::Invalid(format!("the provider answered with {what}"))
}

/// `publisher.name.local` → `local`.
fn local_id<'a>(extension: &str, id: &'a str) -> &'a str {
    id.strip_prefix(extension).and_then(|r| r.strip_prefix('.')).unwrap_or(id)
}

impl Workbench {
    /// Active code extensions with a `[[pull-requests]]` provider, each with that provider.
    fn pull_providers(&self) -> Vec<(Active, ext_manifest::PullRequestProvider)> {
        crate::ext::current()
            .active
            .iter()
            .filter(|e| e.manifest.code.is_some() && !self.cfg.exts.code.disabled.contains(e.id()))
            .flat_map(|e| e.manifest.contributions.pull_requests.iter().map(move |p| (e.clone(), p.clone())))
            .collect()
    }

    /// Ask every provider that claims one of `remotes` (`(name, url)`) for its open requests.
    pub(super) fn ask_pull_providers(&mut self, remotes: &[(String, String)], window: &mut Window, cx: &mut Context<Self>) -> Vec<Asked> {
        let mut asked = Vec::new();
        for (active, provider) in self.pull_providers() {
            let local = local_id(active.id(), &provider.id).to_string();
            for (name, url) in remotes {
                let Some(remote) = ext_pulls::claimed(&provider, &local, name, url) else { continue };
                let answer = self.ask_pull(&active, PullQuery::List { remote: remote.clone() }, window, cx);
                asked.push(Asked { extension: active.id().to_string(), label: provider.label.clone(), remote, answer });
            }
        }
        asked
    }

    fn ask_pull(&mut self, active: &Active, query: PullQuery, window: &mut Window, cx: &mut Context<Self>) -> oneshot::Receiver<Result<Response, v::Error>> {
        let reason = format!("pull-requests:{}", query.remote().provider);
        let request = Request::PullRequests { query };
        self.ask_provider_within(active, &reason, request, None, limits::PULL_REQUEST_DEADLINE_MILLIS, window, cx)
    }

    /// Ask `pull`'s provider again; `None` (with a notice) when its extension is no longer active.
    fn ask_about(
        &mut self,
        pull: &ExtPull,
        query: PullQuery,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<oneshot::Receiver<Result<Response, v::Error>>> {
        let active = self.pull_providers().into_iter().map(|(a, _)| a).find(|a| a.id() == pull.extension);
        let Some(active) = active else {
            self.notice = Some(Notice::Error(crate::i18n::tf("{} is no longer active.", &[&pull.extension]).into()));
            cx.notify();
            return None;
        };
        Some(self.ask_pull(&active, query, window, cx))
    }

    /// A provider's error: noted in its log, and shown unless the user moved on.
    fn pull_failed(&mut self, extension: &str, what: &str, e: &v::Error, cx: &mut Context<Self>) {
        if matches!(e, v::Error::Cancelled) {
            return;
        }
        let why = super::code_glue::explain(e);
        self.log_extension(extension, LogLevel::Warn, format!("pull requests: {what}: {why}"));
        self.notice = Some(Notice::Error(format!("{extension}: {why}").into()));
        cx.notify();
    }

    /// The providers' lists as rows of the view: items, title lines and failures (also noted in the Inspector).
    pub(super) fn add_extension_pulls(
        &mut self,
        listed: Vec<Listed>,
        rows: &mut Vec<pulls_glue::PullRow>,
        items: &mut Vec<PickItem>,
        notes: &mut pulls_glue::Notes,
        now: i64,
    ) {
        for l in listed {
            let place = format!("{} {}", l.label, l.remote.path);
            match l.result {
                Ok(list) if list.is_empty() => notes.empty.push(crate::i18n::tf("{}: no open pull requests.", &[&place])),
                Ok(list) => {
                    notes.lines.push(format!("{} · {} · {}", l.remote.path, crate::i18n::tf("{} open", &[&list.len()]), l.label));
                    for pull in list {
                        let p = ExtPull { extension: l.extension.clone(), label: l.label.clone(), remote: l.remote.clone(), pull };
                        // Every extension surface names the extension: the label and the remote, then the extension id.
                        let detail = format!("{} · {}", ext_pulls::detail(&p.pull, &l.label, now), l.extension);
                        items.push(PickItem { label: p.title().into(), detail: Some(detail.into()), keys: None });
                        rows.push(pulls_glue::PullRow::Extension(p));
                    }
                }
                Err(v::Error::Cancelled) => {}
                Err(e) => {
                    let why = super::code_glue::explain(&e);
                    self.log_extension(&l.extension, LogLevel::Warn, format!("pull requests: listing {}: {why}", l.remote.path));
                    notes.failed.push(format!("{place}: {why}"));
                }
            }
        }
    }

    /// The actions for an extension's request.
    pub(super) fn pick_ext_pull(&mut self, pull: ExtPull, window: &mut Window, cx: &mut Context<Self>) {
        let mut choices = vec![ExtPullAction::Details, ExtPullAction::Files, ExtPullAction::Comment, ExtPullAction::Checkout];
        let mut items = vec![
            PickItem { label: crate::i18n::t("Show details and comments").into(), detail: None, keys: None },
            PickItem { label: crate::i18n::t("Show changed files").into(), detail: None, keys: None },
            PickItem { label: crate::i18n::t("Add comment…").into(), detail: None, keys: None },
            PickItem {
                label: crate::i18n::t("Check out").into(),
                detail: Some(crate::i18n::tf("fetches {} from {}", &[&pull.pull.source_branch, &pull.remote.name]).into()),
                keys: None,
            },
        ];
        if pull.pull.url.is_some() {
            choices.extend([ExtPullAction::Open, ExtPullAction::CopyLink]);
            items.push(PickItem { label: crate::i18n::tf("Open on {}", &[&pull.label]).into(), detail: None, keys: None });
            items.push(PickItem { label: crate::i18n::t("Copy link").into(), detail: None, keys: None });
        }
        choices.push(ExtPullAction::CopyBranch);
        items.push(PickItem { label: crate::i18n::t("Copy branch name").into(), detail: Some(pull.pull.source_branch.clone().into()), keys: None });
        let title = format!("{} · {}", pull.title(), pull.extension);
        let picker =
            cx.new(|cx| Picker::new(vec![PickSource::List { prefix: "", items }], crate::i18n::t("Choose an action"), "", None, cx).with_message(title));
        self.show_overlay(picker, OverlayKind::ExtPullActions { pull: Box::new(pull), choices }, window, cx);
    }

    pub(super) fn run_ext_pull_action(&mut self, pull: ExtPull, action: ExtPullAction, window: &mut Window, cx: &mut Context<Self>) {
        match action {
            ExtPullAction::Details => self.show_ext_pull_details(pull, window, cx),
            ExtPullAction::Files => self.show_ext_pull_files(pull, window, cx),
            ExtPullAction::Comment => {
                let verb = crate::i18n::t("Add comment");
                let hint = crate::i18n::tf("Comment on {} (Markdown)", &[&pull.title()]);
                self.show_input(InputPurpose::PullComment(Box::new(pull)), verb, &hint, "", window, cx);
            }
            ExtPullAction::Checkout => self.checkout_ext_pull(pull, window, cx),
            ExtPullAction::Open => {
                if let Some(url) = &pull.pull.url {
                    cx.open_url(url);
                }
            }
            ExtPullAction::CopyLink => {
                if let Some(url) = &pull.pull.url {
                    cx.write_to_clipboard(gpui::ClipboardItem::new_string(url.clone()));
                    self.notice = Some(Notice::Info(crate::i18n::t("Link copied.").into()));
                    cx.notify();
                }
            }
            ExtPullAction::CopyBranch => {
                cx.write_to_clipboard(gpui::ClipboardItem::new_string(pull.pull.source_branch.clone()));
                self.notice = Some(Notice::Info(crate::i18n::tf("Copied {}.", &[&pull.pull.source_branch]).into()));
                cx.notify();
            }
        }
    }

    /// Details and comments, asked together, as a Markdown page (a scratch file and its preview).
    fn show_ext_pull_details(&mut self, pull: ExtPull, window: &mut Window, cx: &mut Context<Self>) {
        let (remote, id) = (pull.remote.clone(), pull.pull.id.clone());
        let Some(details) = self.ask_about(&pull, PullQuery::Details { remote: remote.clone(), id: id.clone() }, window, cx) else { return };
        let Some(comments) = self.ask_about(&pull, PullQuery::Comments { remote, id }, window, cx) else { return };
        self.notice = Some(Notice::Info(crate::i18n::tf("Loading {}…", &[&pull.title()]).into()));
        cx.notify();
        cx.spawn_in(window, async move |this, cx| {
            let details = details.await.unwrap_or(Err(v::Error::Cancelled));
            let comments = comments.await.unwrap_or(Err(v::Error::Cancelled));
            this.update_in(cx, |this, window, cx| {
                let details = match details {
                    Ok(Response::PullRequests(PullAnswer::Details(d))) => d,
                    Ok(other) => return this.pull_failed(&pull.extension, "details", &unexpected(&other), cx),
                    Err(e) => return this.pull_failed(&pull.extension, "details", &e, cx),
                };
                // Comments are optional: without them the page says there are none.
                let comments = match comments {
                    Ok(Response::PullRequests(PullAnswer::Comments(c))) => c,
                    Ok(other) => {
                        this.log_extension(
                            &pull.extension,
                            LogLevel::Warn,
                            format!("pull requests: comments: {}", super::code_glue::explain(&unexpected(&other))),
                        );
                        Vec::new()
                    }
                    Err(e) => {
                        this.log_extension(&pull.extension, LogLevel::Warn, format!("pull requests: comments: {}", super::code_glue::explain(&e)));
                        Vec::new()
                    }
                };
                let page = ext_pulls::details_markdown(&details, &comments, &pull.label, now_secs());
                this.notice = None;
                let name = format!("{}.md", ext_pulls::shown(&pull.title(), 60).replace(['/', '\\', ':'], "-"));
                this.show_pull_page(name, &page, window, cx);
            })
            .ok();
        })
        .detach();
    }

    /// A Markdown page in a scratch file, opened at once (it is small and local) and shown as its preview.
    fn show_pull_page(&mut self, name: String, page: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(path) = self.write_scratch(name, page, cx) else { return };
        let host = self.remote.local_host.clone();
        match crate::editor::load(host.as_ref(), &path, cx.global::<Settings>().fallback_encoding) {
            Ok(loaded) => {
                self.add_editor(host, path, loaded, window, cx);
                let ix = self.tabs.editors.len() - 1;
                self.activate(ix, window, cx);
                self.open_markdown_preview(&OpenMarkdownPreview, window, cx);
            }
            Err(_) => self.open_file_on(host, path, window, cx),
        }
    }

    fn show_ext_pull_files(&mut self, pull: ExtPull, window: &mut Window, cx: &mut Context<Self>) {
        let query = PullQuery::Files { remote: pull.remote.clone(), id: pull.pull.id.clone() };
        let Some(answer) = self.ask_about(&pull, query, window, cx) else { return };
        cx.spawn_in(window, async move |this, cx| {
            let answer = answer.await.unwrap_or(Err(v::Error::Cancelled));
            this.update_in(cx, |this, window, cx| {
                let mut files = match answer {
                    Ok(Response::PullRequests(PullAnswer::Files(f))) => f,
                    Ok(other) => return this.pull_failed(&pull.extension, "files", &unexpected(&other), cx),
                    Err(e) => return this.pull_failed(&pull.extension, "files", &e, cx),
                };
                files.truncate(ext_pulls::MAX_FILES);
                if files.is_empty() {
                    this.notice = Some(Notice::Info(crate::i18n::tf("{}: no changed files.", &[&pull.title()]).into()));
                    cx.notify();
                    return;
                }
                let items = files
                    .iter()
                    .map(|f| PickItem { label: ext_pulls::shown(&f.path, 512).into(), detail: Some(ext_pulls::file_detail(f).into()), keys: None })
                    .collect();
                let title = format!("{} · {}", pull.title(), crate::i18n::tf("{} files", &[&files.len()]));
                let picker =
                    cx.new(|cx| Picker::new(vec![PickSource::List { prefix: "", items }], crate::i18n::t("Changed files"), "", None, cx).with_message(title));
                this.show_overlay(picker, OverlayKind::ExtPullFiles { pull: Box::new(pull), files }, window, cx);
            })
            .ok();
        })
        .detach();
    }

    /// One changed file as a diff: the target branch's text on the left, the source branch's on the right.
    pub(super) fn open_ext_pull_file(&mut self, pull: ExtPull, file: v::ChangedFile, window: &mut Window, cx: &mut Context<Self>) {
        let query = PullQuery::Contents { remote: pull.remote.clone(), id: pull.pull.id.clone(), file: file.clone() };
        let Some(answer) = self.ask_about(&pull, query, window, cx) else { return };
        cx.spawn_in(window, async move |this, cx| {
            let answer = answer.await.unwrap_or(Err(v::Error::Cancelled));
            this.update_in(cx, |this, window, cx| match answer {
                Ok(Response::PullRequests(PullAnswer::Contents(c))) => {
                    let title = format!("{} ({})", ext_pulls::shown(&file.path, 256), pull.title());
                    let (base, head) = (c.base.unwrap_or_default(), c.head.unwrap_or_default());
                    this.open_compare(title, crate::i18n::t("Pull request changes"), &base.replace("\r\n", "\n"), &head.replace("\r\n", "\n"), window, cx);
                }
                Ok(other) => this.pull_failed(&pull.extension, "contents", &unexpected(&other), cx),
                Err(e) => this.pull_failed(&pull.extension, "contents", &e, cx),
            })
            .ok();
        })
        .detach();
    }

    /// The comment the user wrote: sent to the provider (a user action), then confirmed.
    pub(super) fn add_ext_pull_comment(&mut self, pull: ExtPull, body: String, window: &mut Window, cx: &mut Context<Self>) {
        if body.trim().is_empty() {
            return;
        }
        let query = PullQuery::AddComment { remote: pull.remote.clone(), id: pull.pull.id.clone(), body };
        let Some(answer) = self.ask_about(&pull, query, window, cx) else { return };
        self.notice = Some(Notice::Info(crate::i18n::t("Adding comment…").into()));
        cx.notify();
        cx.spawn_in(window, async move |this, cx| {
            let answer = answer.await.unwrap_or(Err(v::Error::Cancelled));
            this.update(cx, |this, cx| match answer {
                Ok(Response::PullRequests(PullAnswer::Done)) => {
                    this.notice = Some(Notice::Info(crate::i18n::tf("Comment added to {}.", &[&pull.title()]).into()));
                    cx.notify();
                }
                Ok(other) => this.pull_failed(&pull.extension, "add comment", &unexpected(&other), cx),
                Err(e) => this.pull_failed(&pull.extension, "add comment", &e, cx),
            })
            .ok();
        })
        .detach();
    }

    /// Ask the provider how (a user action), check its answer, then fetch and switch with git on the workspace's host.
    fn checkout_ext_pull(&mut self, pull: ExtPull, window: &mut Window, cx: &mut Context<Self>) {
        let Some(root) = self.repo_root(cx) else { return };
        let query = PullQuery::Checkout { remote: pull.remote.clone(), id: pull.pull.id.clone() };
        let Some(answer) = self.ask_about(&pull, query, window, cx) else { return };
        cx.spawn_in(window, async move |this, cx| {
            let answer = answer.await.unwrap_or(Err(v::Error::Cancelled));
            this.update(cx, |this, cx| {
                let plan = match answer {
                    Ok(Response::PullRequests(PullAnswer::Checkout(c))) => c,
                    Ok(other) => return this.pull_failed(&pull.extension, "checkout", &unexpected(&other), cx),
                    Err(e) => return this.pull_failed(&pull.extension, "checkout", &e, cx),
                };
                if let Err(e) = ext_pulls::checkout_plan(&plan) {
                    return this.pull_failed(&pull.extension, "checkout", &v::Error::Invalid(e), cx);
                }
                let from = plan.fetch_url.clone().unwrap_or_else(|| pull.remote.name.clone());
                let (title, branch) = (pull.title(), plan.branch.clone());
                this.git_task(&crate::i18n::tf("Checking out {}…", &[&title]), cx, move |h| {
                    // `-B`: an existing branch is reset to the request's tip; git refuses if a dirty tree conflicts.
                    crate::git::git_ok(h, &root, &["fetch", "--quiet", "--", &from, &plan.r#ref])?;
                    crate::git::git_ok(h, &root, &["checkout", "-q", "-B", &branch, "FETCH_HEAD"])?;
                    Ok(crate::i18n::tf("Checked out {} as {}", &[&title, &branch]))
                });
            })
            .ok();
        })
        .detach();
    }
}

#[cfg(test)]
impl Workbench {
    /// The text of the Markdown page shown (a request's details).
    pub fn pull_page_for_test(&self, cx: &App) -> Option<String> {
        match &self.tabs.page {
            Some(Page::Markdown(view)) => Some(view.read(cx).source().read(cx).text_lines().join("\n")),
            _ => None,
        }
    }
}
