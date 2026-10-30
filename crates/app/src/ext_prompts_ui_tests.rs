//! Run-time guards of code extensions in a window, end to end with the probe extension (a real component in
//! codeyard-exthost): the first clipboard read and the first remote process per session ask (§6.3), the resident-memory
//! cap stops an exthost that grows past it (macOS and Linux), and 100 installed code extensions start nothing at
//! startup. Skipped when codeyard-exthost is not built (`cargo build -p ext-host`).

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use ext_api::value::Why;
use gpui::{Entity, TestAppContext, VisualTestContext};

use crate::ui::ActiveTheme;
use crate::workbench::Workbench;

/// The probe with `clipboard`, `run` and `hold` (scripts/build-exthost-fixtures.sh).
const PROBE: &[u8] = include_bytes!("../../ext-host/testdata/probe-guards.wasm");

const MANIFEST: &str = r#"[extension]
id = "you.probe"
version = "1.0.0"
name = "Probe"
license = "MIT"

[code]
entry = "Cargo.toml"

[permissions]
read = ["a.txt"]
net = ["api.example.com"]
auth = ["github", "acme"]
secrets = true
clipboard = "read"
run = ["echo"]
why = "Tests the run-time prompts."

[[commands]]
id = "clipboard"
title = "Probe: Clipboard"

[[commands]]
id = "run"
title = "Probe: Run"

[[commands]]
id = "echo"
title = "Probe: Echo"

[[commands]]
id = "hold"
title = "Probe: Hold"

[[oauth]]
id = "acme"
name = "Acme"
authorize-url = "https://acme.example.com/oauth/authorize"
token-url = "https://acme.example.com/oauth/token"
client-id = "cy-123"
scopes = ["read"]
hosts = ["api.acme.example.com"]
"#;

fn config_dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("codeyard-extprompts-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn install(cfg: &Path, manifest: &str) {
    let entries = vec![
        ext_pkg::Entry { path: ext_manifest::FILE_NAME.into(), data: manifest.as_bytes().to_vec(), executable: false },
        ext_pkg::Entry { path: "Cargo.toml".into(), data: Vec::new(), executable: false },
        ext_pkg::Entry { path: ext_pkg::COMPONENT.into(), data: PROBE.to_vec(), executable: false },
    ];
    let (bytes, _) = ext_pkg::write(entries).expect("the probe passes the import check");
    crate::ext::store(cfg).install(&bytes, ext_core::Source::File { path: "/downloads/probe.cyx".into() }).expect("installs");
}

fn setup<'a>(cx: &'a mut TestAppContext, cfg: &Path) -> (Entity<Workbench>, &'a mut VisualTestContext) {
    cx.update(|cx| {
        crate::bind_keys(cx);
        cx.set_global(ActiveTheme(theme::DARK));
        cx.set_global(crate::settings::Settings { lsp_enabled: false, ..Default::default() });
    });
    let dir = cfg.to_path_buf();
    let (wb, cx) = cx.add_window_view(move |window, cx| Workbench::with_state(state::State::in_memory().ok(), Some(dir), window, cx));
    wb.update_in(cx, |wb, window, cx| wb.reload_config_for_test(window, cx));
    cx.run_until_parked();
    (wb, cx)
}

