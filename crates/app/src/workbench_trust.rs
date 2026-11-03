//! Workbench: Workspace Trust. Which folders are trusted and why, Restricted Mode, the Manage Workspace Trust picker, and
//! the question a folder asks when it opens. The gate itself (what an untrusted folder may not do) is in
//! `workbench_connect.rs`, `settings.rs` (`RESTRICTED_KEYS`) and `ext-core`'s `trust.rs`.
//!
//! A record is the state key `trust:<ssh alias>:<folder>`: `1` trusts that folder, `tree` also every folder below it.

use super::*;

gpui::actions!(workbench, [ManageWorkspaceTrust]);

const RECORD_PREFIX: &str = "trust:";
/// The record's value for a folder trusted together with everything below it.
const TREE: &str = "tree";
/// Suffix of the session cache's entry for a folder trusted with its subfolders.
const TREE_CACHE: &str = "/**";
/// Marks a folder whose opening question was answered (`startupPrompt: once` asks no more).
const ASKED_PREFIX: &str = "trust-asked:";

/// What a row of the Manage Workspace Trust picker does.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum TrustChoice {
    TrustFolder(String),
    TrustWithSubfolders(String),
    /// Stop trusting: the record's state key.
    Forget(String),
}

/// The folder that holds `path`, unless that is a filesystem root (trusting every folder on a disk is no choice to offer).
pub(super) fn parent_folder(path: &str) -> Option<String> {
    let trimmed = path.trim_end_matches(['/', '\\']);
    let (parent, _) = trimmed.rsplit_once(['/', '\\'])?;
    let parent = parent.trim_end_matches(['/', '\\']);
    let is_disk_root = parent.is_empty() || (parent.len() == 2 && parent.ends_with(':'));
    (!is_disk_root).then(|| parent.to_string())
}

/// A trust record's key split into the host (empty on this computer; an SSH alias, or `docker:name`, `wsl:distro`) and the
/// folder, which is the part that starts like a path: `/…`, `\…` or a drive (`C:\…`).
fn split_record(key: &str) -> Option<(&str, &str)> {
    let rest = key.strip_prefix(RECORD_PREFIX)?;
    let starts_path = |s: &str| {
        s.starts_with(['/', '\\']) || s.as_bytes().get(..3).is_some_and(|b| b[0].is_ascii_alphabetic() && b[1] == b':' && matches!(b[2], b'/' | b'\\'))
    };
    rest.char_indices()
        .filter(|(_, c)| *c == ':')
        .find_map(|(i, _)| starts_path(&rest[i + 1..]).then(|| (&rest[..i], &rest[i + 1..])))
        .or_else(|| rest.split_once(':'))
}

/// The picker's rows for the current folder (`None`: no folder open) and the stored records, in the order shown.
pub(super) fn trust_rows(current: Option<(&str, bool, bool)>, records: &[(String, String)]) -> Vec<(String, String, TrustChoice)> {
    use crate::i18n::t;
    let mut rows = Vec::new();
    // `current`: (folder, trusted, trusted by its own record rather than a parent's).
    if let Some((root, trusted, own)) = current {
        if !trusted {
            rows.push((t("Trust This Folder").to_string(), root.to_string(), TrustChoice::TrustFolder(root.to_string())));
            if let Some(parent) = parent_folder(root) {
                rows.push((t("Trust Parent Folder and Subfolders").to_string(), parent.clone(), TrustChoice::TrustWithSubfolders(parent)));
            }
        } else if own {
            rows.push((t("Don't Trust This Folder").to_string(), root.to_string(), TrustChoice::Forget(String::new())));
        }
    }
    for (key, value) in records {
        let Some((alias, folder)) = split_record(key) else { continue };
        let place = if alias.is_empty() { String::new() } else { format!("ssh://{alias} · ") };
        let scope = if value == TREE { t("with subfolders").to_string() } else { t("this folder only").to_string() };
        // The folder's name leads, since a full path is cut off long before it says anything.
        rows.push((ui::file_name(folder).to_string(), format!("{place}{folder} · {scope}"), TrustChoice::Forget(key.clone())));
    }
    rows
}

impl Workbench {
    /// The trust records on disk, for the picker.
    fn trust_records(&self) -> Vec<(String, String)> {
        self.persist.state.as_ref().map(|s| s.entries_with_prefix(RECORD_PREFIX)).unwrap_or_default()
    }

    /// The folder whose record makes `root` trusted: `root` itself, or a parent trusted with its subfolders.
    pub(super) fn trust_source(&self, root: &str) -> Option<String> {
        let own = self.trust_key(root);
        let stored = |key: &str| self.persist.state.as_ref().and_then(|s| s.get(key));
        if self.cfg.trusted.contains(&own) || stored(&own).is_some() {
            return Some(root.to_string());
        }
        let mut folder = parent_folder(root);
        while let Some(parent) = folder {
            let key = self.trust_key(&parent);
            if self.cfg.trusted.contains(&format!("{key}{TREE_CACHE}")) || stored(&key).as_deref() == Some(TREE) {
                return Some(parent);
            }
            folder = parent_folder(&parent);
        }
        None
    }

