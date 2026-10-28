use super::*;

fn fixture(name: &str) -> (Server, std::path::PathBuf, mpsc::Receiver<Event>) {
    let root = crate::qa_workbench_tests::fresh(name);
    std::fs::write(root.join("index.html"), "<!doctype html><link rel=stylesheet href=/style.css><h1>Hello</h1>").unwrap();
    std::fs::write(root.join("style.css"), "body { color: red; }").unwrap();
    let (server, rx) = Server::start(Arc::new(host::local()), crate::ui::local_path(&root)).unwrap();
    (server, root, rx)
}

pub(crate) fn request(server: &Server, method: &str, path: &str, authorized: bool, authority: Option<&str>) -> String {
    let mut stream = TcpStream::connect(server.addr).unwrap();
    stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    let cookie = if authorized { format!("Cookie: codeyard_preview_{}={}\r\n", &server.shared.token[..16], server.shared.token) } else { String::new() };
    write!(stream, "{method} {path} HTTP/1.1\r\nHost: {}\r\n{cookie}\r\n", authority.unwrap_or(&server.shared.authority)).unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).unwrap();
    response
}

#[test]
fn web_preview_http_auth_root_assets_head_and_shutdown() {
    let (mut server, _, _rx) = fixture("preview-http");
    assert!(request(&server, "GET", "/", false, None).starts_with("HTTP/1.1 403"));
    let entry = server.url("index.html").strip_prefix(&format!("http://{}", server.addr)).unwrap().to_string();
    let bootstrap = request(&server, "GET", &entry, false, None);
    assert!(bootstrap.starts_with("HTTP/1.1 302"));
    assert!(bootstrap.contains("HttpOnly; SameSite=Strict; Path=/"));
    assert!(bootstrap.contains("Location: /index.html\r\n"));
    let html = request(&server, "GET", "/", true, None);
    assert!(html.starts_with("HTTP/1.1 200"), "{html}");
    assert!(html.contains("<h1>Hello</h1>"));
    assert!(html.contains("/__codeyard_live.js"));
    assert!(request(&server, "GET", "/style.css", true, None).contains("Content-Type: text/css"));
    let head = request(&server, "HEAD", "/style.css", true, None);
    assert!(head.starts_with("HTTP/1.1 200"));
    assert!(head.split_once("\r\n\r\n").unwrap().1.is_empty());
    assert!(request(&server, "POST", "/index.html", true, None).starts_with("HTTP/1.1 405"));
    assert!(request(&server, "GET", "/index.html", true, Some("attacker.example")).starts_with("HTTP/1.1 403"));
    let cross_site = format!("{}\r\nSec-Fetch-Site: cross-site", server.shared.authority);
    assert!(request(&server, "GET", "/index.html", true, Some(&cross_site)).starts_with("HTTP/1.1 403"));
    let origin = format!("{}\r\nOrigin: http://127.0.0.1:1", server.shared.authority);
    assert!(request(&server, "GET", "/index.html", true, Some(&origin)).starts_with("HTTP/1.1 403"));
    let addr = server.addr;
    server.stop();
    assert!(TcpStream::connect(addr).is_err());
}

#[test]
fn web_preview_paths_reject_traversal_dotfiles_special_names_and_unknown_types() {
    let (server, root, _rx) = fixture("preview-paths");
    std::fs::write(root.join("secret.pem"), "private").unwrap();
    std::fs::write(root.join(".env"), "secret").unwrap();
    std::fs::create_dir(root.join("nested")).unwrap();
    std::fs::write(root.join("nested/index.html"), "nested").unwrap();
    std::fs::write(root.join("türk çe.html"), "unicode").unwrap();
    for path in ["/../index.html", "/%2e%2e/index.html", "/.env", "/.git/config", "/x\\index.html", "/index.html:stream", "/%00.html", "/%ZZ", "/secret.pem"] {
        let response = request(&server, "GET", path, true, None);
        assert!(response.starts_with("HTTP/1.1 403"), "{path}: {response}");
        assert!(!response.contains("private"));
    }
    assert!(request(&server, "GET", "/nested", true, None).contains("Location: /nested/"));
    assert!(request(&server, "GET", "/nested/", true, None).contains("nested\n<script"));
    assert!(request(&server, "GET", &format!("/{}", encode_path("türk çe.html")), true, None).contains("unicode"));
    assert!(request(&server, "GET", "/missing.html", true, None).starts_with("HTTP/1.1 404"));
    let huge = std::fs::File::create(root.join("huge.html")).unwrap();
    huge.set_len(proto::MAX_FILE + 1).unwrap();
    assert!(request(&server, "GET", "/huge.html", true, None).starts_with("HTTP/1.1 413"));
}

