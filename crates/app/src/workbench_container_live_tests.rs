//! Opt-in real SSH/container acceptance test. Never selects an existing service for mutation.
use super::*;
use crate::container_history::Tab;

#[derive(serde::Deserialize)]
struct Fixture {
    host: String,
    id: String,
    name: String,
    root: String,
    token: String,
}

fn wait(cx: &mut gpui::VisualTestContext, label: &str, mut done: impl FnMut(&mut gpui::VisualTestContext) -> bool) {
    crate::qa_integration_tests::wait_secs(cx, 90, label, |cx| {
        cx.executor().advance_clock(Duration::from_millis(50));
        done(cx)
    });
    eprintln!("LIVE PASS: {label}");
}

fn preview(wb: &Entity<Workbench>, cx: &mut gpui::VisualTestContext, root: &str) {
    use crate::web_preview::tests::request;
    let host = wb.read_with(cx, |wb, _| wb.remote.host.clone());
    let html = format!("{root}/preview-smoke.html");
    let css = format!("{root}/preview-smoke.css");
    host.write(&html, b"<!doctype html><link rel=stylesheet href=/preview-smoke.css><h1>Remote preview</h1>".to_vec(), None).unwrap();
    host.write(&css, b"h1 { color: red; }".to_vec(), None).unwrap();
    cx.update(|_, cx| cx.global_mut::<Settings>().web_preview_enabled = true);
    wb.update_in(cx, |wb, window, cx| wb.open_file_in_root(html.clone(), window, cx));
    wait(cx, "preview source opened", |cx| wb.read_with(cx, |wb, cx| wb.active_editor().is_some_and(|ed| ed.read(cx).path == html)));
    wb.update_in(cx, |wb, window, cx| wb.open_web_preview(window, cx));
    wait(cx, "agent-backed local HTTP listener", |cx| wb.read_with(cx, |wb, _| wb.chrome.web_preview.server.is_some()));
    wb.read_with(cx, |wb, _| {
        let server = wb.chrome.web_preview.server.as_ref().unwrap();
        let response = request(server, "GET", "/preview-smoke.html", true, None);
        assert!(response.starts_with("HTTP/1.1 200"), "{response}");
        assert!(response.contains("Remote preview") && response.contains("/__codeyard_live.js"));
        assert!(request(server, "GET", "/preview-smoke.css", true, None).contains("color: red"));
    });
    wait(cx, "agent watches served remote assets", |cx| wb.read_with(cx, |wb, _| wb.watched.contains(&css)));
    // Let the agent apply its asynchronous watch update before an external write.
    for _ in 0..20 {
        cx.run_until_parked();
        std::thread::sleep(Duration::from_millis(25));
    }
    let before = wb.read_with(cx, |wb, _| request(wb.chrome.web_preview.server.as_ref().unwrap(), "GET", "/__codeyard_changes", true, None));
    let before: serde_json::Value = serde_json::from_str(before.split_once("\r\n\r\n").unwrap().1).unwrap();
    host.write(&css, b"h1 { color: blue; }".to_vec(), None).unwrap();
    let route = format!("/__codeyard_changes?since={}", before["revision"]);
    wait(cx, "remote CSS save reaches browser refresh endpoint", |cx| {
        wb.read_with(cx, |wb, _| {
            let response = request(wb.chrome.web_preview.server.as_ref().unwrap(), "GET", &route, true, None);
            response.contains("/preview-smoke.css") && response.contains("\"cssOnly\":true")
        })
    });
    wb.update(cx, |wb, cx| wb.stop_web_preview(cx));
    host.delete(&html).unwrap();
    host.delete(&css).unwrap();
}

