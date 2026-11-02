//! Source control providers on the mock host: ask for a folder's state, a file's original text, a check of the input
//! box, and press a button the way Source Control does.

use codeyard_extension::native::Note;
use codeyard_extension::protocol::{Request, Response};
use codeyard_extension::scm::{ScmAction, ScmState};
use codeyard_extension::{Error, Result, Value, json};

use crate::MockHost;

impl MockHost {
    /// `request.scm-status`: the state of `root` (`""` for the mock folder, a path in it, or a uri) for provider `id`.
    pub fn scm_status(&self, id: &str, root: &str) -> Result<ScmState> {
        match self.request(Request::ScmStatus { provider: id.to_string(), root: self.folder(root) })? {
            Response::Scm(s) => Ok(s),
            other => Err(Error::Other(format!("the source control provider answered {other:?}"))),
        }
    }

    /// `request.scm-original`: the original text of `file` for provider `id`.
    pub fn scm_original(&self, id: &str, file: &str) -> Result<Option<String>> {
        match self.request(Request::ScmOriginal { provider: id.to_string(), uri: self.uri(file) })? {
            Response::Text(t) => Ok(t),
            other => Err(Error::Other(format!("the source control provider answered {other:?}"))),
        }
    }

    /// `request.scm-validate`: the message for the input box's `text`, if any.
    pub fn scm_validate(&self, id: &str, root: &str, text: &str) -> Result<Option<String>> {
        match self.request(Request::ScmValidate { provider: id.to_string(), root: self.folder(root), value: text.to_string() })? {
            Response::Text(t) => Ok(t),
            other => Err(Error::Other(format!("the source control provider answered {other:?}"))),
        }
    }

    /// Press `action` as Source Control does: its command, with the provider, folder, files and input box text.
    pub fn scm_press(&self, id: &str, action: &ScmAction, root: &str, files: &[&str], message: &str) -> Result<Value> {
        let uris: Vec<String> = files.iter().map(|f| self.uri(f)).collect();
        self.command(&action.command, json!({ "provider": id, "root": self.folder(root), "uris": uris, "message": message }))
    }

    /// A folder: `""` is the mock workspace folder itself.
    fn folder(&self, root: &str) -> String {
        if root.is_empty() { crate::host::FOLDER.to_string() } else { self.uri(root) }
    }

    /// Providers the extension asked to refresh (`ctx.scm_refresh`), in order.
    pub fn scm_refreshed(&self) -> Vec<String> {
        self.notes().into_iter().filter_map(|n| if let Note::ScmRefresh(p) = n { Some(p) } else { None }).collect()
    }
}
