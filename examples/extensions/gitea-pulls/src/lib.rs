//! Gitea Pull Requests: a `[[pull-requests]]` provider for Codeberg, which runs Forgejo (Gitea's API). Codeyard asks it
//! for the open pull requests of each repository remote on `codeberg.org` and shows them in its Pull Requests view,
//! next to GitHub's. Every call to the forge goes through `ctx.fetch_as` with the remote's account: Codeyard adds the
//! token, so the extension never sees it.
//!
//! The same code serves any Gitea or Forgejo server: add its host to `hosts` in the manifest and to the `[[oauth]]`
//! table, or declare a second provider.

use codeyard_extension::prelude::*;

struct GiteaPulls;

/// The API of the remote's server: `https://<host>/api/v1/repos/<owner>/<repo>`.
fn api(remote: &Remote, rest: &str) -> String {
    format!("https://{}/api/v1/repos/{}{rest}", remote.host, remote.path)
}

/// GET the API as the remote's account, signing in on first use.
async fn get(ctx: &Context, remote: &Remote, rest: &str) -> Result<Response> {
    let account = remote.account.clone().unwrap_or_default();
    ctx.session(&account, &[]).await?;
    ctx.fetch_as(&account, Request::get(api(remote, rest)).header("accept", "application/json")).await
}

fn text(v: &Value) -> String {
    v.as_str().unwrap_or_default().to_string()
}

/// `2024-05-01T12:30:00Z` or `…+02:00` in Unix milliseconds; 0 when it does not parse.
fn millis(iso: &str) -> u64 {
    let n = |s: Option<&str>| s.and_then(|s| s.parse::<i64>().ok());
    let (date, time) = iso.split_once('T').unwrap_or((iso, "00:00:00Z"));
    let mut d = date.split('-');
    let (Some(y), Some(m), Some(day)) = (n(d.next()), n(d.next()), n(d.next())) else { return 0 };
    let (clock, offset) = match time.find(['Z', '+', '-']) {
        Some(at) => time.split_at(at),
        None => (time, "Z"),
    };
    let mut c = clock.split(':');
    let (hh, mm, ss) = (n(c.next()).unwrap_or(0), n(c.next()).unwrap_or(0), n(c.next().map(|s| s.split('.').next().unwrap_or(s))).unwrap_or(0));
    let shift = match offset.split_once(':') {
        Some((h, mins)) => {
            let sign = if h.starts_with('-') { -1 } else { 1 };
            sign * (n(Some(&h[1..])).unwrap_or(0) * 3600 + n(Some(mins)).unwrap_or(0) * 60)
        }
        None => 0,
    };
    // Days from 1970-01-01 (Howard Hinnant's days_from_civil).
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (m + if m > 2 { -3 } else { 9 }) + 2) / 5 + day - 1;
    let days = era * 146_097 + yoe * 365 + yoe / 4 - yoe / 100 + doy - 719_468;
    let secs = days * 86_400 + hh * 3600 + mm * 60 + ss - shift;
    u64::try_from(secs).map_or(0, |s| s * 1000)
}

/// A pull request as the API returns it.
fn pull(p: &Value) -> PullRequest {
    let number = p["number"].as_u64().unwrap_or_default();
    let state = if p["merged"].as_bool() == Some(true) {
        PullState::Merged
    } else if p["state"] == "closed" {
        PullState::Closed
    } else if p["draft"].as_bool() == Some(true) {
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
        updated: millis(p["updated_at"].as_str().unwrap_or_default()),
        url: p["html_url"].as_str().map(str::to_string),
    }
}

fn check_state(s: &str) -> CheckState {
    match s {
        "success" => CheckState::Success,
        "failure" | "error" => CheckState::Failure,
        "pending" => CheckState::Pending,
        _ => CheckState::Neutral,
    }
}

/// One file at `reference`, or none when it is not there (added or deleted on that side).
async fn raw(ctx: &Context, remote: &Remote, path: &str, reference: &str) -> Result<Option<String>> {
    let res = get(ctx, remote, &format!("/raw/{path}?ref={reference}")).await?;
    if res.status == 404 {
        return Ok(None);
    }
    Ok(res.error_for_status()?.text().ok())
}