    /// The folder the window works in while it is not trusted: Restricted Mode is on for it.
    pub(super) fn restricted_root(&self, cx: &App) -> Option<String> {
        let root = self.active_root(cx)?;
        (!self.is_trusted_root(&root, cx)).then_some(root)
    }

    /// Trust `folder` and everything below it.
    fn trust_with_subfolders(&mut self, folder: &str, window: &mut Window, cx: &mut Context<Self>) {
        let key = self.trust_key(folder);
        self.cfg.trusted.insert(format!("{key}{TREE_CACHE}"));
        if let Some(s) = &self.persist.state {
            let _ = s.set(&key, TREE);
        }
        self.trust_applied(crate::i18n::tf("Trusted “{}”", &[&ui::file_name(folder)]), window, cx);
    }

    /// The state after the trust records changed: settings, language servers and extension source control are read again for
    /// what is trusted now.
    pub(super) fn trust_applied(&mut self, notice: String, window: &mut Window, cx: &mut Context<Self>) {
        self.lsp.restricted_notified.clear();
        // The extension SCM claims are keyed on the number of session records, which a record from an earlier session or a
        // folder already cached does not change.
        self.cfg.exts.scm.invalidate();
        self.reload_config(window, cx);
        self.restart_lsp(cx);
        self.notice = Some(Notice::Info(notice.into()));
        cx.notify();
    }

    /// Stop trusting the record `key` (or, empty, the active folder's own record).
    fn forget_trust(&mut self, key: String, window: &mut Window, cx: &mut Context<Self>) {
        let key = if key.is_empty() { self.active_root(cx).map(|r| self.trust_key(&r)).unwrap_or_default() } else { key };
        if key.is_empty() {
            return;
        }
        self.cfg.trusted.remove(&key);
        self.cfg.trusted.remove(&format!("{key}{TREE_CACHE}"));
        if let Some(s) = &self.persist.state {
            let _ = s.remove(&key);
            // A folder forgotten asks again when it next opens, even after "once".
            let _ = s.remove(&format!("{ASKED_PREFIX}{key}"));
        }
        let folder = split_record(&key).map_or(key.as_str(), |(_, f)| f).to_string();
        self.trust_applied(crate::i18n::tf("No longer trusting “{}”", &[&ui::file_name(&folder)]), window, cx);
    }

    /// "Workspace: Manage Workspace Trust…": trust or stop trusting the open folder, and the list of trusted folders.
    pub(super) fn manage_workspace_trust(&mut self, _: &ManageWorkspaceTrust, window: &mut Window, cx: &mut Context<Self>) {
        if !cx.global::<Settings>().workspace_trust {
            self.notice = Some(Notice::Info(crate::i18n::t("Workspace Trust is off (security.workspaceTrust.enabled).").into()));
            return cx.notify();
        }
        let current = self.active_root(cx).map(|root| {
            let source = self.trust_source(&root);
            let own = source.as_deref() == Some(root.as_str());
            (root, source.is_some(), own)
        });
        // The open folder's own record is already the "Don't Trust This Folder" row.
        let own_key = current.as_ref().filter(|(_, _, own)| *own).map(|(root, _, _)| self.trust_key(root));
        let mut records = self.trust_records();
        records.retain(|(key, _)| Some(key) != own_key.as_ref());
        let rows = trust_rows(current.as_ref().map(|(r, t, o)| (r.as_str(), *t, *o)), &records);
        if rows.is_empty() {
            self.notice = Some(Notice::Info(crate::i18n::t("No trusted folders yet.").into()));
            return cx.notify();
        }
        if self.overlay.is_some() {
            self.close_overlay(window, cx);
        }
        let items = rows.iter().map(|(label, detail, _)| PickItem { label: label.clone().into(), detail: Some(detail.clone().into()), keys: None }).collect();
        let placeholder = match &current {
            Some((_, true, _)) => crate::i18n::t("Workspace Trust: this folder is trusted. Select a folder to stop trusting it"),
            Some(_) => crate::i18n::t("Workspace Trust: Restricted Mode, this folder is not trusted. Select a folder to stop trusting it"),
            None => crate::i18n::t("Workspace Trust: select a trusted folder to stop trusting it"),
        };
        let picker = cx.new(|cx| Picker::new(vec![PickSource::List { prefix: "", items }], placeholder, "", None, cx));
        let context = super::types::InputContext::Workspace { host: self.remote.host.clone(), root: self.root(cx) };
        let choices = rows.into_iter().map(|(_, _, choice)| choice).collect();
        self.show_overlay(picker, OverlayKind::WorkspaceTrust { context, choices }, window, cx);
    }

