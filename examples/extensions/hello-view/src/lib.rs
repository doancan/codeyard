//! Hello View: a view with a button, and a command that resets it. `cargo test` runs the test below against a mock
//! Codeyard; `cyx dev` shows the view in the sidebar.

use codeyard_extension::prelude::*;

struct HelloView;

fn clicks(ctx: &Context) -> Result<u32> {
    Ok(ctx.storage().get_json("clicks")?.unwrap_or(0))
}

impl Extension for HelloView {
    fn activate(ctx: Context) -> Result<()> {
        // The view's tree (docs/extensions/views.md). Codeyard draws it with the user's theme.
        ctx.view("main", async |ctx| {
            Ok(ui::column().gap(2).pad(2).child(ui::heading("Clicks", 2)).child(ui::text(clicks(&ctx)?.to_string())).child(ui::button("add", "Add one")))
        });
        // A click on the button arrives as {"click": {"path": ["add"]}}: count it and draw the view again.
        ctx.on_view_event("main", async |ctx, event| {
            if event["click"]["path"] == json!(["add"]) {
                ctx.storage().set_json("clicks", &(clicks(&ctx)? + 1))?;
                ctx.invalidate("main");
            }
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
        let click = json!({ "click": { "path": ["add"] } });
        host.view_event("main", click.clone()).unwrap();
        host.view_event("main", click).unwrap();
        // `render` fails if Codeyard would refuse the tree.
        let count = |host: &MockHost| host.render("main").unwrap().pointer("/children/1/element/text/text").cloned();
        assert_eq!(count(&host), Some(json!("2")));
        host.command("reset", Value::Null).unwrap();
        assert_eq!(count(&host), Some(json!("0")));
        assert_eq!(host.invalidated(), ["main", "main", "main"]);
    }
}
