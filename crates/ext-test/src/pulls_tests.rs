//! A pull request provider against the mock host: its forge calls go through `auth` fixtures, unset handlers answer
//! as documented, and an unknown provider says how to register it.

use codeyard_extension::native::Call;
use codeyard_extension::prelude::*;
use codeyard_extension::pulls::{PullAnswer, PullQuery};

use crate::MockHost;

const MANIFEST: &str = r#"
[extension]
id      = "you.forge"
version = "0.1.0"
name    = "Forge"
license = "MIT"

[code]
entry = "Cargo.toml"

[permissions]
auth = ["forge"]
why  = "Lists merge requests."

[[oauth]]
id            = "forge"
name          = "Forge"
authorize-url = "https://forge.example.com/oauth/authorize"
token-url     = "https://forge.example.com/oauth/token"
client-id     = "x"
hosts         = ["forge.example.com"]

[[pull-requests]]
id    = "forge"
label = "Forge"
hosts = ["forge.example.com"]
auth  = "forge"
"#;

struct Forge;

impl Extension for Forge {
    fn activate(ctx: Context) -> Result<()> {
        ctx.pull_requests("forge")
            .list(async |ctx, remote| {
                let url = format!("https://forge.example.com/api/{}/merge_requests", remote.path);
                let text = ctx.fetch_as(remote.account.as_deref().unwrap_or_default(), Request::get(&url)).await?.text()?;
                Ok(text
                    .lines()
                    .map(|l| PullRequest { id: l.into(), reference: format!("!{l}"), title: format!("Request {l}"), ..Default::default() })
                    .collect())
            })
            .details(async |_ctx, _remote, id| Ok(PullRequestDetails { description: format!("About **{id}**"), ..Default::default() }))
            .files(async |_ctx, _remote, _id| Ok(vec![ChangedFile { path: "a.rs".into(), ..Default::default() }]))
            .contents(async |_ctx, _remote, _id, file| Ok(FileContents { base: None, head: Some(file.path) }))
            .checkout(async |_ctx, _remote, id| {
                Ok(Checkout { fetch_url: None, reference: format!("refs/merge-requests/{id}/head"), branch: format!("mr-{id}") })
            });
        Ok(())
    }
}

fn remote(provider: &str) -> Remote {
    Remote {
        provider: provider.into(),
        name: "origin".into(),
        url: "git@forge.example.com:team/app.git".into(),
        host: "forge.example.com".into(),
        path: "team/app".into(),
        account: Some("forge".into()),
    }
}

#[test]
fn a_provider_lists_through_auth_and_answers_each_query() {
    let host = MockHost::new();
    host.manifest(MANIFEST);
    host.http("GET", "https://forge.example.com/api/team/app/merge_requests", 200, "4\n9\n");
    host.activate::<Forge>().unwrap();

    let list = host.list_pull_requests(remote("forge")).unwrap();
    assert_eq!(list.iter().map(|p| p.reference.as_str()).collect::<Vec<_>>(), ["!4", "!9"]);
    // Signed by the host: the extension's request names the provider and carries no authorization of its own.
    let fetches: Vec<_> = host.calls().into_iter().filter(|c| matches!(c, Call::AuthFetch { .. })).collect();
    assert!(matches!(&fetches[..], [Call::AuthFetch { provider, request }] if provider == "forge" && request.headers.is_empty()));

    let r = || remote("forge");
    assert!(matches!(host.pull_requests(PullQuery::Details(r(), "4".into())), Ok(PullAnswer::Details(d)) if d.description == "About **4**"));
    assert!(matches!(host.pull_requests(PullQuery::Files(r(), "4".into())), Ok(PullAnswer::Files(f)) if f[0].path == "a.rs"));
    let file = ChangedFile { path: "a.rs".into(), ..Default::default() };
    assert!(matches!(host.pull_requests(PullQuery::Contents(r(), "4".into(), file)), Ok(PullAnswer::Contents(c)) if c.head.as_deref() == Some("a.rs")));
    assert!(matches!(host.pull_requests(PullQuery::Checkout(r(), "4".into())), Ok(PullAnswer::Checkout(c)) if c.branch == "mr-4"));
    // No comments handler: none to show, and adding one is refused.
    assert_eq!(host.pull_requests(PullQuery::Comments(r(), "4".into())), Ok(PullAnswer::Comments(vec![])));
    assert!(matches!(host.pull_requests(PullQuery::AddComment(r(), "4".into(), "hi".into())), Err(Error::Invalid(m)) if m.contains("does not take comments")));
    // A provider the extension did not register.
    let err = host.list_pull_requests(remote("other")).unwrap_err();
    assert!(err.to_string().contains("ctx.pull_requests(\"other\")"), "{err}");
}
