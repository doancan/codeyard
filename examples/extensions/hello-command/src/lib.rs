//! Hello Command: one command. `cargo test` runs the test below against a mock Codeyard; `cyx dev` runs it for real.

use codeyard_extension::prelude::*;

struct HelloCommand;

impl Extension for HelloCommand {
    fn activate(ctx: Context) -> Result<()> {
        // `hello` is the command's id in codeyard-extension.toml.
        ctx.command("hello", async |ctx, _args| {
            let name = ctx.input(InputOptions::prompt("Your name?")).await?;
            let name = name.filter(|n| !n.trim().is_empty()).unwrap_or_else(|| "world".into());
            // Storage keeps its values between sessions.
            let count = ctx.storage().get_json::<u32>("greetings")?.unwrap_or(0) + 1;
            ctx.storage().set_json("greetings", &count)?;
            ctx.notice(format!("Hello, {name}! ({count} so far)"));
            Ok(count)
        });
        Ok(())
    }
}

codeyard_extension::export!(HelloCommand);

#[cfg(test)]
mod tests {
    use super::*;
    use codeyard_extension_test::MockHost;

    #[test]
    fn says_hello() {
        let host = MockHost::new();
        host.activate::<HelloCommand>().unwrap();
        host.answer_input("Ada");
        assert_eq!(host.command("hello", Value::Null).unwrap(), json!(1));
        host.dismiss_input();
        assert_eq!(host.command("hello", Value::Null).unwrap(), json!(2));
        assert_eq!(host.notices(), ["Hello, Ada! (1 so far)", "Hello, world! (2 so far)"]);
    }
}
