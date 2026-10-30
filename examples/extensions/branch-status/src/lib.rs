//! Branch Status: a command that runs a program. The program runs where the workspace is (also over SSH), in the first
//! workspace folder, with a minimal environment.

use codeyard_extension::prelude::*;

struct BranchStatus;

impl Extension for BranchStatus {
    fn activate(ctx: Context) -> Result<()> {
        ctx.command("show", async |ctx, _args| {
            let out = ctx.run(Command::new("git").args(["branch", "--show-current"])).await?;
            if !out.success() {
                ctx.notice(format!("git: {}", out.stderr_text().trim()));
                return Ok(Value::Null);
            }
            let branch = out.stdout_text().trim().to_string();
            ctx.set_status_item(StatusItem { id: "branch".into(), text: format!("⎇ {branch}"), ..Default::default() })?;
            Ok(json!(branch))
        });
        Ok(())
    }
}

codeyard_extension::export!(BranchStatus);

#[cfg(test)]
mod tests {
    use super::*;
    use codeyard_extension_test::{MockHost, ProcessFixture};

    #[test]
    fn shows_the_branch() {
        let host = MockHost::new();
        host.process("git", &["branch", "--show-current"], ProcessFixture::ok("main\n"));
        host.activate::<BranchStatus>().unwrap();
        assert_eq!(host.command("show", Value::Null).unwrap(), json!("main"));
        assert_eq!(host.status_items()["branch"].text, "⎇ main");
        assert_eq!(host.ran()[0].args, ["branch", "--show-current"]);
    }

    #[test]
    fn says_why_when_git_fails() {
        let host = MockHost::new();
        host.process("git", &["branch", "--show-current"], ProcessFixture::failed(128, "fatal: not a git repository\n"));
        host.activate::<BranchStatus>().unwrap();
        assert_eq!(host.command("show", Value::Null).unwrap(), Value::Null);
        assert_eq!(host.notices(), ["git: fatal: not a git repository"]);
    }
}
