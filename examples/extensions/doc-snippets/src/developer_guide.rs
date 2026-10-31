//! docs/extensions/developer-guide.md

use codeyard_extension::prelude::*;

mod line_118 {
// src/lib.rs
use codeyard_extension::prelude::*;

struct Hello;

impl Extension for Hello {
    fn activate(ctx: Context) -> Result<()> {
        ctx.command("hello", async |ctx, _args| {
            if let Some(name) = ctx.input(InputOptions::prompt("Your name?")).await? {
                ctx.notice(format!("Hello, {name}!"));
            }
            Ok(())
        });
        Ok(())
    }
}

codeyard_extension::export!(Hello);
}

fn line_145(ctx: Context) -> Result<()> {
let n: u32 = ctx.state().get("clicks").unwrap_or(0) + 1;   // Rust
ctx.state().set("clicks", &n)?;
    Ok(())
}

fn line_286(ctx: Context) {
ctx.scm_status("jj", async |ctx, root| {
    let summary = ctx.run(Command::new("jj").args(["diff", "--summary"]).cwd(&root)).await?.stdout_text();
    let mut changes = ScmGroup::new("working-copy", "Working Copy").resource_action(ScmAction::new("restore", "Discard").icon("undo-2"));
    for line in summary.lines() {
        changes = changes.resource(ScmResource::new(format!("{root}/{}", &line[2..]), FileState::Modified));
    }
    Ok(ScmState::new().group(changes).input(ScmInput::new("Describe the change").accept(ScmAction::new("commit", "Commit"))))
});
ctx.command("commit", async |ctx, args| {
    let args = ScmArgs::from(args)?;
    ctx.run(Command::new("jj").args(["commit", "-m", &args.message]).cwd(&args.root)).await?;
    Ok(Value::Null)
});
}