impl Extension for GiteaPulls {
    fn activate(ctx: Context) -> Result<()> {
        ctx.pull_requests("codeberg")
            .list(async |ctx, remote| {
                let list: Vec<Value> = get(&ctx, &remote, "/pulls?state=open&sort=recentupdate&limit=50").await?.error_for_status()?.json()?;
                Ok(list.iter().map(pull).collect())
            })
            .details(async |ctx, remote, id| {
                let p: Value = get(&ctx, &remote, &format!("/pulls/{id}")).await?.error_for_status()?.json()?;
                let sha = text(&p["head"]["sha"]);
                let status: Value = get(&ctx, &remote, &format!("/commits/{sha}/status")).await?.json().unwrap_or(Value::Null);
                let checks = status["statuses"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|s| Check {
                        name: text(&s["context"]),
                        state: check_state(s["status"].as_str().unwrap_or_default()),
                        url: s["target_url"].as_str().map(str::to_string),
                    })
                    .collect();
                let labels = p["labels"].as_array().into_iter().flatten().map(|l| text(&l["name"])).collect();
                Ok(PullRequestDetails { summary: pull(&p), description: text(&p["body"]), checks, labels })
            })
            .files(async |ctx, remote, id| {
                let files: Vec<Value> = get(&ctx, &remote, &format!("/pulls/{id}/files")).await?.error_for_status()?.json()?;
                Ok(files
                    .iter()
                    .map(|f| ChangedFile {
                        path: text(&f["filename"]),
                        old_path: f["previous_filename"].as_str().filter(|s| !s.is_empty()).map(str::to_string),
                        status: match f["status"].as_str().unwrap_or_default() {
                            "added" => FileStatus::Added,
                            "deleted" | "removed" => FileStatus::Deleted,
                            "renamed" => FileStatus::Renamed,
                            _ => FileStatus::Modified,
                        },
                    })
                    .collect())
            })
            .contents(async |ctx, remote, id, file| {
                let p: Value = get(&ctx, &remote, &format!("/pulls/{id}")).await?.error_for_status()?.json()?;
                let base_path = file.old_path.clone().unwrap_or_else(|| file.path.clone());
                let base = match file.status {
                    FileStatus::Added => None,
                    _ => raw(&ctx, &remote, &base_path, &text(&p["base"]["sha"])).await?,
                };
                let head = match file.status {
                    FileStatus::Deleted => None,
                    _ => raw(&ctx, &remote, &file.path, &text(&p["head"]["sha"])).await?,
                };
                Ok(FileContents { base, head })
            })
            .comments(async |ctx, remote, id| {
                let list: Vec<Value> = get(&ctx, &remote, &format!("/issues/{id}/comments")).await?.error_for_status()?.json()?;
                Ok(list
                    .iter()
                    .map(|c| Comment {
                        author: text(&c["user"]["login"]),
                        body: text(&c["body"]),
                        created: millis(c["created_at"].as_str().unwrap_or_default()),
                        ..Default::default()
                    })
                    .collect())
            })
            .on_comment(async |ctx, remote, id, body| {
                let account = remote.account.clone().unwrap_or_default();
                let request = Request::post(api(&remote, &format!("/issues/{id}/comments"))).json(&json!({ "body": body }))?;
                ctx.fetch_as(&account, request).await?.error_for_status()?;
                Ok(())
            })
            // Gitea keeps every pull request's head at `refs/pull/<n>/head`, forks included.
            .checkout(async |_ctx, _remote, id| Ok(Checkout { fetch_url: None, reference: format!("refs/pull/{id}/head"), branch: format!("pr-{id}") }));
        Ok(())
    }
}

codeyard_extension::export!(GiteaPulls);

#[cfg(test)]
mod tests {
    use super::*;
    use codeyard_extension::native::Call;
    use codeyard_extension::pulls::{PullAnswer, PullQuery};
    use codeyard_extension_test::MockHost;

    const API: &str = "https://codeberg.org/api/v1/repos/ada/app";

    fn remote() -> Remote {
        Remote {
            provider: "codeberg".into(),
            name: "origin".into(),
            url: "git@codeberg.org:ada/app.git".into(),
            host: "codeberg.org".into(),
            path: "ada/app".into(),
            account: Some("codeberg".into()),
        }
    }

