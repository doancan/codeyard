//! Every question the workbench asks with the system dialog, built as plain data (`crate::question::Question`): the
//! title, the text, the buttons in order, which one changes nothing (Escape) and which one Return answers. The call
//! sites gather what the question needs (names, counts, hosts) and ask it; these functions decide what it says, so each
//! is tested without a window, an SSH configuration, Docker, a registry or the network (`questions_tests.rs`).

use gpui::PromptLevel;

use crate::i18n::{t, tf, tn};
use crate::question::Question;

/// "Delete “a.txt” permanently?" and the like: the verb confirms, Cancel changes nothing.
fn confirm(level: PromptLevel, title: String, detail: String, verb: &str) -> Question {
    Question::new(level, title).detail(detail).button(verb).cancel(t("Cancel"))
}

/// A batch of files or folders (copy, move, delete): a delete that cannot be undone is critical.
pub(crate) fn file_batch(title: &str, detail: &str, verb: &str, destructive: bool) -> Question {
    confirm(if destructive { PromptLevel::Critical } else { PromptLevel::Warning }, title.into(), detail.into(), verb)
}

pub(crate) fn uninstall_extension(id: &str) -> Question {
    confirm(
        PromptLevel::Warning,
        tf("Uninstall {}?", &[&id]),
        t("Its contributions are removed from every window, with the data it stored.").into(),
        t("Uninstall"),
    )
}

/// After a host was saved to ~/.ssh/config: connect now, or not.
pub(crate) fn host_added(alias: &str) -> Question {
    Question::new(PromptLevel::Info, tf("Added “{}”", &[&alias]))
        .detail(t("Saved to ~/.ssh/config. Connecting uses your SSH keys and ssh-agent; a password prompt appears in this window if needed."))
        .button(t("Connect"))
        .cancel(t("Not Now"))
}

pub(crate) fn remove_host(alias: &str) -> Question {
    confirm(
        PromptLevel::Warning,
        tf("Remove “{}” from ~/.ssh/config?", &[&alias]),
        t("Only this host's entry is removed. The previous file is kept as config.codeyard-bak.").into(),
        t("Remove"),
    )
}

pub(crate) fn key_setup(alias: &str) -> Question {
    confirm(
        PromptLevel::Info,
        tf("Set up key login for “{}”?", &[&alias]),
        t("Your public key (~/.ssh/id_ed25519.pub; created if you have none) is added to the server's authorized_keys, so later connections need no password. You may be asked for the server's password once.").into(),
        t("Set Up"),
    )
}

/// Quitting while an update installs: waiting is the safe answer, and the one Escape gives.
pub(crate) fn quit_while_installing() -> Question {
    Question::new(PromptLevel::Warning, t("Codeyard is still installing an update"))
        .detail(t("Quitting now can leave the installation unfinished. It takes a few seconds."))
        .button(t("Quit Anyway"))
        .cancel(t("Keep Waiting"))
}

pub(crate) fn container_setup(name: &str) -> Question {
    confirm(
        PromptLevel::Info,
        t("Container setup required").into(),
        tf("{} is not installed. Open the setup instructions, then try again.", &[&name]),
        t("Open Setup Instructions"),
    )
}

pub(crate) fn start_container(name: &str) -> Question {
    confirm(PromptLevel::Info, t("Start this container?").into(), name.into(), t("Start and Connect"))
}

/// An extension's first use of a GitHub account (`detail`: who asks and what Codeyard does with it).
pub(crate) fn github_account(extension: &str, account: &str, detail: &str) -> Question {
    confirm(PromptLevel::Warning, tf("Let {0} act as your GitHub account {1}?", &[&extension, &format!("@{account}")]), detail.into(), t("Allow"))
}

/// An extension's first sign-in to an OAuth provider.
pub(crate) fn oauth_sign_in(extension: &str, provider: &str, detail: &str) -> Question {
    confirm(PromptLevel::Warning, tf("Let {0} sign in to your {1} account?", &[&extension, &provider]), detail.into(), t("Sign In…"))
}

/// An extension asking to use an account on another instance (a self-hosted server).
pub(crate) fn add_instance(extension: &str, host: &str, detail: &str) -> Question {
    confirm(PromptLevel::Warning, tf("Let {0} use your account on {1}?", &[&extension, &host]), detail.into(), t("Add Instance"))
}

/// Workspace Trust before running anything a folder or workspace defines.
pub(crate) fn trust(workspace: Option<&str>, folder: &str) -> Question {
    let (title, detail, button) = match workspace {
        Some(name) => (
            t("Do you trust the authors of this workspace?"),
            tf(
                "“{}” and its folders can run programs through their tasks, debug configurations and project settings. Only continue if you trust where this workspace came from.",
                &[&name],
            ),
            t("Trust Workspace and Continue"),
        ),
        None => (
            t("Do you trust the authors of this folder?"),
            tf(
                "“{}” can run programs through its tasks, debug configurations and project settings. Only continue if you trust where this folder came from.",
                &[&folder],
            ),
            t("Trust Folder and Continue"),
        ),
    };
    confirm(PromptLevel::Warning, title.into(), detail, button)
}

