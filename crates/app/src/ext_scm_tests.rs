//! Source control providers of code extensions end to end (#144): the probe's toy provider (probe-scm.wasm, a real
//! component in codeyard-exthost) claims a folder with a `.toyvcs` folder. Its changes show in Source Control and in
//! the gutter (AC1), the input box's button runs its commit command (AC2), and over SSH the program that command
//! starts runs on the remote host (AC3). An untrusted folder is not claimed. Skipped when codeyard-exthost is not built
//! (`cargo build -p ext-host`).

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use ext_api::value::FileState;
use gpui::{Entity, TestAppContext, VisualTestContext};

use crate::git::LineMark;
use crate::ui::ActiveTheme;
use crate::workbench::Workbench;

/// The probe with the toy provider (scripts/build-exthost-fixtures.sh).
const PROBE: &[u8] = include_bytes!("../../ext-host/testdata/probe-scm.wasm");

const MANIFEST: &str = r#"[extension]
id = "you.probe"
version = "1.0.0"
name = "Probe"
license = "MIT"

[code]
entry = "Cargo.toml"

# The probe imports `net`, `auth` and `secrets` too (for the exthost's own tests).
[permissions]
read = [".toyvcs/**"]
net = ["api.example.com"]
auth = ["github"]
secrets = true
run = ["echo"]
why = "Tests a source control provider."

[[commands]]
id = "scm-commit"
title = "Toy: Commit"

[[commands]]
id = "scm-stage"
title = "Toy: Stage"

[[commands]]
id = "scm-unstage"
title = "Toy: Unstage"

[[commands]]
id = "scm-discard"
title = "Toy: Discard"

[[commands]]
id = "scm-refresh"
title = "Toy: Refresh"

[[scm]]
id = "toy"
label = "Toy"
markers = [".toyvcs"]
"#;

fn skip() -> bool {
    if let Err(why) = crate::ext_runtime::code_runs_here(&"tests") {
        eprintln!("skipped: {why}");
        return true;
    }
    false
}

fn temp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("codeyard-extscm-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d.canonicalize().unwrap()
}

/// A folder the toy provider claims: `a.txt` changed, `b.txt` added and staged, `c.txt` unchanged.
fn workspace(name: &str) -> String {
    let ws = temp(&format!("{name}-ws"));
    std::fs::create_dir_all(ws.join(".toyvcs/base")).unwrap();
    std::fs::write(ws.join(".toyvcs/changes"), "changes M a.txt\nstaged A b.txt\n").unwrap();
    std::fs::write(ws.join(".toyvcs/base/a.txt"), "one\ntwo\nthree\n").unwrap();
    std::fs::write(ws.join(".toyvcs/base/c.txt"), "same\n").unwrap();
    std::fs::write(ws.join("a.txt"), "one\nTWO\nthree\n").unwrap();
    std::fs::write(ws.join("b.txt"), "new\n").unwrap();
    std::fs::write(ws.join("c.txt"), "same\n").unwrap();
    crate::ui::local_path(&ws)
}

fn install(cfg: &Path) {
    let entries = vec![
        ext_pkg::Entry { path: ext_manifest::FILE_NAME.into(), data: MANIFEST.as_bytes().to_vec(), executable: false },
        ext_pkg::Entry { path: "Cargo.toml".into(), data: Vec::new(), executable: false },
        ext_pkg::Entry { path: ext_pkg::COMPONENT.into(), data: PROBE.to_vec(), executable: false },
    ];
    let (bytes, _) = ext_pkg::write(entries).expect("the probe passes the import check");
    crate::ext::store(cfg).install(&bytes, ext_core::Source::File { path: "/downloads/probe.cyx".into() }).expect("installs");
}

