//! Source control providers (`[[scm]]`): the state Source Control shows for a folder the provider claims, the original
//! text of a file for the gutter's change marks, and a check of the input box. Buttons run the extension's own
//! commands with [`ScmArgs`].
//!
//! ```ignore
//! ctx.scm_status("jj", async |ctx, root| {
//!     let changes = ScmGroup::new("changes", "Changes").resource_action(ScmAction::new("restore", "Discard").icon("undo-2"));
//!     Ok(ScmState::new().group(changes).input(ScmInput::new("Describe the change").accept(ScmAction::new("commit", "Commit"))))
//! });
//! ctx.command("commit", async |ctx, args| {
//!     let args = ScmArgs::from(args)?;
//!     // run `jj commit -m <args.message>` in args.root …
//!     ctx.scm_refresh(&args.provider);
//!     Ok(())
//! });
//! ```

use std::collections::HashMap;
use std::rc::Rc;

use serde::Deserialize;
use serde_json::Value;

use crate::error::{Error, Result};
use crate::guest::{self, BoxFuture};
use crate::protocol::{Request, Response};
use crate::types::Uri;
use crate::{Context, sys};

/// How a file differs from what the provider last recorded (`scm.file-state`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileState {
    Modified,
    Added,
    Deleted,
    Renamed,
    Untracked,
    Ignored,
    Conflicted,
}

/// A button that runs one of the extension's own commands (local id) with [`ScmArgs`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ScmAction {
    pub command: String,
    /// The tooltip and the screen reader's name.
    pub title: String,
    /// An icon name (reference/ui.md): `plus`, `minus`, `undo-2`, `check`.
    pub icon: Option<String>,
}

impl ScmAction {
    pub fn new(command: impl Into<String>, title: impl Into<String>) -> ScmAction {
        ScmAction { command: command.into(), title: title.into(), icon: None }
    }

