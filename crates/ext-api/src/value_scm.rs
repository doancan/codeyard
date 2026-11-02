//! `scm`: source control providers (`[[scm]]`), in the frames' types. Re-exported from `value`.

use serde::{Deserialize, Serialize};

use super::Uri;

/// `scm.file-state`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum FileState {
    Modified,
    Added,
    Deleted,
    Renamed,
    Untracked,
    Ignored,
    Conflicted,
}

/// `scm.scm-action`: one of the extension's own commands, by local id.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScmAction {
    pub command: String,
    pub title: String,
    pub icon: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScmResource {
    pub uri: Uri,
    pub state: FileState,
    pub detail: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScmGroup {
    pub id: String,
    pub label: String,
    pub resources: Vec<ScmResource>,
    pub resource_actions: Vec<ScmAction>,
    pub group_actions: Vec<ScmAction>,
    pub hide_when_empty: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScmInput {
    pub placeholder: String,
    pub value: Option<String>,
    pub accept: Option<ScmAction>,
}

/// `scm.scm-state`: what Source Control shows for one folder.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScmState {
    pub head: Option<String>,
    pub input: ScmInput,
    pub groups: Vec<ScmGroup>,
}