/// What a structural replace offers: everything, or (when some files would not parse) skipping those or replacing
/// anyway; the last button cancels. `choices` are the acting buttons' labels in order.
pub(crate) fn structural_replace(title: &str, detail: &str, choices: &[&str]) -> Question {
    let q = Question::new(PromptLevel::Warning, title).detail(detail);
    choices.iter().fold(q, |q, c| q.button(*c)).cancel(t("Cancel"))
}

pub(crate) fn open_link(url: &str) -> Question {
    confirm(PromptLevel::Info, t("Open this link in your browser?").into(), url.into(), t("Open"))
}

pub(crate) fn rust_setup(reset: bool, diff: &str) -> Question {
    let (title, button) =
        if reset { (t("Reset the Rust setup in settings.jsonc?"), t("Reset")) } else { (t("Apply the Rust setup to settings.jsonc?"), t("Apply")) };
    confirm(PromptLevel::Info, title.into(), diff.into(), button)
}

/// A request in a `.http` file that should not be sent without a look (`reasons`, one per line).
pub(crate) fn send_request(file: &str, reasons: &[String]) -> Question {
    let detail = format!("{}\n• {}\n\n{}", tf("{} contains a request that:", &[&file]), reasons.join("\n• "), t("Only send it if you trust this file."));
    confirm(PromptLevel::Warning, t("Send this request?").into(), detail, t("Send"))
}

/// macOS: Full Disk Access is missing. Two acting buttons; Not Now changes nothing.
pub(crate) fn disk_access() -> Question {
    let detail = format!(
        "{}\n\n{}\n\n{}",
        t("Without Full Disk Access, Desktop, Documents or external drives may look empty."),
        t("In Privacy & Security, press + and add Codeyard, then turn it on."),
        t("Already listed after an update? Remove it with − and add it again.")
    );
    Question::new(PromptLevel::Info, t("Allow file access"))
        .detail(detail)
        .button(t("Open System Settings"))
        .button(t("Show Codeyard in Finder"))
        .cancel(t("Not Now"))
}

/// Discarding changes: Cancel comes first, so Return (and Escape) never discard by accident.
pub(crate) fn discard(files: &[&str], deletes_untracked: bool, repository: Option<&str>) -> Question {
    let title = match files {
        [one] => tf("Discard changes in {}?", &[&format!("“{one}”")]),
        many => tf("Discard changes in {} files?", &[&many.len()]),
    };
    let mut detail = if deletes_untracked { t("Untracked files will be deleted. This can't be undone.") } else { t("This can't be undone.") }.to_string();
    if let Some(root) = repository {
        detail = format!("{}\n{detail}", tf("Repository: {}", &[&root]));
    }
    Question::new(PromptLevel::Warning, title).detail(detail).cancel(t("Cancel")).button(t("Discard"))
}

/// An extension's first use of a permission it asks for at run time (`title`, `detail` from `ext_prompts::dialog`).
pub(crate) fn first_use(title: &str, detail: &str) -> Question {
    Question::new(PromptLevel::Warning, title).detail(detail).button(t("Allow for this session")).cancel(t("Deny"))
}

pub(crate) fn hard_reset(short: &str, repository: Option<&str>) -> Question {
    let mut detail = t("Commits after it leave this branch and every uncommitted change is lost. This can't be undone.").to_string();
    if let Some(root) = repository {
        detail = format!("{}\n{detail}", tf("Repository: {}", &[&root]));
    }
    confirm(PromptLevel::Warning, tf("Hard reset to {}?", &[&short]), detail, t("Reset"))
}

/// Copying between the two sides of a folder comparison (`detail` says what goes where).
pub(crate) fn compare_copy(name: &str, to: &str, detail: &str) -> Question {
    confirm(PromptLevel::Warning, tf("Copy “{0}” to {1}?", &[&name, &to]), detail.into(), t("Copy"))
}

/// Deleting on one side of a folder comparison: for good, with what the comparison hides in it.
pub(crate) fn compare_delete(name: &str, full: &str, side: &str, hidden: usize) -> Question {
    let mut detail = tf("{0} on {1} is deleted for good, with everything in it. It is not moved to the Trash.", &[&full, &side]);
    if hidden > 0 {
        detail.push(' ');
        detail.push_str(&tn(
            "{} more entry that the comparison hides (ignored by .gitignore or the compare settings) goes too.",
            "{} more entries that the comparison hides (ignored by .gitignore or the compare settings) go too.",
            hidden,
            &[],
        ));
    }
    confirm(PromptLevel::Critical, tf("Delete “{0}” from {1}?", &[&name, &side]), detail, t("Delete"))
}