#[gpui::test]
#[ignore = "requires target/live-container-test.json and a disposable labeled remote container; see docs/testing.md"]
fn live_ssh_container_acceptance(cx: &mut gpui::TestAppContext) {
    let config = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/live-container-test.json");
    let f: Fixture = serde_json::from_slice(&std::fs::read(config).expect("provision the explicit live fixture first")).unwrap();
    assert_eq!(f.token.len(), 32);
    assert!(f.token.bytes().all(|b| b.is_ascii_hexdigit()));
    assert_eq!(f.name, format!("codeyard-smoke-{}", f.token));
    assert_eq!(f.root, format!("/tmp/{}", f.name));
    assert_eq!(f.id.len(), 64);
    assert!(f.id.bytes().all(|b| b.is_ascii_hexdigit()));
    let (wb, cx) = crate::qa_workbench_tests::setup(cx);
    cx.update(|_, cx| cx.global_mut::<Settings>().docker_enabled = true);
    wb.update_in(cx, |wb, window, cx| {
        wb.remote.pending_path = Some(f.root.clone());
        wb.connect(f.host.clone(), window, cx);
    });
    wait(cx, "real SSH agent connection", |cx| {
        wb.read_with(cx, |wb, _| {
            if let Conn::Failed { message, .. } = &wb.remote.conn {
                panic!("SSH connection: {message}");
            }
            matches!(&wb.remote.conn, Conn::Connected { alias } if alias == &f.host)
        })
    });
    let source = wb.read_with(cx, |wb, _| wb.remote.host.clone());
    preview(&wb, cx, &f.root);
    wb.update(cx, |wb, cx| {
        wb.chrome.panel = Some(Panel::Containers);
        wb.open_container_dashboard(cx);
    });
    wait(cx, "automatic inventory and two live resource samples", |cx| {
        wb.read_with(cx, |wb, _| {
            let m = &wb.remote.docker.monitor;
            if let Some(error) = &m.error {
                panic!("inventory: {error}");
            }
            m.containers.iter().any(|c| c.id == f.id) && m.history.get(&f.id).is_some_and(|samples| samples.len() >= 2)
        })
    });
    wb.update(cx, |wb, cx| {
        wb.remote.docker.monitor.paused = true;
        wb.select_container(f.id.clone(), Tab::Details, cx);
    });
    wait(cx, "selected container inspect", |cx| wb.read_with(cx, |wb, _| wb.remote.docker.monitor.browser.detail.is_some()));
    wb.read_with(cx, |wb, _| {
        let m = &wb.remote.docker.monitor;
        assert!(m.warning.is_none(), "{:?}", m.warning);
        let d = m.browser.detail.as_ref().unwrap();
        assert_eq!(d["Config"]["Labels"]["codeyard.smoke"].as_str(), Some(f.token.as_str()));
        assert_eq!(d["Name"].as_str(), Some(format!("/{}", f.name).as_str()));
        assert!(d["Mounts"].as_array().unwrap().iter().any(|mount| mount["Destination"] == "/workspace"));
        let sample = m.history[&f.id].back().unwrap();
        assert!(sample.cpu.is_some() && sample.memory.is_some() && sample.disk.is_some() && sample.network.is_some());
    });
    wb.update(cx, |wb, cx| wb.select_container(f.id.clone(), Tab::Logs, cx));
    wait(cx, "logs read through Codeyard agent", |cx| wb.read_with(cx, |wb, _| wb.remote.docker.monitor.browser.logs.contains("codeyard-live-log")));
    let target = wb.read_with(cx, |wb, _| wb.remote.docker.monitor.context.as_ref().unwrap().container_alias(&f.id).unwrap());
    for cycle in 0..2 {
        wb.update_in(cx, |wb, window, cx| {
            wb.remote.pending_path = Some("/workspace".into());
            wb.connect(target.clone(), window, cx);
        });
        wait(cx, "container agent attach", |cx| {
            wb.read_with(cx, |wb, _| {
                if let Conn::Failed { message, .. } = &wb.remote.conn {
                    panic!("container connection: {message}");
                }
                matches!(&wb.remote.conn, Conn::Connected { alias } if alias == &target)
            })
        });
        wb.update_in(cx, |wb, window, cx| wb.open_file_in_root("/workspace/hello.txt".into(), window, cx));
        wait(cx, "remote file opened in editor", |cx| wb.read_with(cx, |wb, _| wb.active_editor().is_some()));
        let editor = wb.read_with(cx, |wb, _| wb.active_editor().unwrap().clone());
        if cycle == 0 {
            assert_eq!(editor.read_with(cx, |ed, _| ed.text()), "initial content\n");
            cx.simulate_input("saved through Codeyard\n");
            assert!(wb.read_with(cx, |wb, cx| wb.any_dirty(cx)));
            cx.simulate_keystrokes("cmd-s");
            wait(cx, "editor save completed", |cx| !wb.read_with(cx, |wb, cx| wb.any_dirty(cx)));
        }
        let remote = wb.read_with(cx, |wb, _| wb.remote.host.clone());
        let (bytes, _) = remote.read("/workspace/hello.txt").unwrap();
        assert!(String::from_utf8(bytes).unwrap().contains("saved through Codeyard"));
        preview(&wb, cx, "/workspace");
        wb.update_in(cx, |wb, window, cx| wb.open_terminal(window, cx));
        wait(cx, "container terminal created", |cx| wb.read_with(cx, |wb, _| !wb.terminals.is_empty()));
        let terminal = wb.read_with(cx, |wb, _| wb.terminals[0].clone());
        crate::qa_integration_tests::type_line(cx, "echo codeyard-terminal-$((40+2))");
        wait(cx, "terminal command executed inside container", |cx| {
            terminal.read_with(cx, |t, _| t.screen_text().lines().any(|line| line.trim() == "codeyard-terminal-42"))
        });
        wb.update_in(cx, |wb, window, cx| wb.return_from_container(window, cx));
        wb.read_with(cx, |wb, cx| {
            assert!(Arc::ptr_eq(&wb.remote.host, &source));
            assert_eq!(wb.root(cx).as_deref(), Some(f.root.as_str()));
            assert!(matches!(&wb.remote.conn, Conn::Connected { alias } if alias == &f.host));
        });
        eprintln!("LIVE PASS: original SSH workspace restored (cycle {cycle})");
        if cycle == 0 {
            let command = crate::docker::Command::docker(None, &["restart", "--time", "1", &f.id]);
            crate::container_monitor::execute(source.as_ref(), &command, &std::sync::atomic::AtomicBool::new(false)).unwrap();
            eprintln!("LIVE PASS: disposable container restarted");
        }
    }
    wb.update(cx, |wb, cx| wb.disconnect(cx));
    assert!(wb.read_with(cx, |wb, _| matches!(wb.remote.conn, Conn::Local)));
    eprintln!("LIVE PASS: disconnected cleanly");
}
