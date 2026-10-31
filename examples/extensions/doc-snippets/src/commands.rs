//! docs/extensions/commands.md

use codeyard_extension::prelude::*;

/// Stand-ins for the helpers the snippets call.
async fn load(_ctx: &Context) -> Result<Vec<String>> {
    Ok(Vec::new())
}

fn whole(_doc: &Document) -> Range {
    Range::line(0, 0, 0)
}

fn format(text: &str, _tab_size: u32) -> String {
    text.to_string()
}

fn to_pull_request(_value: &Value) -> PullRequest {
    PullRequest::default()
}

mod line_58 {
use codeyard_extension::prelude::*;

struct Hello;

impl Extension for Hello {
    fn activate(ctx: Context) -> Result<()> {
        ctx.command("hello", async |ctx, _args| {
            let name = ctx.input(InputOptions::prompt("Your name?")).await?;
            let name = name.unwrap_or_else(|| "world".into());
            ctx.notice(format!("Hello, {name}!"));
            Ok(())
        });
        Ok(())
    }
}

codeyard_extension::export!(Hello);
}

fn line_97(ctx: Context) {
ctx.command("refresh", async |ctx, args| {
    // `args` is the JSON the command was run with (`Value::Null` from the palette).
    let deployments = load(&ctx).await?;
    Ok(json!({ "count": deployments.len() }))     // anything Serialize; Ok(()) for nothing
});
}

async fn line_111(ctx: Context, root: &str) -> Result<()> {
ctx.execute("core:editor.open", json!(format!("file://{root}/docs/guide.md"))).await?;
    Ok(())
}

fn line_130(ctx: Context) {
ctx.view("main", async |ctx| {
    Ok(ui::column().gap(2).child(ui::text("3 clicks")).child(ui::button("add", "Add one")))
});
ctx.on_view_event("main", async |ctx, event| {
    // What the user did, as Codeyard sends it: {"click": {"path": ["add"]}}.
    if event["click"]["path"] == json!(["add"]) {
        ctx.invalidate("main");     // draw the view again
    }
    Ok(())
});
}

async fn line_151(ctx: Context) -> Result<()> {
ctx.notice("Deployed.");                                                     // does not wait
let choice = ctx.notify(Severity::Warning, "Deploy failed", &["Retry", "Logs"]).await?;   // Some(0), Some(1) or None
let env = ctx.quick_pick(["production", "preview"], PickOptions::default()).await?;      // Some(index) or None
let name = ctx.input(InputOptions::prompt("Branch?")).await?;               // Some(text) or None

let progress = ctx.progress("Building…", true);
progress.report(Some("step 2 of 3"), Some(66));
if progress.is_cancelled() { /* the user pressed Cancel */ }
drop(progress);                                                              // or progress.end()

ctx.set_status_item(StatusItem { id: "deploy".into(), text: "Deploy: ready".into(), tone: Tone::Success, ..Default::default() })?;
ctx.open_external("https://vercel.com/dashboard").await?;                    // only while handling a user action
ctx.log(LogLevel::Info, "refreshed");                                        // println! goes to the log too
    Ok(())
}

async fn line_171(ctx: Context, uri: String, decorations: Vec<Decoration>) -> Result<()> {
if let Some(doc) = ctx.active_document().await? {
    let edits = [TextEdit::replace(Range::line(0, 0, 0), "// header\n")];
    ctx.apply_edits(&doc.uri, doc.version, &edits).await?;      // fails if the document changed since
}
ctx.set_diagnostics(&uri, &[Diagnostic::new(Range::line(3, 0, 4), DiagnosticSeverity::Information, "TODO left here")]);
ctx.set_decorations(&uri, "coverage", &decorations);
    Ok(())
}

fn line_186(ctx: Context) {
    // A constant, so each answer gets its own copy.
    const edits: Vec<TextEdit> = Vec::new();
ctx.formatter(async |ctx, doc, range, options| {
    Ok(vec![TextEdit::replace(whole(&doc), format(&doc.text, options.tab_size))])
});

ctx.code_actions(async |ctx, doc, range, diagnostics| {
    Ok(vec![CodeAction { id: "fix-todo".into(), title: "Remove TODO".into(), edits: vec![(doc.uri, edits)], ..Default::default() }])
});
ctx.on_code_action(async |ctx, id| Ok(()));   // for actions with a `command` instead of edits
}