fn setup<'a>(cx: &'a mut TestAppContext, name: &str, settings: &str) -> (Entity<Workbench>, &'a mut VisualTestContext) {
    let cfg = temp(name);
    install(&cfg);
    std::fs::write(cfg.join("settings.jsonc"), settings).unwrap();
    std::fs::write(cfg.join("keybindings.jsonc"), "[]").unwrap();
    cx.update(|cx| {
        crate::bind_keys(cx);
        cx.set_global(ActiveTheme(theme::DARK));
        cx.set_global(crate::settings::Settings { lsp_enabled: false, ..Default::default() });
    });
    let (wb, cx) = cx.add_window_view(move |window, cx| Workbench::with_state(state::State::in_memory().ok(), Some(cfg), window, cx));
    wb.update_in(cx, |wb, window, cx| wb.reload_config_for_test(window, cx));
    cx.run_until_parked();
    (wb, cx)
}

/// Run the window until `done`, or fail after 20 s. Each round moves the test clock on a little, for timers.
fn until(cx: &mut VisualTestContext, what: &str, done: impl Fn(&mut VisualTestContext) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while Instant::now() < deadline {
        cx.run_until_parked();
        if done(cx) {
            return;
        }
        cx.executor().advance_clock(Duration::from_millis(100));
        std::thread::sleep(Duration::from_millis(20));
    }
    panic!("timed out waiting for {what}");
}

/// Open the folder and wait for the provider's first state.
fn open(wb: &Entity<Workbench>, cx: &mut VisualTestContext, root: &str) {
    wb.update(cx, |wb, cx| wb.set_root(root.to_string(), cx));
    let wb2 = wb.clone();
    until(cx, "the provider's state", move |cx| wb2.read_with(cx, |wb, cx| wb.ext_scm_sources_for_test(cx).first().is_some_and(|s| s.state.is_some())));
}

fn answer(wb: &Entity<Workbench>, cx: &mut VisualTestContext) -> String {
    wb.read_with(cx, |wb, _| wb.extension_answer_for_test("you.probe")).unwrap_or_default()
}

#[gpui::test]
fn a_providers_changes_show_in_source_control_and_in_the_gutter(cx: &mut TestAppContext) {
    if skip() {
        return;
    }
    let root = workspace("ac1");
    let (wb, cx) = setup(cx, "ac1", "{}");
    open(&wb, cx, &root);

    let source = wb.read_with(cx, |wb, cx| wb.ext_scm_sources_for_test(cx)).remove(0);
    assert_eq!(source.root, root);
    assert!(source.title.starts_with("Toy · "), "{}", source.title);
    let state = source.state.unwrap();
    assert_eq!(state.head.as_deref(), Some("toy main"));
    let r = root.as_str();
    let files: Vec<(String, String, FileState)> =
        state.groups.iter().flat_map(|g| g.resources.iter().map(move |res| (g.id.clone(), crate::scm::ext_relative(r, &res.uri), res.state))).collect();
    assert_eq!(files, [("staged".into(), "b.txt".into(), FileState::Added), ("changes".into(), "a.txt".into(), FileState::Modified)]);
    assert_eq!(state.input.placeholder, "Message (toy)");
    // Screen readers reach the folder's section, its box and its files, as they do git's.
    let access = wb.update_in(cx, |wb, window, cx| wb.scm_panel().read(cx).a11y_view(window, cx));
    assert_eq!(access.root, None, "git has nothing to say about this folder");
    assert_eq!(access.ext.len(), 1);
    assert_eq!(access.ext[0].open, ["staged", "changes"]);

    // The gutter: a.txt's second line differs from the provider's original; c.txt, which it does not list, has no marks.
    wb.update_in(cx, |wb, window, cx| wb.open_file_in_root(format!("{root}/a.txt"), window, cx));
    let wb2 = wb.clone();
    let a = format!("{root}/a.txt");
    until(cx, "the gutter's marks", move |cx| {
        wb2.read_with(cx, |wb, cx| wb.editor_for_path_for_test(&a, cx))
            .is_some_and(|ed| ed.read_with(cx, |e, _| e.git_marks().get(1).copied().flatten() == Some(LineMark::Modified)))
    });
    let marks = wb.read_with(cx, |wb, cx| wb.editor_for_path_for_test(&format!("{root}/a.txt"), cx)).unwrap().read_with(cx, |e, _| e.git_marks());
    assert_eq!(marks.iter().filter(|m| m.is_some()).count(), 1, "{marks:?}");
    wb.update_in(cx, |wb, window, cx| wb.open_file_in_root(format!("{root}/c.txt"), window, cx));
    cx.run_until_parked();
    let c = wb.read_with(cx, |wb, cx| wb.editor_for_path_for_test(&format!("{root}/c.txt"), cx)).unwrap();
    assert!(c.read_with(cx, |e, _| e.git_marks().iter().all(Option::is_none)));

    // A change on disk asks again: the provider now lists c.txt too.
    std::fs::write(Path::new(&root).join(".toyvcs/changes"), "changes M a.txt\nchanges M c.txt\nstaged A b.txt\n").unwrap();
    wb.update_in(cx, |wb, window, cx| wb.refresh_ext_scm_for_test(window, cx));
    let wb2 = wb.clone();
    until(cx, "the new state", move |cx| {
        wb2.read_with(cx, |wb, cx| {
            wb.ext_scm_sources_for_test(cx)[0].state.as_ref().is_some_and(|s| s.groups.iter().map(|g| g.resources.len()).sum::<usize>() == 3)
        })
    });
}

