//! Pull (merge) request providers: `[[pull-requests]]` in the manifest, registered with
//! [`Context::pull_requests`]. Codeyard asks for the open requests of each repository remote whose host the provider
//! claims, and shows them in its Pull Requests view next to GitHub's.
//!
//! ```ignore
//! ctx.pull_requests("gitea")
//!     .list(async |ctx, remote| {
//!         let url = format!("https://{}/api/v1/repos/{}/pulls?state=open", remote.host, remote.path);
//!         let account = remote.account.clone().unwrap_or_default();
//!         let res = ctx.fetch_as(&account, Request::get(&url)).await?.error_for_status()?;
//!         parse(&res.text()?)
//!     })
//!     .details(async |ctx, remote, id| todo!())
//!     .files(async |ctx, remote, id| todo!())
//!     .contents(async |ctx, remote, id, file| todo!());
//! ```
//!
//! Call the forge with [`Context::fetch_as`] and `remote.account`: Codeyard signs the request, so the extension never
//! sees the token.

use std::collections::HashMap;
use std::rc::Rc;

use crate::Context;
use crate::error::{Error, Result};
use crate::guest::{self, BoxFuture};
use crate::protocol::Response;

/// A repository remote whose host the provider claims (`pull-requests.remote`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Remote {
    /// The provider's local id from the manifest.
    pub provider: String,
    /// The remote's name in the repository, such as `origin`.
    pub name: String,
    /// The URL as git has it.
    pub url: String,
    /// The host, lowercase: `codeberg.org`.
    pub host: String,
    /// The repository's path on the host, without `.git`: `group/app`.
    pub path: String,
    /// The contribution's `auth` provider, for [`Context::fetch_as`].
    pub account: Option<String>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PullState {
    #[default]
    Open,
    Draft,
    Merged,
    Closed,
}

/// One request in the list (`pull-requests.summary`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PullRequest {
    /// The provider's id for it, passed back to the other handlers.
    pub id: String,
    /// Shown before the title: `#42`, `!42`.
    pub reference: String,
    pub title: String,
    pub author: String,
    pub state: PullState,
    /// The branch with the changes.
    pub source_branch: String,
    /// The branch it merges into.
    pub target_branch: String,
    /// Last change, Unix milliseconds; 0 when unknown.
    pub updated: u64,
    /// Its web page (`https:` only).
    pub url: Option<String>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CheckState {
    Pending,
    Success,
    Failure,
    #[default]
    Neutral,
}

/// A CI result (`pull-requests.check`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Check {
    pub name: String,
    pub state: CheckState,
    pub url: Option<String>,
}

/// `pull-requests.details`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PullRequestDetails {
    pub summary: PullRequest,
    /// Markdown.
    pub description: String,
    /// Empty when the forge has none.
    pub checks: Vec<Check>,
    pub labels: Vec<String>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum FileStatus {
    Added,
    #[default]
    Modified,
    Deleted,
    Renamed,
}

/// `pull-requests.changed-file`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ChangedFile {
    /// In the source branch (in the target branch for a deleted file).
    pub path: String,
    /// In the target branch, for a renamed file.
    pub old_path: Option<String>,
    pub status: FileStatus,
}

/// One changed file's text on both sides (`pull-requests.file-contents`); none for a side where it does not exist or
/// is binary.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FileContents {
    pub base: Option<String>,
    pub head: Option<String>,
}

/// `pull-requests.comment`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Comment {
    pub author: String,
    /// Markdown.
    pub body: String,
    /// Unix milliseconds; 0 when unknown.
    pub created: u64,
    /// For a comment on the diff: the file and its line (1-based) in the source branch.
    pub path: Option<String>,
    pub line: Option<u32>,
}