fn line_203(ctx: Context) {
ctx.hover(async |ctx, doc, position| {
    Ok(Some(Hover::new("**TODO**: work left to do.")))   // Markdown, shown as plain text; `None` for nothing
});

ctx.completion(async |ctx, doc, position, trigger| {
    // `trigger` is the trigger character typed, `None` when the user typed a word or asked.
    Ok(vec![CompletionItem { insert_text: Some("TODO: $0".into()), snippet: true, ..CompletionItem::new("TODO", CompletionKind::Keyword) }])
});
}

fn line_226(ctx: Context) {
ctx.code_lenses(async |ctx, doc, range| {
    // Runs `run-test` (one of your [[commands]]) with { "name": "parses" } when clicked.
    Ok(vec![CodeLens::new(12, "Run test", "run-test", json!({ "name": "parses" }))])
});
ctx.inlay_hints(async |ctx, doc, range| Ok(vec![InlayHint::new(Position::new(3, 18), ": u32")]));
}

fn line_238(ctx: Context) {
ctx.resolve_code_lens(async |ctx, doc, mut lens| {
    lens.command = Some(LensCommand { title: "3 references".into(), command: "refs".into(), args: json!(lens.id) });
    Ok(lens)
});
}

fn line_262(ctx: Context) {
    const old: String = String::new();
    const new: String = String::new();
ctx.pull_requests("codeberg")
    .list(async |ctx, remote| {
        let account = remote.account.clone().unwrap_or_default();
        ctx.session(&account, &[]).await?;   // asks on first use
        let url = format!("https://{}/api/v1/repos/{}/pulls?state=open", remote.host, remote.path);
        let list: Vec<Value> = ctx.fetch_as(&account, Request::get(url)).await?.error_for_status()?.json()?;
        Ok(list.iter().map(to_pull_request).collect())
    })
    .details(async |ctx, remote, id| Ok(PullRequestDetails { /* summary, description, checks, labels */ ..Default::default() }))
    .files(async |ctx, remote, id| Ok(vec![ChangedFile { path: "src/main.rs".into(), ..Default::default() }]))
    .contents(async |ctx, remote, id, file| Ok(FileContents { base: Some(old), head: Some(new) }))
    .comments(async |ctx, remote, id| Ok(vec![]))
    .on_comment(async |ctx, remote, id, body| Ok(()))
    .checkout(async |ctx, remote, id| Ok(Checkout { fetch_url: None, reference: format!("refs/pull/{id}/head"), branch: format!("pr-{id}") }));
}

async fn line_309(ctx: Context) -> Result<()> {
let res = ctx.fetch(Request::get("https://api.vercel.com/v6/deployments").header("accept", "application/json")).await?;
let body: Value = res.error_for_status()?.json()?;

let config = ctx.read_text(&format!("{}/vercel.json", ctx.folders()[0])).await?;
let found = ctx.find_files("**/vercel.json", 10).await?;

// The whole output at once…
let out = ctx.run(Command::new("vercel").args(["ls", "--json"])).await?;
// …or as it comes.
let mut build = ctx.spawn(Command::new("vercel").arg("build"));
while let Some(chunk) = build.next().await {
    if let Chunk::Stdout(bytes) = chunk { ctx.log(LogLevel::Info, String::from_utf8_lossy(&bytes)); }
}
let code = build.wait().await?;            // dropping `build` before it ends kills the process
    Ok(())
}

fn line_330(ctx: Context) -> Result<()> {
let n = ctx.storage().get_json::<u32>("runs")?.unwrap_or(0);   // persisted, 50 MiB in all
ctx.storage().set_json("runs", &(n + 1))?;

let seconds = ctx.setting::<u32>("interval")?.unwrap_or(60);   // declared under [settings.interval]
ctx.on_settings_changed(async |ctx, keys| { /* restart the timer */ });

let timer = ctx.every(seconds, async |ctx| { /* poll */ });      // at least 30 s, paused while unfocused
timer.clear();

let clicks: u32 = ctx.state().get("clicks").unwrap_or(0);       // in memory; kept across a hot reload in cyx dev
ctx.state().set("clicks", &(clicks + 1))?;
    Ok(())
}
