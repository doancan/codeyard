//! Every question the workbench asks, as data: title, level, buttons in order, which one Escape answers (Cancel, Not
//! Now, Keep…, always the button that changes nothing) and which one Return answers. These run without a window,
//! an SSH configuration, Docker, a registry or the network; `a11y_confirm_tests.rs` asks the ones a test can reach in
//! a real workbench and checks what Cancel and the confirming answer leave behind.

use gpui::{PromptButton, PromptLevel};

use crate::question::Question;
use crate::questions::*;

/// The shape every question must have: its buttons, the Cancel button's place, Return's button, and the Cancel button
/// marked as such for the platform (so Escape answers it in every language).
fn check(q: &Question, level: PromptLevel, title: &str, buttons: &[&str], cancel: usize, default: Option<usize>) {
    assert_eq!(q.level, level, "{title}");
    assert_eq!(q.title, title);
    assert_eq!(q.buttons, buttons, "{title}");
    assert_eq!(q.cancel, Some(cancel), "{title}: the button that changes nothing");
    assert_eq!(q.default_button(), default, "{title}: Return");
    let answers = q.answers();
    for (i, a) in answers.iter().enumerate() {
        assert_eq!(matches!(a, PromptButton::Cancel(_)), i == cancel, "{title}: {:?}", a.label());
        assert_eq!(a.label().as_ref(), buttons[i]);
    }
    assert!(!q.acts(Some(cancel)) && !q.acts(None), "{title}: Cancel and a closed dialog change nothing");
    assert!(q.detail.as_ref().is_some_and(|d| !d.is_empty()), "{title}: says what happens");
}

#[test]
fn file_and_extension_questions() {
    let _en = crate::i18n::LangGuard::set("en");
    check(&file_batch("Apply?", "Destination: x", "Copy", false), PromptLevel::Warning, "Apply?", &["Copy", "Cancel"], 1, Some(0));
    assert_eq!(file_batch("Delete?", "gone", "Delete", true).level, PromptLevel::Critical);
    check(&uninstall_extension("you.night"), PromptLevel::Warning, "Uninstall you.night?", &["Uninstall", "Cancel"], 1, Some(0));
    check(&move_into("a.txt", "src"), PromptLevel::Info, "Move “a.txt” into “src”?", &["Move", "Cancel"], 1, Some(0));
    check(
        &delete_entry("Move “a” to the Trash?", "You can restore it.", "Move to Trash", true),
        PromptLevel::Warning,
        "Move “a” to the Trash?",
        &["Move to Trash", "Cancel"],
        1,
        Some(0),
    );
    assert_eq!(delete_entry("Delete?", "x", "Delete", false).level, PromptLevel::Critical);
    let q = delete_permanently("a.txt", "the volume has no Trash");
    check(&q, PromptLevel::Warning, "Delete “a.txt” permanently?", &["Delete Permanently", "Cancel"], 1, Some(0));
    assert!(q.detail.unwrap().contains("the volume has no Trash"));
    check(&forget_recent("proj", "host:/p", "gone"), PromptLevel::Warning, "“proj” can't be opened", &["Remove from Recent", "Keep"], 1, Some(0));
    check(&open_link("https://example.com"), PromptLevel::Info, "Open this link in your browser?", &["Open", "Cancel"], 1, Some(0));
}