/// How to check a request out (`pull-requests.checkout`): Codeyard fetches `reference` from `fetch_url` (or the
/// remote) and creates or resets `branch` on it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Checkout {
    /// A fork's clone URL, or none for the remote itself.
    pub fetch_url: Option<String>,
    /// `refs/merge-requests/42/head`, `refs/pull/42/head`, or a branch name.
    pub reference: String,
    /// The local branch: `mr-42`.
    pub branch: String,
}

/// What the host asks a provider (`pull-requests.query`). The mock host sends these; authors register handlers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PullQuery {
    List(Remote),
    Details(Remote, String),
    Files(Remote, String),
    Contents(Remote, String, ChangedFile),
    Comments(Remote, String),
    AddComment(Remote, String, String),
    Checkout(Remote, String),
}

impl PullQuery {
    pub fn remote(&self) -> &Remote {
        match self {
            PullQuery::List(r)
            | PullQuery::Details(r, _)
            | PullQuery::Files(r, _)
            | PullQuery::Contents(r, _, _)
            | PullQuery::Comments(r, _)
            | PullQuery::AddComment(r, _, _)
            | PullQuery::Checkout(r, _) => r,
        }
    }
}

/// A provider's answer (`pull-requests.answer`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PullAnswer {
    List(Vec<PullRequest>),
    Details(Box<PullRequestDetails>),
    Files(Vec<ChangedFile>),
    Contents(FileContents),
    Comments(Vec<Comment>),
    Checkout(Checkout),
    Done,
}

type ListFn = Rc<dyn Fn(Context, Remote) -> BoxFuture<Result<Vec<PullRequest>>>>;
type DetailsFn = Rc<dyn Fn(Context, Remote, String) -> BoxFuture<Result<PullRequestDetails>>>;
type FilesFn = Rc<dyn Fn(Context, Remote, String) -> BoxFuture<Result<Vec<ChangedFile>>>>;
type ContentsFn = Rc<dyn Fn(Context, Remote, String, ChangedFile) -> BoxFuture<Result<FileContents>>>;
type CommentsFn = Rc<dyn Fn(Context, Remote, String) -> BoxFuture<Result<Vec<Comment>>>>;
type AddCommentFn = Rc<dyn Fn(Context, Remote, String, String) -> BoxFuture<Result<()>>>;
type CheckoutFn = Rc<dyn Fn(Context, Remote, String) -> BoxFuture<Result<Checkout>>>;

/// The handlers of one provider.
#[derive(Clone, Default)]
pub(crate) struct Provider {
    list: Option<ListFn>,
    details: Option<DetailsFn>,
    files: Option<FilesFn>,
    contents: Option<ContentsFn>,
    comments: Option<CommentsFn>,
    add_comment: Option<AddCommentFn>,
    checkout: Option<CheckoutFn>,
}

pub(crate) type Providers = HashMap<String, Provider>;

/// Registers the handlers of the manifest's `[[pull-requests]]` provider `id`; see [`Context::pull_requests`].
pub struct PullRequests {
    id: String,
}

