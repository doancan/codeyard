//! Jujutsu: a source control provider (`[[scm]]`) that runs `jj` in folders with a `.jj` folder. Source Control shows
//! the working-copy change's files and the gutter compares open files with the parent change; the input box commits,
//! and each file's button restores it. Every `jj` runs where the workspace is (also over SSH), in the folder.

use std::cell::RefCell;

use codeyard_extension::prelude::*;

struct Jujutsu;

thread_local! {
    /// The folders Codeyard asked about: a file's original text is read in the folder it is in.
    static ROOTS: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
}

/// `jj` in `root` with `args`; its output, or an error with what jj said.
async fn jj(ctx: &Context, root: &str, args: &[&str]) -> Result<String> {
    let out = ctx.run(Command::new("jj").args(args.iter().copied()).cwd(root)).await?;
    if !out.success() {
        return Err(Error::other(format!("jj {}: {}", args.first().copied().unwrap_or_default(), out.stderr_text().trim())));
    }
    Ok(out.stdout_text())
}

/// A file of the folder: its path relative to `root`.
fn relative<'a>(root: &str, uri: &'a str) -> Option<&'a str> {
    uri.strip_prefix(root)?.strip_prefix('/')
}

/// `jj diff --summary`: `M src/a.rs`, `A b.rs`, `D c.rs`, `R {old.rs => new.rs}` or `R dir/{a => b}.rs`.
fn parse_summary(root: &str, text: &str) -> Vec<ScmResource> {
    let mut out = Vec::new();
    for line in text.lines() {
        let Some((letter, path)) = line.split_once(' ') else { continue };
        let state = match letter {
            "A" | "C" => FileState::Added,
            "D" => FileState::Deleted,
            "R" => FileState::Renamed,
            _ => FileState::Modified,
        };
        let (path, from) = match (path.find('{'), path.find(" => "), path.find('}')) {
            (Some(open), Some(arrow), Some(close)) if open < arrow && arrow < close => {
                let (head, tail) = (&path[..open], &path[close + 1..]);
                (format!("{head}{}{tail}", &path[arrow + 4..close]), Some(format!("{head}{}{tail}", &path[open + 1..arrow])))
            }
            _ => (path.to_string(), None),
        };
        let mut r = ScmResource::new(format!("{root}/{path}"), state);
        r.detail = from.map(|f| format!("from {f}"));
        out.push(r);
    }
    out
}

impl Extension for Jujutsu {
    fn activate(ctx: Context) -> Result<()> {
        ctx.scm_status("jj", async |ctx, root| {
            ROOTS.with(|r| {
                if !r.borrow().contains(&root) {
                    r.borrow_mut().push(root.clone());
                }
            });
            let summary = jj(&ctx, &root, &["diff", "--summary", "--color", "never"]).await?;
            let head = jj(&ctx, &root, &["log", "-r", "@", "--no-graph", "--color", "never", "-T", "change_id.short()"]).await?;
            let mut changes = ScmGroup::new("working-copy", "Working Copy").resource_action(ScmAction::new("restore", "Discard Changes").icon("undo-2"));
            changes = changes.group_action(ScmAction::new("restore", "Discard All Changes").icon("undo-2"));
            for r in parse_summary(&root, &summary) {
                changes = changes.resource(r);
            }
            let input = ScmInput::new("Describe the change (Enter commits)").accept(ScmAction::new("commit", "Commit").icon("check"));
            Ok(ScmState::new().head(format!("@ {}", head.trim())).input(input).group(changes))
        });
        // The parent change's text: what the working copy started from. A file it does not have is new: no marks.
        ctx.scm_original("jj", async |ctx, uri| {
            let Some(root) = ROOTS.with(|r| r.borrow().iter().find(|f| relative(f, &uri).is_some()).cloned()) else { return Ok(None) };
            let Some(path) = relative(&root, &uri) else { return Ok(None) };
            Ok(jj(&ctx, &root, &["file", "show", "-r", "@-", path]).await.ok())
        });
        ctx.scm_validate("jj", async |_ctx, _root, text| Ok(text.trim().is_empty().then(|| "Describe the change first.".to_string())));
        ctx.command("commit", async |ctx, args| {
            let args = ScmArgs::from(args)?;
            jj(&ctx, &args.root, &["commit", "-m", args.message.trim()]).await?;
            ctx.scm_refresh(&args.provider);
            Ok(Value::Null)
        });
        ctx.command("restore", async |ctx, args| {
            let args = ScmArgs::from(args)?;
            let paths: Vec<&str> = args.uris.iter().filter_map(|u| relative(&args.root, u)).collect();
            if paths.is_empty() {
                return Ok(Value::Null);
            }
            let mut argv = vec!["restore"];
            argv.extend(paths);
            jj(&ctx, &args.root, &argv).await?;
            ctx.scm_refresh(&args.provider);
            Ok(Value::Null)
        });
        Ok(())
    }
}