#[test]
fn remote_questions_need_no_ssh_config_or_docker() {
    let _en = crate::i18n::LangGuard::set("en");
    check(&host_added("box"), PromptLevel::Info, "Added “box”", &["Connect", "Not Now"], 1, Some(0));
    check(&remove_host("box"), PromptLevel::Warning, "Remove “box” from ~/.ssh/config?", &["Remove", "Cancel"], 1, Some(0));
    check(&key_setup("box"), PromptLevel::Info, "Set up key login for “box”?", &["Set Up", "Cancel"], 1, Some(0));
    let q = container_setup("Docker");
    check(&q, PromptLevel::Info, "Container setup required", &["Open Setup Instructions", "Cancel"], 1, Some(0));
    assert!(q.detail.unwrap().starts_with("Docker is not installed"));
    check(&start_container("web-1"), PromptLevel::Info, "Start this container?", &["Start and Connect", "Cancel"], 1, Some(0));
    check(&replace_on_transfer(&["a.txt".into()]), PromptLevel::Warning, "“a.txt” already exists. Replace it?", &["Replace", "Cancel"], 1, Some(0));
    assert_eq!(replace_on_transfer(&["a".into(), "b".into()]).title, "2 items already exist. Replace them?");
    check(&cancel_transfer("Upload a.txt"), PromptLevel::Warning, "Cancel “Upload a.txt”?", &["Cancel Transfer", "Keep Going"], 1, Some(0));
    check(&schema_download("schemas.example"), PromptLevel::Warning, "Download a schema from schemas.example?", &["Download", "Not Now"], 1, Some(0));
    check(&compare_copy("a.txt", "Right", "1 file goes"), PromptLevel::Warning, "Copy “a.txt” to Right?", &["Copy", "Cancel"], 1, Some(0));
    let q = compare_delete("old", "/r/old", "Right", 2);
    check(&q, PromptLevel::Critical, "Delete “old” from Right?", &["Delete", "Cancel"], 1, Some(0));
    assert!(q.detail.unwrap().contains("2 more entries"));
}

#[test]
fn account_and_permission_questions_need_no_network() {
    let _en = crate::i18n::LangGuard::set("en");
    check(&github_account("Pulls", "me", "who and what"), PromptLevel::Warning, "Let Pulls act as your GitHub account @me?", &["Allow", "Cancel"], 1, Some(0));
    check(&oauth_sign_in("Pulls", "GitLab", "who"), PromptLevel::Warning, "Let Pulls sign in to your GitLab account?", &["Sign In…", "Cancel"], 1, Some(0));
    check(
        &add_instance("Pulls", "git.example", "who"),
        PromptLevel::Warning,
        "Let Pulls use your account on git.example?",
        &["Add Instance", "Cancel"],
        1,
        Some(0),
    );
    check(&first_use("Let Probe read a.txt?", "Probe asks"), PromptLevel::Warning, "Let Probe read a.txt?", &["Allow for this session", "Deny"], 1, Some(0));
    check(&send_request("api.http", &["goes to another host".into()]), PromptLevel::Warning, "Send this request?", &["Send", "Cancel"], 1, Some(0));
    check(&trust(None, "proj"), PromptLevel::Warning, "Do you trust the authors of this folder?", &["Trust Folder and Continue", "Cancel"], 1, Some(0));
    check(
        &trust(Some("ws"), "proj"),
        PromptLevel::Warning,
        "Do you trust the authors of this workspace?",
        &["Trust Workspace and Continue", "Cancel"],
        1,
        Some(0),
    );
}

#[test]
fn source_control_and_editor_questions() {
    let _en = crate::i18n::LangGuard::set("en");
    // Discard: Cancel first, so Return never discards; Escape answers it too.
    let q = discard(&["a.txt"], false, Some("/repo"));
    check(&q, PromptLevel::Warning, "Discard changes in “a.txt”?", &["Cancel", "Discard"], 0, None);
    assert!(q.acts(Some(1)));
    assert!(q.detail.unwrap().starts_with("Repository: /repo"));
    assert_eq!(discard(&["a", "b"], true, None).title, "Discard changes in 2 files?");
    check(&hard_reset("1a2b3c4", None), PromptLevel::Warning, "Hard reset to 1a2b3c4?", &["Reset", "Cancel"], 1, Some(0));
    check(&git("Delete branch “x”?", "It is gone.", "Delete Branch"), PromptLevel::Warning, "Delete branch “x”?", &["Delete Branch", "Cancel"], 1, Some(0));
    check(
        &structural_replace("Replace 3 matches?", "Some files", &["Skip Those Files", "Replace Anyway"]),
        PromptLevel::Warning,
        "Replace 3 matches?",
        &["Skip Those Files", "Replace Anyway", "Cancel"],
        2,
        Some(0),
    );
    check(&rust_setup(false, "+ x"), PromptLevel::Info, "Apply the Rust setup to settings.jsonc?", &["Apply", "Cancel"], 1, Some(0));
    check(&rust_setup(true, "- x"), PromptLevel::Info, "Reset the Rust setup in settings.jsonc?", &["Reset", "Cancel"], 1, Some(0));
    check(
        &save_before_closing("a.txt"),
        PromptLevel::Warning,
        "Do you want to save the changes you made to a.txt?",
        &["Save", "Don't Save", "Cancel"],
        2,
        Some(0),
    );
    check(
        &close_terminals(&["top".into()]),
        PromptLevel::Warning,
        "“top” is still running in this terminal. Close it anyway?",
        &["Close", "Cancel"],
        1,
        Some(0),
    );
    assert_eq!(close_terminals(&[]).title, "Close the terminal?");
}