#[gpui::test]
fn commit_from_the_input_box_runs_the_providers_command(cx: &mut TestAppContext) {
    if skip() {
        return;
    }
    let root = workspace("ac2");
    let (wb, cx) = setup(cx, "ac2", "{}");
    open(&wb, cx, &root);

    // An empty message: the provider's check says so and nothing runs.
    wb.update(cx, |wb, cx| wb.ext_scm_accept_for_test(&root, "", cx));
    let wb2 = wb.clone();
    until(cx, "the check's message", move |cx| {
        wb2.read_with(cx, |wb, cx| wb.ext_scm_sources_for_test(cx)[0].validation.as_deref() == Some("Write a message first"))
    });
    assert_eq!(answer(&wb, cx), "");

    // A message: the provider's commit command runs with it and every file it lists, and the box empties.
    wb.update(cx, |wb, cx| wb.ext_scm_accept_for_test(&root, "Fix the toy", cx));
    let wb2 = wb.clone();
    until(cx, "the commit", move |cx| answer(&wb2, cx).starts_with("committed Fix the toy 2"));
    let wb2 = wb.clone();
    let r = root.clone();
    until(cx, "an empty box", move |cx| wb2.read_with(cx, |wb, cx| wb.scm_panel().read(cx).ext_message(&r, cx).is_empty()));
    assert_eq!(wb.read_with(cx, |wb, cx| wb.ext_scm_sources_for_test(cx)[0].validation.clone()), None);

    // A file's button runs its command with that file; a group's with all of the group's.
    wb.update(cx, |wb, cx| wb.ext_scm_press_for_test(&root, "a.txt", "Stage", cx));
    let wb2 = wb.clone();
    until(cx, "the stage", move |cx| answer(&wb2, cx) == "scm-stage 1");
    wb.update(cx, |wb, cx| wb.ext_scm_press_for_test(&root, "staged", "Unstage All", cx));
    let wb2 = wb.clone();
    until(cx, "the unstage", move |cx| answer(&wb2, cx) == "scm-unstage 1");
}

/// The local host, presented as a remote one, noting each program it starts.
struct Remote(Arc<dyn host::Host>, Arc<Mutex<Vec<String>>>);

