//! QA (integrations): exploratory end-to-end tests for git / SCM, language
//! servers, terminal and remote SSH. Each test encodes the behaviour a VS Code
//! user expects; a failing test is a product bug (see the QA report).
//! Everything lives in a throwaway directory; remote tests only talk to the
//! local test sshd fixture (`crate::test_sshd`).

use std::path::{Path, PathBuf};
use std::time::Duration;

use gpui::{Entity, Focusable, Modifiers, MouseButton, TestAppContext, VisualTestContext};

use crate::git::Mark;
use crate::settings::Settings;
use crate::ui::ActiveTheme;
use crate::workbench::Workbench;

// --- helpers -------------------------------------------------------------

fn base_dir() -> PathBuf {
    let base =
        std::env::var_os("QA_TMP").map(PathBuf::from).unwrap_or_else(|| std::env::temp_dir().join(format!("codeyard-qa-integration-{}", std::process::id())));
    std::fs::create_dir_all(&base).unwrap();
    std::fs::canonicalize(base).unwrap()
}

fn qa_dir(name: &str) -> PathBuf {
    let dir = base_dir().join(format!("{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

pub(crate) fn setup_with(cx: &mut TestAppContext, settings: Settings) -> (Entity<Workbench>, &mut VisualTestContext) {
    cx.update(|cx| {
        crate::bind_keys(cx);
        cx.set_global(ActiveTheme(theme::DARK));
        cx.set_global(settings);
    });
    let (wb, cx) = cx.add_window_view(|window, cx| Workbench::with_state(state::State::in_memory().ok(), None, window, cx));
    wb.update_in(cx, |wb, window, cx| window.focus(&wb.focus_handle(cx)));
    (wb, cx)
}

fn setup(cx: &mut TestAppContext) -> (Entity<Workbench>, &mut VisualTestContext) {
    setup_with(cx, Settings { lsp_enabled: false, ..Default::default() })
}

fn open(wb: &Entity<Workbench>, path: &Path, cx: &mut VisualTestContext) {
    let p = path.to_string_lossy().into_owned();
    wb.update_in(cx, |wb, window, cx| wb.open_path(&p, window, cx));
    cx.run_until_parked();
}

pub(crate) fn wait_secs(cx: &mut VisualTestContext, secs: u64, what: &str, mut done: impl FnMut(&mut VisualTestContext) -> bool) {
    let limit = Duration::from_secs(secs * crate::test_sshd::timeout_scale());
    let start = std::time::Instant::now();
    loop {
        cx.run_until_parked();
        if done(cx) {
            return;
        }
        assert!(start.elapsed() < limit, "timeout: {what}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn wait_for(cx: &mut VisualTestContext, what: &str, done: impl FnMut(&mut VisualTestContext) -> bool) {
    wait_secs(cx, 10, what, done);
}

/// Also advances the fake clock (debounced git refreshes, LSP restart timers).
pub(crate) fn wait_ticking(cx: &mut VisualTestContext, what: &str, mut done: impl FnMut(&mut VisualTestContext) -> bool) {
    wait_for(cx, what, |cx| {
        cx.executor().advance_clock(Duration::from_millis(300));
        done(cx)
    });
}

pub(crate) fn git_in(dir: &Path, args: &[&str]) -> String {
    let out = crate::test_git::git().args(["-c", "core.hooksPath=.git/hooks"]).args(args).current_dir(dir).output().unwrap();
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// A repo with one commit, local identity (the app's own `git commit` uses it).
pub(crate) fn repo(name: &str) -> PathBuf {
    let dir = qa_dir(name);
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::write(dir.join("README.md"), "# proje\n").unwrap();
    std::fs::write(dir.join("src/lib.rs"), (1..=10).map(|i| format!("// satır {i}\n")).collect::<String>()).unwrap();
    git_in(&dir, &["init", "-q"]);
    git_in(&dir, &["config", "user.name", "QA"]);
    git_in(&dir, &["config", "user.email", "qa@example.com"]);
    git_in(&dir, &["config", "core.hooksPath", ".git/hooks"]);
    git_in(&dir, &["add", "-A"]);
    git_in(&dir, &["commit", "-q", "-m", "ilk"]);
    dir
}

/// Repo + bare origin + a second clone ("someone else").
pub(crate) fn repo_with_remote(name: &str) -> (PathBuf, PathBuf, PathBuf) {
    let dir = repo(name);
    let bare = dir.with_extension("origin.git");
    let other = dir.with_extension("other");
    let _ = std::fs::remove_dir_all(&bare);
    let _ = std::fs::remove_dir_all(&other);
    git_in(dir.parent().unwrap(), &["init", "-q", "--bare", bare.to_str().unwrap()]);
    git_in(&dir, &["remote", "add", "origin", bare.to_str().unwrap()]);
    git_in(&dir, &["push", "-q", "-u", "origin", "main"]);
    git_in(dir.parent().unwrap(), &["clone", "-q", bare.to_str().unwrap(), other.to_str().unwrap()]);
    git_in(&other, &["config", "user.name", "Other"]);
    git_in(&other, &["config", "user.email", "o@example.com"]);
    (dir, bare, other)
}

pub(crate) fn changes(wb: &Entity<Workbench>, cx: &mut VisualTestContext) -> Option<Vec<(String, Option<Mark>, Option<Mark>)>> {
    wb.read_with(cx, |wb, _| wb.git_status().map(|g| g.changes.iter().map(|c| (c.path.clone(), c.staged, c.unstaged)).collect()))
}

pub(crate) fn notice(wb: &Entity<Workbench>, cx: &mut VisualTestContext) -> String {
    wb.read_with(cx, |wb, _| wb.notice_text()).unwrap_or_default()
}

fn run(wb: &Entity<Workbench>, cx: &mut VisualTestContext, label: &str) {
    wb.update_in(cx, |wb, window, cx| wb.run_command_for_test(label, window, cx));
}

// --- Git / Source Control -------------------------------------------------

/// VS Code keeps the commit message when `git commit` fails (hook, identity,
/// nothing staged…); the user fixes the problem and presses ↵ again.
#[gpui::test]
fn qa_scm_commit_failure_keeps_message(cx: &mut TestAppContext) {
    let dir = repo("qa-commitfail");
    let hook = dir.join(".git/hooks/pre-commit");
    std::fs::create_dir_all(hook.parent().unwrap()).unwrap();
    std::fs::write(&hook, "#!/bin/sh\necho 'lint failed' >&2\nexit 1\n").unwrap();
    #[cfg(unix)]
    crate::test_files::make_executable(&hook);
    std::fs::write(dir.join("README.md"), "# proje\nyeni\n").unwrap();

    let (wb, cx) = setup(cx);
    open(&wb, &dir, cx);
    wait_ticking(cx, "status", |cx| changes(&wb, cx).is_some_and(|c| c.len() == 1));
    let scm = wb.read_with(cx, |wb, _| wb.scm_panel());
    scm.update(cx, |s, cx| s.set_message("Uzun ve özenle yazılmış commit mesajı", cx));
    scm.update(cx, |s, cx| s.commit(cx));
    wait_for(cx, "commit attempt", |cx| !scm.read_with(cx, |s, _| s.is_busy()) && scm.read_with(cx, |s, _| s.result_text()).is_some());
    assert!(scm.read_with(cx, |s, _| s.result_text()).unwrap().contains("lint failed"));

    // Hook fixed; commit again without retyping.
    std::fs::remove_file(&hook).unwrap();
    scm.update(cx, |s, cx| s.commit(cx));
    wait_for(cx, "second commit", |cx| !scm.read_with(cx, |s, _| s.is_busy()));
    let result = scm.read_with(cx, |s, _| s.result_text());
    assert!(result.as_deref().is_some_and(|r| r.starts_with("Committed")), "message was thrown away after the failed commit: {result:?}");
}

/// Commit messages have a subject and a body. Pasting a multi-line message into
/// the SCM box must keep the line breaks (VS Code's box is multi-line).
#[gpui::test]
fn qa_scm_multiline_commit_message(cx: &mut TestAppContext) {
    let dir = repo("qa-multiline");
    std::fs::write(dir.join("README.md"), "# proje\nyeni\n").unwrap();
    let (wb, cx) = setup(cx);
    open(&wb, &dir, cx);
    wait_ticking(cx, "status", |cx| changes(&wb, cx).is_some_and(|c| c.len() == 1));
    cx.simulate_keystrokes("ctrl-shift-g");
    cx.write_to_clipboard(gpui::ClipboardItem::new_string("Başlık satırı\n\nGövde: neden değişti.".into()));
    cx.simulate_keystrokes("cmd-v");
    cx.simulate_keystrokes("enter");
    let scm = wb.read_with(cx, |wb, _| wb.scm_panel());
    wait_ticking(cx, "commit", |cx| !scm.read_with(cx, |s, _| s.is_busy()) && changes(&wb, cx).is_some_and(|c| c.is_empty()));
    let body = git_in(&dir, &["log", "-1", "--format=%B"]);
    assert_eq!(body.trim_end(), "Başlık satırı\n\nGövde: neden değişti.");
}

/// A file edited in place outside the editor (terminal `>>`, a formatter, a
/// code generator) must show up in Source Control without user action.
#[gpui::test]
fn qa_git_status_follows_in_place_edit_of_unopened_file(cx: &mut TestAppContext) {
    let dir = repo("qa-inplace");
    let (wb, cx) = setup(cx);
    open(&wb, &dir, cx);
    wait_ticking(cx, "clean status", |cx| changes(&wb, cx).is_some_and(|c| c.is_empty()));
    {
        use std::io::Write;
        let mut f = std::fs::OpenOptions::new().append(true).open(dir.join("README.md")).unwrap();
        f.write_all(b"terminalden eklendi\n").unwrap();
    }
    wait_ticking(cx, "README shows as modified", |cx| changes(&wb, cx).is_some_and(|c| c.iter().any(|(p, ..)| p == "README.md")));
}

/// Same for a repo whose `.git` is a file (git worktree / submodule): a commit
/// made in the terminal must clear the Source Control list.
#[gpui::test]
fn qa_git_worktree_external_commit_refreshes(cx: &mut TestAppContext) {
    let main = repo("qa-wtmain");
    let wt = main.with_extension("wt");
    let _ = std::fs::remove_dir_all(&wt);
    git_in(&main, &["worktree", "add", "-q", "-b", "wtb", wt.to_str().unwrap()]);
    std::fs::write(wt.join("README.md"), "# proje\nworktree\n").unwrap();
    let (wb, cx) = setup(cx);
    open(&wb, &wt, cx);
    wait_ticking(cx, "status", |cx| changes(&wb, cx).is_some_and(|c| c.len() == 1));
    git_in(&wt, &["commit", "-qam", "terminalden"]);
    wait_ticking(cx, "external commit picked up", |cx| changes(&wb, cx).is_some_and(|c| c.is_empty()));
}

/// Pull that ends in a merge conflict: the user must be told there is a
/// conflict (VS Code: "Merge conflicts: resolve them before committing").
#[gpui::test]
fn qa_git_pull_conflict_is_reported(cx: &mut TestAppContext) {
    let (dir, _bare, other) = repo_with_remote("qa-pullconflict");
    std::fs::write(other.join("README.md"), "# uzak başlık\n").unwrap();
    git_in(&other, &["commit", "-qam", "uzak"]);
    git_in(&other, &["push", "-q"]);
    std::fs::write(dir.join("README.md"), "# yerel başlık\n").unwrap();
    git_in(&dir, &["commit", "-qam", "yerel"]);

    let (wb, cx) = setup(cx);
    open(&wb, &dir, cx);
    wait_ticking(cx, "status", |cx| changes(&wb, cx).is_some());
    run(&wb, cx, "Git: Pull");
    wait_ticking(cx, "pull done", |cx| !notice(&wb, cx).ends_with('…') && !notice(&wb, cx).is_empty());
    wait_ticking(cx, "conflict in status", |cx| changes(&wb, cx).is_some_and(|c| c.iter().any(|(p, _, u)| p == "README.md" && *u == Some(Mark::Conflict))));
    let n = notice(&wb, cx);
    assert!(n.to_lowercase().contains("conflict"), "notice after a conflicting pull: {n:?}");
}

/// Rejected push (remote has new commits): the message must say why
/// (VS Code: "Can't push refs to remote. Try running Pull first").
#[gpui::test]
fn qa_git_rejected_push_message_says_why(cx: &mut TestAppContext) {
    let (dir, _bare, other) = repo_with_remote("qa-pushrejected");
    std::fs::write(other.join("README.md"), "# uzak\n").unwrap();
    git_in(&other, &["commit", "-qam", "uzak"]);
    git_in(&other, &["push", "-q"]);
    std::fs::write(dir.join("src/lib.rs"), "// yerel\n").unwrap();
    git_in(&dir, &["commit", "-qam", "yerel"]);

    let (wb, cx) = setup(cx);
    open(&wb, &dir, cx);
    wait_ticking(cx, "status", |cx| changes(&wb, cx).is_some());
    run(&wb, cx, "Git: Push");
    wait_ticking(cx, "push done", |cx| notice(&wb, cx).starts_with("Push failed") || notice(&wb, cx) == "Pushed");
    let n = notice(&wb, cx);
    assert!(n.contains("rejected") || n.contains("fetch first") || n.to_lowercase().contains("pull"), "notice after a rejected push: {n:?}");
}

/// Unstaging a staged rename must put both sides back (VS Code: the file shows
/// as deleted + untracked in Changes, nothing left in Staged Changes).
#[gpui::test]
fn qa_scm_unstage_rename_unstages_both_sides(cx: &mut TestAppContext) {
    let dir = repo("qa-unstagerename");
    git_in(&dir, &["mv", "README.md", "BENIOKU.md"]);
    let (wb, cx) = setup(cx);
    open(&wb, &dir, cx);
    wait_ticking(cx, "rename staged", |cx| changes(&wb, cx).is_some_and(|c| c.iter().any(|(p, s, _)| p == "BENIOKU.md" && *s == Some(Mark::Renamed))));
    let scm = wb.read_with(cx, |wb, _| wb.scm_panel());
    scm.update(cx, |s, cx| s.unstage(vec!["BENIOKU.md".into()], cx));
    wait_ticking(cx, "unstaged", |cx| !scm.read_with(cx, |s, _| s.is_busy()));
    wb.update(cx, |wb, cx| wb.refresh_git(0, cx));
    wait_ticking(cx, "refresh", |cx| changes(&wb, cx).is_some_and(|c| c.iter().any(|(p, _, u)| p == "BENIOKU.md" && *u == Some(Mark::Untracked))));
    let staged: Vec<_> = changes(&wb, cx).unwrap().into_iter().filter(|(_, s, _)| s.is_some()).collect();
    assert!(staged.is_empty(), "still staged after unstaging the rename: {staged:?}");
}

/// Diff of a staged pure rename: left side is the old path's content, so there
/// is nothing to show (VS Code shows the rename with an empty diff).
#[gpui::test]
fn qa_diff_of_staged_rename_uses_old_path(cx: &mut TestAppContext) {
    let dir = repo("qa-diffrename");
    git_in(&dir, &["mv", "src/lib.rs", "src/kutuphane.rs"]);
    let (wb, cx) = setup(cx);
    open(&wb, &dir, cx);
    wait_ticking(cx, "rename staged", |cx| changes(&wb, cx).is_some_and(|c| c.iter().any(|(p, s, _)| p == "src/kutuphane.rs" && *s == Some(Mark::Renamed))));
    wb.update_in(cx, |wb, window, cx| wb.open_diff("src/kutuphane.rs".into(), true, window, cx));
    wait_for(cx, "diff", |cx| wb.read_with(cx, |wb, _| wb.diff_view().is_some()));
    let view = wb.read_with(cx, |wb, _| wb.diff_view()).unwrap();
    assert_eq!(view.read_with(cx, |v, _| v.summary()), (0, 0, 0), "pure rename shown as a whole new file (+added, −removed, hunks)");
}

/// Opening the folder at a sub-directory of a repo: status, root and branch come
/// from the enclosing repo (VS Code finds the parent repository).
#[gpui::test]
fn qa_git_repo_opened_at_subfolder(cx: &mut TestAppContext) {
    let dir = repo("qa-subfolder");
    std::fs::write(dir.join("src/lib.rs"), "// değişti\n").unwrap();
    std::fs::write(dir.join("README.md"), "# dışarıda\n").unwrap();
    let (wb, cx) = setup(cx);
    open(&wb, &dir.join("src"), cx);
    wait_ticking(cx, "status", |cx| changes(&wb, cx).is_some_and(|c| c.len() == 2));
    let st = wb.read_with(cx, |wb, _| wb.git_status()).unwrap();
    assert_eq!(st.root, dir.to_string_lossy());
    assert_eq!(st.branch.as_deref(), Some("main"));
}

// --- Language servers ----------------------------------------------------

/// A fake server that crashes with a signal shortly after a file is opened.
fn crashing_server(dir: &Path) -> PathBuf {
    let script = dir.join("crashing-ls.py");
    std::fs::write(
        &script,
        r#"#!/usr/bin/env python3
import sys, os, json, signal, time
with open(sys.argv[1], 'a') as f:
    f.write('start\n')
def read():
    headers = {}
    while True:
        line = sys.stdin.buffer.readline()
        if not line:
            sys.exit(0)
        line = line.strip()
        if not line:
            break
        k, v = line.split(b':', 1)
        headers[k.strip().lower()] = v.strip()
    n = int(headers[b'content-length'])
    return json.loads(sys.stdin.buffer.read(n))
def send(m):
    b = json.dumps(m).encode()
    sys.stdout.buffer.write(b'Content-Length: %d\r\n\r\n' % len(b) + b)
    sys.stdout.buffer.flush()
while True:
    m = read()
    if m.get('method') == 'initialize':
        send({'jsonrpc': '2.0', 'id': m['id'], 'result': {'capabilities': {'textDocumentSync': 1}}})
    elif m.get('method') == 'textDocument/didOpen':
        time.sleep(0.2)
        # SIGKILL (like an OOM kill): SIGSEGV takes the same path, but macOS shows a
        # "Python quit unexpectedly" crash dialog every time.
        os.kill(os.getpid(), signal.SIGKILL)
"#,
    )
    .unwrap();
    #[cfg(unix)]
    crate::test_files::make_executable(&script);
    script
}

/// A server that keeps crashing (segfault, abort, OOM kill = killed by a
/// signal) must be restarted only a few times, then the user is told
/// (VS Code: "The X server crashed 5 times in the last 3 minutes. It will not be restarted.").
#[gpui::test]
fn qa_lsp_crash_loop_by_signal_is_bounded(cx: &mut TestAppContext) {
    if std::process::Command::new("python3").arg("-V").output().is_err() {
        crate::test_sshd::skip_missing("python3");
        return;
    }
    let dir = qa_dir("qa-lspcrash");
    let script = crashing_server(&dir);
    let starts = dir.join("starts.log");
    std::fs::write(dir.join("main.c"), "int main(void) { return 0; }\n").unwrap();
    let settings = Settings {
        lsp_servers: vec![("c".into(), Some((script.to_string_lossy().into_owned(), vec![starts.to_string_lossy().into_owned()]).into()))],
        ..Settings::default()
    };
    let (wb, cx) = setup_with(cx, settings);
    open(&wb, &dir.join("main.c"), cx);
    let count = || std::fs::read_to_string(&starts).map(|s| s.lines().count()).unwrap_or(0);
    // Let it crash and restart for a while (restart timer is 1 s on the fake clock).
    let start = std::time::Instant::now();
    while start.elapsed() < Duration::from_secs(15) && count() < 8 && !notice(&wb, cx).contains("keeps crashing") {
        cx.executor().advance_clock(Duration::from_millis(1500));
        cx.run_until_parked();
        std::thread::sleep(Duration::from_millis(100));
    }
    let n = count();
    assert!(n <= 4, "server was started {n} times (signal crashes are never counted); notice: {:?}", notice(&wb, cx));
    assert!(notice(&wb, cx).contains("keeps crashing"), "{:?}", notice(&wb, cx));
}

/// Configured server binary does not exist: the user gets a clear message.
#[gpui::test]
fn qa_lsp_missing_server_is_reported(cx: &mut TestAppContext) {
    let dir = qa_dir("qa-lspmissing");
    std::fs::write(dir.join("main.c"), "int main(void) { return 0; }\n").unwrap();
    let settings = Settings { lsp_servers: vec![("c".into(), Some(("/nonexistent/qa-ls".into(), vec![]).into()))], ..Settings::default() };
    let (wb, cx) = setup_with(cx, settings);
    open(&wb, &dir.join("main.c"), cx);
    wait_ticking(cx, "missing server notice", |cx| notice(&wb, cx).contains("qa-ls"));
    let n = notice(&wb, cx);
    assert!(n.contains("isn't available"), "{n:?}");
}

/// New languages follow the same rules as the old ones: an opt-in key (toml) starts only when configured and reports
/// a missing binary once; JSON has no server by default; a default turned off with `null` starts nothing.
#[gpui::test]
fn qa_lsp_new_language_keys_follow_settings(cx: &mut TestAppContext) {
    let dir = qa_dir("qa-lspnewlangs");
    std::fs::write(dir.join("a.json"), "{}\n").unwrap();
    std::fs::write(dir.join("a.sh"), "echo hi\n").unwrap();
    std::fs::write(dir.join("Cargo.toml"), "[package]\n").unwrap();
    let settings = Settings {
        lsp_servers: vec![("bash".into(), None), ("toml".into(), Some(("/nonexistent/qa-taplo".into(), vec!["lsp".into()]).into()))],
        ..Settings::default()
    };
    let (wb, cx) = setup_with(cx, settings);
    open(&wb, &dir.join("a.json"), cx);
    open(&wb, &dir.join("a.sh"), cx);
    cx.run_until_parked();
    assert_eq!(notice(&wb, cx), "", "no default server for JSON and a disabled bash: nothing starts, nothing is reported");
    open(&wb, &dir.join("Cargo.toml"), cx);
    wait_ticking(cx, "missing toml server notice", |cx| notice(&wb, cx).contains("qa-taplo"));
    assert!(notice(&wb, cx).contains("isn't available"), "{:?}", notice(&wb, cx));
}

fn has_clangd() -> bool {
    std::process::Command::new("clangd").arg("--version").output().is_ok_and(|o| o.status.success())
}

/// Accepting a function completion inserts a snippet; its placeholders must be
/// selected so typing replaces them (VS Code: `topla(|int a|, int b)`, Tab → next).
#[gpui::test]
fn qa_lsp_snippet_placeholder_is_selected(cx: &mut TestAppContext) {
    if !has_clangd() {
        crate::test_sshd::skip_missing("clangd");
        return;
    }
    let dir = qa_dir("qa-snippet");
    std::fs::write(dir.join("main.c"), "int topla(int a, int b) { return a + b; }\nint main(void) {\n    \n}\n").unwrap();
    let (wb, cx) = setup_with(cx, Settings::default());
    open(&wb, &dir.join("main.c"), cx);
    let ed = wb.read_with(cx, |wb, _| wb.active_editor_entity()).unwrap();
    wait_for(cx, "server", |cx| ed.read_with(cx, |e, _| e.lsp_client().is_some()));
    cx.simulate_keystrokes("down down end");
    cx.simulate_input("topl");
    cx.simulate_keystrokes("ctrl-space");
    wait_secs(cx, 20, "completions", |cx| {
        cx.executor().advance_clock(Duration::from_millis(100));
        ed.read_with(cx, |e, _| e.completion_labels()).is_some_and(|l| l.iter().any(|t| t.contains("topla")))
    });
    let ix = ed.read_with(cx, |e, _| e.completion_labels()).unwrap().iter().position(|t| t.contains("topla")).unwrap();
    for _ in 0..ix {
        cx.simulate_keystrokes("down");
    }
    cx.simulate_keystrokes("enter");
    let line = |cx: &mut VisualTestContext| ed.read_with(cx, |e, _| e.text()).lines().nth(2).unwrap().to_string();
    let inserted = line(cx);
    assert!(inserted.contains("topla("), "{inserted:?}");
    cx.simulate_input("5");
    let after = line(cx);
    assert!(!after.contains("5int"), "typing after accepting the snippet did not replace the placeholder: {after:?}");
    // Tab: the second placeholder is selected; typing replaces it.
    cx.simulate_keystrokes("tab");
    cx.simulate_input("6");
    let done = line(cx);
    assert!(done.contains("topla(5, 6)"), "{done:?}");
}

/// Format on save with clangd (editor.formatOnSave).
#[gpui::test]
fn qa_lsp_format_on_save(cx: &mut TestAppContext) {
    if !has_clangd() {
        crate::test_sshd::skip_missing("clangd");
        return;
    }
    let dir = qa_dir("qa-fmtsave");
    std::fs::write(dir.join("main.c"), "int main(void){return 0;}\n").unwrap();
    let settings = Settings { format_on_save: true, ..Settings::default() };
    let (wb, cx) = setup_with(cx, settings);
    open(&wb, &dir.join("main.c"), cx);
    let ed = wb.read_with(cx, |wb, _| wb.active_editor_entity()).unwrap();
    wait_for(cx, "server", |cx| ed.read_with(cx, |e, _| e.lsp_client().is_some()));
    cx.simulate_keystrokes("cmd-end");
    cx.simulate_input("\n");
    cx.simulate_keystrokes("cmd-s");
    wait_secs(cx, 20, "formatted on disk", |cx| {
        cx.executor().advance_clock(Duration::from_millis(100));
        std::fs::read_to_string(dir.join("main.c")).unwrap().contains("int main(void) {")
    });
}

/// `editor.codeActionsOnSave`: the edit of the `source.*` action requested from the server on save
/// goes into the text written to disk; unrequested kinds (quickfix) are not applied.
#[gpui::test]
fn qa_lsp_code_actions_on_save(cx: &mut TestAppContext) {
    if std::process::Command::new("python3").arg("-V").output().is_err() {
        crate::test_sshd::skip_missing("python3");
        return;
    }
    let dir = qa_dir("qa-actionsave");
    let script = dir.join("organize-ls.py");
    std::fs::write(
        &script,
        r#"#!/usr/bin/env python3
import sys, json
def read():
    headers = {}
    while True:
        line = sys.stdin.buffer.readline()
        if not line:
            sys.exit(0)
        line = line.strip()
        if not line:
            break
        k, v = line.split(b':', 1)
        headers[k.strip().lower()] = v.strip()
    return json.loads(sys.stdin.buffer.read(int(headers[b'content-length'])))
def send(m):
    b = json.dumps(m).encode()
    sys.stdout.buffer.write(b'Content-Length: %d\r\n\r\n' % len(b) + b)
    sys.stdout.buffer.flush()
def edit(uri, text):
    return {'changes': {uri: [{'range': {'start': {'line': 0, 'character': 0}, 'end': {'line': 0, 'character': 0}}, 'newText': text}]}}
while True:
    m = read()
    if m.get('method') == 'initialize':
        send({'jsonrpc': '2.0', 'id': m['id'], 'result': {'capabilities': {'textDocumentSync': 1, 'codeActionProvider': True}}})
    elif m.get('method') == 'textDocument/codeAction':
        uri = m['params']['textDocument']['uri']
        only = m['params']['context'].get('only', [])
        acts = [{'title': 'Quick fix', 'kind': 'quickfix', 'edit': edit(uri, '// quickfix\n')}]
        if 'source.organizeImports' in only:
            acts.append({'title': 'Organize Imports', 'kind': 'source.organizeImports', 'edit': edit(uri, '// organized\n')})
        send({'jsonrpc': '2.0', 'id': m['id'], 'result': acts})
    elif m.get('method') == 'shutdown':
        send({'jsonrpc': '2.0', 'id': m['id'], 'result': None})
"#,
    )
    .unwrap();
    #[cfg(unix)]
    crate::test_files::make_executable(&script);
    std::fs::write(dir.join("main.c"), "int a;\n").unwrap();
    let settings = Settings {
        code_actions_on_save: vec!["source.organizeImports".into()],
        lsp_servers: vec![("c".into(), Some((script.to_string_lossy().into_owned(), Vec::new()).into()))],
        ..Settings::default()
    };
    let (wb, cx) = setup_with(cx, settings);
    open(&wb, &dir.join("main.c"), cx);
    let ed = wb.read_with(cx, |wb, _| wb.active_editor_entity()).unwrap();
    wait_for(cx, "server", |cx| ed.read_with(cx, |e, _| e.lsp_client().is_some()));
    cx.simulate_keystrokes("cmd-end");
    cx.simulate_input("int b;\n");
    cx.simulate_keystrokes("cmd-s");
    wait_for(cx, "organized on disk", |_| std::fs::read_to_string(dir.join("main.c")).unwrap().starts_with("// organized\n"));
    let disk = std::fs::read_to_string(dir.join("main.c")).unwrap();
    assert!(!disk.contains("quickfix"), "unwanted kind must not be applied: {disk:?}");
    assert!(disk.contains("int a;") && disk.contains("int b;"), "typed text must be kept: {disk:?}");
}

/// A task whose output has compiler errors fills the Problems panel (like a VS Code problem
/// matcher) and underlines the line once the file is open; a new run clears the old ones.
#[gpui::test]
fn qa_task_output_becomes_problems(cx: &mut TestAppContext) {
    let dir = qa_dir("qa-taskprob");
    std::fs::create_dir_all(dir.join(".codeyard")).unwrap();
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::write(dir.join("src/a.c"), "int main(void) { return x; }\n").unwrap();
    std::fs::write(dir.join(".codeyard/tasks.jsonc"), r#"[ { "label": "build", "command": "printf 'src/a.c:1:27: error: x undeclared\\nnoise line\\n'" } ]"#)
        .unwrap();
    // Language server off: a real `clangd` on this machine would send its own diagnostics and skew the count.
    let (wb, cx) = setup_with(cx, Settings { lsp_enabled: false, ..Settings::default() });
    open(&wb, &dir, cx);
    wb.update(cx, |wb, _| wb.set_shell("/bin/sh"));
    run(&wb, cx, "Tasks: Run Build Task");
    wait_ticking(cx, "problem from task output", |cx| wb.read_with(cx, |wb, _| wb.problems()).iter().any(|(p, n)| p.ends_with("/src/a.c") && *n == 1));
    assert_eq!(wb.read_with(cx, |wb, _| wb.problems().len()), 1, "a message line is not a problem");
    // Opening the file later still gives the editor its squiggle.
    open(&wb, &dir.join("src/a.c"), cx);
    let ed = wb.read_with(cx, |wb, _| wb.active_editor_entity()).unwrap();
    wait_ticking(cx, "squiggle on the opened file", |cx| ed.read_with(cx, |e, _| !e.diagnostics().is_empty()));
    let d = ed.read_with(cx, |e, _| e.diagnostics().to_vec());
    assert_eq!((d.len(), d[0].start, d[0].severity), (1, (0, 26), 1), "{d:?}");
}

/// A server that formats whatever range it is asked about and answers `onTypeFormatting` for `;`.
pub(crate) fn formatting_server(dir: &Path) -> PathBuf {
    let script = dir.join("fmt-ls.py");
    std::fs::write(
        &script,
        r#"#!/usr/bin/env python3
import sys, json
def read():
    headers = {}
    while True:
        line = sys.stdin.buffer.readline()
        if not line:
            sys.exit(0)
        line = line.strip()
        if not line:
            break
        k, v = line.split(b':', 1)
        headers[k.strip().lower()] = v.strip()
    return json.loads(sys.stdin.buffer.read(int(headers[b'content-length'])))
def send(m):
    b = json.dumps(m).encode()
    sys.stdout.buffer.write(b'Content-Length: %d\r\n\r\n' % len(b) + b)
    sys.stdout.buffer.flush()
while True:
    m = read()
    meth = m.get('method')
    if meth == 'initialize':
        send({'jsonrpc': '2.0', 'id': m['id'], 'result': {'capabilities': {'textDocumentSync': 1,
            'documentRangeFormattingProvider': True,
            'documentOnTypeFormattingProvider': {'firstTriggerCharacter': ';'}}}})
    elif meth == 'textDocument/rangeFormatting':
        send({'jsonrpc': '2.0', 'id': m['id'], 'result': [{'range': m['params']['range'], 'newText': 'PASTED_FORMATTED'}]})
    elif meth == 'textDocument/onTypeFormatting':
        send({'jsonrpc': '2.0', 'id': m['id'], 'result': [{'range': {'start': {'line': 0, 'character': 0}, 'end': {'line': 0, 'character': 0}}, 'newText': '/*typed*/'}]})
    elif meth == 'shutdown':
        send({'jsonrpc': '2.0', 'id': m['id'], 'result': None})
"#,
    )
    .unwrap();
    #[cfg(unix)]
    crate::test_files::make_executable(&script);
    script
}

pub(crate) fn c_server(script: &Path) -> Vec<crate::settings::ServerOverride> {
    vec![("c".into(), Some((script.to_string_lossy().into_owned(), Vec::new()).into()))]
}

/// `editor.formatOnPaste` / `editor.formatOnType` are off by default: nothing is reformatted.
#[gpui::test]
fn qa_lsp_format_on_paste_is_off_by_default(cx: &mut TestAppContext) {
    if std::process::Command::new("python3").arg("-V").output().is_err() {
        crate::test_sshd::skip_missing("python3");
        return;
    }
    let dir = qa_dir("qa-fmtoff");
    let script = formatting_server(&dir);
    std::fs::write(dir.join("off.c"), "").unwrap();
    let (wb, cx) = setup_with(cx, Settings { lsp_servers: c_server(&script), ..Settings::default() });
    open(&wb, &dir.join("off.c"), cx);
    let ed = wb.read_with(cx, |wb, _| wb.active_editor_entity()).unwrap();
    wait_for(cx, "server", |cx| ed.read_with(cx, |e, _| e.lsp_client().is_some()));
    cx.write_to_clipboard(gpui::ClipboardItem::new_string("int  x;".into()));
    cx.simulate_keystrokes("cmd-v");
    for _ in 0..10 {
        cx.run_until_parked();
        std::thread::sleep(Duration::from_millis(30));
    }
    assert_eq!(ed.read_with(cx, |e, _| e.text()), "int  x;");
}

/// Enabled: the pasted range goes to `rangeFormatting`, `;` to `onTypeFormatting`.
#[gpui::test]
fn qa_lsp_format_on_paste_and_type(cx: &mut TestAppContext) {
    if std::process::Command::new("python3").arg("-V").output().is_err() {
        crate::test_sshd::skip_missing("python3");
        return;
    }
    let dir = qa_dir("qa-fmton");
    let script = formatting_server(&dir);
    std::fs::write(dir.join("main.c"), "").unwrap();
    let settings = Settings { format_on_paste: true, format_on_type: true, lsp_servers: c_server(&script), ..Settings::default() };
    let (wb, cx) = setup_with(cx, settings);
    open(&wb, &dir.join("main.c"), cx);
    let ed = wb.read_with(cx, |wb, _| wb.active_editor_entity()).unwrap();
    wait_for(cx, "server", |cx| ed.read_with(cx, |e, _| e.lsp_client().is_some()));
    cx.write_to_clipboard(gpui::ClipboardItem::new_string("int  x".into()));
    cx.simulate_keystrokes("cmd-v");
    wait_for(cx, "pasted range formatted", |cx| ed.read_with(cx, |e, _| e.text()) == "PASTED_FORMATTED");
    cx.simulate_input(";");
    wait_for(cx, "typed trigger formatted", |cx| ed.read_with(cx, |e, _| e.text()).starts_with("/*typed*/"));
}

/// Real scenario: a cargo project with one passing and one failing test. "Run Test at Cursor"
/// runs only the test under the cursor and its panic lands in Problems at the right line.
#[gpui::test]
fn qa_run_test_at_cursor_reports_the_failing_assert(cx: &mut TestAppContext) {
    if std::process::Command::new("cargo").arg("--version").output().is_err() {
        return;
    }
    let dir = qa_dir("qa-runtest");
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::write(dir.join("Cargo.toml"), "[package]\nname = \"demo\"\nversion = \"0.1.0\"\nedition = \"2021\"\n").unwrap();
    let src = "pub fn two() -> i32 { 2 }\n\n#[cfg(test)]\nmod tests {\n    use super::*;\n\n    #[test]\n    fn passes() {\n        assert_eq!(two(), 2);\n    }\n\n    #[test]\n    fn fails() {\n        assert_eq!(two(), 3);\n    }\n}\n";
    std::fs::write(dir.join("src/lib.rs"), src).unwrap();
    let (wb, cx) = setup(cx);
    open(&wb, &dir, cx);
    wb.update(cx, |wb, _| wb.set_shell("/bin/sh"));
    let lib = dir.join("src/lib.rs").to_string_lossy().into_owned();
    wb.update_in(cx, |wb, window, cx| wb.open_file_in_root(lib.clone(), window, cx));
    cx.run_until_parked();
    let ed = wb.read_with(cx, |wb, _| wb.active_editor_entity()).unwrap();
    // Line 14 (1-based): `assert_eq!(two(), 3);`, inside the `fails` test.
    ed.update_in(cx, |e, window, cx| e.go_to_line(14, window, cx));
    run(&wb, cx, "Test: Run Test at Cursor");
    let started = std::time::Instant::now();
    wait_secs(cx, 120, "failing assert shows up in Problems", |cx| {
        cx.executor().advance_clock(Duration::from_millis(300));
        let _ = started;
        wb.read_with(cx, |wb, _| wb.problems()).iter().any(|(p, n)| p.ends_with("/src/lib.rs") && *n == 1)
    });
    // Only `fails` ran (`passes` was filtered out) and the location is the right line.
    let d = ed.read_with(cx, |e, _| e.diagnostics().to_vec());
    assert_eq!(d.len(), 1, "{d:?}");
    assert_eq!(d[0].start.0, 13, "panic line should be the assert_eq! line: {d:?}");
    assert!(d[0].message.starts_with("test panicked:"), "{d:?}");
}

/// The Testing view on a real cargo project: the tests are found, "Run All Tests" runs them, and each test shows its
/// outcome; the file with a failure opens and shows the failure's first line under the failed test.
#[gpui::test]
fn qa_testing_view_finds_runs_and_reports_tests(cx: &mut TestAppContext) {
    if std::process::Command::new("cargo").arg("--version").output().is_err() {
        return;
    }
    let dir = qa_dir("qa-testing");
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::write(dir.join("Cargo.toml"), "[package]\nname = \"demo\"\nversion = \"0.1.0\"\nedition = \"2021\"\n").unwrap();
    let src = "pub fn two() -> i32 { 2 }\n\n#[cfg(test)]\nmod tests {\n    use super::*;\n\n    #[test]\n    fn passes() {\n        assert_eq!(two(), 2);\n    }\n\n    #[test]\n    fn fails() {\n        assert!(two() == 3, \"two is not three\");\n    }\n}\n";
    std::fs::write(dir.join("src/lib.rs"), src).unwrap();
    let (wb, cx) = setup(cx);
    open(&wb, &dir, cx);
    run(&wb, cx, "View: Testing");
    wait_secs(cx, 30, "tests found", |cx| wb.read_with(cx, |wb, _| !wb.testing_rows_for_test().is_empty()));
    assert_eq!(wb.read_with(cx, |wb, _| wb.testing_rows_for_test()), ["file:src/lib.rs:NotRun"], "files start closed");
    run(&wb, cx, "Test: Run All Tests");
    wait_secs(cx, 180, "the run ends", |cx| {
        cx.executor().advance_clock(Duration::from_millis(300));
        wb.read_with(cx, |wb, _| !wb.testing_busy_for_test())
    });
    assert_eq!(
        wb.read_with(cx, |wb, _| wb.testing_rows_for_test()),
        ["file:src/lib.rs:Failed", "test:passes:Passed", "test:fails:Failed", "message:two is not three"]
    );
}

/// Stopping a run: the runner is killed at once and the tests go back to what they showed before.
#[gpui::test]
fn qa_testing_view_stop_ends_the_run(cx: &mut TestAppContext) {
    if std::process::Command::new("cargo").arg("--version").output().is_err() {
        return;
    }
    let dir = qa_dir("qa-testing-stop");
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::write(dir.join("Cargo.toml"), "[package]\nname = \"slow\"\nversion = \"0.1.0\"\nedition = \"2021\"\n").unwrap();
    let src = "#[cfg(test)]\nmod tests {\n    #[test]\n    fn waits() {\n        std::thread::sleep(std::time::Duration::from_secs(600));\n    }\n}\n";
    std::fs::write(dir.join("src/lib.rs"), src).unwrap();
    let (wb, cx) = setup(cx);
    open(&wb, &dir, cx);
    run(&wb, cx, "View: Testing");
    wait_secs(cx, 30, "tests found", |cx| wb.read_with(cx, |wb, _| !wb.testing_rows_for_test().is_empty()));
    run(&wb, cx, "Test: Run All Tests");
    assert_eq!(wb.read_with(cx, |wb, _| wb.testing_rows_for_test()), ["file:src/lib.rs:Running"]);
    wait_secs(cx, 60, "the runner started", |cx| wb.read_with(cx, |wb, _| wb.testing_process_for_test()));
    run(&wb, cx, "Test: Stop Running Tests");
    assert!(!wb.read_with(cx, |wb, _| wb.testing_busy_for_test()));
    assert_eq!(wb.read_with(cx, |wb, _| wb.testing_rows_for_test()), ["file:src/lib.rs:NotRun"]);
}

/// Debugger end to end with a scripted adapter: breakpoint → F5 → stops on that line with the
/// stack and variables → step → a breakpoint added while paused reaches the adapter → continue
/// stops there → continue ends the session and clears the yellow line.
#[gpui::test]
fn qa_debug_session_breakpoints_steps_and_end(cx: &mut TestAppContext) {
    if std::process::Command::new("python3").arg("-V").output().is_err() {
        crate::test_sshd::skip_missing("python3");
        return;
    }
    let dir = qa_dir("qa-debug");
    let log = dir.join("dap.log");
    let script = crate::dap::tests::write_fake_adapter(&dir);
    std::fs::write(dir.join("main.c"), (1..=12).map(|i| format!("int line{i};\n")).collect::<String>()).unwrap();
    std::fs::create_dir_all(dir.join(".vscode")).unwrap();
    std::fs::write(
        dir.join(".vscode/launch.json"),
        format!(
            r#"{{ "configurations": [ {{ "name": "Fake", "type": "custom", "request": "launch", "program": "/bin/true",
                "source": "${{workspaceFolder}}/main.c", "debugAdapter": {{ "command": "{}", "args": ["{}"] }} }} ] }}"#,
            script.display(),
            log.display()
        ),
    )
    .unwrap();
    let (wb, cx) = setup(cx);
    open(&wb, &dir, cx);
    let main = dir.join("main.c").to_string_lossy().into_owned();
    wb.update_in(cx, |wb, window, cx| wb.open_file_in_root(main.clone(), window, cx));
    cx.run_until_parked();
    let ed = wb.read_with(cx, |wb, _| wb.active_editor_entity()).unwrap();
    ed.update(cx, |e, cx| e.toggle_breakpoint_at(4, cx));
    assert_eq!(ed.read_with(cx, |e, _| e.breakpoint_lines()), [4]);

    run(&wb, cx, "Debug: Start Debugging");
    let snap = |cx: &mut VisualTestContext| wb.read_with(cx, |wb, _| wb.debug_snapshot());
    wait_for(cx, "stopped on the breakpoint", |cx| snap(cx).is_some_and(|s| s.0 == "stopped:breakpoint" && s.1.len() == 2 && s.2.is_some()));
    let (_, frames, location, console) = snap(cx).unwrap();
    assert_eq!(frames, ["main", "caller"]);
    assert_eq!(location, Some((main.clone(), 4)), "stopped location is the breakpoint line");
    assert!(console.iter().any(|l| l.contains("hello from the program")), "{console:?}");
    wait_for(cx, "yellow line in the editor", |cx| ed.read_with(cx, |e, _| e.debug_line()) == Some(4));
    assert_eq!(wb.read_with(cx, |wb, _| wb.debug_variables()), [("x".to_string(), "50".to_string()), ("point".to_string(), "{x, y}".to_string())]);

    // Step: to line 6 (0-based 5).
    run(&wb, cx, "Debug: Step Over");
    wait_for(cx, "stepped", |cx| snap(cx).is_some_and(|s| s.0 == "stopped:step" && s.2 == Some((main.clone(), 5))));
    wait_for(cx, "yellow line moved", |cx| ed.read_with(cx, |e, _| e.debug_line()) == Some(5));

    // A breakpoint added while paused (line 9) goes to the adapter; continuing stops there.
    ed.update(cx, |e, cx| e.toggle_breakpoint_at(8, cx));
    wait_for(cx, "setBreakpoints with both lines", |_| {
        std::fs::read_to_string(&log).unwrap_or_default().lines().any(|l| l.starts_with("setBreakpoints") && l.contains("[5, 9]"))
    });
    run(&wb, cx, "Debug: Continue");
    wait_for(cx, "stopped on the second breakpoint", |cx| snap(cx).is_some_and(|s| s.0 == "stopped:breakpoint" && s.2 == Some((main.clone(), 8))));

    run(&wb, cx, "Debug: Continue");
    wait_for(cx, "session ended", |cx| snap(cx).is_some_and(|s| s.0 == "ended"));
    wait_for(cx, "yellow line cleared", |cx| ed.read_with(cx, |e, _| e.debug_line()).is_none());
    let console = snap(cx).unwrap().3;
    assert!(console.iter().any(|l| l.contains("Process exited with code 0")), "{console:?}");
    // Breakpoints keep stopping after the session too.
    assert_eq!(ed.read_with(cx, |e, _| e.breakpoint_lines()), [4, 8]);
}

/// A failed launch reports the adapter's own message instead of hanging in "starting".
#[gpui::test]
fn qa_debug_launch_failure_is_reported(cx: &mut TestAppContext) {
    if std::process::Command::new("python3").arg("-V").output().is_err() {
        crate::test_sshd::skip_missing("python3");
        return;
    }
    let dir = qa_dir("qa-debugfail");
    let script = crate::dap::tests::write_fake_adapter(&dir);
    std::fs::create_dir_all(dir.join(".vscode")).unwrap();
    std::fs::write(
        dir.join(".vscode/launch.json"),
        format!(r#"{{ "configurations": [ {{ "name": "Bad", "type": "custom", "program": "/no/such", "fail": true, "debugAdapter": {{ "command": "{}", "args": ["{}"] }} }} ] }}"#, script.display(), dir.join("l.log").display()),
    )
    .unwrap();
    let (wb, cx) = setup(cx);
    open(&wb, &dir, cx);
    run(&wb, cx, "Debug: Start Debugging");
    wait_for(cx, "failure reported", |cx| notice(&wb, cx).contains("cannot run /no/such"));
    wait_for(cx, "session ended", |cx| wb.read_with(cx, |wb, _| wb.debug_snapshot()).is_some_and(|s| s.0 == "ended"));
}

/// A missing build tool fails the pre-launch step; its output reaches Problems.
#[gpui::test]
fn qa_debug_build_failure_fills_problems(cx: &mut TestAppContext) {
    let dir = qa_dir("qa-debugbuild");
    std::fs::create_dir_all(dir.join(".vscode")).unwrap();
    std::fs::write(dir.join(".vscode/tasks.json"), r#"{ "tasks": [ { "label": "build", "command": "printf 'src/a.c:2:3: error: nope\\n' >&2; exit 1" } ] }"#)
        .unwrap();
    std::fs::write(
        dir.join(".vscode/launch.json"),
        r#"{ "configurations": [ { "name": "Run", "type": "lldb", "program": "${workspaceFolder}/a.out", "preLaunchTask": "build" } ] }"#,
    )
    .unwrap();
    let (wb, cx) = setup(cx);
    open(&wb, &dir, cx);
    run(&wb, cx, "Debug: Start Debugging");
    wait_for(cx, "build failure", |cx| notice(&wb, cx).contains("Build failed"));
    wait_for(cx, "compiler error in Problems", |cx| wb.read_with(cx, |wb, _| wb.problems()).iter().any(|(p, n)| p.ends_with("/src/a.c") && *n == 1));
}

/// The real thing: clang -g + lldb-dap. A breakpoint inside a loop stops with the real local
/// variables, stepping moves, and continuing to the end finishes the session with the
/// program's own output in the debug console.
#[gpui::test]
fn qa_debug_real_lldb_dap_session(cx: &mut TestAppContext) {
    let have = |cmd: &str, args: &[&str]| std::process::Command::new(cmd).args(args).output().map(|o| o.status.success()).unwrap_or(false);
    if !(have("clang", &["--version"]) && (have("lldb-dap", &["--help"]) || have("xcrun", &["-f", "lldb-dap"]))) {
        crate::test_sshd::skip_missing("clang,lldb-dap,xcrun");
        return;
    }
    let dir = qa_dir("qa-reallldb");
    let src = "#include <stdio.h>\n\nint total = 0;\n\nint main(void) {\n    for (int i = 1; i <= 3; i++) {\n        int square = i * i;\n        total += square;\n        printf(\"square %d\\n\", square);\n    }\n    return 0;\n}\n";
    std::fs::write(dir.join("main.c"), src).unwrap();
    let build = std::process::Command::new("clang").args(["-g", "-O0", "-o", "demo", "main.c"]).current_dir(&dir).output().unwrap();
    assert!(build.status.success(), "{}", String::from_utf8_lossy(&build.stderr));
    std::fs::create_dir_all(dir.join(".vscode")).unwrap();
    std::fs::write(
        dir.join(".vscode/launch.json"),
        r#"{ "configurations": [ { "name": "Demo", "type": "lldb", "request": "launch", "program": "${workspaceFolder}/demo", "args": [], "cwd": "${workspaceFolder}" } ] }"#,
    )
    .unwrap();
    let (wb, cx) = setup(cx);
    open(&wb, &dir, cx);
    let main = std::fs::canonicalize(dir.join("main.c")).unwrap().to_string_lossy().into_owned();
    wb.update_in(cx, |wb, window, cx| wb.open_file_in_root(main.clone(), window, cx));
    cx.run_until_parked();
    let ed = wb.read_with(cx, |wb, _| wb.active_editor_entity()).unwrap();
    // Line 8 (0-based 7): `total += square;`
    ed.update(cx, |e, cx| e.toggle_breakpoint_at(7, cx));
    run(&wb, cx, "Debug: Start Debugging");
    let snap = |cx: &mut VisualTestContext| wb.read_with(cx, |wb, _| wb.debug_snapshot());
    wait_secs(cx, 60, "stopped in the loop", |cx| snap(cx).is_some_and(|s| s.0.starts_with("stopped") && s.2.as_ref().is_some_and(|(_, l)| *l == 7)));
    // Real locals: i = 1, square = 1.
    wait_secs(cx, 20, "locals loaded", |cx| wb.read_with(cx, |wb, _| wb.debug_variables()).iter().any(|(n, v)| n == "square" && v == "1"));
    let vars = wb.read_with(cx, |wb, _| wb.debug_variables());
    assert!(vars.iter().any(|(n, v)| n == "i" && v == "1"), "{vars:?}");
    // Step: line 9 (printf).
    run(&wb, cx, "Debug: Step Over");
    wait_secs(cx, 30, "stepped", |cx| snap(cx).is_some_and(|s| s.0.starts_with("stopped") && s.2.as_ref().is_some_and(|(_, l)| *l == 8)));
    // Continue: the loop stops at the same breakpoint a second time, i = 2, square = 4.
    run(&wb, cx, "Debug: Continue");
    wait_secs(cx, 30, "second iteration", |cx| wb.read_with(cx, |wb, _| wb.debug_variables()).iter().any(|(n, v)| n == "square" && v == "4"));
    // Remove the breakpoint, continue: the program exits.
    ed.update(cx, |e, cx| e.toggle_breakpoint_at(7, cx));
    run(&wb, cx, "Debug: Continue");
    wait_secs(cx, 30, "session ended", |cx| snap(cx).is_some_and(|s| s.0 == "ended"));
    let console = snap(cx).unwrap().3.join("\n");
    assert!(console.contains("exited with code 0") || console.contains("exited with status = 0"), "{console}");
    assert!(console.contains("square 9") || console.contains("square 4"), "program output is in the console: {console}");
}

/// A Rust project with no launch.json: F5 builds it, resolves its custom Cargo target directory,
/// and shows the real values of a Rust local at the breakpoint.
#[gpui::test]
fn qa_debug_real_cargo_project_without_config(cx: &mut TestAppContext) {
    let have = |cmd: &str, args: &[&str]| std::process::Command::new(cmd).args(args).output().map(|o| o.status.success()).unwrap_or(false);
    if !(have("cargo", &["--version"]) && (have("lldb-dap", &["--help"]) || have("xcrun", &["-f", "lldb-dap"]))) {
        crate::test_sshd::skip_missing("cargo,lldb-dap,xcrun");
        return;
    }
    let dir = qa_dir("qa-realrust");
    std::fs::create_dir_all(dir.join(".cargo")).unwrap();
    std::fs::write(dir.join(".cargo/config.toml"), "[build]\ntarget-dir = \"debug artifacts\"\n").unwrap();
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::write(dir.join("Cargo.toml"), "[package]\nname = \"demo\"\nversion = \"0.1.0\"\nedition = \"2021\"\n").unwrap();
    let src = "fn triple(n: u32) -> u32 {\n    let result = n * 3;\n    println!(\"computed {result}\");\n    result\n}\n\nfn main() {\n    let answer = triple(14);\n    println!(\"answer = {answer}\");\n}\n";
    std::fs::write(dir.join("src/main.rs"), src).unwrap();
    let (wb, cx) = setup(cx);
    open(&wb, &dir, cx);
    let main = std::fs::canonicalize(dir.join("src/main.rs")).unwrap().to_string_lossy().into_owned();
    wb.update_in(cx, |wb, window, cx| wb.open_file_in_root(main.clone(), window, cx));
    cx.run_until_parked();
    let ed = wb.read_with(cx, |wb, _| wb.active_editor_entity()).unwrap();
    // Line 2: `let result = n * 3;`
    ed.update(cx, |e, cx| e.toggle_breakpoint_at(1, cx));
    run(&wb, cx, "Debug: Start Debugging");
    let snap = |cx: &mut VisualTestContext| wb.read_with(cx, |wb, _| wb.debug_snapshot());
    wait_secs(cx, 180, "stopped in triple()", |cx| snap(cx).is_some_and(|s| s.0.starts_with("stopped") && s.2.as_ref().is_some_and(|(_, l)| *l == 1)));
    wait_secs(cx, 30, "argument n", |cx| wb.read_with(cx, |wb, _| wb.debug_variables()).iter().any(|(n, v)| n == "n" && v == "14"));
    let (_, frames, ..) = snap(cx).unwrap();
    assert!(frames.first().is_some_and(|f| f.contains("triple")), "{frames:?}");
    assert!(frames.iter().any(|f| f.contains("main")), "{frames:?}");
    // One step: `result` is computed.
    run(&wb, cx, "Debug: Step Over");
    wait_secs(cx, 30, "result after the step", |cx| wb.read_with(cx, |wb, _| wb.debug_variables()).iter().any(|(n, v)| n == "result" && v == "42"));
    run(&wb, cx, "Debug: Stop");
    wait_secs(cx, 30, "stopped by the user", |cx| snap(cx).is_some_and(|s| s.0 == "ended"));
}

/// Conditional breakpoints and logpoints reach the adapter: a breakpoint whose condition is false
/// is skipped, a logpoint prints instead of stopping.
#[gpui::test]
fn qa_debug_conditional_and_logpoints(cx: &mut TestAppContext) {
    if std::process::Command::new("python3").arg("-V").output().is_err() {
        crate::test_sshd::skip_missing("python3");
        return;
    }
    let dir = qa_dir("qa-debugcond");
    let log = dir.join("dap.log");
    let script = crate::dap::tests::write_fake_adapter(&dir);
    std::fs::write(dir.join("main.c"), (1..=14).map(|i| format!("int line{i};\n")).collect::<String>()).unwrap();
    std::fs::create_dir_all(dir.join(".vscode")).unwrap();
    std::fs::write(
        dir.join(".vscode/launch.json"),
        format!(
            r#"{{ "configurations": [ {{ "name": "Fake", "type": "custom", "program": "x", "source": "${{workspaceFolder}}/main.c",
                "debugAdapter": {{ "command": "{}", "args": ["{}"] }} }} ] }}"#,
            script.display(),
            log.display()
        ),
    )
    .unwrap();
    let (wb, cx) = setup(cx);
    open(&wb, &dir, cx);
    let main = dir.join("main.c").to_string_lossy().into_owned();
    wb.update_in(cx, |wb, window, cx| wb.open_file_in_root(main.clone(), window, cx));
    cx.run_until_parked();
    let ed = wb.read_with(cx, |wb, _| wb.active_editor_entity()).unwrap();
    use crate::editor::BpMeta;
    ed.update(cx, |e, cx| {
        e.set_breakpoints(
            &[
                (4, BpMeta::default()),
                (6, BpMeta { condition: Some("skip".into()), ..Default::default() }),
                (8, BpMeta { log: Some("value is {x}".into()), ..Default::default() }),
                (10, BpMeta::default()),
            ],
            cx,
        );
    });
    run(&wb, cx, "Debug: Start Debugging");
    let snap = |cx: &mut VisualTestContext| wb.read_with(cx, |wb, _| wb.debug_snapshot());
    wait_for(cx, "first stop", |cx| snap(cx).is_some_and(|s| s.0 == "stopped:breakpoint" && s.2 == Some((main.clone(), 4))));
    run(&wb, cx, "Debug: Continue");
    // The false-condition one (7) and the logpoint (9) are skipped; stops at line 11, the message goes to the console.
    wait_for(cx, "second stop skips the false condition and the logpoint", |cx| {
        snap(cx).is_some_and(|s| s.0 == "stopped:breakpoint" && s.2 == Some((main.clone(), 10)))
    });
    let console = snap(cx).unwrap().3;
    assert!(console.iter().any(|l| l == "LOG value is {x}"), "{console:?}");
    let sent = std::fs::read_to_string(&log).unwrap();
    assert!(sent.contains(r#""condition": "skip""#) && sent.contains(r#""logMessage": "value is {x}""#), "{sent}");
    // A metadata edit doesn't shift or lose it: add two lines, the condition stays on the next breakpoint.
    let metas = ed.read_with(cx, |e, _| e.breakpoints());
    assert_eq!(metas.len(), 4);
    assert_eq!(metas[1].1.condition.as_deref(), Some("skip"));
    assert_eq!(metas[2].1.log.as_deref(), Some("value is {x}"));
}

/// Real lldb-dap: a breakpoint with condition `i == 3` stops only on the third pass, and the
/// locals appear at the end of the lines that use them.
#[gpui::test]
fn qa_debug_real_conditional_breakpoint_and_inline_values(cx: &mut TestAppContext) {
    let have = |cmd: &str, args: &[&str]| std::process::Command::new(cmd).args(args).output().map(|o| o.status.success()).unwrap_or(false);
    if !(have("clang", &["--version"]) && (have("lldb-dap", &["--help"]) || have("xcrun", &["-f", "lldb-dap"]))) {
        crate::test_sshd::skip_missing("clang,lldb-dap,xcrun");
        return;
    }
    let dir = qa_dir("qa-realcond");
    let src = "#include <stdio.h>\n\nint main(void) {\n    int total = 0;\n    for (int i = 1; i <= 5; i++) {\n        int square = i * i;\n        total += square;\n    }\n    printf(\"%d\\n\", total);\n    return 0;\n}\n";
    std::fs::write(dir.join("main.c"), src).unwrap();
    let build = std::process::Command::new("clang").args(["-g", "-O0", "-o", "demo", "main.c"]).current_dir(&dir).output().unwrap();
    assert!(build.status.success());
    std::fs::create_dir_all(dir.join(".vscode")).unwrap();
    std::fs::write(dir.join(".vscode/launch.json"), r#"{ "configurations": [ { "name": "Demo", "type": "lldb", "program": "${workspaceFolder}/demo" } ] }"#)
        .unwrap();
    let (wb, cx) = setup(cx);
    open(&wb, &dir, cx);
    let main = std::fs::canonicalize(dir.join("main.c")).unwrap().to_string_lossy().into_owned();
    wb.update_in(cx, |wb, window, cx| wb.open_file_in_root(main.clone(), window, cx));
    cx.run_until_parked();
    let ed = wb.read_with(cx, |wb, _| wb.active_editor_entity()).unwrap();
    use crate::editor::BpMeta;
    // Line 7 (0-based 6): `total += square;` only when i == 3.
    ed.update(cx, |e, cx| e.set_breakpoints(&[(6, BpMeta { condition: Some("i == 3".into()), ..Default::default() })], cx));
    run(&wb, cx, "Debug: Start Debugging");
    wait_secs(cx, 60, "stopped with i == 3", |cx| wb.read_with(cx, |wb, _| wb.debug_variables()).iter().any(|(n, v)| n == "i" && v == "3"));
    let vars = wb.read_with(cx, |wb, _| wb.debug_variables());
    assert!(vars.iter().any(|(n, v)| n == "square" && v == "9"), "{vars:?}");
    // Watch expressions: evaluated immediately when added while stopped, again on later stops.
    wb.update(cx, |wb, cx| {
        wb.add_watch_for_test("i * 2", cx);
        wb.add_watch_for_test("total + square", cx);
        wb.add_watch_for_test("no_such_variable", cx);
    });
    wait_secs(cx, 20, "watch values", |cx| {
        wb.read_with(cx, |wb, _| wb.debug_watch_values()).len() == 3
            && wb.read_with(cx, |wb, _| wb.debug_watch_values()).iter().all(|(_, v)| !v.is_empty() && v != "…")
    });
    let watches = wb.read_with(cx, |wb, _| wb.debug_watch_values());
    assert_eq!(watches[0], ("i * 2".to_string(), "6".to_string()), "{watches:?}");
    assert_eq!(watches[1], ("total + square".to_string(), "14".to_string()), "{watches:?}");
    assert!(watches[2].1.starts_with('<'), "invalid expression shows as an error: {watches:?}");
    // Inline values: i and square on the `int square = i * i;` line, total and square on `total += square;`.
    wait_secs(cx, 20, "inline values", |cx| ed.read_with(cx, |e, _| e.debug_values()).contains_key(&6));
    let values = ed.read_with(cx, |e, _| e.debug_values());
    assert!(values[&6].contains("total = ") && values[&6].contains("square = 9"), "{values:?}");
    assert!(values[&5].contains("i = 3"), "{values:?}");
    // Value on hover: cursor on the stopped line (`total`); ⌘K ⌘I imitates the real mouse's 450 ms dwell.
    cx.simulate_keystrokes("alt-right cmd-k cmd-i"); // cursor to the end of the word `total`, then hover
    wait_secs(cx, 20, "debugger hover value", |cx| {
        cx.executor().advance_clock(Duration::from_millis(500));
        ed.read_with(cx, |e, _| e.hover_text().is_some_and(|t| t.contains("total = 5")))
    });
    run(&wb, cx, "Debug: Stop");
    wait_secs(cx, 30, "ended", |cx| wb.read_with(cx, |wb, _| wb.debug_snapshot()).is_some_and(|s| s.0 == "ended"));
    wait_secs(cx, 10, "inline values cleared", |cx| ed.read_with(cx, |e, _| e.debug_values()).is_empty());
}

/// Real debugpy: the open .py file is debugged without any configuration; a breakpoint in a
/// function stops with real locals, a step moves on, and the program output reaches the console.
#[gpui::test]
fn qa_debug_real_debugpy_session(cx: &mut TestAppContext) {
    if !std::process::Command::new("python3").args(["-c", "import debugpy"]).output().map(|o| o.status.success()).unwrap_or(false) {
        crate::test_sshd::skip_missing("python3,debugpy");
        return;
    }
    let dir = qa_dir("qa-realpy");
    let src = "def area(w, h):\n    result = w * h\n    print('area', result)\n    return result\n\n\nif __name__ == '__main__':\n    total = area(6, 7)\n    print('done', total)\n";
    std::fs::write(dir.join("app.py"), src).unwrap();
    let (wb, cx) = setup(cx);
    open(&wb, &dir, cx);
    let app = std::fs::canonicalize(dir.join("app.py")).unwrap().to_string_lossy().into_owned();
    wb.update_in(cx, |wb, window, cx| wb.open_file_in_root(app.clone(), window, cx));
    cx.run_until_parked();
    let ed = wb.read_with(cx, |wb, _| wb.active_editor_entity()).unwrap();
    // Line 3 (0-based 2): `print('area', result)`.
    ed.update(cx, |e, cx| e.toggle_breakpoint_at(2, cx));
    run(&wb, cx, "Debug: Start Debugging");
    let snap = |cx: &mut VisualTestContext| wb.read_with(cx, |wb, _| wb.debug_snapshot());
    wait_secs(cx, 60, "stopped in area()", |cx| snap(cx).is_some_and(|s| s.0.starts_with("stopped") && s.2.as_ref().is_some_and(|(_, l)| *l == 2)));
    wait_secs(cx, 20, "locals", |cx| wb.read_with(cx, |wb, _| wb.debug_variables()).iter().any(|(n, v)| n == "result" && v == "42"));
    wait_secs(cx, 20, "inline values", |cx| ed.read_with(cx, |e, _| e.debug_values()).get(&1).is_some_and(|v| v.contains("result = 42")));
    run(&wb, cx, "Debug: Step Over");
    wait_secs(cx, 30, "stepped to the return", |cx| snap(cx).is_some_and(|s| s.0.starts_with("stopped") && s.2.as_ref().is_some_and(|(_, l)| *l == 3)));
    ed.update(cx, |e, cx| e.toggle_breakpoint_at(2, cx));
    run(&wb, cx, "Debug: Continue");
    wait_secs(cx, 60, "ended", |cx| snap(cx).is_some_and(|s| s.0 == "ended"));
    let console = snap(cx).unwrap().3.join("\n");
    assert!(console.contains("area 42") && console.contains("done 42"), "{console}");
}

/// Real delve: a Go module is debugged with no configuration; the breakpoint stops inside a
/// function with real locals, and the program output reaches the console.
#[gpui::test]
fn qa_debug_real_delve_session(cx: &mut TestAppContext) {
    let have = |cmd: &str, args: &[&str]| std::process::Command::new(cmd).args(args).output().map(|o| o.status.success()).unwrap_or(false);
    if !(have("go", &["version"]) && have("dlv", &["version"])) {
        crate::test_sshd::skip_missing("go,dlv");
        return;
    }
    let dir = qa_dir("qa-realgo");
    std::fs::write(dir.join("go.mod"), "module demo\n\ngo 1.21\n").unwrap();
    let src = "package main\n\nimport \"fmt\"\n\nfunc cube(n int) int {\n\tresult := n * n * n\n\tfmt.Println(\"cube\", result)\n\treturn result\n}\n\nfunc main() {\n\tfmt.Println(\"total\", cube(3))\n}\n";
    std::fs::write(dir.join("main.go"), src).unwrap();
    let (wb, cx) = setup(cx);
    open(&wb, &dir, cx);
    let main = std::fs::canonicalize(dir.join("main.go")).unwrap().to_string_lossy().into_owned();
    wb.update_in(cx, |wb, window, cx| wb.open_file_in_root(main.clone(), window, cx));
    cx.run_until_parked();
    let ed = wb.read_with(cx, |wb, _| wb.active_editor_entity()).unwrap();
    // Line 7 (0-based 6): fmt.Println("cube", result)
    ed.update(cx, |e, cx| e.toggle_breakpoint_at(6, cx));
    run(&wb, cx, "Debug: Start Debugging");
    let snap = |cx: &mut VisualTestContext| wb.read_with(cx, |wb, _| wb.debug_snapshot());
    wait_secs(cx, 120, "stopped in cube()", |cx| snap(cx).is_some_and(|s| s.0.starts_with("stopped") && s.2.as_ref().is_some_and(|(_, l)| *l == 6)));
    wait_secs(cx, 20, "locals", |cx| wb.read_with(cx, |wb, _| wb.debug_variables()).iter().any(|(n, v)| n == "result" && v == "27"));
    ed.update(cx, |e, cx| e.toggle_breakpoint_at(6, cx));
    run(&wb, cx, "Debug: Continue");
    wait_secs(cx, 60, "ended", |cx| snap(cx).is_some_and(|s| s.0 == "ended"));
    let console = snap(cx).unwrap().3.join("\n");
    assert!(console.contains("cube 27") && console.contains("total 27"), "{console}");
}

/// Real js-debug (Microsoft's Node adapter): talks over TCP and runs the program in a child
/// session it asks us to open (`startDebugging`). A breakpoint stops with real locals and the
/// program output reaches the console. Skipped when js-debug is not installed.
#[gpui::test]
fn qa_debug_real_node_session(cx: &mut TestAppContext) {
    if std::process::Command::new("node").arg("--version").output().map(|o| !o.status.success()).unwrap_or(true) {
        crate::test_sshd::skip_missing("node");
        return;
    }
    if std::path::Path::new("/tmp/cy-js-debug/js-debug/src/dapDebugServer.js").is_file() && crate::debug_config::find_js_debug().is_none() {
        // SAFETY: only this test reads/writes it (the others don't look the adapter up through the env path).
        unsafe { std::env::set_var("CODEYARD_JS_DEBUG", "/tmp/cy-js-debug") };
    }
    if crate::debug_config::find_js_debug().is_none() {
        return;
    }
    eprintln!("js-debug: {:?}", crate::debug_config::find_js_debug());
    let dir = qa_dir("qa-realnode");
    let src = "function double(n) {\n  const result = n * 2;\n  console.log('double', result);\n  return result;\n}\n\nconst answer = double(21);\nconsole.log('done', answer);\n";
    std::fs::write(dir.join("app.js"), src).unwrap();
    let (wb, cx) = setup(cx);
    open(&wb, &dir, cx);
    let app = std::fs::canonicalize(dir.join("app.js")).unwrap().to_string_lossy().into_owned();
    wb.update_in(cx, |wb, window, cx| wb.open_file_in_root(app.clone(), window, cx));
    cx.run_until_parked();
    let ed = wb.read_with(cx, |wb, _| wb.active_editor_entity()).unwrap();
    // Line 3 (0-based 2): console.log('double', result)
    ed.update(cx, |e, cx| e.toggle_breakpoint_at(2, cx));
    run(&wb, cx, "Debug: Start Debugging");
    let snap = |cx: &mut VisualTestContext| wb.read_with(cx, |wb, _| wb.debug_snapshot());
    wait_secs(cx, 90, "stopped in double()", |cx| snap(cx).is_some_and(|s| s.0.starts_with("stopped") && s.2.as_ref().is_some_and(|(_, l)| *l == 2)));
    wait_secs(cx, 30, "locals", |cx| wb.read_with(cx, |wb, _| wb.debug_variables()).iter().any(|(n, v)| n == "result" && v == "42"));
    run(&wb, cx, "Debug: Step Over");
    wait_secs(cx, 30, "stepped", |cx| snap(cx).is_some_and(|s| s.0.starts_with("stopped") && s.2.as_ref().is_some_and(|(_, l)| *l == 3)));
    ed.update(cx, |e, cx| e.toggle_breakpoint_at(2, cx));
    run(&wb, cx, "Debug: Continue");
    wait_secs(cx, 60, "ended", |cx| snap(cx).is_some_and(|s| s.0 == "ended"));
    let console = snap(cx).unwrap().3.join("\n");
    assert!(console.contains("double 42") && console.contains("done 42"), "{console}");
}

/// Opening a .code-workspace file opens its first existing folder in this window.
#[gpui::test]
fn qa_workspace_file_opens_its_first_folder(cx: &mut TestAppContext) {
    let base = qa_dir("qa-workspace");
    std::fs::create_dir_all(base.join("api")).unwrap();
    std::fs::write(base.join("api/main.go"), "package main\n").unwrap();
    std::fs::write(base.join("mono.code-workspace"), r#"{ "folders": [ { "path": "missing" }, { "path": "api" } ] }"#).unwrap();
    let (wb, cx) = setup(cx);
    open(&wb, &base.join("mono.code-workspace"), cx);
    let want = std::fs::canonicalize(base.join("api")).unwrap();
    assert_eq!(wb.read_with(cx, |wb, cx| wb.root_for_test(cx)).map(|r| std::fs::canonicalize(r).unwrap()), Some(want));

    // File without a folder: the root doesn't change, clear error.
    std::fs::write(base.join("empty.code-workspace"), r#"{ "folders": [ { "path": "nope" } ] }"#).unwrap();
    open(&wb, &base.join("empty.code-workspace"), cx);
    assert!(notice(&wb, cx).contains("no folder that exists"), "{}", notice(&wb, cx));
}

/// Found in the real window: the project lives behind a symlink (macOS `/tmp` → `/private/tmp`),
/// clang recorded the logical path in the debug info while the editor opens the canonical one.
/// The breakpoint must still hit, and the stop must reuse the open editor.
#[gpui::test]
#[cfg(unix)]
fn qa_debug_real_symlinked_project_path(cx: &mut TestAppContext) {
    let have = |cmd: &str, args: &[&str]| std::process::Command::new(cmd).args(args).output().map(|o| o.status.success()).unwrap_or(false);
    if !(have("clang", &["--version"]) && (have("lldb-dap", &["--help"]) || have("xcrun", &["-f", "lldb-dap"]))) {
        crate::test_sshd::skip_missing("clang,lldb-dap,xcrun");
        return;
    }
    let real = qa_dir("qa-symreal");
    let link = std::env::temp_dir().join(format!("codeyard-qa-symlink-{}", std::process::id()));
    let _ = std::fs::remove_file(&link);
    std::os::unix::fs::symlink(&real, &link).unwrap();
    let src = "#include <stdio.h>\n\nint twice(int n) {\n    int r = n * 2;\n    return r;\n}\n\nint main(void) {\n    printf(\"%d\\n\", twice(21));\n    return 0;\n}\n";
    std::fs::write(link.join("main.c"), src).unwrap();
    // The compiler writes the logical path (PWD) into the debug info.
    let build = std::process::Command::new("clang").args(["-g", "-O0", "-o", "demo", "main.c"]).current_dir(&link).env("PWD", &link).output().unwrap();
    assert!(build.status.success());
    std::fs::create_dir_all(link.join(".vscode")).unwrap();
    std::fs::write(link.join(".vscode/launch.json"), r#"{ "configurations": [ { "name": "Demo", "type": "lldb", "program": "${workspaceFolder}/demo" } ] }"#)
        .unwrap();
    let (wb, cx) = setup(cx);
    open(&wb, &link.join("main.c"), cx);
    let ed = wb.read_with(cx, |wb, _| wb.active_editor_entity()).unwrap();
    let canonical = ed.read_with(cx, |e, _| e.path.clone());
    assert!(canonical.contains(&format!("{}", real.file_name().unwrap().to_string_lossy())), "editor is open under the canonical path: {canonical}");
    ed.update(cx, |e, cx| e.toggle_breakpoint_at(3, cx));
    run(&wb, cx, "Debug: Start Debugging");
    let snap = |cx: &mut VisualTestContext| wb.read_with(cx, |wb, _| wb.debug_snapshot());
    wait_secs(cx, 60, "stopped behind the symlink", |cx| {
        snap(cx).is_some_and(|s| s.0.starts_with("stopped") && s.2.as_ref().is_some_and(|(p, l)| *l == 3 && *p == canonical))
    });
    assert_eq!(wb.read_with(cx, |wb, cx| wb.open_paths(cx)).len(), 1, "stopping must not open a new editor");
    run(&wb, cx, "Debug: Stop");
    wait_secs(cx, 30, "ended", |cx| snap(cx).is_some_and(|s| s.0 == "ended"));
    let _ = std::fs::remove_file(&link);
}

/// A breakpoint on a line without code never hits: the console explains why instead of staying silent.
#[gpui::test]
fn qa_debug_program_finishing_without_a_hit_explains_itself(cx: &mut TestAppContext) {
    let have = |cmd: &str, args: &[&str]| std::process::Command::new(cmd).args(args).output().map(|o| o.status.success()).unwrap_or(false);
    if !(have("clang", &["--version"]) && (have("lldb-dap", &["--help"]) || have("xcrun", &["-f", "lldb-dap"]))) {
        crate::test_sshd::skip_missing("clang,lldb-dap,xcrun");
        return;
    }
    let dir = qa_dir("qa-nohit");
    std::fs::write(dir.join("main.c"), "#include <stdio.h>\n\nint main(void) {\n    puts(\"hi\");\n    return 0;\n}\n").unwrap();
    let build = std::process::Command::new("clang").args(["-g", "-O0", "-o", "demo", "main.c"]).current_dir(&dir).output().unwrap();
    assert!(build.status.success());
    std::fs::create_dir_all(dir.join(".vscode")).unwrap();
    std::fs::write(dir.join(".vscode/launch.json"), r#"{ "configurations": [ { "name": "Demo", "type": "lldb", "program": "${workspaceFolder}/demo" } ] }"#)
        .unwrap();
    let (wb, cx) = setup(cx);
    open(&wb, &dir, cx);
    let main = std::fs::canonicalize(dir.join("main.c")).unwrap().to_string_lossy().into_owned();
    wb.update_in(cx, |wb, window, cx| wb.open_file_in_root(main, window, cx));
    cx.run_until_parked();
    let ed = wb.read_with(cx, |wb, _| wb.active_editor_entity()).unwrap();
    ed.update(cx, |e, cx| e.toggle_breakpoint_at(0, cx)); // no code on the `#include` line
    run(&wb, cx, "Debug: Start Debugging");
    wait_secs(cx, 60, "ended", |cx| wb.read_with(cx, |wb, _| wb.debug_snapshot()).is_some_and(|s| s.0 == "ended"));
    let console = wb.read_with(cx, |wb, _| wb.debug_snapshot()).unwrap().3.join("\n");
    assert!(console.contains("finished without stopping at any breakpoint"), "{console}");
}

/// A workspace with two folders in one window: the second shows in the Explorer, files open from
/// it, roots resolve per file, the extra folder survives switching projects, and it can be removed.
#[gpui::test]
fn qa_multi_root_explorer_open_persist_remove(cx: &mut TestAppContext) {
    let api = std::fs::canonicalize(qa_dir("qa-mr-api")).unwrap();
    let web = std::fs::canonicalize(qa_dir("qa-mr-web")).unwrap();
    let other = std::fs::canonicalize(qa_dir("qa-mr-other")).unwrap();
    std::fs::write(api.join("main.go"), "package main\n").unwrap();
    std::fs::create_dir_all(web.join("src")).unwrap();
    std::fs::write(web.join("src/app.ts"), "export {}\n").unwrap();
    std::fs::write(other.join("o.txt"), "o\n").unwrap();
    let (wb, cx) = setup(cx);
    open(&wb, &api, cx);
    let (w, a) = (web.to_string_lossy().into_owned(), api.to_string_lossy().into_owned());
    assert!(wb.update_in(cx, |wb, window, cx| wb.add_workspace_folder_now(w.clone(), window, cx)));
    cx.run_until_parked();
    assert_eq!(wb.read_with(cx, |wb, cx| wb.all_roots(cx)), [a.clone(), w.clone()]);
    wait_for(cx, "extra tree lists the folder", |cx| wb.read_with(cx, |wb, cx| wb.extra_tree_rows_for_test(0, cx)).iter().any(|r| r.contains("src")));

    // A duplicate and the first root are rejected.
    assert!(!wb.update_in(cx, |wb, window, cx| wb.add_workspace_folder_now(w.clone(), window, cx)));
    assert!(!wb.update_in(cx, |wb, window, cx| wb.add_workspace_folder_now(a.clone(), window, cx)));
    assert!(notice(&wb, cx).contains("already in the workspace"), "{}", notice(&wb, cx));

    // A file in the extra folder opens; its root and relative path resolve correctly.
    let ts = web.join("src/app.ts").to_string_lossy().into_owned();
    wb.update_in(cx, |wb, window, cx| wb.open_file_in_root(ts.clone(), window, cx));
    cx.run_until_parked();
    assert_eq!(wb.read_with(cx, |wb, cx| wb.root_of(&ts, cx)), Some(w.clone()));
    assert_eq!(wb.read_with(cx, |wb, cx| wb.rel_in_roots(&ts, cx)), Some("src/app.ts".to_string()));
    assert_eq!(wb.read_with(cx, |wb, cx| wb.root_of("/elsewhere/x.rs", cx)), None);

    // Switch to another project and back: the extra folder comes back.
    open(&wb, &other, cx);
    assert!(wb.read_with(cx, |wb, _| wb.extra_roots_for_test()).is_empty(), "no extra folder in another project");
    open(&wb, &api, cx);
    wait_for(cx, "extra folder restored", |cx| wb.read_with(cx, |wb, _| wb.extra_roots_for_test()) == [w.clone()]);

    wb.update(cx, |wb, cx| wb.remove_workspace_folder(&w, cx));
    assert!(wb.read_with(cx, |wb, _| wb.extra_roots_for_test()).is_empty());
    open(&wb, &other, cx);
    open(&wb, &api, cx);
    cx.run_until_parked();
    assert!(wb.read_with(cx, |wb, _| wb.extra_roots_for_test()).is_empty(), "a removed folder does not come back");
}

/// Opening a .code-workspace file now puts every folder in this window.
#[gpui::test]
fn qa_workspace_file_adds_all_folders_to_this_window(cx: &mut TestAppContext) {
    let base = std::fs::canonicalize(qa_dir("qa-mr-file")).unwrap();
    for d in ["one", "two", "three"] {
        std::fs::create_dir_all(base.join(d)).unwrap();
        std::fs::write(base.join(d).join("f.txt"), d).unwrap();
    }
    std::fs::write(base.join("w.code-workspace"), r#"{ "folders": [ { "path": "one" }, { "path": "two" }, { "path": "three" }, { "path": "missing" } ] }"#)
        .unwrap();
    let (wb, cx) = setup(cx);
    open(&wb, &base.join("w.code-workspace"), cx);
    cx.run_until_parked();
    let roots = wb.read_with(cx, |wb, cx| wb.all_roots(cx));
    assert_eq!(roots.iter().map(|r| r.rsplit('/').next().unwrap().to_string()).collect::<Vec<_>>(), ["one", "two", "three"]);
}

/// Two roots in one window: search finds matches in both, Go to file lists both, a task runs in
/// the root of the active file, and each root gets its own language server.
#[gpui::test]
fn qa_multi_root_search_quick_open_tasks_and_lsp(cx: &mut TestAppContext) {
    if std::process::Command::new("python3").arg("-V").output().is_err() {
        crate::test_sshd::skip_missing("python3");
        return;
    }
    let api = std::fs::canonicalize(qa_dir("qa-mr2-api")).unwrap();
    let web = std::fs::canonicalize(qa_dir("qa-mr2-web")).unwrap();
    std::fs::write(api.join("main.c"), "int needle_api;\n").unwrap();
    std::fs::create_dir_all(web.join("src")).unwrap();
    std::fs::write(web.join("src/app.c"), "int needle_web;\n").unwrap();
    std::fs::create_dir_all(web.join(".codeyard")).unwrap();
    std::fs::write(web.join(".codeyard/tasks.jsonc"), r#"[ { "label": "build", "command": "pwd > cwd.txt && echo ${workspaceFolderBasename} > base.txt" } ]"#)
        .unwrap();
    // Server: logs the rootUri from initialize.
    let log = api.join("roots.log");
    let script = api.join("root-ls.py");
    std::fs::write(
        &script,
        r#"#!/usr/bin/env python3
import sys, json
log = open(sys.argv[1], 'a')
def read():
    headers = {}
    while True:
        line = sys.stdin.buffer.readline()
        if not line:
            sys.exit(0)
        line = line.strip()
        if not line:
            break
        k, v = line.split(b':', 1)
        headers[k.strip().lower()] = v.strip()
    return json.loads(sys.stdin.buffer.read(int(headers[b'content-length'])))
def send(m):
    b = json.dumps(m).encode()
    sys.stdout.buffer.write(b'Content-Length: %d\r\n\r\n' % len(b) + b)
    sys.stdout.buffer.flush()
while True:
    m = read()
    if m.get('method') == 'initialize':
        log.write(str(m['params'].get('rootUri')) + '\n'); log.flush()
        send({'jsonrpc': '2.0', 'id': m['id'], 'result': {'capabilities': {'textDocumentSync': 1}}})
    elif m.get('method') == 'shutdown':
        send({'jsonrpc': '2.0', 'id': m['id'], 'result': None})
"#,
    )
    .unwrap();
    #[cfg(unix)]
    crate::test_files::make_executable(&script);
    let settings = Settings {
        lsp_servers: vec![("c".into(), Some((script.to_string_lossy().into_owned(), vec![log.to_string_lossy().into_owned()]).into()))],
        ..Settings::default()
    };
    let (wb, cx) = setup_with(cx, settings);
    open(&wb, &api, cx);
    let w = web.to_string_lossy().into_owned();
    wb.update_in(cx, |wb, window, cx| wb.add_workspace_folder_now(w.clone(), window, cx));
    cx.run_until_parked();

    // Search: finds in both roots; the extra-folder result comes as `folder › path` and can be opened.
    let search = wb.read_with(cx, |wb, _| wb.search_panel());
    cx.simulate_keystrokes("cmd-shift-f");
    cx.simulate_input("needle");
    wait_for(cx, "search in both roots", |cx| {
        cx.executor().advance_clock(Duration::from_millis(300));
        search.read_with(cx, |s, _| !s.is_searching() && s.results_for_test().len() == 2)
    });
    let found: Vec<String> = search.read_with(cx, |s, _| s.results_for_test()).into_iter().map(|(p, _)| p).collect();
    let label = web.file_name().unwrap().to_string_lossy().into_owned();
    assert!(found.contains(&"main.c".to_string()) && found.iter().any(|p| *p == format!("{label} › src/app.c")), "{found:?}");
    cx.simulate_keystrokes("escape");

    // Go to file: the extra-folder file resolves as `web › src/app.c`.
    assert_eq!(
        wb.read_with(cx, |wb, _| wb.palette_path_for_test(&api.to_string_lossy(), &format!("{label} › src/app.c"))),
        web.join("src/app.c").to_string_lossy()
    );
    assert_eq!(wb.read_with(cx, |wb, _| wb.palette_path_for_test(&api.to_string_lossy(), "main.c")), api.join("main.c").to_string_lossy());

    // Language servers: one process per root, each with its own rootUri.
    let main_c = api.join("main.c").to_string_lossy().into_owned();
    let app_c = web.join("src/app.c").to_string_lossy().into_owned();
    wb.update_in(cx, |wb, window, cx| wb.open_file_in_root(main_c.clone(), window, cx));
    cx.run_until_parked();
    wb.update_in(cx, |wb, window, cx| wb.open_file_in_root(app_c.clone(), window, cx));
    wait_for(cx, "two servers with their own roots", |cx| {
        cx.run_until_parked();
        let roots = std::fs::read_to_string(&log).unwrap_or_default();
        roots.lines().count() == 2 && roots.contains(api.file_name().unwrap().to_str().unwrap()) && roots.contains(web.file_name().unwrap().to_str().unwrap())
    });

    // Task: the active file is in the extra folder, so the command runs there.
    run(&wb, cx, "Tasks: Run Build Task");
    wait_for(cx, "task ran in the web root", |_| web.join("cwd.txt").exists() && web.join("base.txt").exists());
    assert_eq!(std::fs::read_to_string(web.join("cwd.txt")).unwrap().trim(), web.to_string_lossy());
    assert_eq!(std::fs::read_to_string(web.join("base.txt")).unwrap().trim(), web.file_name().unwrap().to_str().unwrap());
    assert!(!api.join("cwd.txt").exists(), "must not run in the first root");
}

/// Source Control follows the folder of the file you are in: two repos, one window.
#[gpui::test]
fn qa_multi_root_source_control_follows_the_active_file(cx: &mut TestAppContext) {
    let api = std::fs::canonicalize(repo("qa-mr3-api")).unwrap();
    let web = std::fs::canonicalize(repo("qa-mr3-web")).unwrap();
    std::fs::write(web.join("README.md"), "# web değişti\n").unwrap();
    let (wb, cx) = setup(cx);
    open(&wb, &api, cx);
    wait_ticking(cx, "api status", |cx| changes(&wb, cx).is_some_and(|c| c.is_empty()));
    let w = web.to_string_lossy().into_owned();
    wb.update_in(cx, |wb, window, cx| wb.add_workspace_folder_now(w.clone(), window, cx));
    // Active file in web: Source Control shows the web repo (README changed).
    let readme = web.join("README.md").to_string_lossy().into_owned();
    wb.update_in(cx, |wb, window, cx| wb.open_file_in_root(readme.clone(), window, cx));
    wait_ticking(cx, "scm shows the web repo", |cx| changes(&wb, cx).is_some_and(|c| c.iter().any(|(p, ..)| p == "README.md")));
    assert_eq!(wb.read_with(cx, |wb, _| wb.git_status().map(|g| std::fs::canonicalize(&g.root).unwrap())), Some(web.clone()));
    // Active file in api: back to the clean api repo.
    let lib = api.join("src/lib.rs").to_string_lossy().into_owned();
    wb.update_in(cx, |wb, window, cx| wb.open_file_in_root(lib.clone(), window, cx));
    wait_ticking(cx, "scm back on the api repo", |cx| {
        wb.read_with(cx, |wb, _| wb.git_status().is_some_and(|g| std::fs::canonicalize(&g.root).unwrap() == api))
            && changes(&wb, cx).is_some_and(|c| c.is_empty())
    });
}

/// Tabs and bookmarks of files in an extra folder survive leaving the project and coming back.
#[gpui::test]
fn qa_multi_root_tabs_and_bookmarks_are_restored(cx: &mut TestAppContext) {
    let api = std::fs::canonicalize(qa_dir("qa-mr4-api")).unwrap();
    let web = std::fs::canonicalize(qa_dir("qa-mr4-web")).unwrap();
    let other = std::fs::canonicalize(qa_dir("qa-mr4-other")).unwrap();
    std::fs::write(api.join("main.go"), "package main\n").unwrap();
    std::fs::write(web.join("app.ts"), "a\nb\nc\nd\n").unwrap();
    std::fs::write(other.join("o.txt"), "o\n").unwrap();
    let (wb, cx) = setup(cx);
    open(&wb, &api, cx);
    let w = web.to_string_lossy().into_owned();
    wb.update_in(cx, |wb, window, cx| wb.add_workspace_folder_now(w.clone(), window, cx));
    let ts = web.join("app.ts").to_string_lossy().into_owned();
    wb.update_in(cx, |wb, window, cx| wb.open_file_in_root(ts.clone(), window, cx));
    cx.run_until_parked();
    let ed = wb.read_with(cx, |wb, _| wb.active_editor_entity()).unwrap();
    ed.update(cx, |e, cx| e.toggle_bookmark_for_test(2, cx));
    // Switch to another project and back.
    open(&wb, &other, cx);
    open(&wb, &api, cx);
    wait_for(cx, "web tab restored", |cx| {
        cx.run_until_parked();
        wb.read_with(cx, |wb, cx| wb.open_paths(cx)).contains(&ts)
    });
    let ed = wb.read_with(cx, |wb, cx| wb.editor_for_path_for_test(&ts, cx)).expect("restored editor");
    wait_for(cx, "bookmark restored", |cx| ed.read_with(cx, |e, _| e.bookmark_lines()) == [2]);
}

/// The workspace holds up to 32 extra folders; the next one is refused with a clear message.
#[gpui::test]
fn qa_multi_root_has_a_clear_limit(cx: &mut TestAppContext) {
    let base = std::fs::canonicalize(qa_dir("qa-mr-limit")).unwrap();
    let first = base.join("first");
    std::fs::create_dir_all(&first).unwrap();
    let (wb, cx) = setup(cx);
    open(&wb, &first, cx);
    for i in 0..32 {
        let d = base.join(format!("extra-{i}"));
        std::fs::create_dir_all(&d).unwrap();
        assert!(wb.update_in(cx, |wb, window, cx| wb.add_workspace_folder_now(d.to_string_lossy().into_owned(), window, cx)), "folder {i}");
    }
    let over = base.join("one-too-many");
    std::fs::create_dir_all(&over).unwrap();
    assert!(!wb.update_in(cx, |wb, window, cx| wb.add_workspace_folder_now(over.to_string_lossy().into_owned(), window, cx)));
    assert!(notice(&wb, cx).contains("At most 32"), "{}", notice(&wb, cx));
    assert_eq!(wb.read_with(cx, |wb, cx| wb.all_roots(cx).len()), 33);
}

/// Each extra folder has its own settings: indentation and save behaviour follow the folder of
/// the file, the first folder is untouched, and editing the settings file re-reads them.
#[gpui::test]
fn qa_multi_root_folder_settings_apply_per_file(cx: &mut TestAppContext) {
    let api = std::fs::canonicalize(qa_dir("qa-mr5-api")).unwrap();
    let web = std::fs::canonicalize(qa_dir("qa-mr5-web")).unwrap();
    std::fs::create_dir_all(web.join(".vscode")).unwrap();
    std::fs::write(
        web.join(".vscode/settings.json"),
        r#"{ "editor.tabSize": 2, "editor.insertSpaces": true, "files.trimTrailingWhitespace": true, "files.insertFinalNewline": true }"#,
    )
    .unwrap();
    std::fs::write(api.join("a.txt"), "x\n").unwrap();
    std::fs::write(web.join("w.txt"), "y\n").unwrap();
    let (wb, cx) = setup(cx);
    open(&wb, &api, cx);
    let w = crate::ui::local_path(&web);
    wb.update_in(cx, |wb, window, cx| wb.add_workspace_folder_now(w.clone(), window, cx));
    wait_for(cx, "folder settings loaded", |cx| {
        cx.run_until_parked();
        wb.read_with(cx, |wb, _| wb.folder_settings_for_test(&w)).is_some_and(|f| f.tab_size == Some(2))
    });
    let (a_path, w_path) = (crate::ui::local_path(&api.join("a.txt")), crate::ui::local_path(&web.join("w.txt")));
    wb.update_in(cx, |wb, window, cx| wb.open_file_in_root(a_path.clone(), window, cx));
    wait_for(cx, "api file open", |cx| {
        cx.run_until_parked();
        wb.read_with(cx, |wb, cx| wb.editor_for_path_for_test(&a_path, cx).is_some())
    });
    wb.update_in(cx, |wb, window, cx| wb.open_file_in_root(w_path.clone(), window, cx));
    wait_for(cx, "both files open", |cx| {
        cx.run_until_parked();
        wb.read_with(cx, |wb, cx| wb.editor_for_path_for_test(&a_path, cx).is_some() && wb.editor_for_path_for_test(&w_path, cx).is_some())
    });
    let ed_of = |cx: &mut VisualTestContext, p: &str| wb.read_with(cx, |wb, cx| wb.editor_for_path_for_test(p, cx)).unwrap();
    let (a, wed) = (ed_of(cx, &a_path), ed_of(cx, &w_path));
    assert_eq!(wed.read_with(cx, |e, _| e.indent_unit_for_test()), 2, "web: 2 spaces");
    assert_ne!(a.read_with(cx, |e, _| e.indent_unit_for_test()), 2, "api: global setting (4 spaces)");

    // Save: trailing whitespace is trimmed and a final newline added in the web file; api is untouched.
    for ed in [&a, &wed] {
        ed.update(cx, |e, cx| {
            e.apply_buffer_edits(|b| vec![(b.len_chars()..b.len_chars(), "tail   ".to_string())], cx);
            e.save_now(cx);
        });
    }
    wait_for(cx, "both saved", |cx| {
        cx.run_until_parked();
        std::fs::read_to_string(&w_path).unwrap().contains("tail") && std::fs::read_to_string(&a_path).unwrap().contains("tail")
    });
    assert_eq!(std::fs::read_to_string(&w_path).unwrap(), "y\ntail\n", "web: trimmed + final newline");
    assert_eq!(std::fs::read_to_string(&a_path).unwrap(), "x\ntail   ", "api: no setting, left as is");

    // The settings file is re-read when it changes.
    std::fs::write(web.join(".vscode/settings.json"), r#"{ "editor.tabSize": 8, "editor.insertSpaces": true }"#).unwrap();
    wb.update(cx, |wb, cx| wb.load_folder_settings_for_test(w.clone(), cx));
    wait_for(cx, "settings re-read", |cx| {
        cx.run_until_parked();
        wed.read_with(cx, |e, _| e.indent_unit_for_test()) == 8
    });
}

/// Exception breakpoints: the adapter's defaults go out before `configurationDone`, a change while
/// paused reaches the adapter, and the choice is remembered for the next session.
#[gpui::test]
fn qa_debug_exception_filters_are_sent_and_remembered(cx: &mut TestAppContext) {
    if std::process::Command::new("python3").arg("-V").output().is_err() {
        crate::test_sshd::skip_missing("python3");
        return;
    }
    let dir = qa_dir("qa-debugexc");
    let log = dir.join("dap.log");
    let script = crate::dap::tests::write_fake_adapter(&dir);
    std::fs::write(dir.join("main.c"), (1..=8).map(|i| format!("int line{i};\n")).collect::<String>()).unwrap();
    std::fs::create_dir_all(dir.join(".vscode")).unwrap();
    std::fs::write(
        dir.join(".vscode/launch.json"),
        format!(r#"{{ "configurations": [ {{ "name": "Fake", "type": "custom", "program": "x", "source": "${{workspaceFolder}}/main.c", "debugAdapter": {{ "command": "{}", "args": ["{}"] }} }} ] }}"#, script.display(), log.display()),
    )
    .unwrap();
    let (wb, cx) = setup(cx);
    open(&wb, &dir, cx);
    let main = dir.join("main.c").to_string_lossy().into_owned();
    wb.update_in(cx, |wb, window, cx| wb.open_file_in_root(main.clone(), window, cx));
    cx.run_until_parked();
    let ed = wb.read_with(cx, |wb, _| wb.active_editor_entity()).unwrap();
    ed.update(cx, |e, cx| e.toggle_breakpoint_at(3, cx));
    run(&wb, cx, "Debug: Start Debugging");
    wait_for(cx, "stopped", |cx| wb.read_with(cx, |wb, _| wb.debug_snapshot()).is_some_and(|s| s.0.starts_with("stopped")));
    let sent = std::fs::read_to_string(&log).unwrap();
    let order: Vec<&str> = sent.lines().map(|l| l.split(' ').next().unwrap()).collect();
    let (exc, done) = (order.iter().position(|c| *c == "setExceptionBreakpoints").unwrap(), order.iter().position(|c| *c == "configurationDone").unwrap());
    assert!(exc < done, "exception filters go before configurationDone: {order:?}");
    assert!(sent.contains(r#"setExceptionBreakpoints {"filters": ["uncaught"]}"#), "adapter default: {sent}");

    // While paused, expand "raised": goes to the adapter.
    wb.update(cx, |wb, cx| wb.toggle_exception_filter_for_test("raised", cx));
    wait_for(cx, "filters updated", |_| std::fs::read_to_string(&log).unwrap().contains(r#"{"filters": ["uncaught", "raised"]}"#));
    run(&wb, cx, "Debug: Stop");
    wait_for(cx, "ended", |cx| wb.read_with(cx, |wb, _| wb.debug_snapshot()).is_some_and(|s| s.0 == "ended"));

    // The next session remembers the choice (doesn't reset to the default).
    run(&wb, cx, "Debug: Start Debugging");
    wait_for(cx, "stopped again", |cx| wb.read_with(cx, |wb, _| wb.debug_snapshot()).is_some_and(|s| s.0.starts_with("stopped")));
    let sent = std::fs::read_to_string(&log).unwrap();
    assert!(sent.matches(r#"setExceptionBreakpoints {"filters": ["uncaught", "raised"]}"#).count() >= 2, "{sent}");
}

/// Real debugpy: an exception that the program catches itself still stops the debugger when
/// "raised" is on, at the line that raised it, with reason "exception".
#[gpui::test]
fn qa_debug_real_debugpy_breaks_on_a_caught_exception(cx: &mut TestAppContext) {
    if !std::process::Command::new("python3").args(["-c", "import debugpy"]).output().map(|o| o.status.success()).unwrap_or(false) {
        crate::test_sshd::skip_missing("python3,debugpy");
        return;
    }
    let dir = qa_dir("qa-realpyexc");
    let src = "def risky():\n    raise ValueError('boom')\n\n\ntry:\n    risky()\nexcept ValueError:\n    print('caught')\n";
    std::fs::write(dir.join("app.py"), src).unwrap();
    let (wb, cx) = setup(cx);
    open(&wb, &dir, cx);
    let app = std::fs::canonicalize(dir.join("app.py")).unwrap().to_string_lossy().into_owned();
    wb.update_in(cx, |wb, window, cx| wb.open_file_in_root(app.clone(), window, cx));
    cx.run_until_parked();
    wb.update(cx, |wb, _| wb.set_exception_filters_for_test(Some(vec!["raised".into()])));
    run(&wb, cx, "Debug: Start Debugging");
    let snap = |cx: &mut VisualTestContext| wb.read_with(cx, |wb, _| wb.debug_snapshot());
    wait_secs(cx, 60, "stopped on the raise", |cx| snap(cx).is_some_and(|s| s.0 == "stopped:exception" && s.2.as_ref().is_some_and(|(_, l)| *l == 1)));
    run(&wb, cx, "Debug: Stop");
    wait_secs(cx, 30, "ended", |cx| snap(cx).is_some_and(|s| s.0 == "ended"));
}

/// Threads: the panel lists them, picking one asks the adapter for that thread's stack; an `attach`
/// configuration starts through the same flow with the `attach` request.
#[gpui::test]
fn qa_debug_threads_and_attach_configuration(cx: &mut TestAppContext) {
    if std::process::Command::new("python3").arg("-V").output().is_err() {
        crate::test_sshd::skip_missing("python3");
        return;
    }
    let dir = qa_dir("qa-debugthreads");
    let log = dir.join("dap.log");
    let script = crate::dap::tests::write_fake_adapter(&dir);
    std::fs::write(dir.join("main.c"), (1..=8).map(|i| format!("int line{i};\n")).collect::<String>()).unwrap();
    std::fs::create_dir_all(dir.join(".vscode")).unwrap();
    std::fs::write(
        dir.join(".vscode/launch.json"),
        format!(r#"{{ "configurations": [ {{ "name": "Attach", "type": "custom", "request": "attach", "pid": 1234, "source": "${{workspaceFolder}}/main.c", "debugAdapter": {{ "command": "{}", "args": ["{}"] }} }} ] }}"#, script.display(), log.display()),
    )
    .unwrap();
    let (wb, cx) = setup(cx);
    open(&wb, &dir, cx);
    let main = dir.join("main.c").to_string_lossy().into_owned();
    wb.update_in(cx, |wb, window, cx| wb.open_file_in_root(main.clone(), window, cx));
    cx.run_until_parked();
    let ed = wb.read_with(cx, |wb, _| wb.active_editor_entity()).unwrap();
    ed.update(cx, |e, cx| e.toggle_breakpoint_at(2, cx));
    run(&wb, cx, "Debug: Start Debugging");
    wait_for(cx, "stopped", |cx| wb.read_with(cx, |wb, _| wb.debug_snapshot()).is_some_and(|s| s.0.starts_with("stopped") && !s.1.is_empty()));
    let sent = std::fs::read_to_string(&log).unwrap();
    assert!(sent.lines().any(|l| l.starts_with("attach ") && l.contains("\"pid\": 1234")), "attach request was sent: {sent}");
    assert!(!sent.lines().any(|l| l.starts_with("launch ")), "launch gitmemeli: {sent}");
    wait_for(cx, "thread list", |cx| wb.read_with(cx, |wb, _| wb.debug_threads_for_test()) == [(1, "main".to_string()), (2, "worker".to_string())]);
    wb.update(cx, |wb, cx| wb.select_thread_for_test(2, cx));
    wait_for(cx, "stack of thread 2 requested", |_| {
        std::fs::read_to_string(&log).unwrap().lines().any(|l| l.starts_with("stackTrace") && l.contains("\"threadId\": 2"))
    });
}

/// Real debugpy attach: a separate Python process waits for a client on a port (`--listen
/// --wait-for-client`), the debugger attaches from a launch.json `attach` configuration, stops at a
/// breakpoint inside the running loop, lists both threads and then detaches.
#[gpui::test]
fn qa_debug_real_debugpy_attach_and_threads(cx: &mut TestAppContext) {
    if !std::process::Command::new("python3").args(["-c", "import debugpy"]).output().map(|o| o.status.success()).unwrap_or(false) {
        crate::test_sshd::skip_missing("python3,debugpy");
        return;
    }
    let dir = qa_dir("qa-realattach");
    let src = "import threading, time\n\n\ndef worker():\n    while True:\n        time.sleep(0.05)\n\n\nthreading.Thread(target=worker, name='background-worker', daemon=True).start()\nfor i in range(1000):\n    value = i * 2\n    time.sleep(0.05)\n";
    std::fs::write(dir.join("loop.py"), src).unwrap();
    let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    let mut child = std::process::Command::new("python3")
        .args(["-m", "debugpy", "--listen", &format!("127.0.0.1:{port}"), "--wait-for-client"])
        .arg(dir.join("loop.py"))
        .current_dir(&dir)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    std::thread::sleep(Duration::from_millis(1500));
    std::fs::create_dir_all(dir.join(".vscode")).unwrap();
    std::fs::write(
        dir.join(".vscode/launch.json"),
        format!(
            r#"{{ "configurations": [ {{ "name": "Attach", "type": "python", "request": "attach", "connect": {{ "host": "127.0.0.1", "port": {port} }} }} ] }}"#
        ),
    )
    .unwrap();
    let (wb, cx) = setup(cx);
    open(&wb, &dir, cx);
    let file = std::fs::canonicalize(dir.join("loop.py")).unwrap().to_string_lossy().into_owned();
    wb.update_in(cx, |wb, window, cx| wb.open_file_in_root(file.clone(), window, cx));
    cx.run_until_parked();
    let ed = wb.read_with(cx, |wb, _| wb.active_editor_entity()).unwrap();
    // Line 11 (0-based 10): `value = i * 2`
    ed.update(cx, |e, cx| e.toggle_breakpoint_at(10, cx));
    run(&wb, cx, "Debug: Start Debugging");
    // Two configurations (Attach from launch.json, detected Python): the picker opens, the first is Attach.
    wait_for(cx, "configuration picker", |cx| wb.read_with(cx, |wb, cx| wb.overlay_labels(cx)).is_some_and(|l| l.first().is_some_and(|f| f == "Attach")));
    cx.simulate_keystrokes("enter");
    let snap = |cx: &mut VisualTestContext| wb.read_with(cx, |wb, _| wb.debug_snapshot());
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        wait_secs(cx, 60, "stopped inside the running loop", |cx| {
            snap(cx).is_some_and(|s| s.0.starts_with("stopped") && s.2.as_ref().is_some_and(|(_, l)| *l == 10))
        });
        wait_secs(cx, 20, "both threads listed", |cx| {
            let t = wb.read_with(cx, |wb, _| wb.debug_threads_for_test());
            t.len() >= 2 && t.iter().any(|(_, n)| n.contains("background-worker"))
        });
        wait_secs(cx, 20, "loop variable", |cx| wb.read_with(cx, |wb, _| wb.debug_variables()).iter().any(|(n, _)| n == "i"));
    }));
    run(&wb, cx, "Debug: Stop");
    // Attach: disconnecting doesn't kill the program (launch sends terminateDebuggee, this must not).
    let ended = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| wait_secs(cx, 30, "session ended", |cx| snap(cx).is_some_and(|s| s.0 == "ended"))));
    std::thread::sleep(Duration::from_millis(500));
    let still_running = child.try_wait().ok().flatten().is_none();
    let _ = child.kill();
    let _ = child.wait();
    if let Err(e) = result.and(ended) {
        std::panic::resume_unwind(e);
    }
    assert!(still_running, "target process must keep running after detach");
}

/// Real lldb-dap: a data breakpoint on a local stops the program exactly when the variable is
/// written, with no source breakpoint involved.
#[gpui::test]
fn qa_debug_real_data_breakpoint(cx: &mut TestAppContext) {
    let have = |cmd: &str, args: &[&str]| std::process::Command::new(cmd).args(args).output().map(|o| o.status.success()).unwrap_or(false);
    if !(have("clang", &["--version"]) && (have("lldb-dap", &["--help"]) || have("xcrun", &["-f", "lldb-dap"]))) {
        crate::test_sshd::skip_missing("clang,lldb-dap,xcrun");
        return;
    }
    let dir = qa_dir("qa-realdata");
    let src = "#include <stdio.h>\n\nint main(void) {\n    int total = 0;\n    for (int i = 1; i <= 5; i++) {\n        total += i * i;\n        printf(\"%d\\n\", i);\n    }\n    printf(\"%d\\n\", total);\n    return 0;\n}\n";
    std::fs::write(dir.join("main.c"), src).unwrap();
    let build = std::process::Command::new("clang").args(["-g", "-O0", "-o", "demo", "main.c"]).current_dir(&dir).output().unwrap();
    assert!(build.status.success());
    std::fs::create_dir_all(dir.join(".vscode")).unwrap();
    std::fs::write(dir.join(".vscode/launch.json"), r#"{ "configurations": [ { "name": "Demo", "type": "lldb", "program": "${workspaceFolder}/demo" } ] }"#)
        .unwrap();
    let (wb, cx) = setup(cx);
    open(&wb, &dir, cx);
    let main = std::fs::canonicalize(dir.join("main.c")).unwrap().to_string_lossy().into_owned();
    wb.update_in(cx, |wb, window, cx| wb.open_file_in_root(main, window, cx));
    cx.run_until_parked();
    let ed = wb.read_with(cx, |wb, _| wb.active_editor_entity()).unwrap();
    // Line 6 (0-based 5): `total += i * i;` only for the first stop.
    ed.update(cx, |e, cx| e.toggle_breakpoint_at(5, cx));
    run(&wb, cx, "Debug: Start Debugging");
    let value_of = |cx: &mut VisualTestContext, n: &str| wb.read_with(cx, |wb, _| wb.debug_variables()).into_iter().find(|(k, _)| k == n).map(|(_, v)| v);
    wait_secs(cx, 60, "first stop", |cx| value_of(cx, "i").as_deref() == Some("1"));
    // Remove the source breakpoint, put a data breakpoint on `total`, continue.
    ed.update(cx, |e, cx| e.toggle_breakpoint_at(5, cx));
    wb.update(cx, |wb, cx| wb.add_data_breakpoint_for_test("total", cx));
    wait_secs(cx, 20, "data breakpoint set", |cx| !wb.read_with(cx, |wb, _| wb.data_breakpoints_for_test()).is_empty());
    run(&wb, cx, "Debug: Continue");
    // `total += 1` was written: the program stops right after it (total = 1; i still 1).
    wait_secs(cx, 30, "stopped by the write", |cx| {
        wb.read_with(cx, |wb, _| wb.debug_snapshot()).is_some_and(|s| s.0.starts_with("stopped")) && value_of(cx, "total").as_deref() == Some("1")
    });
    assert_eq!(value_of(cx, "i").as_deref(), Some("1"));
    // Devam: sonraki yazma `total += 4` (= 5), i = 2.
    run(&wb, cx, "Debug: Continue");
    wait_secs(cx, 30, "second write", |cx| value_of(cx, "total").as_deref() == Some("5"));
    run(&wb, cx, "Debug: Remove Data Breakpoints");
    run(&wb, cx, "Debug: Continue");
    wait_secs(cx, 60, "runs to the end", |cx| wb.read_with(cx, |wb, _| wb.debug_snapshot()).is_some_and(|s| s.0 == "ended"));
}

// --- Terminal ------------------------------------------------------------

fn terminal(wb: &Entity<Workbench>, dir: &Path, cx: &mut VisualTestContext) -> Entity<crate::terminal::TerminalView> {
    open(wb, dir, cx);
    wb.update(cx, |wb, _| wb.set_shell("/bin/sh"));
    cx.simulate_keystrokes("ctrl-`");
    let term = wb.read_with(cx, |wb, _| wb.active_terminal()).expect("terminal");
    cx.simulate_keystrokes("e c h o space r e a d y enter");
    wait_for(cx, "shell ready", |cx| term.read_with(cx, |t, _| t.screen_text().lines().any(|l| l == "ready")));
    term
}

pub(crate) fn type_line(cx: &mut VisualTestContext, s: &str) {
    cx.simulate_input(s);
    cx.simulate_keystrokes("enter");
}

#[gpui::test]
fn qa_terminal_ctrl_c_interrupts(cx: &mut TestAppContext) {
    let dir = qa_dir("qa-termctrlc");
    let (wb, cx) = setup(cx);
    let term = terminal(&wb, &dir, cx);
    // Wait until the child is really running before interrupting: a fixed sleep let Ctrl-C reach the shell before it had started
    // `sleep` on a loaded machine, and then `sleep 60` ran to the end. The echoed command line contains `$((1+1))`, only the output
    // line is exactly `running-2`.
    type_line(cx, "sh -c 'echo running-$((1+1)); sleep 60'");
    wait_for(cx, "child running", |cx| term.read_with(cx, |t, _| t.screen_text().lines().any(|l| l == "running-2")));
    cx.simulate_keystrokes("ctrl-c");
    type_line(cx, "echo bitti-$((2+3))");
    wait_for(cx, "after ctrl-c", |cx| term.read_with(cx, |t, _| t.screen_text().lines().any(|l| l == "bitti-5")));
}

/// The pty size must match what is drawn (`stty size` rows == visible rows).
#[gpui::test]
fn qa_terminal_pty_size_matches_grid(cx: &mut TestAppContext) {
    let dir = qa_dir("qa-termsize");
    let (wb, cx) = setup(cx);
    let term = terminal(&wb, &dir, cx);
    // On the first frame the grid may resize to the window; it takes a moment to reach the pty.
    // Ask again until the size settles (at most 6 tries): a persistent mismatch still fails.
    let mut last = String::new();
    for attempt in 0..6 {
        type_line(cx, &format!("echo SIZE{attempt}=$(stty size)"));
        let marker = format!("SIZE{attempt}=");
        wait_for(cx, "stty", |cx| term.read_with(cx, |t, _| t.screen_text().lines().any(|l| l.starts_with(&marker) && l.len() > marker.len() + 1)));
        let text = term.read_with(cx, |t, _| t.screen_text());
        let line = text.lines().find(|l| l.starts_with(&marker) && l.len() > marker.len() + 1).unwrap().to_string();
        let rows: usize = line[marker.len()..].split_whitespace().next().unwrap().parse().unwrap();
        last = format!("{line} vs grid {}", text.split('\n').count());
        if rows == text.split('\n').count() {
            return;
        }
        std::thread::sleep(Duration::from_millis(250));
        cx.run_until_parked();
    }
    panic!("pty rows vs grid rows never matched: {last}");
}

/// Full-screen programs (less, man, git log pager, vim) run on the alternate
/// screen; the mouse wheel there must send ↑/↓ (xterm "alternate scroll",
/// VS Code does this), otherwise the wheel does nothing in `less`.
#[gpui::test]
fn qa_terminal_wheel_in_alternate_screen_sends_arrows(cx: &mut TestAppContext) {
    let dir = qa_dir("qa-termaltscroll");
    let (wb, cx) = setup(cx);
    let term = terminal(&wb, &dir, cx);
    type_line(cx, "printf '\\033[?1049h'; cat -v");
    std::thread::sleep(Duration::from_millis(300));
    cx.run_until_parked();
    let at = term.read_with(cx, |t, _| t.cell_center(5, 5));
    for _ in 0..3 {
        cx.simulate_event(gpui::ScrollWheelEvent {
            position: at,
            delta: gpui::ScrollDelta::Lines(gpui::point(0., 3.)),
            modifiers: Modifiers::none(),
            touch_phase: gpui::TouchPhase::Moved,
        });
    }
    let got = echoed(cx, &term, "^[");
    // Control: a real ↑ key press is echoed the same way, so the setup works.
    cx.simulate_keystrokes("up");
    assert!(echoed(cx, &term, "^["), "control: ↑ key not echoed");
    assert!(got, "wheel in the alternate screen sent nothing to the program");
}

fn echoed(cx: &mut VisualTestContext, term: &Entity<crate::terminal::TerminalView>, what: &str) -> bool {
    let start = std::time::Instant::now();
    while start.elapsed() < Duration::from_secs(3) {
        cx.run_until_parked();
        if term.read_with(cx, |t, _| t.screen_text().contains(what)) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    false
}

/// Programs that enable mouse reporting (htop, vim `mouse=a`, tmux, mc) must
/// receive clicks (VS Code forwards them).
#[gpui::test]
fn qa_terminal_mouse_reporting_reaches_program(cx: &mut TestAppContext) {
    let dir = qa_dir("qa-termmouse");
    let (wb, cx) = setup(cx);
    let term = terminal(&wb, &dir, cx);
    type_line(cx, "printf '\\033[?1000h'; cat -v");
    std::thread::sleep(Duration::from_millis(300));
    cx.run_until_parked();
    let at = term.read_with(cx, |t, _| t.cell_center(8, 10));
    cx.simulate_mouse_down(at, MouseButton::Left, Modifiers::none());
    cx.simulate_mouse_up(at, MouseButton::Left, Modifiers::none());
    let got = echoed(cx, &term, "^[[M");
    cx.simulate_keystrokes("up");
    assert!(echoed(cx, &term, "^[[A"), "control: ↑ key not echoed");
    assert!(got, "click was not reported to the program (mouse mode 1000 on)");
}

/// macOS line editing in the shell: ⌘← / ⌘→ jump to the ends of the line, ⌥← moves back a word, ⌘⌫ clears the line.
#[cfg(target_os = "macos")]
#[gpui::test]
fn qa_terminal_macos_line_editing_keys(cx: &mut TestAppContext) {
    let dir = qa_dir("qa-termlineedit");
    let (wb, cx) = setup(cx);
    let term = terminal(&wb, &dir, cx);
    cx.simulate_input("echo world-end");
    cx.simulate_keystrokes("cmd-left");
    cx.simulate_input("echo hello-$((1+1)); ");
    cx.simulate_keystrokes("cmd-right alt-left");
    cx.simulate_input("X");
    cx.simulate_keystrokes("enter");
    wait_for(cx, "edited line ran", |cx| {
        term.read_with(cx, |t, _| {
            let text = t.screen_text();
            text.lines().any(|l| l == "hello-2") && text.lines().any(|l| l == "world-Xend")
        })
    });
    cx.simulate_input("echo wrong-$((1+4))");
    cx.simulate_keystrokes("cmd-backspace");
    type_line(cx, "echo right-$((3+4))");
    wait_for(cx, "cleared line", |cx| term.read_with(cx, |t, _| t.screen_text().lines().any(|l| l == "right-7")));
    assert!(!term.read_with(cx, |t, _| t.screen_text()).lines().any(|l| l == "wrong-5"), "⌘⌫ left the old text on the line");
}

/// Find in the terminal: matches in the output are counted and selected, typing in the bar does not reach the shell,
/// Escape closes it and the shell has the keys again.
#[gpui::test]
fn qa_terminal_find_counts_selects_and_closes(cx: &mut TestAppContext) {
    let dir = qa_dir("qa-termfind");
    let (wb, cx) = setup(cx);
    let term = terminal(&wb, &dir, cx);
    type_line(cx, "for i in 1 2 3; do echo row-$i-needle; done");
    wait_for(cx, "rows", |cx| term.read_with(cx, |t, _| t.screen_text().lines().any(|l| l == "row-3-needle")));
    // The platform's own key opens the bar and typing searches (⌘F on macOS; Ctrl+F belongs to the shell elsewhere).
    cx.simulate_keystrokes(if cfg!(target_os = "macos") { "cmd-f" } else { "ctrl-shift-f" });
    cx.simulate_input("needle");
    assert_eq!(term.update_in(cx, |t, window, cx| t.find_state_for_test(window, cx)), (true, 4), "find bar opened by its key");
    cx.simulate_keystrokes("escape");
    let (total, selected) = term.update_in(cx, |t, window, cx| t.find_for_test("needle", window, cx));
    // The command line holds the word too.
    assert_eq!(total, 4);
    assert_eq!(selected.as_deref(), Some("needle"));
    // Walking up and down stays on matches and wraps around.
    for down in [false, false, false, false, true, true] {
        assert_eq!(term.update(cx, |t, cx| t.find_step_for_test(down, cx)).as_deref(), Some("needle"));
    }
    // A query that is not in the output.
    let (none, _) = term.update_in(cx, |t, window, cx| t.find_for_test("haystack", window, cx));
    assert_eq!(none, 0);
    // The bar has focus: typed text stays there.
    cx.simulate_input("zz");
    cx.simulate_keystrokes("escape");
    type_line(cx, "echo back-$((2+3))");
    wait_for(cx, "shell has the keys again", |cx| term.read_with(cx, |t, _| t.screen_text().lines().any(|l| l == "back-5")));
    assert!(!term.read_with(cx, |t, _| t.screen_text()).contains("zz"), "keys typed in the find bar reached the shell");
}

// --- Remote SSH (local throwaway sshd only) ------------------------------

/// Over SSH: git status comes from the server, staging works, the terminal runs
/// on the server; after the transport drops it reconnects and the same
/// terminal keeps working.
#[gpui::test]
fn qa_ssh_git_terminal_and_reconnect(cx: &mut TestAppContext) {
    let Some(sshd) = crate::test_sshd::start("qa-remote") else { return };
    let dir = repo("qa-remote-repo");
    std::fs::write(dir.join("README.md"), "# uzakta değişti\n").unwrap();
    let (wb, cx) = setup(cx);
    wb.update_in(cx, |wb, window, cx| wb.connect(crate::test_sshd::ALIAS.into(), window, cx));
    let tick = |cx: &mut VisualTestContext| cx.executor().advance_clock(Duration::from_secs(1));
    wait_secs(cx, 30, "connected", |cx| {
        tick(cx);
        wb.read_with(cx, |wb, _| wb.ports_ready())
    });
    open(&wb, &dir, cx);
    wait_secs(cx, 20, "remote git status", |cx| {
        tick(cx);
        changes(&wb, cx).is_some_and(|c| c.len() == 1)
    });
    let scm = wb.read_with(cx, |wb, _| wb.scm_panel());
    scm.update(cx, |s, cx| s.stage(vec!["README.md".into()], cx));
    wait_secs(cx, 20, "remote stage", |cx| {
        tick(cx);
        changes(&wb, cx).is_some_and(|c| c.iter().any(|(p, s, _)| p == "README.md" && *s == Some(Mark::Modified)))
    });

    wb.update(cx, |wb, _| wb.set_shell("/bin/sh"));
    cx.simulate_keystrokes("ctrl-`");
    let term = wb.read_with(cx, |wb, _| wb.active_terminal()).expect("terminal");
    type_line(cx, "echo uzak-$((40+2))");
    wait_secs(cx, 20, "remote terminal", |cx| term.read_with(cx, |t, _| t.screen_text().lines().any(|l| l == "uzak-42")));

    // Drop the transport (the fixture's bridge process, started by us).
    let _ = std::process::Command::new("pkill").arg("-f").arg(format!("{}/remote/agent.*--attach", sshd.dir.display())).status();
    std::thread::sleep(Duration::from_secs(2));
    wait_secs(cx, 40, "reconnected", |cx| {
        tick(cx);
        notice(&wb, cx).starts_with("Reconnected")
    });
    assert_eq!(notice(&wb, cx), "Reconnected. Terminals kept running.");
    type_line(cx, "echo tekrar-$((1+1))");
    wait_secs(cx, 20, "terminal after reconnect", |cx| term.read_with(cx, |t, _| t.screen_text().lines().any(|l| l == "tekrar-2")));
}

/// Connects the workbench to the throwaway sshd and opens `dir` on it.
pub(crate) fn ssh_workbench<'a>(cx: &'a mut TestAppContext, dir: &Path) -> (Entity<Workbench>, &'a mut VisualTestContext) {
    let (wb, cx) = setup(cx);
    wb.update_in(cx, |wb, window, cx| wb.connect(crate::test_sshd::ALIAS.into(), window, cx));
    wait_secs(cx, 30, "connected", |cx| {
        cx.executor().advance_clock(Duration::from_secs(1));
        wb.read_with(cx, |wb, _| wb.ports_ready())
    });
    open(&wb, dir, cx);
    (wb, cx)
}

/// Opens `file` over ssh and returns its editor.
pub(crate) fn ssh_open_file(wb: &Entity<Workbench>, cx: &mut VisualTestContext, file: &Path) -> Entity<crate::editor::EditorView> {
    let path = std::fs::canonicalize(file).unwrap().to_string_lossy().into_owned();
    wb.update_in(cx, |wb, window, cx| wb.open_file_in_root(path, window, cx));
    wait_secs(cx, 20, "file opened over ssh", |cx| {
        cx.executor().advance_clock(Duration::from_secs(1));
        wb.read_with(cx, |wb, _| wb.active_editor_entity()).is_some()
    });
    wb.read_with(cx, |wb, _| wb.active_editor_entity()).unwrap()
}

/// Debugging over SSH: the adapter (debugpy) starts on the server through the agent, and the
/// breakpoint, stack and variables travel back over the connection.
#[gpui::test]
fn qa_ssh_debug_session(cx: &mut TestAppContext) {
    if !std::process::Command::new("python3").args(["-c", "import debugpy"]).output().map(|o| o.status.success()).unwrap_or(false) {
        crate::test_sshd::skip_missing("python3,debugpy");
        return;
    }
    let Some(_sshd) = crate::test_sshd::start("qa-remote-debug") else { return };
    let dir = qa_dir("qa-remote-debug-proj");
    let src = "def area(w, h):\n    result = w * h\n    print('area', result)\n    return result\n\n\nif __name__ == '__main__':\n    print(area(6, 7))\n";
    std::fs::write(dir.join("app.py"), src).unwrap();
    let (wb, cx) = ssh_workbench(cx, &dir);
    let ed = ssh_open_file(&wb, cx, &dir.join("app.py"));
    ed.update(cx, |e, cx| e.toggle_breakpoint_at(2, cx));
    run(&wb, cx, "Debug: Start Debugging");
    let snap = |cx: &mut VisualTestContext| wb.read_with(cx, |wb, _| wb.debug_snapshot());
    let tick = |cx: &mut VisualTestContext| cx.executor().advance_clock(Duration::from_secs(1));
    wait_secs(cx, 90, "stopped over ssh", |cx| {
        tick(cx);
        snap(cx).is_some_and(|s| s.0.starts_with("stopped") && s.2.as_ref().is_some_and(|(_, l)| *l == 2))
    });
    wait_secs(cx, 30, "locals over ssh", |cx| {
        tick(cx);
        wb.read_with(cx, |wb, _| wb.debug_variables()).iter().any(|(n, v)| n == "result" && v == "42")
    });
    run(&wb, cx, "Debug: Stop");
    wait_secs(cx, 30, "ended", |cx| {
        tick(cx);
        snap(cx).is_some_and(|s| s.0 == "ended")
    });
}

/// Attach over SSH: the program runs on the "server" with its debug server listening; the client
/// port is forwarded (`ssh -L`) and the debugger attaches through it, then detaches without killing it.
#[gpui::test]
fn qa_ssh_debug_attach_through_a_port_forward(cx: &mut TestAppContext) {
    if !std::process::Command::new("python3").args(["-c", "import debugpy"]).output().map(|o| o.status.success()).unwrap_or(false) {
        crate::test_sshd::skip_missing("python3,debugpy");
        return;
    }
    let Some(_sshd) = crate::test_sshd::start("qa-remote-attach") else { return };
    let dir = qa_dir("qa-remote-attach-proj");
    std::fs::write(dir.join("loop.py"), "import time\nfor i in range(1000):\n    value = i * 2\n    time.sleep(0.05)\n").unwrap();
    let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    let mut child = std::process::Command::new("python3")
        .args(["-m", "debugpy", "--listen", &format!("127.0.0.1:{port}"), "--wait-for-client"])
        .arg(dir.join("loop.py"))
        .current_dir(&dir)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    std::thread::sleep(Duration::from_millis(1500));
    std::fs::create_dir_all(dir.join(".vscode")).unwrap();
    std::fs::write(
        dir.join(".vscode/launch.json"),
        format!(
            r#"{{ "configurations": [ {{ "name": "Attach", "type": "python", "request": "attach", "connect": {{ "host": "127.0.0.1", "port": {port} }} }} ] }}"#
        ),
    )
    .unwrap();
    let (wb, cx) = ssh_workbench(cx, &dir);
    let ed = ssh_open_file(&wb, cx, &dir.join("loop.py"));
    ed.update(cx, |e, cx| e.toggle_breakpoint_at(2, cx));
    run(&wb, cx, "Debug: Start Debugging");
    let tick = |cx: &mut VisualTestContext| cx.executor().advance_clock(Duration::from_secs(1));
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        wait_secs(cx, 20, "configuration picker", |cx| {
            tick(cx);
            wb.read_with(cx, |wb, cx| wb.overlay_labels(cx)).is_some_and(|l| l.first().is_some_and(|f| f == "Attach"))
        });
        cx.simulate_keystrokes("enter");
        wait_secs(cx, 90, "stopped over ssh after attaching", |cx| {
            tick(cx);
            wb.read_with(cx, |wb, _| wb.debug_snapshot()).is_some_and(|s| s.0.starts_with("stopped") && s.2.as_ref().is_some_and(|(_, l)| *l == 2))
        });
        wait_secs(cx, 20, "loop variable over ssh", |cx| {
            tick(cx);
            wb.read_with(cx, |wb, _| wb.debug_variables()).iter().any(|(n, _)| n == "value" || n == "i")
        });
    }));
    run(&wb, cx, "Debug: Stop");
    let ended = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        wait_secs(cx, 30, "ended", |cx| {
            tick(cx);
            wb.read_with(cx, |wb, _| wb.debug_snapshot()).is_some_and(|s| s.0 == "ended")
        })
    }));
    std::thread::sleep(Duration::from_millis(500));
    let alive = child.try_wait().ok().flatten().is_none();
    let _ = child.kill();
    let _ = child.wait();
    if let Err(e) = result.and(ended) {
        std::panic::resume_unwind(e);
    }
    assert!(alive, "SSH attach: program must survive detach");
}

/// Delve dials in to the client: over SSH that needs a reverse port forward (`ssh -R`).
#[gpui::test]
fn qa_ssh_debug_go_over_reverse_forward(cx: &mut TestAppContext) {
    let have = |cmd: &str, args: &[&str]| std::process::Command::new(cmd).args(args).output().map(|o| o.status.success()).unwrap_or(false);
    if !(have("go", &["version"]) && have("dlv", &["version"])) {
        crate::test_sshd::skip_missing("go,dlv");
        return;
    }
    let Some(_sshd) = crate::test_sshd::start("qa-remote-go") else { return };
    let dir = qa_dir("qa-remote-go-proj");
    std::fs::write(dir.join("go.mod"), "module demo\n\ngo 1.21\n").unwrap();
    std::fs::write(dir.join("main.go"), "package main\n\nimport \"fmt\"\n\nfunc cube(n int) int {\n\tresult := n * n * n\n\tfmt.Println(\"cube\", result)\n\treturn result\n}\n\nfunc main() {\n\tcube(3)\n}\n").unwrap();
    let (wb, cx) = ssh_workbench(cx, &dir);
    let ed = ssh_open_file(&wb, cx, &dir.join("main.go"));
    ed.update(cx, |e, cx| e.toggle_breakpoint_at(6, cx));
    run(&wb, cx, "Debug: Start Debugging");
    let snap = |cx: &mut VisualTestContext| wb.read_with(cx, |wb, _| wb.debug_snapshot());
    let tick = |cx: &mut VisualTestContext| cx.executor().advance_clock(Duration::from_secs(1));
    wait_secs(cx, 150, "stopped in cube() over ssh", |cx| {
        tick(cx);
        snap(cx).is_some_and(|s| s.0.starts_with("stopped") && s.2.as_ref().is_some_and(|(_, l)| *l == 6))
    });
    wait_secs(cx, 30, "locals", |cx| {
        tick(cx);
        wb.read_with(cx, |wb, _| wb.debug_variables()).iter().any(|(n, v)| n == "result" && v == "27")
    });
    run(&wb, cx, "Debug: Stop");
    wait_secs(cx, 30, "ended", |cx| {
        tick(cx);
        snap(cx).is_some_and(|s| s.0 == "ended")
    });
}

/// js-debug listens on a server port: over SSH the client port is forwarded to it (`ssh -L`),
/// child sessions use the same forward.
#[gpui::test]
fn qa_ssh_debug_node_over_forward(cx: &mut TestAppContext) {
    if std::process::Command::new("node").arg("--version").output().map(|o| !o.status.success()).unwrap_or(true) {
        crate::test_sshd::skip_missing("node");
        return;
    }
    if std::path::Path::new("/tmp/cy-js-debug/js-debug/src/dapDebugServer.js").is_file() && crate::debug_config::find_js_debug().is_none() {
        // SAFETY: only tests that look for js-debug read it.
        unsafe { std::env::set_var("CODEYARD_JS_DEBUG", "/tmp/cy-js-debug") };
    }
    if crate::debug_config::find_js_debug().is_none() {
        return;
    }
    let Some(_sshd) = crate::test_sshd::start("qa-remote-node") else { return };
    let dir = qa_dir("qa-remote-node-proj");
    std::fs::write(dir.join("app.js"), "function double(n) {\n  const result = n * 2;\n  console.log('double', result);\n  return result;\n}\ndouble(21);\n")
        .unwrap();
    let (wb, cx) = ssh_workbench(cx, &dir);
    let ed = ssh_open_file(&wb, cx, &dir.join("app.js"));
    ed.update(cx, |e, cx| e.toggle_breakpoint_at(2, cx));
    run(&wb, cx, "Debug: Start Debugging");
    let snap = |cx: &mut VisualTestContext| wb.read_with(cx, |wb, _| wb.debug_snapshot());
    let tick = |cx: &mut VisualTestContext| cx.executor().advance_clock(Duration::from_secs(1));
    wait_secs(cx, 120, "stopped in double() over ssh", |cx| {
        tick(cx);
        snap(cx).is_some_and(|s| s.0.starts_with("stopped") && s.2.as_ref().is_some_and(|(_, l)| *l == 2))
    });
    wait_secs(cx, 30, "locals", |cx| {
        tick(cx);
        wb.read_with(cx, |wb, _| wb.debug_variables()).iter().any(|(n, v)| n == "result" && v == "42")
    });
    run(&wb, cx, "Debug: Stop");
    wait_secs(cx, 30, "ended", |cx| {
        tick(cx);
        snap(cx).is_some_and(|s| s.0 == "ended")
    });
}

/// `window/showMessageRequest`: the server's question is shown to the user with its options;
/// the choice goes back to the server, Esc returns `null` (the server is never left without a reply).
#[gpui::test]
fn qa_lsp_question_is_asked_and_answered(cx: &mut TestAppContext) {
    if std::process::Command::new("python3").arg("-V").output().is_err() {
        crate::test_sshd::skip_missing("python3");
        return;
    }
    let dir = qa_dir("qa-lspask");
    let script = dir.join("asking-ls.py");
    std::fs::write(
        &script,
        r#"#!/usr/bin/env python3
import sys, json
log = open(sys.argv[1], 'a')
def read():
    headers = {}
    while True:
        line = sys.stdin.buffer.readline()
        if not line:
            sys.exit(0)
        line = line.strip()
        if not line:
            break
        k, v = line.split(b':', 1)
        headers[k.strip().lower()] = v.strip()
    return json.loads(sys.stdin.buffer.read(int(headers[b'content-length'])))
def send(m):
    b = json.dumps(m).encode()
    sys.stdout.buffer.write(b'Content-Length: %d\r\n\r\n' % len(b) + b)
    sys.stdout.buffer.flush()
def ask(i, text):
    send({'jsonrpc': '2.0', 'id': i, 'method': 'window/showMessageRequest',
          'params': {'type': 3, 'message': text, 'actions': [{'title': 'Reload'}, {'title': 'Later'}]}})
while True:
    m = read()
    if m.get('method') == 'initialize':
        send({'jsonrpc': '2.0', 'id': m['id'], 'result': {'capabilities': {'textDocumentSync': 1}}})
    elif m.get('method') == 'textDocument/didOpen':
        ask(901, 'Reload the project?')
    elif m.get('id') == 901 and 'method' not in m:
        log.write(json.dumps(m.get('result')) + '\n'); log.flush()
        ask(902, 'Index again?')
    elif m.get('id') == 902 and 'method' not in m:
        log.write(json.dumps(m.get('result')) + '\n'); log.flush()
"#,
    )
    .unwrap();
    #[cfg(unix)]
    crate::test_files::make_executable(&script);
    let answers = dir.join("answers.log");
    std::fs::write(dir.join("main.c"), "int main(void) { return 0; }\n").unwrap();
    let settings = Settings {
        lsp_servers: vec![("c".into(), Some((script.to_string_lossy().into_owned(), vec![answers.to_string_lossy().into_owned()]).into()))],
        ..Settings::default()
    };
    let (wb, cx) = setup_with(cx, settings);
    open(&wb, &dir.join("main.c"), cx);
    let labels = |cx: &mut VisualTestContext| wb.read_with(cx, |wb, cx| wb.overlay_labels(cx));
    let wait = |cx: &mut VisualTestContext, what: &str, mut done: Box<dyn FnMut(&mut VisualTestContext) -> bool>| {
        let start = std::time::Instant::now();
        while !done(cx) {
            assert!(start.elapsed() < Duration::from_secs(10 * crate::test_sshd::timeout_scale()), "timed out: {what}");
            cx.run_until_parked();
            std::thread::sleep(Duration::from_millis(20));
        }
    };
    wait(cx, "ilk soru", Box::new(move |cx| labels(cx).is_some_and(|l| l == ["Reload", "Later"])));
    cx.simulate_input("Later");
    cx.simulate_keystrokes("enter");
    let read = || std::fs::read_to_string(&answers).unwrap_or_default();
    wait(cx, "ilk yanıt ve ikinci soru", Box::new(move |cx| labels(cx).is_some()));
    assert_eq!(read().lines().next(), Some(r#"{"title": "Later"}"#));
    // Esc: dismissed, so null.
    cx.simulate_keystrokes("escape");
    let start = std::time::Instant::now();
    while read().lines().count() < 2 {
        assert!(start.elapsed() < Duration::from_secs(10 * crate::test_sshd::timeout_scale()), "second reply never arrived");
        cx.run_until_parked();
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(read().lines().nth(1), Some("null"));
}

/// A hung `git status` is abandoned after 20 s and the user is told; later refreshes
/// still run (the lock used to stay closed forever).
#[gpui::test]
fn qa_hung_git_status_is_abandoned_and_refresh_recovers(cx: &mut TestAppContext) {
    let Some(dir) = crate::tests::git_repo("qa-githang") else { return };
    let dir = std::fs::canonicalize(dir).unwrap();
    let (wb, cx) = setup_with(cx, Settings::default());
    wb.update(cx, |wb, _| wb.git_status_hangs = true);
    open(&wb, &dir, cx);
    let tick = |cx: &mut VisualTestContext, s: u64| {
        cx.executor().advance_clock(Duration::from_secs(s));
        cx.run_until_parked();
    };
    tick(cx, 5);
    assert!(wb.read_with(cx, |wb, _| wb.git_status()).is_none(), "no status yet");
    tick(cx, 20);
    assert!(notice(&wb, cx).contains("did not finish"), "{}", notice(&wb, cx));
    // git recovered: the next poll/refresh brings the status.
    wb.update(cx, |wb, cx| {
        wb.git_status_hangs = false;
        wb.refresh_git(0, cx);
    });
    let start = std::time::Instant::now();
    while wb.read_with(cx, |wb, _| wb.git_status()).is_none() {
        assert!(start.elapsed() < Duration::from_secs(10 * crate::test_sshd::timeout_scale()), "refresh got stuck");
        tick(cx, 1);
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// A scripted language server that answers inlay hints, folding ranges, document links,
/// prepareRename and logMessage, and records every method it saw.
fn scripted_server(dir: &Path) -> PathBuf {
    let script = dir.join("scripted-ls.py");
    std::fs::write(
        &script,
        r#"#!/usr/bin/env python3
import sys, json
log = open(sys.argv[1], 'a')
def read():
    headers = {}
    while True:
        line = sys.stdin.buffer.readline()
        if not line:
            sys.exit(0)
        line = line.strip()
        if not line:
            break
        k, v = line.split(b':', 1)
        headers[k.strip().lower()] = v.strip()
    return json.loads(sys.stdin.buffer.read(int(headers[b'content-length'])))
def send(m):
    b = json.dumps(m).encode()
    sys.stdout.buffer.write(b'Content-Length: %d\r\n\r\n' % len(b) + b)
    sys.stdout.buffer.flush()
caps = {
    'textDocumentSync': 1,
    'inlayHintProvider': True,
    'foldingRangeProvider': True,
    'documentLinkProvider': {},
    'renameProvider': {'prepareProvider': True},
    'completionProvider': {'resolveProvider': True},
    'definitionProvider': True,
    'documentHighlightProvider': True,
    'codeLensProvider': {'resolveProvider': True},
    'semanticTokensProvider': {'legend': {'tokenTypes': ['function', 'variable'], 'tokenModifiers': []}, 'full': True},
    'callHierarchyProvider': True,
    'typeHierarchyProvider': True,
    'workspace': {'fileOperations': {'willRename': {'filters': []}}},
}
OTHER = 'file://' + sys.argv[3] if len(sys.argv) > 3 else ''
TARGET = 'file://' + sys.argv[2]
ITEM = {'name': 'main', 'kind': 12, 'uri': TARGET, 'range': {'start': {'line': 2, 'character': 0}, 'end': {'line': 2, 'character': 5}}, 'selectionRange': {'start': {'line': 2, 'character': 4}, 'end': {'line': 2, 'character': 8}}}
while True:
    m = read()
    meth = m.get('method')
    if meth:
        log.write(meth + '\n'); log.flush()
    if meth == 'initialize':
        send({'jsonrpc': '2.0', 'id': m['id'], 'result': {'capabilities': caps}})
    elif meth == 'initialized':
        send({'jsonrpc': '2.0', 'method': 'window/logMessage', 'params': {'type': 3, 'message': 'scripted server ready'}})
    elif meth == 'textDocument/inlayHint':
        send({'jsonrpc': '2.0', 'id': m['id'], 'result': [
            {'position': {'line': 1, 'character': 5}, 'label': ': i32', 'kind': 1},
            {'position': {'line': 1, 'character': 9}, 'label': [{'value': 'x:'}], 'kind': 2}]})
    elif meth == 'textDocument/foldingRange':
        send({'jsonrpc': '2.0', 'id': m['id'], 'result': [{'startLine': 0, 'endLine': 2}, {'startLine': 5, 'endLine': 5}]})
    elif meth == 'textDocument/documentLink':
        send({'jsonrpc': '2.0', 'id': m['id'], 'result': [
            {'range': {'start': {'line': 0, 'character': 0}, 'end': {'line': 0, 'character': 3}}, 'target': 'https://example.com/x'}]})
    elif meth == 'textDocument/codeLens':
        send({'jsonrpc': '2.0', 'id': m['id'], 'result': [
            {'range': {'start': {'line': 0, 'character': 0}, 'end': {'line': 0, 'character': 3}}, 'command': {'title': '2 references', 'command': 'scripted.refs', 'arguments': []}},
            {'range': {'start': {'line': 2, 'character': 0}, 'end': {'line': 2, 'character': 3}}, 'data': 7}]})
    elif meth == 'codeLens/resolve':
        m['params']['command'] = {'title': 'Run test', 'command': 'scripted.run', 'arguments': []}
        send({'jsonrpc': '2.0', 'id': m['id'], 'result': m['params']})
    elif meth == 'workspace/executeCommand':
        send({'jsonrpc': '2.0', 'id': m['id'], 'result': None})
    elif meth == 'textDocument/semanticTokens/full':
        send({'jsonrpc': '2.0', 'id': m['id'], 'result': {'data': [0, 4, 4, 0, 0, 1, 4, 1, 1, 0]}})
    elif meth == 'textDocument/documentHighlight':
        send({'jsonrpc': '2.0', 'id': m['id'], 'result': [
            {'range': {'start': {'line': 1, 'character': 8}, 'end': {'line': 1, 'character': 9}}, 'kind': 1},
            {'range': {'start': {'line': 2, 'character': 11}, 'end': {'line': 2, 'character': 12}}, 'kind': 2}]})
    elif meth == 'textDocument/definition':
        send({'jsonrpc': '2.0', 'id': m['id'], 'result': [{'uri': TARGET, 'range': {'start': {'line': 2, 'character': 4}, 'end': {'line': 2, 'character': 8}}}]})
    elif meth in ('textDocument/prepareCallHierarchy', 'textDocument/prepareTypeHierarchy'):
        send({'jsonrpc': '2.0', 'id': m['id'], 'result': [ITEM]})
    elif meth == 'callHierarchy/incomingCalls':
        send({'jsonrpc': '2.0', 'id': m['id'], 'result': [{'from': ITEM, 'fromRanges': []}]})
    elif meth == 'typeHierarchy/supertypes':
        send({'jsonrpc': '2.0', 'id': m['id'], 'result': [ITEM]})
    elif meth == 'workspace/willRenameFiles':
        send({'jsonrpc': '2.0', 'id': m['id'], 'result': {'changes': {OTHER: [{'range': {'start': {'line': 0, 'character': 0}, 'end': {'line': 0, 'character': 0}}, 'newText': '// imports updated\n'}]}}})
    elif meth == 'textDocument/prepareRename':
        send({'jsonrpc': '2.0', 'id': m['id'], 'result': None})
    elif 'id' in m:
        send({'jsonrpc': '2.0', 'id': m['id'], 'result': None})
"#,
    )
    .unwrap();
    #[cfg(unix)]
    crate::test_files::make_executable(&script);
    script
}

#[gpui::test]
fn lsp_inlay_hints_folding_links_prepare_rename_and_output(cx: &mut TestAppContext) {
    if std::process::Command::new("python3").arg("-V").output().is_err() {
        crate::test_sshd::skip_missing("python3");
        return;
    }
    let dir = qa_dir("lsp-extras");
    let script = scripted_server(&dir);
    let seen = dir.join("seen.log");
    std::fs::write(dir.join("main.c"), "int main(void) {\n    int x = 1;\n    return x;\n}\n\n{\n}\n").unwrap();
    std::fs::write(dir.join("other.c"), "int other;\n").unwrap();
    std::fs::write(dir.join("moveme.c"), "int m;\n").unwrap();
    let settings = Settings {
        lsp_servers: vec![(
            "c".into(),
            Some(
                (
                    script.to_string_lossy().into_owned(),
                    vec![
                        seen.to_string_lossy().into_owned(),
                        dir.join("main.c").to_string_lossy().into_owned(),
                        dir.join("other.c").to_string_lossy().into_owned(),
                    ],
                )
                    .into(),
            ),
        )],
        ..Settings::default()
    };
    let (wb, cx) = setup_with(cx, settings);
    open(&wb, &dir.join("main.c"), cx);
    let ed = wb.read_with(cx, |wb, _| wb.active_editor_entity()).unwrap();
    wait_ticking(cx, "extras", |cx| {
        let (hints, folds, links) = ed.read_with(cx, |e, _| e.lsp_extras_for_test());
        !hints.is_empty() && !folds.is_empty() && links == 1
    });
    let (hints, folds, _) = ed.read_with(cx, |e, _| e.lsp_extras_for_test());
    assert_eq!(hints.get(&1).map(String::as_str), Some(": i32  x:"), "hints on the same line are merged");
    let (lens, sem) = ed.read_with(cx, |e, _| e.lens_and_semantic_for_test());
    assert_eq!(lens, vec![(0, "2 references".to_string()), (2, "Run test".to_string())], "a lens without a command gets resolved");
    assert_eq!(sem, 1, "only function/type roles are kept, variables are skipped");
    assert_eq!(folds, vec![(0, 2)], "single-line range is dropped");
    // The server's folding range folds even without indentation; folding header 0 hides 1-2.
    ed.update(cx, |e, cx| e.toggle_fold_for_test(0, cx));
    assert!(ed.read_with(cx, |e, _| e.fold_headers()).contains(&0));
    // The Output panel shows the server's log.
    assert!(
        wb.read_with(cx, |wb, _| wb.output_lines()).iter().any(|l| l.contains("scripted server ready")),
        "{:?}",
        wb.read_with(cx, |wb, _| wb.output_lines())
    );
    // When prepareRename returns null no input opens, a notification appears.
    ed.update(cx, |e, cx| e.toggle_fold_for_test(0, cx));
    assert!(!ed.read_with(cx, |e, _| e.fold_headers()).contains(&0));
    cx.simulate_keystrokes("down right right right right right");
    wb.update_in(cx, |wb, window, cx| wb.run_command_for_test("Editor: Rename Symbol", window, cx));
    wait_ticking(cx, "prepareRename", |cx| notice(&wb, cx).contains("can't be renamed"));
    assert!(std::fs::read_to_string(&seen).unwrap().contains("textDocument/prepareRename"));
    // Symbol highlight: cursor on `x` (line 1, column 8): the server's two usages come back.
    cx.simulate_keystrokes("cmd-up down end left left left left left left");
    wait_ticking(cx, "documentHighlight", |cx| ed.read_with(cx, |e, _| e.symbol_highlights_for_test().len() == 2));
    // Code lens: cursor on line 0, the command goes to the server.
    cx.simulate_keystrokes("cmd-up");
    wb.update_in(cx, |wb, window, cx| wb.run_command_for_test("Editor: Run Code Lens on This Line", window, cx));
    wait_ticking(cx, "executeCommand", |_| std::fs::read_to_string(&seen).unwrap().contains("workspace/executeCommand"));
    // File move: the server's willRenameFiles edit is applied before the move.
    wb.update_in(cx, |wb, window, cx| {
        wb.move_path_for_test(dir.join("moveme.c").to_string_lossy().into_owned(), dir.join("moved.c").to_string_lossy().into_owned(), window, cx)
    });
    wait_ticking(cx, "willRename edit + move", |_| {
        dir.join("moved.c").exists() && std::fs::read_to_string(dir.join("other.c")).is_ok_and(|t| t.starts_with("// imports updated"))
    });
    assert!(!dir.join("moveme.c").exists());
    // Peek: definition card; Escape closes it.
    cx.simulate_keystrokes("alt-f12");
    wait_ticking(cx, "peek", |cx| ed.read_with(cx, |e, _| e.peek_for_test().is_some()));
    assert_eq!(ed.read_with(cx, |e, _| e.peek_for_test()), Some((2, 4)));
    cx.simulate_keystrokes("escape");
    assert!(ed.read_with(cx, |e, _| e.peek_for_test().is_none()));
    // Call and type hierarchy open a location list.
    wb.update_in(cx, |wb, window, cx| wb.run_command_for_test("Editor: Show Incoming Calls", window, cx));
    wait_ticking(cx, "incoming calls", |cx| wb.read_with(cx, |wb, cx| wb.overlay_query(cx).is_some()));
    cx.simulate_keystrokes("escape");
    wb.update_in(cx, |wb, window, cx| wb.run_command_for_test("Editor: Show Supertypes", window, cx));
    wait_ticking(cx, "supertypes", |cx| wb.read_with(cx, |wb, cx| wb.overlay_query(cx).is_some()));
}

#[gpui::test]
fn diff_inline_toggle_renders_both_modes(cx: &mut TestAppContext) {
    let dir = repo("qa-diffinline");
    std::fs::write(dir.join("src/lib.rs"), (1..=10).map(|i| if i == 3 { "// yeni\n".to_string() } else { format!("// satır {i}\n") }).collect::<String>())
        .unwrap();
    let (wb, cx) = setup(cx);
    open(&wb, &dir, cx);
    wait_ticking(cx, "status", |cx| changes(&wb, cx).is_some_and(|c| !c.is_empty()));
    wb.update_in(cx, |wb, window, cx| wb.open_diff("src/lib.rs".into(), false, window, cx));
    wait_for(cx, "diff", |cx| wb.read_with(cx, |wb, _| wb.diff_view().is_some()));
    let view = wb.read_with(cx, |wb, _| wb.diff_view()).unwrap();
    assert!(!view.read_with(cx, |v, _| v.is_inline()), "side by side by default");
    cx.simulate_keystrokes("cmd-k cmd-i");
    assert!(view.read_with(cx, |v, _| v.is_inline()));
    cx.run_until_parked();
    cx.simulate_keystrokes("cmd-k cmd-i");
    assert!(!view.read_with(cx, |v, _| v.is_inline()));
}

#[gpui::test]
fn terminal_split_shows_two_and_caps_the_count(cx: &mut TestAppContext) {
    let dir = qa_dir("qa-termsplit");
    let (wb, cx) = setup(cx);
    let first = terminal(&wb, &dir, cx);
    wb.update_in(cx, |wb, window, cx| wb.run_command_for_test("Terminal: Split", window, cx));
    // The split asks the shell for its folder first.
    wait_for(cx, "the split", |cx| wb.read_with(cx, |wb, _| wb.terminal_count()) == 2);
    let second = wb.read_with(cx, |wb, _| wb.active_terminal()).unwrap();
    assert_ne!(first, second, "after a split the new terminal is active");
    assert_eq!(wb.read_with(cx, |wb, _| wb.terminal_count()), 2);
    assert_eq!(wb.read_with(cx, |wb, _| wb.split_partner()), Some(first.clone()));
    // Closing the split terminal removes the split.
    wb.update_in(cx, |wb, window, cx| wb.run_command_for_test("Terminal: Close", window, cx));
    wait_for(cx, "closed", |cx| {
        // A newly split shell can still have a startup child on a loaded machine. Respect the
        // normal close confirmation rather than assuming the shell is already idle.
        if cx.has_pending_prompt() {
            cx.simulate_prompt_answer("Close");
        }
        wb.read_with(cx, |wb, _| wb.terminal_count()) == 1
    });
    assert_eq!(wb.read_with(cx, |wb, _| wb.split_partner()), None);
    // At most eight terminals.
    for _ in 0..10 {
        wb.update_in(cx, |wb, window, cx| wb.run_command_for_test("Terminal: New", window, cx));
    }
    assert_eq!(wb.read_with(cx, |wb, _| wb.terminal_count()), 8);
    wb.update_in(cx, |wb, window, cx| wb.run_command_for_test("Terminal: Close", window, cx));
}

/// Dev container: a `docker:<name>` target connects through the same agent protocol (fake docker).
#[gpui::test]
fn qa_docker_target_connects_lists_and_opens_files(cx: &mut TestAppContext) {
    let Some(_docker) = crate::test_sshd::start_fake_docker("qa-docker") else { return };
    assert_eq!(host::ssh::list_containers(), Ok(vec![("fake-box".to_string(), "debian:12".to_string())]));
    let dir = qa_dir("qa-docker-files");
    std::fs::write(dir.join("hello.txt"), "kapsayıcıdan merhaba\n").unwrap();
    let (wb, cx) = setup(cx);
    cx.update(|_, cx| {
        let mut settings = cx.global::<crate::settings::Settings>().clone();
        settings.docker_enabled = true;
        cx.set_global(settings);
    });
    wb.update_in(cx, |wb, window, cx| wb.connect("docker:fake-box".into(), window, cx));
    let tick = |cx: &mut VisualTestContext| cx.executor().advance_clock(Duration::from_secs(1));
    wait_secs(cx, 30, "connected", |cx| {
        tick(cx);
        wb.read_with(cx, |wb, _| wb.conn_for_test()) == "connected"
    });
    // Forwarding is ssh only, so nothing scans the container's ports.
    assert!(!wb.read_with(cx, |wb, _| wb.ports_ready()));
    open(&wb, &dir.join("hello.txt"), cx);
    let ed = wb.read_with(cx, |wb, _| wb.active_editor_entity()).expect("remote file opened");
    assert_eq!(ed.read_with(cx, |e, _| e.text()), "kapsayıcıdan merhaba\n");
    // Port forwarding is ssh only: a clear error.
    assert!(host::ssh::forward_port("docker:fake-box", 8080, 8080, false).is_err());
}

// --- Workspace Trust ------------------------------------------------------

/// Window whose user settings turn the trust gate on (tests default to "off"; a real install has it on).
fn setup_trusting<'a>(cx: &'a mut TestAppContext, name: &str) -> (Entity<Workbench>, &'a mut VisualTestContext) {
    let config = qa_dir(&format!("{name}-config"));
    std::fs::write(config.join("settings.jsonc"), r#"{ "security.workspaceTrust.enabled": true }"#).unwrap();
    cx.update(|cx| {
        crate::bind_keys(cx);
        cx.set_global(ActiveTheme(theme::DARK));
        cx.set_global(Settings { workspace_trust: true, ..Settings::default() });
    });
    let (wb, cx) = cx.add_window_view(move |window, cx| Workbench::with_state(state::State::in_memory().ok(), Some(config), window, cx));
    wb.update_in(cx, |wb, window, cx| window.focus(&wb.focus_handle(cx)));
    (wb, cx)
}

/// Language servers can run a project's code (build scripts, macros, node_modules): in an untrusted folder none starts and
/// the user is told once how to turn them on; trusting the folder starts them without reopening the file.
#[gpui::test]
fn language_servers_wait_for_trust(cx: &mut TestAppContext) {
    let config = qa_dir("qa-lsp-trust-config");
    std::fs::write(
        config.join("settings.jsonc"),
        r#"{ "security.workspaceTrust.enabled": true, "lsp.enabled": true, "lsp.servers": { "rust": { "command": "codeyard-no-such-language-server" } } }"#,
    )
    .unwrap();
    cx.update(|cx| {
        crate::bind_keys(cx);
        cx.set_global(ActiveTheme(theme::DARK));
        cx.set_global(Settings { workspace_trust: true, lsp_enabled: true, ..Settings::default() });
    });
    let cfg = config.clone();
    let (wb, cx) = cx.add_window_view(move |window, cx| Workbench::with_state(state::State::in_memory().ok(), Some(cfg), window, cx));
    wb.update_in(cx, |wb, window, cx| window.focus(&wb.focus_handle(cx)));
    let dir = qa_dir("qa-lsp-trust-project");
    std::fs::write(dir.join("main.rs"), "fn main() {}\n").unwrap();
    open(&wb, &dir, cx);
    open(&wb, &dir.join("main.rs"), cx);
    let n = notice(&wb, cx);
    assert!(n.contains("Language servers are off") && n.contains("Trust This Folder"), "{n}");
    cx.run_until_parked();
    assert!(!notice(&wb, cx).contains("isn't available"), "no server start was even attempted");
    // Trusting starts them (here the configured command does not exist, which proves a start was attempted).
    wb.update_in(cx, |wb, window, cx| wb.run_command_for_test("Workspace: Trust This Folder", window, cx));
    wait_for(cx, "server start attempted after trust", |cx| notice(&wb, cx).contains("isn't available"));
}

/// Running or debugging tests runs the project's code (a Rust test is built first, which runs its build script): in an
/// untrusted folder both ask first, and cancelling runs nothing.
#[gpui::test]
fn tests_wait_for_trust_before_building_or_running(cx: &mut TestAppContext) {
    let (wb, cx) = setup_trusting(cx, "qa-tests-trust");
    let dir = qa_dir("qa-tests-trust-project");
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::write(dir.join("Cargo.toml"), "[package]\nname = \"t\"\nversion = \"0.1.0\"\nedition = \"2021\"\n").unwrap();
    std::fs::write(dir.join("src/lib.rs"), "#[cfg(test)]\nmod tests {\n    #[test]\n    fn one() {}\n}\n").unwrap();
    open(&wb, &dir, cx);
    run(&wb, cx, "View: Testing");
    wait_secs(cx, 30, "tests found", |cx| wb.read_with(cx, |wb, _| !wb.testing_rows_for_test().is_empty()));
    let lib = dir.join("src/lib.rs").to_string_lossy().into_owned();
    wb.update_in(cx, |wb, window, cx| wb.open_file_in_root(lib.clone(), window, cx));
    cx.run_until_parked();
    let ed = wb.read_with(cx, |wb, _| wb.active_editor_entity()).unwrap();
    ed.update_in(cx, |e, window, cx| e.go_to_line(4, window, cx));
    for command in ["Test: Debug Test at Cursor", "Test: Run All Tests"] {
        run(&wb, cx, command);
        assert!(cx.has_pending_prompt(), "{command} must ask in an untrusted folder");
        cx.simulate_prompt_answer("Cancel");
        cx.run_until_parked();
        assert!(!wb.read_with(cx, |wb, _| wb.testing_busy_for_test()), "{command}: nothing runs after Cancel");
    }
    assert!(!dir.join("target").exists(), "nothing was built");
}

/// A language server can ask for edits anywhere; only files under the open folders are written, and it is told when nothing was.
#[gpui::test]
fn workspace_edits_outside_the_open_folders_are_dropped(cx: &mut TestAppContext) {
    let dir = qa_dir("qa-edit-roots");
    let outside = qa_dir("qa-edit-outside").join("hosts");
    std::fs::write(&outside, "keep\n").unwrap();
    std::fs::write(dir.join("a.txt"), "keep\n").unwrap();
    let (wb, cx) = setup(cx);
    open(&wb, &dir, cx);
    let edit_of = |path: &Path| {
        let change = serde_json::json!([{ "range": { "start": { "line": 0, "character": 0 }, "end": { "line": 0, "character": 4 } }, "newText": "gone" }]);
        serde_json::json!({ "changes": { crate::lsp::path_to_uri(&path.to_string_lossy()): change } })
    };
    let applied = wb.update(cx, |wb, cx| wb.apply_workspace_edit_for_test(&edit_of(&outside), cx));
    cx.run_until_parked();
    assert!(!applied, "everything was outside: the server hears applied:false");
    assert_eq!(std::fs::read_to_string(&outside).unwrap(), "keep\n");
    assert!(notice(&wb, cx).contains("outside the open folders"), "{}", notice(&wb, cx));
    let applied = wb.update(cx, |wb, cx| wb.apply_workspace_edit_for_test(&edit_of(&dir.join("a.txt")), cx));
    wait_for(cx, "edit written inside the folder", |_| std::fs::read_to_string(dir.join("a.txt")).is_ok_and(|t| t == "gone\n"));
    assert!(applied);
}

/// A response is whatever the server sent, tokens included: the file behind its tab is owner-only and goes away with the tab or the app.
#[cfg(unix)]
#[gpui::test]
fn the_response_file_is_private_and_removed_at_quit(cx: &mut TestAppContext) {
    use std::os::unix::fs::PermissionsExt;
    let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
    let (wb, cx) = setup_trusting(cx, "qa-response-private");
    let r = crate::http_client::Response {
        status: 200,
        reason: "OK".into(),
        headers: vec![("Content-Type".into(), "application/json".into())],
        body: br#"{"token":"abc"}"#.to_vec(),
        millis: 1,
        size: 15,
    };
    let f = qa_dir("qa-response-private-src").join("a.http");
    std::fs::write(&f, "GET http://a/\n").unwrap();
    open(&wb, &f, cx);
    wb.update_in(cx, |wb, window, cx| wb.show_response_for_test(r.clone(), window, cx));
    let path = wb.read_with(cx, |wb, _| wb.response_tab_for_test()).expect("response file");
    let path = Path::new(&path);
    assert!(path.ends_with("response.json"), "{path:?}");
    wait_for(cx, "response tab", |cx| wb.read_with(cx, |wb, cx| wb.active_editor_entity().is_some_and(|e| e.read(cx).path == path.to_string_lossy())));
    assert_eq!(mode(path) & 0o077, 0, "file readable by others: {:o}", mode(path));
    assert_eq!(mode(path.parent().unwrap()) & 0o077, 0, "folder readable by others");
    // The next response replaces it; the request file was never the response tab.
    open(&wb, &f, cx);
    wb.update_in(cx, |wb, window, cx| wb.show_response_for_test(r, window, cx));
    cx.run_until_parked();
    assert!(f.exists(), "sending again must not delete the .http file");
    wb.update(cx, |wb, _| wb.discard_response_on_quit_for_test());
    assert!(!path.exists() && f.exists());
}

/// Trust is per host: a folder trusted on this computer says nothing about the same path on a server.
#[gpui::test]
fn trusting_a_local_folder_does_not_trust_the_same_path_on_a_server(cx: &mut TestAppContext) {
    let dir = qa_dir("qa-trust-per-host");
    let root = dir.to_string_lossy().into_owned();
    let (wb, cx) = setup_trusting(cx, "qa-trust-per-host");
    open(&wb, &dir, cx);
    wb.update_in(cx, |wb, window, cx| wb.run_command_for_test("Workspace: Trust This Folder", window, cx));
    assert!(wb.read_with(cx, |wb, cx| wb.is_trusted_root_for_test(&root, cx)));
    let (req_r, req_w) = std::io::pipe().unwrap();
    let (resp_r, resp_w) = std::io::pipe().unwrap();
    std::thread::spawn(move || agent::serve(req_r, resp_w));
    let remote: std::sync::Arc<dyn host::Host> = std::sync::Arc::new(host::RemoteHost::from_streams(Box::new(resp_r), Box::new(req_w), "ssh://other").unwrap());
    wb.update_in(cx, |wb, window, cx| wb.attach_host(remote, "other".into(), window, cx));
    assert!(!wb.read_with(cx, |wb, cx| wb.is_trusted_root_for_test(&root, cx)), "same path, different host");
}

/// A repository must not be able to pick the program the editor runs just by being opened:
/// `terminal.shell`, `lsp.servers` and the trust switch itself come from the user, or from a
/// folder the user trusted. Harmless keys (tab size) still apply. "Trust This Folder" lifts it.
#[gpui::test]
fn untrusted_project_cannot_choose_a_shell_or_a_language_server(cx: &mut TestAppContext) {
    let dir = qa_dir("qa-trust-settings");
    std::fs::create_dir_all(dir.join(".codeyard")).unwrap();
    std::fs::write(
        dir.join(".codeyard/settings.jsonc"),
        r#"{ "terminal.shell": "/tmp/evil-shell", "lsp.servers": { "c": { "command": "/tmp/evil-ls" } },
             "security.workspaceTrust.enabled": false, "editor.tabSize": 3 }"#,
    )
    .unwrap();
    let (wb, cx) = setup_trusting(cx, "qa-trust-settings");
    open(&wb, &dir, cx);
    let s = cx.update(|_, cx| cx.global::<Settings>().clone());
    assert_eq!(s.terminal_shell, None, "terminal.shell from an untrusted project");
    assert!(s.lsp_servers.is_empty(), "lsp.servers from an untrusted project");
    assert!(s.workspace_trust, "a project cannot switch the trust gate off");
    assert_eq!(s.tab_size, Some(3), "safe keys still apply");

    run(&wb, cx, "Workspace: Trust This Folder");
    cx.run_until_parked();
    let s = cx.update(|_, cx| cx.global::<Settings>().clone());
    assert_eq!(s.terminal_shell.as_deref(), Some("/tmp/evil-shell"), "after trusting the folder");
    assert_eq!(s.lsp_servers.len(), 1);
    assert!(s.workspace_trust, "the switch itself stays user-only even in a trusted folder");
}

/// Running a task of an untrusted folder asks first; Cancel runs nothing; trusting runs it.
#[gpui::test]
fn untrusted_folder_asks_before_running_a_task(cx: &mut TestAppContext) {
    let dir = qa_dir("qa-trust-task");
    std::fs::create_dir_all(dir.join(".codeyard")).unwrap();
    std::fs::write(dir.join(".codeyard/tasks.jsonc"), r#"[ { "label": "build", "command": "touch ran.txt" } ]"#).unwrap();
    let (wb, cx) = setup_trusting(cx, "qa-trust-task");
    open(&wb, &dir, cx);
    wb.update(cx, |wb, _| wb.set_shell("/bin/sh"));

    run(&wb, cx, "Tasks: Run Build Task");
    cx.run_until_parked();
    assert!(cx.has_pending_prompt(), "must ask before running a task of an untrusted folder");
    cx.simulate_prompt_answer("Cancel");
    cx.run_until_parked();
    assert_eq!(wb.read_with(cx, |wb, _| wb.terminal_count()), 0, "Cancel starts nothing");
    assert!(!dir.join("ran.txt").exists());

    run(&wb, cx, "Tasks: Run Build Task");
    cx.run_until_parked();
    cx.simulate_prompt_answer("Trust Folder and Continue");
    wait_ticking(cx, "the task ran after trusting", |_| dir.join("ran.txt").exists());

    // Trusted now: no second question.
    run(&wb, cx, "Tasks: Run Build Task");
    cx.run_until_parked();
    assert!(!cx.has_pending_prompt(), "a trusted folder is not asked again");
}

/// Turning the gate off (user setting) restores the old behaviour: nothing is asked.
#[gpui::test]
fn trust_gate_can_be_turned_off_by_the_user(cx: &mut TestAppContext) {
    let dir = qa_dir("qa-trust-off");
    std::fs::create_dir_all(dir.join(".codeyard")).unwrap();
    std::fs::write(dir.join(".codeyard/tasks.jsonc"), r#"[ { "label": "build", "command": "true" } ]"#).unwrap();
    let (wb, cx) = setup_with(cx, Settings::default()); // test default: gate off
    open(&wb, &dir, cx);
    wb.update(cx, |wb, _| wb.set_shell("/bin/sh"));
    run(&wb, cx, "Tasks: Run Build Task");
    cx.run_until_parked();
    assert!(!cx.has_pending_prompt());
    assert_eq!(wb.read_with(cx, |wb, _| wb.terminal_count()), 1);
}
