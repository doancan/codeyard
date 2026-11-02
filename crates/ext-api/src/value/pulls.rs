//! `pull-requests`: the values of `[[pull-requests]]` providers (`Request::PullRequests`, `Response::PullRequests`).

use serde::{Deserialize, Serialize};

/// `pull-requests.remote`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PullRemote {
    pub provider: String,
    pub name: String,
    pub url: String,
    pub host: String,
    pub path: String,
    pub account: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum PullState {
    Open,
    Draft,
    Merged,
    Closed,
}

/// `pull-requests.summary`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PullSummary {
    pub id: String,
    pub reference: String,
    pub title: String,
    pub author: String,
    pub state: PullState,
    pub source_branch: String,
    pub target_branch: String,
    pub updated: u64,
    pub url: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum CheckState {
    Pending,
    Success,
    Failure,
    Neutral,
}

/// `pull-requests.check`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PullCheck {
    pub name: String,
    pub state: CheckState,
    pub url: Option<String>,
}

/// `pull-requests.details`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PullDetails {
    pub summary: PullSummary,
    pub description: String,
    pub checks: Vec<PullCheck>,
    pub labels: Vec<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum FileStatus {
    Added,
    Modified,
    Deleted,
    Renamed,
}

/// `pull-requests.changed-file`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChangedFile {
    pub path: String,
    pub old_path: Option<String>,
    pub status: FileStatus,
}

/// `pull-requests.file-contents`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileContents {
    pub base: Option<String>,
    pub head: Option<String>,
}

/// `pull-requests.comment`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PullComment {
    pub author: String,
    pub body: String,
    pub created: u64,
    pub path: Option<String>,
    pub line: Option<u32>,
}

/// `pull-requests.checkout`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PullCheckout {
    pub fetch_url: Option<String>,
    pub r#ref: String,
    pub branch: String,
}

/// `pull-requests.query`: what the host asks a provider. Each but `List` names the request by `PullSummary::id`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum PullQuery {
    List { remote: PullRemote },
    Details { remote: PullRemote, id: String },
    Files { remote: PullRemote, id: String },
    Contents { remote: PullRemote, id: String, file: ChangedFile },
    Comments { remote: PullRemote, id: String },
    AddComment { remote: PullRemote, id: String, body: String },
    Checkout { remote: PullRemote, id: String },
}

impl PullQuery {
    pub fn remote(&self) -> &PullRemote {
        match self {
            PullQuery::List { remote }
            | PullQuery::Details { remote, .. }
            | PullQuery::Files { remote, .. }
            | PullQuery::Contents { remote, .. }
            | PullQuery::Comments { remote, .. }
            | PullQuery::AddComment { remote, .. }
            | PullQuery::Checkout { remote, .. } => remote,
        }
    }

    /// Started by the user: adding a comment, checking out.
    pub fn is_user_action(&self) -> bool {
        matches!(self, PullQuery::AddComment { .. } | PullQuery::Checkout { .. })
    }
}

/// `pull-requests.answer`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum PullAnswer {
    List(Vec<PullSummary>),
    Details(Box<PullDetails>),
    Files(Vec<ChangedFile>),
    Contents(FileContents),
    Comments(Vec<PullComment>),
    Checkout(PullCheckout),
    Done,
}
