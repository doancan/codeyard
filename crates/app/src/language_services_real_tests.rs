//! Opt-in acceptance with real pinned servers. Ordinary tests never fetch packages or touch a user profile.

use std::path::Path;
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

use host::Host;
use serde_json::json;

use crate::language_service_install::tests::Fixture;
use crate::language_service_install::{self as installer, Installed, Progress};
use crate::language_services::Service;
use crate::lsp::{Client, ServerEvent};

struct Running(Arc<Client>);
impl Drop for Running {
    fn drop(&mut self) {
        self.0.shutdown();
    }
}

fn probe(host: Arc<dyn Host>, installed: &Installed, project: &Path) {
    std::fs::create_dir_all(project).unwrap();
    std::fs::write(project.join("tsconfig.json"), r#"{"compilerOptions":{"strict":true,"target":"es2022"}}"#).unwrap();
    let cases = [
        (
            Service::TypeScript,
            "typescript",
            "ts",
            "const count: number = \"wrong\";\n",
            "const value = Math.",
            "function   add(a:number,b:number){return a+b}",
            "abs",
        ),
        (Service::Html, "html", "html", "<style>a { color: ; }</style>", "<di", "<div><span>hello</span></div>", "div"),
        (Service::Css, "css", "css", "a { color: ; }", "a { col", "a{color:red}", "color"),
    ];
    for (service, language, ext, bad, completion, formatting, wanted) in cases {
        let (command, args) = installed.command(service);
        let mut config = crate::lsp::server_for(language, &[]).expect("built-in service");
        config.command = command;
        config.args = args;
        config.managed = None; // Use this explicit disposable installation, never the user's profile.
        let (tx, rx) = mpsc::channel();
        let client = Running(Client::start(host.clone(), config, &crate::ui::local_path(project), tx).unwrap());
        assert_eq!(client.0.capabilities["documentFormattingProvider"], true, "{language} must advertise formatting so the editor can expose it");
        let mut uris = Vec::new();
        for (kind, text) in [("bad", bad), ("completion", completion), ("formatting", formatting)] {
            let file = project.join(format!("şablon {language} {kind}.{ext}"));
            std::fs::write(&file, text).unwrap();
            let uri = crate::lsp::path_to_uri(&crate::ui::local_path(&file));
            client.0.notify("textDocument/didOpen", json!({"textDocument":{"uri":uri,"languageId":language,"version":1,"text":text}}));
            uris.push(uri);
        }
        let result = client
            .0
            .request(
                "textDocument/completion",
                json!({"textDocument":{"uri":uris[1]},"position":{"line":0,"character":completion.len()},"context":{"triggerKind":1}}),
                Duration::from_secs(20),
            )
            .unwrap();
        let items = result.as_array().or_else(|| result["items"].as_array()).expect("completion items");
        assert!(items.iter().any(|item| item["label"].as_str().is_some_and(|label| label == wanted)), "{language} did not complete {wanted}");
        let formatting = client
            .0
            .request("textDocument/formatting", json!({"textDocument":{"uri":uris[2]},"options":{"tabSize":2,"insertSpaces":true}}), Duration::from_secs(20))
            .unwrap();
        let edits = formatting.as_array().expect("formatting edits");
        assert!(!edits.is_empty(), "{language} returned no formatting edits");
        let bad_path = crate::lsp::uri_to_path(&uris[0]).unwrap();
        let start = Instant::now();
        let diagnostics = loop {
            assert!(start.elapsed() < Duration::from_secs(20), "{language} returned no diagnostics");
            match rx.recv_timeout(Duration::from_millis(100)) {
                Ok(ServerEvent::Diagnostics { path, diagnostics }) if path == bad_path && !diagnostics.is_empty() => break diagnostics,
                Ok(ServerEvent::Log { level, text }) if level <= 2 => eprintln!("{language} server: {text}"),
                Ok(ServerEvent::Message(text)) => eprintln!("{language} server: {text}"),
                _ => {}
            }
        };
        assert!(diagnostics.iter().all(|diagnostic| !diagnostic.message.is_empty()));
        eprintln!(
            "{}",
            json!({"host":host.label(),"service":language,"completions":items.len(),"formatting_edits":edits.len(),"diagnostics":diagnostics.len()})
        );
    }
}

#[test]
#[ignore = "requires CODEYARD_LANGUAGE_SERVICE_FIXTURE and an owned local SSH test server; optional CODEYARD_TEST_MANAGED_INSTALL=1 enables remote package downloads"]
fn managed_services_real_local_and_ssh_acceptance() {
    let root = std::env::var("CODEYARD_LANGUAGE_SERVICE_FIXTURE").expect("explicit disposable installation fixture");
    let local: Arc<dyn Host> = Arc::new(host::local());
    let installed = installer::discover(local.as_ref(), &root).unwrap().expect("prepared managed services");
    let project = Fixture::new();
    probe(local, &installed, Path::new(&project.path("local project")));

    let sshd = crate::test_sshd::start("managed-services").expect("owned sshd is required for this acceptance test");
    let remote: Arc<dyn Host> = Arc::new(host::ssh::connect(crate::test_sshd::ALIAS, &crate::bundled_agents::discover(), |_| {}).unwrap());
    assert!(remote.is_remote());
    let installed = if std::env::var("CODEYARD_TEST_MANAGED_INSTALL").as_deref() == Ok("1") {
        installer::install(remote.as_ref(), &crate::ui::local_path(&sshd.dir.join("managed")), &Progress::default()).unwrap()
    } else {
        installer::discover(remote.as_ref(), &root).unwrap().expect("same-machine SSH fixture is visible remotely")
    };
    probe(remote.clone(), &installed, &sshd.dir.join("remote project"));
    drop(remote);
}