codeyard_extension::export!(Jujutsu);

#[cfg(test)]
mod tests {
    use super::*;
    use codeyard_extension_test::{MockHost, ProcessFixture};

    const ROOT: &str = "file:///work";

    fn host() -> MockHost {
        let host = MockHost::new();
        host.manifest(include_str!("../codeyard-extension.toml"));
        host.process("jj", &["diff", "--summary", "--color", "never"], ProcessFixture::ok("M src/a.rs\nA new.rs\nR src/{old => b}.rs\n"));
        host.process("jj", &["log", "-r", "@", "--no-graph", "--color", "never", "-T", "change_id.short()"], ProcessFixture::ok("qpvuntsm\n"));
        host.activate::<Jujutsu>().unwrap();
        host
    }

    #[test]
    fn lists_the_working_copy_changes() {
        let host = host();
        let state = host.scm_status("jj", "").unwrap();
        assert_eq!(state.head.as_deref(), Some("@ qpvuntsm"));
        let files: Vec<(&str, FileState, Option<&str>)> = state.groups[0].resources.iter().map(|r| (r.uri.as_str(), r.state, r.detail.as_deref())).collect();
        assert_eq!(
            files,
            [
                ("file:///work/src/a.rs", FileState::Modified, None),
                ("file:///work/new.rs", FileState::Added, None),
                ("file:///work/src/b.rs", FileState::Renamed, Some("from src/old.rs")),
            ]
        );
    }

    #[test]
    fn the_gutter_compares_with_the_parent_change() {
        let host = host();
        host.scm_status("jj", "").unwrap();
        host.process("jj", &["file", "show", "-r", "@-", "src/a.rs"], ProcessFixture::ok("fn a() {}\n"));
        host.process("jj", &["file", "show", "-r", "@-", "new.rs"], ProcessFixture::failed(1, "Error: No such path\n"));
        assert_eq!(host.scm_original("jj", "src/a.rs").unwrap().as_deref(), Some("fn a() {}\n"));
        assert_eq!(host.scm_original("jj", "new.rs").unwrap(), None);
    }

    #[test]
    fn the_input_box_commits_and_files_are_restored() {
        let host = host();
        let state = host.scm_status("jj", "").unwrap();
        assert_eq!(host.scm_validate("jj", "", "  ").unwrap().as_deref(), Some("Describe the change first."));
        host.process("jj", &["commit", "-m", "Fix the parser"], ProcessFixture::ok(""));
        host.scm_press("jj", state.input.accept.as_ref().unwrap(), "", &[], "Fix the parser\n").unwrap();
        host.process("jj", &["restore", "src/a.rs"], ProcessFixture::ok(""));
        host.scm_press("jj", &state.groups[0].resource_actions[0], "", &["src/a.rs"], "").unwrap();
        let ran: Vec<String> = host.ran().iter().map(|c| c.args.join(" ")).collect();
        assert!(ran.contains(&"commit -m Fix the parser".to_string()), "{ran:?}");
        assert!(ran.contains(&"restore src/a.rs".to_string()), "{ran:?}");
        assert_eq!(host.scm_refreshed(), ["jj", "jj"]);
        assert!(host.ran().iter().all(|c| c.cwd.as_deref() == Some(ROOT)));
    }
}
