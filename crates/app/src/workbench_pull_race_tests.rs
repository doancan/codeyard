use super::*;
use crate::qa_integration_tests::{git_in, repo};
use gpui::TestAppContext;
use std::sync::atomic::{AtomicUsize, Ordering};

/// Run a second real fetch after the PR fetch finishes, before returning its reply to the UI worker.
struct FetchRaceHost {
    local: Arc<dyn Host>,
    intervening_fetches: AtomicUsize,
}

impl Host for FetchRaceHost {
    fn info(&self) -> &proto::HostInfo {
        self.local.info()
    }
    fn label(&self) -> String {
        self.local.label()
    }
    fn is_remote(&self) -> bool {
        false
    }
    fn call(&self, request: proto::Request) -> host::Result<proto::Response> {
        self.local.call(request)
    }
    fn search(&self, query: proto::SearchQuery) -> host::Result<host::SearchStream> {
        self.local.search(query)
    }
    fn hash_tree(&self, root: &str, ignore: Vec<String>, gitignore: bool, limit: u32, only: Option<Vec<String>>) -> host::Result<host::HashStream> {
        self.local.hash_tree(root, ignore, gitignore, limit, only)
    }
    fn cancel(&self, id: u32) {
        self.local.cancel(id)
    }
    fn pty_spawn(&self, cols: u16, rows: u16, cwd: Option<String>, shell: Option<String>) -> host::Result<host::Pty> {
        self.local.pty_spawn(cols, rows, cwd, shell)
    }
    fn pty_input(&self, id: u32, data: Vec<u8>) {
        self.local.pty_input(id, data)
    }
    fn pty_resize(&self, id: u32, cols: u16, rows: u16) {
        self.local.pty_resize(id, cols, rows)
    }
    fn pty_kill(&self, id: u32) {
        self.local.pty_kill(id)
    }
    fn watch(&self, paths: Vec<String>) {
        self.local.watch(paths)
    }
    fn fs_events(&self) -> std::sync::mpsc::Receiver<host::FsChanges> {
        self.local.fs_events()
    }
    fn status_events(&self) -> std::sync::mpsc::Receiver<host::ConnStatus> {
        self.local.status_events()
    }
    fn proc_spawn(&self, program: &str, args: Vec<String>, cwd: Option<String>) -> host::Result<host::Proc> {
        self.local.proc_spawn(program, args, cwd)
    }
    fn proc_input(&self, id: u32, data: Vec<u8>) {
        self.local.proc_input(id, data)
    }
    fn proc_kill(&self, id: u32) {
        self.local.proc_kill(id)
    }
    fn exec(&self, program: &str, args: &[&str], cwd: Option<&str>, stdin: Option<Vec<u8>>) -> host::Result<host::ExecOutput> {
        let result = self.local.exec(program, args, cwd, stdin)?;
        if program == "git" && args.first() == Some(&"fetch") && args.iter().any(|arg| arg.contains("pull/42/head")) && result.ok() {
            git_in(std::path::Path::new(cwd.unwrap()), &["fetch", "-q", "origin", "refs/heads/interloper"]);
            self.intervening_fetches.fetch_add(1, Ordering::SeqCst);
        }
        Ok(result)
    }
}

#[gpui::test]
fn another_fetch_cannot_change_which_commit_a_pull_request_checkout_opens(cx: &mut TestAppContext) {
    let root = repo("pull-fetch-race");
    let origin = repo("pull-fetch-race-origin");
    std::fs::write(origin.join("feature.txt"), "PR commit\n").unwrap();
    git_in(&origin, &["add", "-A"]);
    git_in(&origin, &["commit", "-q", "-m", "PR"]);
    let expected = git_in(&origin, &["rev-parse", "HEAD"]);
    git_in(&origin, &["update-ref", "refs/pull/42/head", "HEAD"]);
    std::fs::write(origin.join("interloper.txt"), "unrelated fetch\n").unwrap();
    git_in(&origin, &["add", "-A"]);
    git_in(&origin, &["commit", "-q", "-m", "interloper"]);
    let unrelated = git_in(&origin, &["rev-parse", "HEAD"]);
    git_in(&origin, &["update-ref", "refs/heads/interloper", "HEAD"]);
    git_in(&root, &["remote", "add", "origin", &crate::ui::local_path(&origin)]);
    let host = Arc::new(FetchRaceHost { local: Arc::new(host::local()), intervening_fetches: AtomicUsize::new(0) });
    let (wb, cx) = crate::qa_workbench_tests::setup(cx);
    wb.update(cx, |wb, cx| {
        wb.remote.host = host.clone();
        wb.vcs.status = Some(Arc::new(git::Status { root: crate::ui::local_path(&root), ..Default::default() }));
        wb.checkout_pull_for_test("origin".into(), 42, cx);
    });
    cx.run_until_parked();
    assert_eq!(host.intervening_fetches.load(Ordering::SeqCst), 1, "a real second fetch overwrote the shared FETCH_HEAD");
    assert_eq!(git_in(&root, &["rev-parse", "FETCH_HEAD"]), unrelated);
    assert_eq!(git_in(&root, &["rev-parse", "HEAD"]), expected, "checkout must use the PR's own fetched commit");
    assert_eq!(git_in(&root, &["branch", "--show-current"]).trim(), "pr-42");
    assert!(!root.join("interloper.txt").exists());
    assert!(git_in(&root, &["for-each-ref", "refs/codeyard/pull"]).is_empty(), "the completed operation removes its private ref");
    assert!(wb.read_with(cx, |wb, _| matches!(&wb.notice, Some(Notice::Info(message)) if message.contains("Checked out #42"))));
}
