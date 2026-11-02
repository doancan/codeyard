//! docs/extensions/views.md

use codeyard_extension::Context;

fn line_81(ctx: Context) {
use codeyard_extension::prelude::*;

ctx.view("deployments", async |_ctx| {
    Ok(ui::column()
        .gap(2)
        .child(ui::heading("Deployments", 2))
        .child(ui::button("refresh", "Refresh").icon("refresh-cw"))
        .child(ui::list("list", "Deployments").selection(ui::Selection::One).children([
            ui::text("web: ready").tone(Tone::Success).key("web"),
            ui::text("api: building").tone(Tone::Warning).key("api"),
        ])))
});
}
