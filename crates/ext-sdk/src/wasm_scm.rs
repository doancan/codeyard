//! The SDK's source control types → wit-bindgen's (`scm`), inside a component.

use super::w;
use crate::scm::{FileState, ScmAction, ScmState};

impl From<ScmAction> for w::scm::ScmAction {
    fn from(a: ScmAction) -> Self {
        w::scm::ScmAction { command: a.command, title: a.title, icon: a.icon }
    }
}

impl From<ScmState> for w::scm::ScmState {
    fn from(s: ScmState) -> Self {
        use w::scm::FileState as F;
        let actions = |a: Vec<ScmAction>| a.into_iter().map(Into::into).collect();
        w::scm::ScmState {
            head: s.head,
            input: w::scm::ScmInput { placeholder: s.input.placeholder, value: s.input.value, accept: s.input.accept.map(Into::into) },
            groups: s
                .groups
                .into_iter()
                .map(|g| w::scm::ScmGroup {
                    id: g.id,
                    label: g.label,
                    resources: g
                        .resources
                        .into_iter()
                        .map(|r| w::scm::ScmResource {
                            uri: r.uri,
                            state: match r.state {
                                FileState::Modified => F::Modified,
                                FileState::Added => F::Added,
                                FileState::Deleted => F::Deleted,
                                FileState::Renamed => F::Renamed,
                                FileState::Untracked => F::Untracked,
                                FileState::Ignored => F::Ignored,
                                FileState::Conflicted => F::Conflicted,
                            },
                            detail: r.detail,
                        })
                        .collect(),
                    resource_actions: actions(g.resource_actions),
                    group_actions: actions(g.group_actions),
                    hide_when_empty: g.hide_when_empty,
                })
                .collect(),
        }
    }
}

pub(crate) fn scm_refresh(provider: &str) {
    w::scm::refresh(provider);
}
