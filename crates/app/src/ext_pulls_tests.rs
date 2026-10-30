use ext_api::value as v;

use super::*;

fn provider(hosts: &[&str]) -> PullRequestProvider {
    PullRequestProvider { id: "a.b.forge".into(), label: "Forge".into(), hosts: hosts.iter().map(|h| h.to_string()).collect(), auth: Some("forge".into()) }
}

#[test]
fn remotes_in_every_common_form() {
    let p = |u: &str| parse_remote(u).map(|(h, p)| format!("{h} {p}"));
    assert_eq!(p("https://Codeberg.org/ada/app.git").as_deref(), Some("codeberg.org ada/app"));
    assert_eq!(p("https://user@git.example.com:8443/group/sub/app/").as_deref(), Some("git.example.com group/sub/app"));
    assert_eq!(p("ssh://git@git.example.com:2222/group/app.git").as_deref(), Some("git.example.com group/app"));
    assert_eq!(p("git@gitlab.com:group/sub/app.git").as_deref(), Some("gitlab.com group/sub/app"));
    assert_eq!(p("gitlab.com:group/app").as_deref(), Some("gitlab.com group/app"));
    for none in
        ["/srv/git/app.git", "./app", "file:///srv/app.git", "C:\\repo", "https://example.com/app", "https://x/a/b", "git@host.com:-x/y", "ext::sh -c x"]
    {
        assert_eq!(p(none), None, "{none}");
    }
}

#[test]
fn a_provider_claims_remotes_on_its_hosts_and_gets_its_account() {
    let forge = provider(&["codeberg.org", "*.forge.example.com"]);
    let r = claimed(&forge, "forge", "origin", "git@codeberg.org:ada/app.git").unwrap();
    assert_eq!((r.provider.as_str(), r.host.as_str(), r.path.as_str(), r.account.as_deref()), ("forge", "codeberg.org", "ada/app", Some("forge")));
    assert!(claimed(&forge, "forge", "up", "https://git.forge.example.com/a/b").is_some());
    // A token in the remote's URL stays here.
    let r = claimed(&forge, "forge", "origin", "https://ada:glpat-SECRET@codeberg.org:8443/ada/app.git").unwrap();
    assert_eq!(r.url, "https://codeberg.org:8443/ada/app.git");
    assert!(claimed(&forge, "forge", "gh", "https://github.com/a/b").is_none());
}

fn summary() -> v::PullSummary {
    v::PullSummary {
        id: "7".into(),
        reference: "!7".into(),
        title: "Fix\u{202e} the\nthing".into(),
        author: "ada".into(),
        state: v::PullState::Draft,
        source_branch: "topic".into(),
        target_branch: "main".into(),
        updated: 0,
        url: Some("javascript:alert(1)".into()),
    }
}

#[test]
fn text_from_a_provider_is_cleaned_before_it_is_shown() {
    let list = clean(vec![summary(); MAX_PULLS + 3]);
    assert_eq!(list.len(), MAX_PULLS);
    assert_eq!(list[0].title, "Fix the thing", "no bidirectional override, one line");
    assert_eq!(list[0].url, None, "only https links");
    assert_eq!(shown(&"x".repeat(300), 4), "xxxx…");
    assert_eq!(detail(&list[0], "Forge", 0), "Forge · ada · topic → main · draft");
}

#[test]
fn the_details_page_has_everything_and_cannot_break_its_layout() {
    let d = v::PullDetails {
        summary: v::PullSummary { url: Some("https://forge.example.com/7".into()), title: "# Not `a` heading".into(), ..summary() },
        description: "Body **bold**".into(),
        checks: vec![v::PullCheck { name: "ci".into(), state: v::CheckState::Failure, url: Some("https://ci.example.com/1".into()) }],
        labels: vec!["bug".into()],
    };
    let comments = [v::PullComment { author: "bob".into(), body: "Looks good\nmostly".into(), created: 0, path: Some("a.rs".into()), line: Some(3) }];
    let md = details_markdown(&d, &comments, "Forge", 0);
    assert!(md.starts_with("# !7 # Not  a  heading\n"), "{md}");
    for want in [
        "**draft** · Forge · ada · `topic` → `main`",
        "<https://forge.example.com/7>",
        "Labels: `bug`",
        "- ✗ [ci](https://ci.example.com/1)",
        "Body **bold**",
        "## Comments (1)",
        "**bob** · `a.rs:3`",
        "> Looks good\n> mostly",
    ] {
        assert!(md.contains(want), "{want}\n{md}");
    }
    let empty = details_markdown(&v::PullDetails { description: String::new(), checks: vec![], labels: vec![], ..d }, &[], "Forge", 0);
    assert!(empty.contains("No description.") && empty.contains("## Comments (0)") && !empty.contains("## Checks"));
}

#[test]
fn a_checkout_answer_is_checked_before_git_runs() {
    let c = |r: &str, b: &str, u: Option<&str>| checkout_plan(&v::PullCheckout { fetch_url: u.map(str::to_string), r#ref: r.into(), branch: b.into() });
    assert!(c("refs/merge-requests/7/head", "mr-7", None).is_ok());
    assert!(c("topic", "pr-7", Some("https://forge.example.com/fork/app.git")).is_ok());
    assert!(c("topic", "pr-7", Some("git@forge.example.com:fork/app.git")).is_ok());
    for (r, b) in [("--upload-pack=x", "b"), ("a..b", "b"), ("a b", "b"), ("ok", "-b"), ("ok", "refs/heads/x"), ("ok", "x.lock"), ("", "b")] {
        assert!(c(r, b, None).is_err(), "{r} {b}");
    }
    for u in ["ext::sh -c touch% /tmp/x", "file:///etc", "/srv/app.git", "-oProxyCommand=x", "http://forge.example.com/a", "fd::x"] {
        assert!(c("topic", "b", Some(u)).is_err(), "{u}");
    }
}
