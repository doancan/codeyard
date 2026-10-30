//! A source control provider on the mock host: its state, a file's original text, the input box check, a button.

use codeyard_extension::prelude::*;

use crate::{MockHost, ProcessFixture};

const MANIFEST: &str = r#"
[extension]
id      = "you.toy"
version = "0.1.0"
name    = "Toy"
license = "MIT"

[code]
entry = "Cargo.toml"

[permissions]
read = [".toy/**"]
run  = ["toy"]
why  = "Tests."

[[commands]]
id    = "commit"
title = "Toy: Commit"

[[scm]]
id      = "toy"
label   = "Toy"
markers = [".toy"]
"#;

struct Toy;

impl Extension for Toy {
    fn activate(ctx: Context) -> Result<()> {
        ctx.scm_status("toy", async |ctx, root| {
            let text = ctx.read_text(&format!("{root}/.toy/changes")).await?;
            let mut changes = ScmGroup::new("changes", "Changes");
            for path in text.lines() {
                changes = changes.resource(ScmResource::new(format!("{root}/{path}"), FileState::Modified));
            }
            Ok(ScmState::new().head("main").group(changes).input(ScmInput::new("Message").accept(ScmAction::new("commit", "Commit"))))
        });
        ctx.scm_original("toy", async |ctx, uri| Ok(ctx.read_text(&uri.replace("/work/", "/work/.toy/base/")).await.ok()));
        ctx.scm_validate("toy", async |_ctx, _root, text| Ok(text.trim().is_empty().then(|| "Write a message".to_string())));
        ctx.command("commit", async |ctx, args| {
            let args = ScmArgs::from(args)?;
            let out = ctx.run(Command::new("toy").args(["commit", "-m", &args.message])).await?;
            ctx.scm_refresh(&args.provider);
            Ok(json!({ "code": out.code, "files": args.uris.len() }))
        });
        Ok(())
    }
}

#[test]
fn a_provider_answers_state_original_text_and_checks_and_its_button_runs_its_command() {
    let h = MockHost::new();
    h.manifest(MANIFEST);
    h.file(".toy/changes", "a.txt\nb.txt\n").file(".toy/base/a.txt", "old\n");
    h.process("toy", &["commit", "-m", "Fix"], ProcessFixture::ok(""));
    h.activate::<Toy>().unwrap();

    let state = h.scm_status("toy", "").unwrap();
    assert_eq!(state.head.as_deref(), Some("main"));
    let uris: Vec<&str> = state.groups[0].resources.iter().map(|r| r.uri.as_str()).collect();
    assert_eq!(uris, ["file:///work/a.txt", "file:///work/b.txt"]);
    assert_eq!(h.scm_original("toy", "a.txt").unwrap().as_deref(), Some("old\n"));
    assert_eq!(h.scm_original("toy", "b.txt").unwrap(), None);
    assert_eq!(h.scm_validate("toy", "", " ").unwrap().as_deref(), Some("Write a message"));
    assert_eq!(h.scm_validate("toy", "", "Fix").unwrap(), None);

    let accept = state.input.accept.clone().unwrap();
    assert_eq!(h.scm_press("toy", &accept, "", &["a.txt"], "Fix").unwrap(), json!({ "code": 0, "files": 1 }));
    assert_eq!(h.ran()[0].args, ["commit", "-m", "Fix"]);
    assert_eq!(h.scm_refreshed(), ["toy"]);
    assert!(matches!(h.scm_status("other", ""), Err(Error::NotFound(_))));
}
