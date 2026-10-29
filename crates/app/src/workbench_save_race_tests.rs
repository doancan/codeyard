use super::*;
use gpui::TestAppContext;
use std::sync::atomic::{AtomicBool, Ordering};

struct SaveRaceHost {
    local: Arc<dyn Host>,
    target: String,
    injected: AtomicBool,
}

impl Host for SaveRaceHost {
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
        if matches!(&request, proto::Request::Write { path, .. } | proto::Request::WriteNew { path, .. } if path == &self.target)
            && !self.injected.swap(true, Ordering::SeqCst)
        {
            std::fs::write(&self.target, "created by another process\n").unwrap();
        }
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
}

#[gpui::test]
fn save_as_preserves_a_destination_created_after_its_existence_check(cx: &mut TestAppContext) {
    check_save_race(cx, false);
}

#[gpui::test]
fn save_local_copy_preserves_a_destination_created_after_its_existence_check(cx: &mut TestAppContext) {
    check_save_race(cx, true);
}

fn check_save_race(cx: &mut TestAppContext, local_copy: bool) {
    let root = crate::qa_workbench_tests::fresh(&format!("save-publication-race-{local_copy}"));
    let from = crate::ui::local_path(&root.join("source.txt"));
    let target = crate::ui::local_path(&root.join("copy.txt"));
    std::fs::write(&from, "document being saved\n").unwrap();
    let host = Arc::new(SaveRaceHost { local: Arc::new(host::local()), target: target.clone(), injected: AtomicBool::new(false) });
    let (wb, cx) = crate::qa_workbench_tests::setup(cx);
    wb.update_in(cx, |wb, window, cx| {
        wb.remote.host = host.clone();
        wb.remote.local_host = host.clone();
        wb.replace_root(crate::ui::local_path(&root), cx);
        wb.open_file(from.clone(), window, cx);
    });
    cx.run_until_parked();
    wb.update_in(cx, |wb, window, cx| {
        if local_copy {
            wb.save_local_copy_to(from.clone(), target.clone(), cx);
        } else {
            wb.save_as_to(from.clone(), "copy.txt".into(), window, cx);
        }
    });
    cx.run_until_parked();
    assert!(host.injected.load(Ordering::SeqCst));
    assert_eq!(std::fs::read_to_string(&target).unwrap(), "created by another process\n", "saving must never overwrite a newly occupied destination");
    assert_eq!(std::fs::read_to_string(&from).unwrap(), "document being saved\n");
    assert!(wb.read_with(cx, |wb, cx| wb.tabs.editors.iter().any(|editor| editor.read(cx).path == from)));
    assert!(wb.read_with(cx, |wb, _| matches!(&wb.notice, Some(Notice::Error(message)) if message.contains("already exists"))));
}