/// Run the window until `done`, or fail after 20 s. Each round moves the test clock on a little, for timers.
fn until(cx: &mut VisualTestContext, done: impl Fn(&mut VisualTestContext) -> bool, what: &str) {
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

fn answer(wb: &Entity<Workbench>, cx: &mut VisualTestContext) -> String {
    wb.read_with(cx, |wb, _| wb.extension_answer_for_test("you.probe")).unwrap_or_default()
}

/// Run a palette command and wait for the probe's answer to it, or for a dialog.
fn run(wb: &Entity<Workbench>, cx: &mut VisualTestContext, label: &str) -> String {
    let before = answer(wb, cx);
    wb.update_in(cx, |wb, window, cx| wb.run_command_for_test(label, window, cx));
    let wb2 = wb.clone();
    until(cx, move |cx| answer(&wb2, cx) != before || cx.has_pending_prompt(), label);
    answer(wb, cx)
}

/// Answer the open dialog and wait for the probe's answer.
fn reply(wb: &Entity<Workbench>, cx: &mut VisualTestContext, button: &str) -> String {
    let before = answer(wb, cx);
    cx.simulate_prompt_answer(button);
    let wb2 = wb.clone();
    until(cx, move |cx| answer(&wb2, cx) != before, button);
    answer(wb, cx)
}

fn skip() -> bool {
    if !crate::ext_runtime::code_runs_for_test() {
        return true;
    }
    false
}

#[gpui::test]
fn the_first_clipboard_read_asks_once_per_session(cx: &mut TestAppContext) {
    if skip() {
        return;
    }
    let cfg = config_dir("clipboard");
    install(&cfg, MANIFEST);
    let (wb, cx) = setup(cx, &cfg);
    cx.write_to_clipboard(gpui::ClipboardItem::new_string("copied text".into()));

    // The first read waits for the dialog, which names the extension, its publisher and what it wants.
    assert_eq!(run(&wb, cx, "Probe: Clipboard"), "");
    let (title, detail) = cx.pending_prompt().expect("a dialog");
    assert!(title.contains("Probe") && title.contains("clipboard"), "{title}");
    assert!(detail.contains("you.probe") && detail.contains("you, installed from /downloads/probe.cyx"), "{detail}");
    assert_eq!(reply(&wb, cx, "Allow for this session"), r#"text Some("copied text")"#);
    assert_eq!(wb.read_with(cx, |wb, _| wb.clipboard_answer_for_test("you.probe")), Some(true));

    // The second read in the session does not ask.
    cx.write_to_clipboard(gpui::ClipboardItem::new_string("second".into()));
    assert_eq!(run(&wb, cx, "Probe: Clipboard"), r#"text Some("second")"#);
    assert!(!cx.has_pending_prompt());
}

#[gpui::test]
fn denying_the_clipboard_says_revoked_by_user_and_is_listed(cx: &mut TestAppContext) {
    if skip() {
        return;
    }
    let cfg = config_dir("clipboard-deny");
    install(&cfg, MANIFEST);
    let (wb, cx) = setup(cx, &cfg);
    cx.write_to_clipboard(gpui::ClipboardItem::new_string("private".into()));

    run(&wb, cx, "Probe: Clipboard");
    assert!(cx.has_pending_prompt());
    assert_eq!(reply(&wb, cx, "Deny"), "denied clipboard Why::RevokedByUser");
    let denials = wb.read_with(cx, |wb, _| wb.denials_for_test("you.probe"));
    assert!(denials.iter().any(|d| d.1 == "clipboard" && d.2 == Why::RevokedByUser), "{denials:?}");

    // The answer holds for the session: refused again without asking.
    let n = denials.len();
    wb.update_in(cx, |wb, window, cx| wb.run_command_for_test("Probe: Echo", window, cx));
    let wb2 = wb.clone();
    until(cx, move |cx| answer(&wb2, cx) == "null", "the echo");
    assert_eq!(run(&wb, cx, "Probe: Clipboard"), "denied clipboard Why::RevokedByUser");
    assert!(!cx.has_pending_prompt());
    assert_eq!(wb.read_with(cx, |wb, _| wb.denials_for_test("you.probe")).len(), n + 1);
}

/// The local host, presented as a remote one.
struct Remote(Arc<dyn host::Host>);

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
fn the_first_process_on_a_remote_host_asks_once_per_session(cx: &mut TestAppContext) {
    if skip() || cfg!(windows) {
        return;
    }
    let cfg = config_dir("remote");
    install(&cfg, MANIFEST);
    let (wb, cx) = setup(cx, &cfg);

    // On the local host a declared program runs without asking.
    assert!(run(&wb, cx, "Probe: Run").contains("Exit(Some(0))"), "{}", answer(&wb, cx));
    assert!(!cx.has_pending_prompt());

    wb.update(cx, |wb, _| wb.set_host_for_test(Arc::new(Remote(Arc::new(host::local())))));
    assert_eq!(run(&wb, cx, "Probe: Echo"), "null");
    run(&wb, cx, "Probe: Run");
    let (title, detail) = cx.pending_prompt().expect("a dialog");
    assert!(title.contains("ssh://fixture"), "{title}");
    assert!(detail.contains("echo") && detail.contains("you, installed from /downloads/probe.cyx"), "{detail}");
    assert!(reply(&wb, cx, "Allow for this session").contains("Exit(Some(0))"), "{}", answer(&wb, cx));

    // Once allowed, the next process on that host does not ask.
    assert_eq!(run(&wb, cx, "Probe: Echo"), "null");
    assert!(run(&wb, cx, "Probe: Run").contains("Exit(Some(0))"));
    assert!(!cx.has_pending_prompt());
}

#[gpui::test]
fn denying_a_remote_process_refuses_it(cx: &mut TestAppContext) {
    if skip() || cfg!(windows) {
        return;
    }
    let cfg = config_dir("remote-deny");
    install(&cfg, MANIFEST);
    let (wb, cx) = setup(cx, &cfg);
    wb.update(cx, |wb, _| wb.set_host_for_test(Arc::new(Remote(Arc::new(host::local())))));
    run(&wb, cx, "Probe: Run");
    assert!(cx.has_pending_prompt());
    assert_eq!(reply(&wb, cx, "Deny"), "denied run Why::RevokedByUser");
    let denials = wb.read_with(cx, |wb, _| wb.denials_for_test("you.probe"));
    assert!(denials.iter().any(|d| d.1 == "run" && d.2 == Why::RevokedByUser), "{denials:?}");
}

#[gpui::test]
fn an_exthost_over_its_memory_cap_is_stopped_as_a_crash(cx: &mut TestAppContext) {
    if skip() || !crate::ext_memory::CHECKED_HERE {
        return;
    }
    let cfg = config_dir("memory");
    install(&cfg, MANIFEST);
    let (wb, cx) = setup(cx, &cfg);
    wb.update(cx, |wb, _| wb.set_extension_memory_cap_for_test(ext_manifest::limits::EXTHOST_MEMORY_BYTES, Duration::from_millis(100)));
    for round in 1..=3 {
        // The cap: what the idle exthost holds, plus 32 MB. `hold` keeps 64 MB more, well under the guest's own limit.
        assert_eq!(run(&wb, cx, "Probe: Echo"), "null");
        let pid = wb.read_with(cx, |wb, _| wb.extension_pid_for_test("you.probe")).expect("running");
        let idle = crate::ext_memory::resident_bytes(pid).expect("readable");
        wb.update(cx, |wb, _| wb.set_extension_memory_cap_for_test(idle + (32 << 20), Duration::from_millis(100)));
        // The answer may not arrive: the check can stop it between growing and answering.
        wb.update_in(cx, |wb, window, cx| wb.run_command_for_test("Probe: Hold", window, cx));
        let wb2 = wb.clone();
        until(cx, move |cx| wb2.read_with(cx, |wb, _| wb.extension_pid_for_test("you.probe").is_none()), "the exthost to be stopped");
        let notice = wb.read_with(cx, |wb, _| wb.notice_for_test()).map(|n| n.1).unwrap_or_default();
        if round < 3 {
            assert!(notice.contains("you.probe") && notice.contains("MB of memory, over the limit"), "{notice}");
            // The next round's echo must change the answer.
            wb.update(cx, |wb, _| wb.clear_extension_answer_for_test("you.probe"));
        } else {
            // Three crashes within a minute turn it off.
            assert!(notice.contains("crashed three times"), "{notice}");
        }
        let log = wb.read_with(cx, |wb, _| wb.extension_log_for_test("you.probe"));
        assert!(log.iter().any(|l| l.contains("memory:") && l.contains("over the cap")), "{log:?}");
    }
    assert!(wb.read_with(cx, |wb, _| wb.extension_disabled_for_test("you.probe")));
}

/// §10: 100 installed, inactive code extensions start no process and send nothing at startup.
#[gpui::test]
fn a_hundred_installed_code_extensions_start_nothing(cx: &mut TestAppContext) {
    if skip() {
        return;
    }
    let cfg = config_dir("hundred");
    for i in 0..100 {
        install(&cfg, &MANIFEST.replace("id = \"you.probe\"", &format!("id = \"you.probe{i}\"")));
    }
    let (wb, cx) = setup(cx, &cfg);
    cx.executor().advance_clock(Duration::from_secs(5));
    cx.run_until_parked();
    let active = crate::ext::current().active.iter().filter(|e| e.manifest.code.is_some()).count();
    assert_eq!(active, 100);
    assert_eq!(wb.read_with(cx, |wb, _| wb.extension_processes_for_test()), 0);
    // Nothing was compiled either: compiling is the first step of a start.
    assert!(!crate::ext::data_root(&cfg).join("compiled").exists());
}