    pub fn icon(mut self, icon: impl Into<String>) -> ScmAction {
        self.icon = Some(icon.into());
        self
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScmResource {
    pub uri: Uri,
    pub state: FileState,
    /// Shown after the file name, such as the old name of a renamed file.
    pub detail: Option<String>,
}

impl ScmResource {
    pub fn new(uri: impl Into<Uri>, state: FileState) -> ScmResource {
        ScmResource { uri: uri.into(), state, detail: None }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ScmGroup {
    /// Local to the provider; keeps the group open or closed between refreshes.
    pub id: String,
    pub label: String,
    pub resources: Vec<ScmResource>,
    /// Buttons on each file; they get that file's uri.
    pub resource_actions: Vec<ScmAction>,
    /// Buttons on the header; they get every uri of the group.
    pub group_actions: Vec<ScmAction>,
    pub hide_when_empty: bool,
}

impl ScmGroup {
    pub fn new(id: impl Into<String>, label: impl Into<String>) -> ScmGroup {
        ScmGroup { id: id.into(), label: label.into(), ..ScmGroup::default() }
    }

    pub fn resource(mut self, r: ScmResource) -> ScmGroup {
        self.resources.push(r);
        self
    }

    pub fn resource_action(mut self, a: ScmAction) -> ScmGroup {
        self.resource_actions.push(a);
        self
    }

    pub fn group_action(mut self, a: ScmAction) -> ScmGroup {
        self.group_actions.push(a);
        self
    }

    pub fn hide_when_empty(mut self) -> ScmGroup {
        self.hide_when_empty = true;
        self
    }
}

/// The input box above the groups.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ScmInput {
    pub placeholder: String,
    /// Replaces the box's text when set.
    pub value: Option<String>,
    /// The button under the box (and Ctrl+Enter); none hides the box.
    pub accept: Option<ScmAction>,
}

impl ScmInput {
    pub fn new(placeholder: impl Into<String>) -> ScmInput {
        ScmInput { placeholder: placeholder.into(), ..ScmInput::default() }
    }

    pub fn accept(mut self, a: ScmAction) -> ScmInput {
        self.accept = Some(a);
        self
    }

    pub fn value(mut self, v: impl Into<String>) -> ScmInput {
        self.value = Some(v.into());
        self
    }
}

/// What Source Control shows for one folder (`scm.scm-state`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ScmState {
    /// Where the folder stands: a branch, a change id.
    pub head: Option<String>,
    pub input: ScmInput,
    pub groups: Vec<ScmGroup>,
}

impl ScmState {
    pub fn new() -> ScmState {
        ScmState::default()
    }

    pub fn head(mut self, head: impl Into<String>) -> ScmState {
        self.head = Some(head.into());
        self
    }

    pub fn input(mut self, input: ScmInput) -> ScmState {
        self.input = input;
        self
    }

    pub fn group(mut self, g: ScmGroup) -> ScmState {
        self.groups.push(g);
        self
    }
}

/// The arguments a source control button passes to its command.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
pub struct ScmArgs {
    /// The provider's local id.
    pub provider: String,
    /// The folder.
    pub root: Uri,
    /// The file the button is on, or every file of the group.
    #[serde(default)]
    pub uris: Vec<Uri>,
    /// The input box's text.
    #[serde(default)]
    pub message: String,
}

impl ScmArgs {
    /// Read a command's arguments; fails with `invalid` when the command was not started by a source control button.
    pub fn from(args: Value) -> Result<ScmArgs> {
        serde_json::from_value(args).map_err(|e| Error::Invalid(format!("not the arguments of a source control button: {e}")))
    }
}

type StatusFn = Rc<dyn Fn(Context, Uri) -> BoxFuture<Result<ScmState>>>;
type TextFn = Rc<dyn Fn(Context, Uri) -> BoxFuture<Result<Option<String>>>>;
type ValidateFn = Rc<dyn Fn(Context, Uri, String) -> BoxFuture<Result<Option<String>>>>;

/// The providers the extension registered, by local id.
#[derive(Default)]
pub(crate) struct ScmHandlers {
    status: HashMap<String, StatusFn>,
    original: HashMap<String, TextFn>,
    validate: HashMap<String, ValidateFn>,
}

impl Context {
    /// The state of a folder that `[[scm]]` provider `id` claims: its groups of changed files and its input box.
    /// Asked when Source Control shows the folder, after files change, and after [`Context::scm_refresh`].
    pub fn scm_status<F>(&self, id: &str, f: F)
    where
        F: AsyncFn(Context, Uri) -> Result<ScmState> + 'static,
    {
        let f = Rc::new(f);
        let cb: StatusFn = Rc::new(move |ctx, root| {
            let f = f.clone();
            Box::pin(async move { f(ctx, root).await })
        });
        guest::with(|h| h.scm.status.insert(id.to_string(), cb));
    }

    /// The text of a file as provider `id` last recorded it, for the gutter's change marks; `None` for a file it does
    /// not track.
    pub fn scm_original<F>(&self, id: &str, f: F)
    where
        F: AsyncFn(Context, Uri) -> Result<Option<String>> + 'static,
    {
        let f = Rc::new(f);
        let cb: TextFn = Rc::new(move |ctx, uri| {
            let f = f.clone();
            Box::pin(async move { f(ctx, uri).await })
        });
        guest::with(|h| h.scm.original.insert(id.to_string(), cb));
    }

    /// Check the input box's text (folder, text): a message to show under the box, or `None` when it is fine. The
    /// accept button does not run while there is a message.
    pub fn scm_validate<F>(&self, id: &str, f: F)
    where
        F: AsyncFn(Context, Uri, String) -> Result<Option<String>> + 'static,
    {
        let f = Rc::new(f);
        let cb: ValidateFn = Rc::new(move |ctx, root, text| {
            let f = f.clone();
            Box::pin(async move { f(ctx, root, text).await })
        });
        guest::with(|h| h.scm.validate.insert(id.to_string(), cb));
    }

    /// Ask Codeyard to read provider `id`'s state again now (after the extension changed something itself).
    pub fn scm_refresh(&self, id: &str) {
        sys::scm_refresh(id);
    }
}

/// `request.scm-*`: answered by what the extension registered.
pub(crate) fn handle(id: u64, ctx: Context, request: Request) {
    let missing = |what: &str, provider: &str| {
        sys::respond(
            id,
            Err(Error::NotFound(format!("no source control {what} for '{provider}' is registered; call ctx.{what}(\"{provider}\", …) in activate"))),
        )
    };
    match request {
        Request::ScmStatus { provider, root } => match guest::with(|h| h.scm.status.get(&provider).cloned()) {
            Some(f) => guest::answer(id, async move { f(ctx, root).await.map(Response::Scm) }),
            None => missing("scm_status", &provider),
        },
        Request::ScmOriginal { provider, uri } => match guest::with(|h| h.scm.original.get(&provider).cloned()) {
            Some(f) => guest::answer(id, async move { f(ctx, uri).await.map(Response::Text) }),
            None => sys::respond(id, Ok(Response::Text(None))),
        },
        Request::ScmValidate { provider, root, value } => match guest::with(|h| h.scm.validate.get(&provider).cloned()) {
            Some(f) => guest::answer(id, async move { f(ctx, root, value).await.map(Response::Text) }),
            None => sys::respond(id, Ok(Response::Text(None))),
        },
        _ => {}
    }
}
