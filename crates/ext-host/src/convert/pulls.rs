//! `pull-requests`: queries to the guest, answers from it.

use ext_api::value as v;

use crate::bindings::codeyard::api::pull_requests as g;

// --- guest → app

fn summary(s: g::Summary) -> v::PullSummary {
    v::PullSummary {
        id: s.id,
        reference: s.reference,
        title: s.title,
        author: s.author,
        state: match s.state {
            g::State::Open => v::PullState::Open,
            g::State::Draft => v::PullState::Draft,
            g::State::Merged => v::PullState::Merged,
            g::State::Closed => v::PullState::Closed,
        },
        source_branch: s.source_branch,
        target_branch: s.target_branch,
        updated: s.updated,
        url: s.url,
    }
}

fn file_status(s: g::FileStatus) -> v::FileStatus {
    match s {
        g::FileStatus::Added => v::FileStatus::Added,
        g::FileStatus::Modified => v::FileStatus::Modified,
        g::FileStatus::Deleted => v::FileStatus::Deleted,
        g::FileStatus::Renamed => v::FileStatus::Renamed,
    }
}

impl From<g::Answer> for v::PullAnswer {
    fn from(a: g::Answer) -> Self {
        match a {
            g::Answer::List(l) => v::PullAnswer::List(l.into_iter().map(summary).collect()),
            g::Answer::Details(d) => v::PullAnswer::Details(Box::new(v::PullDetails {
                summary: summary(d.summary),
                description: d.description,
                checks: d
                    .checks
                    .into_iter()
                    .map(|c| v::PullCheck {
                        name: c.name,
                        state: match c.state {
                            g::CheckState::Pending => v::CheckState::Pending,
                            g::CheckState::Success => v::CheckState::Success,
                            g::CheckState::Failure => v::CheckState::Failure,
                            g::CheckState::Neutral => v::CheckState::Neutral,
                        },
                        url: c.url,
                    })
                    .collect(),
                labels: d.labels,
            })),
            g::Answer::Files(f) => {
                v::PullAnswer::Files(f.into_iter().map(|f| v::ChangedFile { path: f.path, old_path: f.old_path, status: file_status(f.status) }).collect())
            }
            g::Answer::Contents(c) => v::PullAnswer::Contents(v::FileContents { base: c.base, head: c.head }),
            g::Answer::Comments(c) => v::PullAnswer::Comments(
                c.into_iter().map(|c| v::PullComment { author: c.author, body: c.body, created: c.created, path: c.path, line: c.line }).collect(),
            ),
            g::Answer::Checkout(c) => v::PullAnswer::Checkout(v::PullCheckout { fetch_url: c.fetch_url, r#ref: c.ref_, branch: c.branch }),
            g::Answer::Done => v::PullAnswer::Done,
        }
    }
}

// --- app → guest

fn remote(r: v::PullRemote) -> g::Remote {
    g::Remote { provider: r.provider, name: r.name, url: r.url, host: r.host, path: r.path, account: r.account }
}

fn changed_file(f: v::ChangedFile) -> g::ChangedFile {
    g::ChangedFile {
        path: f.path,
        old_path: f.old_path,
        status: match f.status {
            v::FileStatus::Added => g::FileStatus::Added,
            v::FileStatus::Modified => g::FileStatus::Modified,
            v::FileStatus::Deleted => g::FileStatus::Deleted,
            v::FileStatus::Renamed => g::FileStatus::Renamed,
        },
    }
}

impl From<v::PullQuery> for g::Query {
    fn from(q: v::PullQuery) -> Self {
        match q {
            v::PullQuery::List { remote: r } => g::Query::List(remote(r)),
            v::PullQuery::Details { remote: r, id } => g::Query::Details((remote(r), id)),
            v::PullQuery::Files { remote: r, id } => g::Query::Files((remote(r), id)),
            v::PullQuery::Contents { remote: r, id, file } => g::Query::Contents((remote(r), id, changed_file(file))),
            v::PullQuery::Comments { remote: r, id } => g::Query::Comments((remote(r), id)),
            v::PullQuery::AddComment { remote: r, id, body } => g::Query::AddComment((remote(r), id, body)),
            v::PullQuery::Checkout { remote: r, id } => g::Query::Checkout((remote(r), id)),
        }
    }
}
