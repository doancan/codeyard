//! docs/extensions/testing.md

use codeyard_extension::prelude::*;

/// The extension the snippets test: `hello` greets, `count` counts in `ctx.state()`.
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

struct Counter;

impl Extension for Counter {
    fn activate(ctx: Context) -> Result<()> {
        ctx.command("count", async |ctx, _args| {
            let n: u32 = ctx.state().get("n").unwrap_or(0) + 1;
            ctx.state().set("n", &n)?;
            Ok(n)
        });
        Ok(())
    }
}

mod line_24 {
use super::Hello;
use codeyard_extension::prelude::*;

#[cfg(test)]
mod tests {
    use super::*;
    use codeyard_extension_test::MockHost;

    #[test]
    fn says_hello() {
        let host = MockHost::new();
        host.activate::<Hello>().unwrap();
        host.answer_input("Ada");
        host.command("hello", Value::Null).unwrap();
        assert_eq!(host.notices(), ["Hello, Ada!"]);
    }
}
}

#[cfg(test)]
mod tests {
    use super::*;
    use codeyard_extension_test::{MockHost, ProcessFixture};

    #[test]
    fn line_69() {
        let host = MockHost::new();
host.file("vercel.json", r#"{ "name": "web" }"#);
host.http("GET", "https://api.vercel.com/v6/deployments", 200, r#"{ "deployments": [] }"#);
host.process("vercel", &["ls"], ProcessFixture::ok("web  ready\n"));
host.active_document("src/main.rs", "rust", "fn main() {}");
host.setting("interval", Some("120"));
host.command_result("other.ext.list", "[1, 2]");

host.answer_input("Ada");        // the next input box
host.dismiss_input();
host.answer_pick(&[1]);          // the next quick pick
host.answer_notice(0);           // the next notice with buttons
    }

    #[test]
    fn line_87() {
        let host = MockHost::new();
        host.activate::<Hello>().unwrap();
host.notices();           // notice texts, in order
host.fetched();           // every HTTP request sent
host.ran();               // every program started
host.read("out.txt");     // a file after the extension wrote it
host.storage();           // the key-value store
host.status_items();
host.diagnostics("src/main.rs");
host.invalidated();       // views asked to draw again
host.applied_edits();
host.secrets();           // the keychain entries (`secrets`), as the extension left them
host.timers(); host.progress(); host.logs(); host.clipboard(); host.terminal();
host.calls(); host.notes();   // everything, as the SDK sent it
host.denied();            // every refused call
    }

    #[test]
    fn line_123() {
        let host = MockHost::new();
host.decline_sign_in("gitlab");   // the user picks Cancel: auth.session fails with `cancelled`
host.signed_out("gitlab");        // after Sign Out: auth.fetch fails with "not signed in to GitLab; call auth.session …",
                                  // and the next auth.session signs in again
host.sign_in("gitlab", "ada");    // signed in, with this account name
host.instance("gitlab", "git.example.com");  // a self-managed instance the user added: account `gitlab@git.example.com`
    }

    #[test]
    fn watches() {
        let host = MockHost::allow_all();
host.open_document("src/a.rs", "rust", "fn a() {}");   // document watches get `opened`
host.edit_document("src/a.rs", "fn a() {} // TODO"); // `changed`, version 2
host.save_document("src/a.rs");                       // `saved`
host.create_file("src/b.rs", "fn b() {}");            // file watches whose glob matches get `created`
host.change_file("src/b.rs", "fn b() { 1 }");
host.delete_file("src/b.rs");
host.allow_host("github.example.com");                // the user allowed this server for `net-from-setting`
    }

    #[test]
    fn line_137() -> Result<()> {
        let host = MockHost::new();
        host.activate::<Counter>()?;
host.command("count", Value::Null)?;
host.reload::<Counter>()?;
assert_eq!(host.command("count", Value::Null)?, json!(2));   // ctx.state() came along
        Ok(())
    }
}