    /// A row of the Manage Workspace Trust picker was chosen.
    pub(super) fn apply_trust_choice(&mut self, choice: TrustChoice, window: &mut Window, cx: &mut Context<Self>) {
        match choice {
            TrustChoice::TrustFolder(root) => self.trust_root(&root, window, cx),
            TrustChoice::TrustWithSubfolders(folder) => self.trust_with_subfolders(&folder, window, cx),
            TrustChoice::Forget(key) => self.forget_trust(key, window, cx),
        }
    }

    /// When a folder has just opened untrusted, asks once whether to trust it (`security.workspaceTrust.startupPrompt`).
    /// Called while rendering: it acts only when the folder changed since the last look, and never over another question.
    pub(super) fn ask_trust_on_open(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let root = self.root(cx);
        if root == self.cfg.trust_prompted {
            return;
        }
        let Some(root) = root.filter(|_| self.overlay.is_none() && !self.persist.panel_restoring) else { return };
        self.cfg.trust_prompted = Some(root.clone());
        let mode = cx.global::<Settings>().workspace_trust_prompt.clone();
        let asked = format!("{ASKED_PREFIX}{}", self.trust_key(&root));
        let already = self.persist.state.as_ref().is_some_and(|s| s.get(&asked).is_some());
        if mode == "never" || self.is_trusted_root(&root, cx) || (mode == "once" && already) {
            return;
        }
        let parent = parent_folder(&root);
        let (workspace, roots) = self.trust_scope(&root);
        let question = crate::questions::trust_on_open(workspace.as_deref(), ui::file_name(&root), parent.as_deref());
        let answer = question.ask(window, cx);
        let name = workspace.unwrap_or_else(|| ui::file_name(&root).to_string());
        cx.spawn_in(window, async move |this, cx| {
            let answer = answer.await;
            this.update_in(cx, |this, window, cx| {
                if let Some(s) = &this.persist.state {
                    let _ = s.set(&asked, "1");
                }
                match answer {
                    Some(1) => this.trust_roots(&roots, &name, window, cx),
                    Some(2) => {
                        if let Some(parent) = &parent {
                            this.trust_with_subfolders(parent, window, cx);
                        }
                    }
                    _ => {}
                }
            })
            .ok();
        })
        .detach();
    }
}

impl Workbench {
    #[cfg(test)]
    pub fn restricted_for_test(&self, cx: &App) -> bool {
        self.restricted_root(cx).is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_parent_folder_is_never_a_disk_root() {
        assert_eq!(parent_folder("/home/a/proj"), Some("/home/a".into()));
        assert_eq!(parent_folder("/home/a/proj/"), Some("/home/a".into()));
        assert_eq!(parent_folder("/home"), None, "/ would trust the whole disk");
        assert_eq!(parent_folder("/"), None);
        assert_eq!(parent_folder(r"C:\Users\a\proj"), Some(r"C:\Users\a".into()));
        assert_eq!(parent_folder(r"C:\Users"), None);
        assert_eq!(parent_folder("proj"), None);
    }

    #[test]
    fn records_split_into_host_and_folder() {
        assert_eq!(split_record("trust::/srv/app"), Some(("", "/srv/app")));
        assert_eq!(split_record("trust:prod:/srv/app"), Some(("prod", "/srv/app")));
        assert_eq!(split_record(r"trust::C:\proj"), Some(("", r"C:\proj")), "a drive letter's colon stays in the folder");
        assert_eq!(split_record("trust:docker:web:/srv/app"), Some(("docker:web", "/srv/app")), "a container's name has a colon");
        assert_eq!(split_record(r"trust:wsl:Ubuntu:C:\x"), Some(("wsl:Ubuntu", r"C:\x")));
        assert_eq!(split_record("trust_migrated_v1"), None);
    }

    #[test]
    fn the_picker_offers_what_fits_the_folders_state() {
        let records = vec![("trust::/a/b".to_string(), "1".to_string()), ("trust:prod:/srv".to_string(), TREE.to_string())];
        let untrusted = trust_rows(Some(("/work/app", false, false)), &records);
        let choices: Vec<_> = untrusted.iter().map(|r| r.2.clone()).collect();
        assert_eq!(
            choices,
            [
                TrustChoice::TrustFolder("/work/app".into()),
                TrustChoice::TrustWithSubfolders("/work".into()),
                TrustChoice::Forget("trust::/a/b".into()),
                TrustChoice::Forget("trust:prod:/srv".into()),
            ]
        );
        assert!(untrusted[3].1.contains("ssh://prod") && untrusted[3].1.contains("/srv") && untrusted[3].1.contains("subfolders"));
        assert_eq!(untrusted[3].0, "srv", "the name leads and the path is in the detail");
        let own = trust_rows(Some(("/work/app", true, true)), &[]);
        assert_eq!(own.iter().map(|r| r.2.clone()).collect::<Vec<_>>(), [TrustChoice::Forget(String::new())]);
        assert!(trust_rows(Some(("/work/app", true, false)), &[]).is_empty(), "trusted through a parent: only the parent's record can go");
        assert!(trust_rows(None, &[]).is_empty());
    }
}