macro_rules! register {
    ($(#[$doc:meta])* $name:ident, $field:ident, ($($arg:ident: $ty:ty),*) -> $out:ty) => {
        $(#[$doc])*
        pub fn $name<F>(self, f: F) -> Self
        where
            F: AsyncFn(Context, $($ty),*) -> Result<$out> + 'static,
        {
            let f = Rc::new(f);
            let cb: Rc<dyn Fn(Context, $($ty),*) -> BoxFuture<Result<$out>>> = Rc::new(move |ctx, $($arg),*| {
                let f = f.clone();
                Box::pin(async move { f(ctx, $($arg),*).await })
            });
            guest::with(|h| h.pulls.entry(self.id.clone()).or_default().$field = Some(cb));
            self
        }
    };
}

impl PullRequests {
    register!(
        /// The open requests of the remote's repository, newest first. Required.
        list, list, (remote: Remote) -> Vec<PullRequest>
    );
    register!(
        /// One request, by [`PullRequest::id`]: its description (Markdown), checks and labels. Required.
        details, details, (remote: Remote, id: String) -> PullRequestDetails
    );
    register!(
        /// The files a request changes. Required.
        files, files, (remote: Remote, id: String) -> Vec<ChangedFile>
    );
    register!(
        /// One changed file's text in the target and source branches, for the diff. Required.
        contents, contents, (remote: Remote, id: String, file: ChangedFile) -> FileContents
    );
    register!(
        /// The request's comments, oldest first. Without it the view shows none.
        comments, comments, (remote: Remote, id: String) -> Vec<Comment>
    );
    register!(
        /// The user wrote a comment (Markdown). A user action. Without it the view offers no "Add comment".
        on_comment, add_comment, (remote: Remote, id: String, body: String) -> ()
    );
    register!(
        /// How to check the request out; Codeyard runs git. Without it, checking out fails and says so.
        checkout, checkout, (remote: Remote, id: String) -> Checkout
    );
}

impl Context {
    /// Provide the manifest's `[[pull-requests]]` entry `id` (its local id): chain the handlers on what this returns.
    pub fn pull_requests(&self, id: &str) -> PullRequests {
        guest::with(|h| {
            h.pulls.entry(id.to_string()).or_default();
        });
        PullRequests { id: id.to_string() }
    }
}

/// `request.pull-requests`: route the query to its provider's handler.
pub(crate) fn handle(id: u64, ctx: Context, query: PullQuery) {
    let name = query.remote().provider.clone();
    let Some(p) = guest::with(|h| h.pulls.get(&name).cloned()) else {
        let e = Error::NotFound(format!("no pull request provider '{name}' is registered; call ctx.pull_requests(\"{name}\") in activate"));
        return crate::sys::respond(id, Err(e));
    };
    let missing =
        |what: &str| Error::NotFound(format!("pull request provider '{name}' has no {what} handler; add .{what}(…) to ctx.pull_requests(\"{name}\")"));
    let answer = |fut: BoxFuture<Result<PullAnswer>>| guest::answer(id, async move { fut.await.map(Response::PullRequests) });
    match query {
        PullQuery::List(remote) => match p.list {
            Some(f) => answer(Box::pin(async move { f(ctx, remote).await.map(PullAnswer::List) })),
            None => crate::sys::respond(id, Err(missing("list"))),
        },
        PullQuery::Details(remote, pr) => match p.details {
            Some(f) => answer(Box::pin(async move { f(ctx, remote, pr).await.map(|d| PullAnswer::Details(Box::new(d))) })),
            None => crate::sys::respond(id, Err(missing("details"))),
        },
        PullQuery::Files(remote, pr) => match p.files {
            Some(f) => answer(Box::pin(async move { f(ctx, remote, pr).await.map(PullAnswer::Files) })),
            None => crate::sys::respond(id, Err(missing("files"))),
        },
        PullQuery::Contents(remote, pr, file) => match p.contents {
            Some(f) => answer(Box::pin(async move { f(ctx, remote, pr, file).await.map(PullAnswer::Contents) })),
            None => crate::sys::respond(id, Err(missing("contents"))),
        },
        PullQuery::Comments(remote, pr) => match p.comments {
            Some(f) => answer(Box::pin(async move { f(ctx, remote, pr).await.map(PullAnswer::Comments) })),
            None => crate::sys::respond(id, Ok(Response::PullRequests(PullAnswer::Comments(Vec::new())))),
        },
        PullQuery::AddComment(remote, pr, body) => match p.add_comment {
            Some(f) => answer(Box::pin(async move { f(ctx, remote, pr, body).await.map(|()| PullAnswer::Done) })),
            None => crate::sys::respond(id, Err(Error::Invalid(format!("pull request provider '{name}' does not take comments")))),
        },
        PullQuery::Checkout(remote, pr) => match p.checkout {
            Some(f) => answer(Box::pin(async move { f(ctx, remote, pr).await.map(PullAnswer::Checkout) })),
            None => crate::sys::respond(id, Err(missing("checkout"))),
        },
    }
}

/// The bindings ↔ these types, inside a component.
#[cfg(target_arch = "wasm32")]
pub(crate) mod wasm {
    use super::*;
    use crate::wasm::bindings::codeyard::api::pull_requests as w;

    fn remote(r: w::Remote) -> Remote {
        Remote { provider: r.provider, name: r.name, url: r.url, host: r.host, path: r.path, account: r.account }
    }

    fn file(f: w::ChangedFile) -> ChangedFile {
        let status = match f.status {
            w::FileStatus::Added => FileStatus::Added,
            w::FileStatus::Modified => FileStatus::Modified,
            w::FileStatus::Deleted => FileStatus::Deleted,
            w::FileStatus::Renamed => FileStatus::Renamed,
        };
        ChangedFile { path: f.path, old_path: f.old_path, status }
    }

    pub(crate) fn query(q: w::Query) -> PullQuery {
        match q {
            w::Query::List(r) => PullQuery::List(remote(r)),
            w::Query::Details((r, id)) => PullQuery::Details(remote(r), id),
            w::Query::Files((r, id)) => PullQuery::Files(remote(r), id),
            w::Query::Contents((r, id, f)) => PullQuery::Contents(remote(r), id, file(f)),
            w::Query::Comments((r, id)) => PullQuery::Comments(remote(r), id),
            w::Query::AddComment((r, id, body)) => PullQuery::AddComment(remote(r), id, body),
            w::Query::Checkout((r, id)) => PullQuery::Checkout(remote(r), id),
        }
    }

    fn summary(s: PullRequest) -> w::Summary {
        let state = match s.state {
            PullState::Open => w::State::Open,
            PullState::Draft => w::State::Draft,
            PullState::Merged => w::State::Merged,
            PullState::Closed => w::State::Closed,
        };
        w::Summary {
            id: s.id,
            reference: s.reference,
            title: s.title,
            author: s.author,
            state,
            source_branch: s.source_branch,
            target_branch: s.target_branch,
            updated: s.updated,
            url: s.url,
        }
    }

    pub(crate) fn answer(a: PullAnswer) -> w::Answer {
        match a {
            PullAnswer::List(l) => w::Answer::List(l.into_iter().map(summary).collect()),
            PullAnswer::Details(d) => w::Answer::Details(w::Details {
                summary: summary(d.summary),
                description: d.description,
                checks: d
                    .checks
                    .into_iter()
                    .map(|c| w::Check {
                        name: c.name,
                        state: match c.state {
                            CheckState::Pending => w::CheckState::Pending,
                            CheckState::Success => w::CheckState::Success,
                            CheckState::Failure => w::CheckState::Failure,
                            CheckState::Neutral => w::CheckState::Neutral,
                        },
                        url: c.url,
                    })
                    .collect(),
                labels: d.labels,
            }),
            PullAnswer::Files(f) => w::Answer::Files(
                f.into_iter()
                    .map(|f| w::ChangedFile {
                        path: f.path,
                        old_path: f.old_path,
                        status: match f.status {
                            FileStatus::Added => w::FileStatus::Added,
                            FileStatus::Modified => w::FileStatus::Modified,
                            FileStatus::Deleted => w::FileStatus::Deleted,
                            FileStatus::Renamed => w::FileStatus::Renamed,
                        },
                    })
                    .collect(),
            ),
            PullAnswer::Contents(c) => w::Answer::Contents(w::FileContents { base: c.base, head: c.head }),
            PullAnswer::Comments(c) => w::Answer::Comments(
                c.into_iter().map(|c| w::Comment { author: c.author, body: c.body, created: c.created, path: c.path, line: c.line }).collect(),
            ),
            PullAnswer::Checkout(c) => w::Answer::Checkout(w::Checkout { fetch_url: c.fetch_url, ref_: c.reference, branch: c.branch }),
            PullAnswer::Done => w::Answer::Done,
        }
    }
}