impl host::Host for Remote {
    fn info(&self) -> &proto::HostInfo {
        self.0.info()
    }
    fn label(&self) -> String {
        "ssh://fixture".into()
    }
    fn is_remote(&self) -> bool {
        true
    }
    fn call(&self, request: proto::Request) -> host::Result<proto::Response> {
        self.0.call(request)
    }
    fn search(&self, q: proto::SearchQuery) -> host::Result<host::SearchStream> {
        self.0.search(q)
    }
    fn hash_tree(&self, r: &str, i: Vec<String>, g: bool, l: u32, o: Option<Vec<String>>) -> host::Result<host::HashStream> {
        self.0.hash_tree(r, i, g, l, o)
    }
    fn cancel(&self, id: u32) {
        self.0.cancel(id)
    }
    fn pty_spawn(&self, c: u16, r: u16, d: Option<String>, s: Option<String>) -> host::Result<host::Pty> {
        self.0.pty_spawn(c, r, d, s)
    }
    fn pty_input(&self, id: u32, data: Vec<u8>) {
        self.0.pty_input(id, data)
    }
    fn pty_resize(&self, id: u32, c: u16, r: u16) {
        self.0.pty_resize(id, c, r)
    }
    fn pty_kill(&self, id: u32) {
        self.0.pty_kill(id)
    }
    fn watch(&self, roots: Vec<String>) {
        self.0.watch(roots)
    }
    fn fs_events(&self) -> std::sync::mpsc::Receiver<host::FsChanges> {
        self.0.fs_events()
    }
    fn status_events(&self) -> std::sync::mpsc::Receiver<host::ConnStatus> {
        self.0.status_events()
    }
    fn proc_spawn(&self, p: &str, a: Vec<String>, c: Option<String>) -> host::Result<host::Proc> {
        self.1.lock().unwrap().push(format!("{p} {}", a.join(" ")));
        self.0.proc_spawn(p, a, c)
    }
    fn proc_input(&self, id: u32, data: Vec<u8>) {
        self.0.proc_input(id, data)
    }
    fn proc_kill(&self, id: u32) {
        self.0.proc_kill(id)
    }
    fn exec(&self, program: &str, args: &[&str], cwd: Option<&str>, stdin: Option<Vec<u8>>) -> host::Result<host::ExecOutput> {
        self.0.exec(program, args, cwd, stdin)
    }
}

#[gpui::test]
fn over_ssh_the_providers_program_runs_on_the_remote_host(cx: &mut TestAppContext) {
    if skip() || cfg!(windows) {
        return;
    }
    let root = workspace("ac3");
    let (wb, cx) = setup(cx, "ac3", "{}");
    let started = Arc::new(Mutex::new(Vec::new()));
    wb.update(cx, |wb, _| wb.set_host_for_test(Arc::new(Remote(Arc::new(host::local()), started.clone()))));
    open(&wb, cx, &root);

    // The provider's state came through the remote host too: it read its files there.
    wb.update(cx, |wb, cx| wb.ext_scm_accept_for_test(&root, "Over ssh", cx));
    // The first process on a remote host asks (§6.3); the program then runs there.
    until(cx, "the remote-process dialog", |cx| cx.has_pending_prompt());
    let (title, detail) = cx.pending_prompt().expect("a dialog");
    assert!(title.contains("ssh://fixture"), "{title}");
    assert!(detail.contains("echo"), "{detail}");
    cx.simulate_prompt_answer("Allow for this session");
    let wb2 = wb.clone();
    until(cx, "the commit", move |cx| answer(&wb2, cx).starts_with("committed Over ssh 2"));
    assert!(answer(&wb, cx).contains("Exit(Some(0))"), "{}", answer(&wb, cx));
    let started = started.lock().unwrap().clone();
    assert!(started.iter().any(|s| s.contains("echo") && s.contains("Over ssh")), "{started:?}");
}

#[gpui::test]
fn an_untrusted_folder_is_not_claimed_until_it_is_trusted(cx: &mut TestAppContext) {
    if skip() {
        return;
    }
    let root = workspace("untrusted");
    let (wb, cx) = setup(cx, "untrusted", r#"{"security.workspaceTrust.enabled": true}"#);
    wb.update(cx, |wb, cx| wb.set_root(root.clone(), cx));
    for _ in 0..10 {
        cx.run_until_parked();
        cx.executor().advance_clock(Duration::from_millis(100));
    }
    assert!(!wb.read_with(cx, |wb, cx| wb.is_trusted_root_for_test(&root, cx)));
    assert!(wb.read_with(cx, |wb, cx| wb.ext_scm_sources_for_test(cx)).is_empty());
    assert!(!wb.read_with(cx, |wb, _| wb.extension_running_for_test("you.probe")), "the provider did not start");

    wb.update_in(cx, |wb, window, cx| wb.trust_root_for_test(&root, window, cx));
    let wb2 = wb.clone();
    until(cx, "the provider's state", move |cx| wb2.read_with(cx, |wb, cx| wb.ext_scm_sources_for_test(cx).first().is_some_and(|s| s.state.is_some())));
}
