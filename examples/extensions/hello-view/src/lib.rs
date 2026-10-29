//! Hello View: a view with a button, and a command that resets it. `cargo test` runs the test below against a mock
//! Codeyard; `cyx dev` shows the view in the sidebar.

use codeyard_extension::prelude::*;

struct HelloView;

fn clicks(ctx: &Context) -> Result<u32> {
    Ok(ctx.storage().get_json("clicks")?.unwrap_or(0))
}

impl Extension for HelloView {
    fn activate(ctx: Context) -> Result<()> {
        // The view's element tree as JSON (docs/extensions/commands.md#views). Codeyard draws it with the user's theme.
        ctx.view("main", async |ctx| {
            Ok(json!({
                "type": "column",
                "children": [
                    { "type": "heading", "text": "Clicks" },
                    { "type": "text", "text": clicks(&ctx)?.to_string() },
                    { "type": "button", "key": "add", "label": "Add one" }
                ]
            }))
        });
        // A click on the button: count it and draw the view again.
        ctx.on_view_event("main", async |ctx, _event| {
            ctx.storage().set_json("clicks", &(clicks(&ctx)? + 1))?;
            ctx.invalidate("main");
            Ok(())
        });
        ctx.command("reset", async |ctx, _args| {
            ctx.storage().delete("clicks");
            ctx.invalidate("main");
            Ok(())
        });
        Ok(())
    }
}

codeyard_extension::export!(HelloView);

#[cfg(test)]
mod tests {
    use super::*;
    use codeyard_extension_test::MockHost;

    #[test]
    fn counts_clicks() {
        let host = MockHost::new();
        host.activate::<HelloView>().unwrap();
        host.view_event("main", json!({ "click": "add" })).unwrap();
        host.view_event("main", json!({ "click": "add" })).unwrap();
        assert_eq!(host.render("main").unwrap()["children"][1]["text"], "2");
        host.command("reset", Value::Null).unwrap();
        assert_eq!(host.render("main").unwrap()["children"][1]["text"], "0");
        assert_eq!(host.invalidated(), ["main", "main", "main"]);
    }
}
