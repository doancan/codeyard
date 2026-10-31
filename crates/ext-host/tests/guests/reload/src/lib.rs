//! A hot-reload subject for the performance scenarios (scripts/perf/run.sh, `extensions.hot_reload_ms`): its command
//! `count` answers `ctx.state` "tally" + 1, so the tally goes on across reloads, and runs its argument through
//! toml, regex and pulldown-cmark, which keep those crates in the component (about 2 MB, a realistic Rust extension).

use codeyard_extension::prelude::*;

/// The constant the two builds differ in (`--features b`), like an author's edit.
const STEP: &str = if cfg!(feature = "b") { "b" } else { "a" };

struct Reload;

/// Use every dependency on `text`, so that none is optimized away.
fn digest(text: &str) -> usize {
    let words = regex::Regex::new(&format!(r"\b{STEP}\w*")).map_or(0, |r| r.find_iter(text).count());
    let table = text.parse::<toml::Table>().map_or(String::new(), |t| t.to_string());
    let mut html = String::new();
    pulldown_cmark::html::push_html(&mut html, pulldown_cmark::Parser::new_ext(text, pulldown_cmark::Options::all()));
    words + table.len() + html.len()
}

impl Extension for Reload {
    fn activate(ctx: Context) -> Result<()> {
        ctx.command("count", async |ctx, args| {
            let n: u64 = ctx.state().get("tally").unwrap_or(0) + 1;
            ctx.state().set("tally", &n)?;
            if let Some(text) = args.as_str() {
                ctx.state().set("digest", &digest(text))?;
            }
            Ok(n)
        });
        Ok(())
    }
}

codeyard_extension::export!(Reload);