#[test]
fn web_preview_tracks_served_files_and_distinguishes_css_reload() {
    let (server, root, rx) = fixture("preview-changes");
    request(&server, "GET", "/style.css", true, None);
    assert!(matches!(rx.recv_timeout(Duration::from_secs(1)).unwrap(), Event::Watch));
    assert_eq!(server.watched_paths(), [crate::ui::local_path(&root.join("style.css"))]);
    server.changed(&crate::ui::local_path(&root.join("unrelated.txt")));
    server.changed(&crate::ui::local_path(&root.join("style.css")));
    let changes = request(&server, "GET", "/__codeyard_changes?since=0", true, None);
    let json: serde_json::Value = serde_json::from_str(changes.split_once("\r\n\r\n").unwrap().1).unwrap();
    assert_eq!(json["revision"], 1);
    assert_eq!(json["cssOnly"], true);
    request(&server, "GET", "/index.html", true, None);
    server.changed(&crate::ui::local_path(&root.join("index.html")));
    let changes = request(&server, "GET", "/__codeyard_changes?since=1", true, None);
    assert!(changes.contains("\"cssOnly\":false"));
    assert!(changes.contains("/index.html"));
}

#[cfg(unix)]
#[test]
fn web_preview_never_follows_symlinks_outside_the_served_root() {
    let (server, root, _rx) = fixture("preview-links");
    let outside = crate::qa_workbench_tests::fresh("preview-private");
    std::fs::write(outside.join("secret.html"), "private contents").unwrap();
    std::os::unix::fs::symlink(&outside, root.join("linked")).unwrap();
    std::os::unix::fs::symlink(outside.join("secret.html"), root.join("direct.html")).unwrap();
    for path in ["/linked/secret.html", "/direct.html"] {
        assert!(request(&server, "GET", path, true, None).starts_with("HTTP/1.1 403"));
    }
}

#[test]
fn web_preview_module_is_user_only_and_off_by_default() {
    use crate::settings::{Settings, parse_layers_trusted};
    assert!(!Settings::default().web_preview_enabled);
    for trusted in [true, false] {
        let (s, _) = parse_layers_trusted("{}", &[("project", r#"{"modules.webPreview.enabled":true}"#, false)], trusted).unwrap();
        assert!(!s.web_preview_enabled);
        let (s, _) = parse_layers_trusted(r#"{"modules.webPreview.enabled":true}"#, &[], trusted).unwrap();
        assert!(s.web_preview_enabled);
    }
    assert_eq!(crate::settings_page::category_of("modules.webPreview.enabled"), "Modules");
    assert!(crate::workbench::settings_ui::entry("modules.webPreview.enabled").is_some());
}

#[test]
fn web_preview_changes_does_not_hold_the_lock_during_response_io() {
    struct CheckWriter<'state>(&'state Shared, Vec<u8>);
    impl Write for CheckWriter<'_> {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            assert!(self.0.changes.try_lock().is_ok(), "HTTP response I/O must not block file changes");
            self.1.extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let (server, _, _rx) = fixture("preview-lock");
    let mut writer = CheckWriter(&server.shared, Vec::new());
    reply_changes(&mut writer, &server.shared, Some(0), false);
    let response = String::from_utf8(writer.1).unwrap();
    assert!(response.starts_with("HTTP/1.1 200"), "{response}");
    assert!(response.contains("\"revision\":0"), "{response}");
}
