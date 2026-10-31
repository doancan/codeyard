//! TITLE: pull requests of a forge in Codeyard's Pull Requests view. It speaks the Gitea and Forgejo API (`/api/v1`);
//! for another forge, change `api` and the fields read below. Every call goes through `ctx.fetch_as` with the remote's
//! account: Codeyard adds the token, so the extension never sees it. `cargo test` runs the tests below against a mock
//! Codeyard with HTTP fixtures; `cyx test` runs the built extension with the answers in `test/fixtures.json`.

use codeyard_extension::prelude::*;

struct TYPE;

/// `https://<the remote's host>/api/v1/repos/<owner>/<repo><rest>`: the same code serves every host the provider claims.
fn api(remote: &Remote, rest: &str) -> String {
    format!("https://{}/api/v1/repos/{}{rest}", remote.host, remote.path)
}

/// GET as the remote's account; the first call asks the user to sign in.
async fn get(ctx: &Context, remote: &Remote, rest: &str) -> Result<Value> {
    let account = remote.account.clone().ok_or_else(|| Error::invalid("the [[pull-requests]] entry needs auth = \"<its [[oauth]] id>\""))?;
    ctx.session(&account, &[]).await?;
    ctx.fetch_as(&account, Request::get(api(remote, rest)).header("accept", "application/json")).await?.error_for_status()?.json()
}

fn text(v: &Value) -> String {
    v.as_str().unwrap_or_default().to_string()
}

/// A pull request as the API returns it.
fn pull(p: &Value) -> PullRequest {
    let number = p["number"].as_u64().unwrap_or_default();
    let state = if p["merged"] == true {
        PullState::Merged
    } else if p["state"] == "closed" {
        PullState::Closed
    } else if p["draft"] == true {
        PullState::Draft
    } else {
        PullState::Open
    };
    PullRequest {
        id: number.to_string(),
        reference: format!("#{number}"),
        title: text(&p["title"]),
        author: text(&p["user"]["login"]),
        state,
        source_branch: text(&p["head"]["ref"]),
        target_branch: text(&p["base"]["ref"]),
        updated: 0,
        url: p["html_url"].as_str().map(str::to_string),
    }
}

impl Extension for TYPE {
    fn activate(ctx: Context) -> Result<()> {
        // `pulls` is the [[pull-requests]] id in codeyard-extension.toml.
        ctx.pull_requests("pulls")
            .list(async |ctx, remote| {
                let list = get(&ctx, &remote, "/pulls?state=open&sort=recentupdate&limit=50").await?;
                Ok(list.as_array().into_iter().flatten().map(pull).collect())
            })
            .details(async |ctx, remote, id| {
                let p = get(&ctx, &remote, &format!("/pulls/{id}")).await?;
                let labels = p["labels"].as_array().into_iter().flatten().map(|l| text(&l["name"])).collect();
                Ok(PullRequestDetails { summary: pull(&p), description: text(&p["body"]), checks: Vec::new(), labels })
            })
            .files(async |ctx, remote, id| {
                let files = get(&ctx, &remote, &format!("/pulls/{id}/files")).await?;
                let status = |s: &str| match s {
                    "added" => FileStatus::Added,
                    "deleted" | "removed" => FileStatus::Deleted,
                    "renamed" => FileStatus::Renamed,
                    _ => FileStatus::Modified,
                };
                Ok(files
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|f| ChangedFile {
                        path: text(&f["filename"]),
                        old_path: f["previous_filename"].as_str().filter(|s| !s.is_empty()).map(str::to_string),
                        status: status(f["status"].as_str().unwrap_or_default()),
                    })
                    .collect())
            })
            // The diff shows nothing until this fetches the file at both ends (`/raw/<path>?ref=<sha>` here).
            .contents(async |_ctx, _remote, _id, _file| Ok(FileContents { base: None, head: None }))
            // Gitea and Forgejo keep every pull request's head at `refs/pull/<n>/head`, forks included.
            .checkout(async |_ctx, _remote, id| Ok(Checkout { fetch_url: None, reference: format!("refs/pull/{id}/head"), branch: format!("pr-{id}") }));
        Ok(())
    }
}

codeyard_extension::export!(TYPE);

#[cfg(test)]
mod tests {
    use super::*;
    use codeyard_extension::pulls::{PullAnswer, PullQuery};
    use codeyard_extension_test::MockHost;

    const API: &str = "https://forge.example.com/api/v1/repos/ada/app";
    const PULL: &str = r#"{"number":3,"title":"Fix the parser","state":"open","user":{"login":"ada"},"head":{"ref":"fix"},"base":{"ref":"main"},"body":"Fixes it.","labels":[{"name":"bug"}]}"#;

    fn remote() -> Remote {
        Remote {
            provider: "pulls".into(),
            name: "origin".into(),
            url: "git@forge.example.com:ada/app.git".into(),
            host: "forge.example.com".into(),
            path: "ada/app".into(),
            account: Some("forge".into()),
        }
    }

    #[test]
    fn lists_and_shows_pull_requests_through_the_account() {
        let host = MockHost::new();
        host.http("GET", &format!("{API}/pulls?state=open&sort=recentupdate&limit=50"), 200, format!("[{PULL}]"));
        host.http("GET", &format!("{API}/pulls/3"), 200, PULL);
        host.activate::<TYPE>().unwrap();
        let list = host.list_pull_requests(remote()).unwrap();
        assert_eq!((list[0].reference.as_str(), list[0].title.as_str(), list[0].author.as_str()), ("#3", "Fix the parser", "ada"));
        let Ok(PullAnswer::Details(details)) = host.pull_requests(PullQuery::Details(remote(), "3".into())) else { panic!("no details") };
        assert_eq!((details.description.as_str(), details.labels.as_slice()), ("Fixes it.", ["bug".to_string()].as_slice()));
        assert!(host.denied().is_empty(), "every call is allowed by the manifest");
    }

    #[test]
    fn a_declined_sign_in_is_an_error_not_a_list() {
        let host = MockHost::new();
        host.activate::<TYPE>().unwrap();
        host.decline_sign_in("forge");
        assert!(matches!(host.list_pull_requests(remote()), Err(Error::Cancelled)));
    }
}