    fn host() -> MockHost {
        let host = MockHost::new();
        let pr = r#"{"number":3,"title":"Faster start","user":{"login":"bob"},"state":"open","merged":false,"draft":false,
            "head":{"ref":"fast","sha":"h1"},"base":{"ref":"main","sha":"b1"},"updated_at":"2024-05-01T12:30:00Z",
            "html_url":"https://codeberg.org/ada/app/pulls/3","body":"Starts **twice** as fast.","labels":[{"name":"perf"}]}"#;
        host.http("GET", &format!("{API}/pulls?state=open&sort=recentupdate&limit=50"), 200, format!("[{pr}]"));
        host.http("GET", &format!("{API}/pulls/3"), 200, pr);
        host.http(
            "GET",
            &format!("{API}/commits/h1/status"),
            200,
            r#"{"statuses":[{"context":"ci","status":"success","target_url":"https://ci.example.com/1"}]}"#,
        );
        host.http("GET", &format!("{API}/pulls/3/files"), 200, r#"[{"filename":"src/main.rs","status":"changed"},{"filename":"NEW.md","status":"added"}]"#);
        host.http("GET", &format!("{API}/raw/src/main.rs?ref=b1"), 200, "fn main() {}\n");
        host.http("GET", &format!("{API}/raw/src/main.rs?ref=h1"), 200, "fn main() { fast() }\n");
        host.http("GET", &format!("{API}/issues/3/comments"), 200, r#"[{"user":{"login":"ada"},"body":"Nice","created_at":"2024-05-02T08:00:00+02:00"}]"#);
        host.http("POST", &format!("{API}/issues/3/comments"), 201, "{}");
        host.activate::<GiteaPulls>().unwrap();
        host
    }

    #[test]
    fn lists_open_pull_requests_through_the_account() {
        let host = host();
        let list = host.list_pull_requests(remote()).unwrap();
        assert_eq!(list.len(), 1);
        let p = &list[0];
        assert_eq!((p.reference.as_str(), p.title.as_str(), p.author.as_str(), p.source_branch.as_str()), ("#3", "Faster start", "bob", "fast"));
        assert_eq!(p.updated, 1_714_566_600_000);
        // Signed by Codeyard: the request names the account and sets no authorization header.
        let fetches: Vec<Call> = host.calls().into_iter().filter(|c| matches!(c, Call::AuthFetch { .. })).collect();
        assert!(fetches.iter().all(
            |c| matches!(c, Call::AuthFetch { provider, request } if provider == "codeberg" && request.headers.iter().all(|(k, _)| k != "authorization"))
        ));
    }

    #[test]
    fn details_files_contents_comments_and_checkout() {
        let host = host();
        let ask = |q: PullQuery| host.pull_requests(q).unwrap();
        let PullAnswer::Details(d) = ask(PullQuery::Details(remote(), "3".into())) else { panic!() };
        assert_eq!((d.description.as_str(), d.labels.as_slice()), ("Starts **twice** as fast.", ["perf".to_string()].as_slice()));
        assert_eq!((d.checks[0].name.as_str(), d.checks[0].state), ("ci", CheckState::Success));
        let PullAnswer::Files(files) = ask(PullQuery::Files(remote(), "3".into())) else { panic!() };
        assert_eq!((files[0].status, files[1].status), (FileStatus::Modified, FileStatus::Added));
        let PullAnswer::Contents(c) = ask(PullQuery::Contents(remote(), "3".into(), files[0].clone())) else { panic!() };
        assert_eq!((c.base.as_deref(), c.head.as_deref()), (Some("fn main() {}\n"), Some("fn main() { fast() }\n")));
        let PullAnswer::Comments(comments) = ask(PullQuery::Comments(remote(), "3".into())) else { panic!() };
        assert_eq!((comments[0].author.as_str(), comments[0].created), ("ada", 1_714_629_600_000));
        assert_eq!(ask(PullQuery::AddComment(remote(), "3".into(), "Thanks".into())), PullAnswer::Done);
        assert_eq!(host.fetched().last().map(|r| String::from_utf8_lossy(&r.body).into_owned()).as_deref(), Some(r#"{"body":"Thanks"}"#));
        let PullAnswer::Checkout(c) = ask(PullQuery::Checkout(remote(), "3".into())) else { panic!() };
        assert_eq!((c.reference.as_str(), c.branch.as_str()), ("refs/pull/3/head", "pr-3"));
    }
}