/// An upload or download would replace `existing` (names at the destination).
pub(crate) fn replace_on_transfer(existing: &[String]) -> Question {
    let title = match existing {
        [one] => tf("“{}” already exists. Replace it?", &[one]),
        many => tn("{} item already exists. Replace it?", "{} items already exist. Replace them?", many.len(), &[]),
    };
    confirm(PromptLevel::Warning, title, t("Files with the same name are overwritten; other files in the folder are kept.").into(), t("Replace"))
}

/// Cancelling a running transfer: going on is the safe answer.
pub(crate) fn cancel_transfer(label: &str) -> Question {
    Question::new(PromptLevel::Warning, tf("Cancel “{}”?", &[&label]))
        .detail(t("A file that is only partly transferred is removed, and so is a new folder. A folder you merge into keeps what was already copied."))
        .button(t("Cancel Transfer"))
        .cancel(t("Keep Going"))
}

/// A schema hosted somewhere not yet allowed (`http.confirmRequests`).
pub(crate) fn schema_download(host: &str) -> Question {
    Question::new(PromptLevel::Warning, tf("Download a schema from {}?", &[&host]))
        .detail(tf(
            "A file you opened names a schema that is hosted on {}. Downloading it sends a request to that server (your IP address and the schema's address). Only allow it if you trust the file and the host.",
            &[&host],
        ))
        .button(t("Download"))
        .cancel(t("Not Now"))
}

/// Closing terminals with programs still running in them (`busy`: what runs; empty when every close asks).
pub(crate) fn close_terminals(busy: &[String]) -> Question {
    let title = match busy {
        [] => t("Close the terminal?").to_string(),
        [one] => tf("“{}” is still running in this terminal. Close it anyway?", &[one]),
        many => tf("{} terminals have programs running. Close them anyway?", &[&many.len()]),
    };
    confirm(PromptLevel::Warning, title, t("Closing a terminal ends the programs running in it.").into(), t("Close"))
}

/// A recent folder that cannot be opened: remove it from the list, or keep it.
pub(crate) fn forget_recent(name: &str, place: &str, why: &str) -> Question {
    Question::new(PromptLevel::Warning, tf("“{}” can't be opened", &[&name]))
        .detail(format!("{place}\n{why}\n\n{}", t("Remove it from Recent?")))
        .button(t("Remove from Recent"))
        .cancel(t("Keep"))
}

/// Dropping an Explorer entry onto a folder.
pub(crate) fn move_into(name: &str, folder: &str) -> Question {
    confirm(
        PromptLevel::Info,
        tf("Move “{0}” into “{1}”?", &[&name, &folder]),
        t("An existing file with the same name is never overwritten.").into(),
        t("Move"),
    )
}

/// Deleting one Explorer entry: to the Trash (recoverable) or for good (critical).
pub(crate) fn delete_entry(title: &str, detail: &str, verb: &str, trash: bool) -> Question {
    confirm(if trash { PromptLevel::Warning } else { PromptLevel::Critical }, title.into(), detail.into(), verb)
}

/// The Trash refused an entry: delete it for good instead?
pub(crate) fn delete_permanently(name: &str, why: &str) -> Question {
    confirm(
        PromptLevel::Warning,
        tf("Delete “{}” permanently?", &[&name]),
        tf("It can't be moved to the Trash ({}). Deleting it permanently can't be undone.", &[&why]),
        t("Delete Permanently"),
    )
}

/// Quitting or restarting with unsaved changes.
pub(crate) fn leave_unsaved(restart: bool) -> Question {
    let (detail, leave) = if restart {
        (t("Restarting loses everything since the last save."), t("Restart without saving"))
    } else {
        (t("Quitting loses everything since the last save."), t("Quit without saving"))
    };
    confirm(PromptLevel::Warning, t("You have unsaved changes").into(), detail.into(), leave)
}

/// Closing this window with unsaved changes.
pub(crate) fn close_window_unsaved() -> Question {
    confirm(
        PromptLevel::Warning,
        t("You have unsaved changes").into(),
        t("Closing the window loses everything since the last save.").into(),
        t("Close without saving"),
    )
}

/// A git operation that needs a yes (title, text and verb already in the UI language).
pub(crate) fn git(title: &str, detail: &str, verb: &str) -> Question {
    confirm(PromptLevel::Warning, title.into(), detail.into(), verb)
}

/// Closing an editor's own window with unsaved changes: Save, Don't Save, or Cancel.
pub(crate) fn save_before_closing(name: &str) -> Question {
    Question::new(PromptLevel::Warning, tf("Do you want to save the changes you made to {}?", &[&name]))
        .detail(t("Your changes will be lost if you don't save them."))
        .button(t("Save"))
        .button(t("Don't Save"))
        .cancel(t("Cancel"))
}

#[cfg(test)]
#[path = "questions_tests.rs"]
mod tests;