#[test]
fn application_questions() {
    let _en = crate::i18n::LangGuard::set("en");
    check(&leave_unsaved(false), PromptLevel::Warning, "You have unsaved changes", &["Quit without saving", "Cancel"], 1, Some(0));
    check(&leave_unsaved(true), PromptLevel::Warning, "You have unsaved changes", &["Restart without saving", "Cancel"], 1, Some(0));
    check(&close_window_unsaved(), PromptLevel::Warning, "You have unsaved changes", &["Close without saving", "Cancel"], 1, Some(0));
    check(&quit_while_installing(), PromptLevel::Warning, "Codeyard is still installing an update", &["Quit Anyway", "Keep Waiting"], 1, Some(0));
    check(&disk_access(), PromptLevel::Info, "Allow file access", &["Open System Settings", "Show Codeyard in Finder", "Not Now"], 2, Some(0));
}

/// In Turkish the Cancel button's label is not "Cancel", and is still the one Escape answers.
#[test]
fn the_cancel_button_is_marked_in_every_language() {
    let _tr = crate::i18n::LangGuard::set("tr");
    let q = uninstall_extension("you.night");
    assert_ne!(q.buttons[1], "Cancel");
    assert!(matches!(q.answers()[1], PromptButton::Cancel(_)));
    assert!(matches!(remove_host("box").answers()[1], PromptButton::Cancel(_)));
}

/// Asking remembers what had the keyboard: if something takes it while the question is open, Cancel gives it back;
/// the confirming answer leaves it to the action.
#[gpui::test]
fn cancel_gives_the_keyboard_back_to_what_had_it(cx: &mut gpui::TestAppContext) {
    let _en = crate::i18n::LangGuard::set("en");
    let dir = crate::qa_workbench_tests::fresh("question-focus");
    std::fs::write(dir.join("a.txt"), "a\n").unwrap();
    let (wb, cx) = crate::qa_workbench_tests::setup(cx);
    crate::qa_workbench_tests::open(&wb, &dir.join("a.txt"), cx);
    let editor = cx.update(|window, cx| window.focused(cx)).expect("the editor has the keyboard");
    for (answer, back) in [("Cancel", true), ("Uninstall", false)] {
        let task = wb.update_in(cx, |_, window, cx| uninstall_extension("you.night").ask(window, cx));
        wb.update_in(cx, |wb, window, cx| wb.focus_tree(window, cx));
        cx.run_until_parked();
        assert_ne!(cx.update(|window, cx| window.focused(cx)).as_ref(), Some(&editor));
        cx.simulate_prompt_answer(answer);
        cx.run_until_parked();
        let got = cx.update(|_, cx| cx.background_executor().block(task));
        assert_eq!(got, Some(if back { 1 } else { 0 }));
        assert_eq!(cx.update(|window, cx| window.focused(cx)).as_ref() == Some(&editor), back, "{answer}");
        wb.update_in(cx, |_, window, _| window.focus(&editor));
    }
}
