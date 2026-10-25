//! GitHub pull requests (read-only): finding the repo from the remote URL, listing open requests using the `gh` CLI (if present and signed in) or the public
//! REST API (with `GITHUB_TOKEN`/`GH_TOKEN` if set). Network calls are made from this
//! machine (even when connected over SSH); git operations run on the workspace's host.

use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Repo {
    pub owner: String,
    pub name: String,
}

impl Repo {
    pub fn slug(&self) -> String {
        format!("{}/{}", self.owner, self.name)
    }

    #[cfg(test)]
    pub fn url(&self) -> String {
        format!("https://github.com/{}/{}", self.owner, self.name)
    }
}

fn valid_part(s: &str) -> bool {
    !s.is_empty() && s.len() <= 100 && !s.starts_with('-') && s.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
}

/// `https://github.com/o/r(.git)`, `git@github.com:o/r.git`, `ssh://git@github.com/o/r`, `git://…`; for multiple accounts
/// `github.com-name` ssh aliases are accepted too.
pub fn parse_remote(url: &str) -> Option<Repo> {
    let url = url.trim();
    let rest = ["https://", "http://", "ssh://", "git://"].iter().find_map(|p| url.strip_prefix(p)).or_else(|| url.strip_prefix("git@")).unwrap_or(url);
    let rest = rest.rsplit_once('@').map_or(rest, |(_, r)| r);
    let (host, path) = rest.split_once([':', '/'])?;
    let host = host.to_ascii_lowercase();
    if host != "github.com" && !host.starts_with("github.com-") && host != "www.github.com" {
        return None;
    }
    // The scp form (`host:o/r`) has no port; a port in the `ssh://host:22/o/r` form is skipped.
    let path = match path.split_once('/') {
        Some((port, tail)) if port.chars().all(|c| c.is_ascii_digit()) && !port.is_empty() => tail,
        _ => path,
    };
    let path = path.trim_matches('/').trim_end_matches(".git").trim_end_matches('/');
    let (owner, name) = path.split_once('/')?;
    let name = name.split('/').next()?;
    (valid_part(owner) && valid_part(name)).then(|| Repo { owner: owner.into(), name: name.into() })
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Checks {
    pub passed: u32,
    pub failed: u32,
    pub pending: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pull {
    pub number: u64,
    pub title: String,
    pub author: String,
    pub head: String,
    pub base: String,
    pub draft: bool,
    /// ISO 8601 (`2026-10-24T12:00:00Z`).
    pub updated: String,
    pub url: String,
    pub review: Option<String>,
    pub checks: Option<Checks>,
}

fn s(v: &Value) -> String {
    v.as_str().unwrap_or_default().to_string()
}

fn classify_check(c: &Value) -> Option<&'static str> {
    // CheckRun: status + conclusion; StatusContext: state.
    if let Some(state) = c["state"].as_str() {
        return Some(match state {
            "SUCCESS" => "pass",
            "FAILURE" | "ERROR" => "fail",
            _ => "pending",
        });
    }
    match (c["status"].as_str(), c["conclusion"].as_str()) {
        (Some("COMPLETED"), Some("SUCCESS" | "NEUTRAL" | "SKIPPED")) => Some("pass"),
        (Some("COMPLETED"), Some(_)) => Some("fail"),
        (Some(_), _) => Some("pending"),
        _ => None,
    }
}

/// `gh pr list --json number,title,author,headRefName,baseRefName,isDraft,updatedAt,url,reviewDecision,statusCheckRollup`
pub fn parse_gh(text: &str) -> Result<Vec<Pull>, String> {
    let v: Value = serde_json::from_str(text).map_err(|e| crate::i18n::tf("unexpected gh output: {}", &[&e]))?;
    let arr = v.as_array().ok_or(crate::i18n::t("unexpected gh output"))?;
    Ok(arr
        .iter()
        .filter_map(|p| {
            let checks = p["statusCheckRollup"].as_array().filter(|a| !a.is_empty()).map(|a| {
                a.iter().fold(Checks::default(), |mut acc, c| {
                    match classify_check(c) {
                        Some("pass") => acc.passed += 1,
                        Some("fail") => acc.failed += 1,
                        Some(_) => acc.pending += 1,
                        None => {}
                    }
                    acc
                })
            });
            Some(Pull {
                number: p["number"].as_u64()?,
                title: s(&p["title"]),
                author: s(&p["author"]["login"]),
                head: s(&p["headRefName"]),
                base: s(&p["baseRefName"]),
                draft: p["isDraft"].as_bool().unwrap_or(false),
                updated: s(&p["updatedAt"]),
                url: s(&p["url"]),
                review: p["reviewDecision"].as_str().filter(|r| !r.is_empty()).map(str::to_string),
                checks,
            })
        })
        .collect())
}

/// `GET /repos/{o}/{r}/pulls`
pub fn parse_api(text: &str) -> Result<Vec<Pull>, String> {
    let v: Value = serde_json::from_str(text).map_err(|e| crate::i18n::tf("unexpected GitHub response: {}", &[&e]))?;
    if let Some(msg) = v["message"].as_str() {
        return Err(msg.to_string());
    }
    let arr = v.as_array().ok_or(crate::i18n::t("unexpected GitHub response"))?;
    Ok(arr
        .iter()
        .filter_map(|p| {
            Some(Pull {
                number: p["number"].as_u64()?,
                title: s(&p["title"]),
                author: s(&p["user"]["login"]),
                head: s(&p["head"]["ref"]),
                base: s(&p["base"]["ref"]),
                draft: p["draft"].as_bool().unwrap_or(false),
                updated: s(&p["updated_at"]),
                url: s(&p["html_url"]),
                review: None,
                checks: None,
            })
        })
        .collect())
}

/// ISO 8601 UTC ("2026-10-24T12:00:00Z") → Unix seconds.
pub fn parse_iso(text: &str) -> Option<i64> {
    let t = text.trim().trim_end_matches('Z');
    let (date, time) = t.split_once('T')?;
    let mut d = date.split('-').map(|x| x.parse::<i64>().ok());
    let (y, m, day) = (d.next()??, d.next()??, d.next()??);
    let mut tm = time.split(':').map(|x| x.split('.').next().and_then(|v| v.parse::<i64>().ok()));
    let (h, mi, sec) = (tm.next()??, tm.next()??, tm.next().flatten().unwrap_or(0));
    // Civil calendar → day count (Howard Hinnant).
    let y2 = if m <= 2 { y - 1 } else { y };
    let era = y2.div_euclid(400);
    let yoe = y2 - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146097 + doe - 719468;
    Some(days * 86400 + h * 3600 + mi * 60 + sec)
}

/// "5m", "3h", "2d"… (`now`: Unix seconds); empty if not understood.
pub fn ago(iso: &str, now: i64) -> String {
    let Some(t) = parse_iso(iso) else { return String::new() };
    let d = (now - t).max(0);
    match d {
        0..=59 => crate::i18n::t("now").into(),
        60..=3599 => crate::i18n::tf("{}m", &[&(d / 60)]),
        3600..=86399 => crate::i18n::tf("{}h", &[&(d / 3600)]),
        86400..=604799 => crate::i18n::tf("{}d", &[&(d / 86400)]),
        604800..=2591999 => crate::i18n::tf("{}w", &[&(d / 604800)]),
        2592000..=31535999 => crate::i18n::tf("{}mo", &[&(d / 2592000)]),
        _ => crate::i18n::tf("{}y", &[&(d / 31536000)]),
    }
}

// --- Execution ---

pub trait Runner {
    fn run(&self, program: &str, args: &[String]) -> Result<String, String>;
    /// Like `run`, with `input` on the program's stdin (for secrets that must not show up in the process list).
    fn run_with_input(&self, program: &str, args: &[String], input: &str) -> Result<String, String>;
}

pub struct SystemRunner;

/// The longest to wait for an external command (gh, curl); a stuck network or authentication prompt must not hold the list forever.
const RUN_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

impl Runner for SystemRunner {
    fn run(&self, program: &str, args: &[String]) -> Result<String, String> {
        run_with_timeout(program, args, RUN_TIMEOUT)
    }

    fn run_with_input(&self, program: &str, args: &[String], input: &str) -> Result<String, String> {
        run_with_input(program, args, Some(input), RUN_TIMEOUT)
    }
}

fn drain<R: std::io::Read + Send + 'static>(mut pipe: Option<R>) -> std::thread::JoinHandle<Vec<u8>> {
    std::thread::spawn(move || {
        let mut buf = Vec::new();
        if let Some(p) = pipe.as_mut() {
            let _ = std::io::Read::read_to_end(p, &mut buf);
        }
        buf
    })
}

/// Runs `program` non-interactively: no input, `gh`/`git` don't ask questions, the process is killed if the time is exceeded.
pub(crate) fn run_with_timeout(program: &str, args: &[String], limit: std::time::Duration) -> Result<String, String> {
    run_with_input(program, args, None, limit)
}

/// `run_with_timeout` with optional stdin contents.
fn run_with_input(program: &str, args: &[String], input: Option<&str>, limit: std::time::Duration) -> Result<String, String> {
    use std::process::Stdio;
    let mut child = std::process::Command::new(program)
        .args(args)
        .env("GH_PROMPT_DISABLED", "1")
        .env("GH_NO_UPDATE_NOTIFIER", "1")
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(if input.is_some() { Stdio::piped() } else { Stdio::null() })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                crate::i18n::tf("`{}` is not installed or not on PATH", &[&program])
            } else {
                format!("{program}: {e}")
            }
        })?;
    // Written from its own thread as well, so a program that answers before it has read everything cannot stall us; the pipe closes with the thread.
    if let (Some(text), Some(mut pipe)) = (input.map(str::to_owned), child.stdin.take()) {
        std::thread::spawn(move || {
            let _ = std::io::Write::write_all(&mut pipe, text.as_bytes());
        });
    }
    // Read the output in separate threads: if the pipe fills, the process must not block while writing and burn the timeout for nothing.
    let out = drain(child.stdout.take());
    let err = drain(child.stderr.take());
    let started = std::time::Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(st)) => break st,
            Ok(None) if started.elapsed() >= limit => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(crate::i18n::tf(
                    "`{}` did not answer within {} seconds; check the network connection and try again",
                    &[&program, &limit.as_secs()],
                ));
            }
            Ok(None) => std::thread::sleep(std::time::Duration::from_millis(20)),
            Err(e) => return Err(format!("{program}: {e}")),
        }
    };
    let stdout = String::from_utf8_lossy(&out.join().unwrap_or_default()).into_owned();
    let stderr = String::from_utf8_lossy(&err.join().unwrap_or_default()).trim().to_string();
    if status.success() { Ok(stdout) } else { Err(if stderr.is_empty() { crate::i18n::tf("{} exited with {}", &[&program, &status]) } else { stderr }) }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// `gh` CLI: signed in, with checks and review status.
    Cli,
    /// Public API (private repos are not reachable without a token).
    Api { authenticated: bool },
}

fn gh_ready(r: &dyn Runner) -> bool {
    r.run("gh", &["auth".into(), "status".into()]).is_ok()
}

/// A single page of the list: if there are more open requests than this, the most recently updated come.
pub const PAGE: usize = 50;

/// Turns a `git fetch`/`checkout` error into a one-line message that says what to do.
pub fn explain_checkout(err: &str, number: u64, remote: &str) -> String {
    let low = err.to_lowercase();
    let first = err.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or(crate::i18n::t("git failed"));
    if low.contains("couldn't find remote ref") || low.contains("could not find remote ref") {
        crate::i18n::tf("Pull request #{} has no head on {} (it may have been closed or deleted); refresh the list", &[&number, &remote])
    } else if low.contains("would be overwritten") || low.contains("local changes") {
        crate::i18n::tf("Cannot check out #{}: you have uncommitted changes that would be overwritten. Commit or stash them, then try again", &[&number])
    } else if low.contains("authentication failed")
        || low.contains("could not read username")
        || low.contains("permission denied")
        || low.contains("terminal prompts disabled")
    {
        crate::i18n::tf("Git could not sign in to {}. Set up credentials (credential helper, SSH key or `gh auth login`) and try again", &[&remote])
    } else if low.contains("could not resolve host")
        || low.contains("unable to access")
        || low.contains("connection timed out")
        || low.contains("network is unreachable")
    {
        crate::i18n::tf("Could not reach {}: check the network connection and try again", &[&remote])
    } else if low.contains("does not appear to be a git repository") || low.contains("repository not found") {
        crate::i18n::tf("The remote “{}” is not reachable as a git repository (it may be private or renamed); check `git remote -v`", &[&remote])
    } else {
        first.to_string()
    }
}

/// Open pull requests. `gh` if present and signed in; otherwise (or on failure) the REST API.
pub fn list_pulls(r: &dyn Runner, repo: &Repo, token: Option<&str>) -> Result<(Vec<Pull>, Source), String> {
    let mut cli_error = None;
    if gh_ready(r) {
        let args: Vec<String> = [
            "pr",
            "list",
            "--repo",
            &repo.slug(),
            "--state",
            "open",
            "--limit",
            "50",
            "--json",
            "number,title,author,headRefName,baseRefName,isDraft,updatedAt,url,reviewDecision,statusCheckRollup",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        match r.run("gh", &args).and_then(|t| parse_gh(&t)) {
            Ok(p) => return Ok((p, Source::Cli)),
            Err(e) => cli_error = Some(e),
        }
    }
    let mut args: Vec<String> =
        ["-fsSL", "--max-time", "20", "-H", "Accept: application/vnd.github+json", "-H", "User-Agent: codeyard"].iter().map(|s| s.to_string()).collect();
    // The token goes in through curl's config on stdin: as an argument it would be readable by every user in `ps`.
    let config = match token {
        Some(t) => {
            args.extend(["--config".into(), "-".into()]);
            Some(bearer_config(t)?)
        }
        None => None,
    };
    args.push(format!("https://api.github.com/repos/{}/pulls?state=open&per_page=50&sort=updated&direction=desc", repo.slug()));
    let reply = match &config {
        Some(c) => r.run_with_input("curl", &args, c),
        None => r.run("curl", &args),
    };
    match reply.and_then(|t| parse_api(&t)) {
        Ok(p) => Ok((p, Source::Api { authenticated: token.is_some() })),
        Err(e) => {
            let hint = if e.contains("401") || e.contains("Bad credentials") {
                format!(" ({})", crate::i18n::t("the token was rejected: it may be expired or revoked — renew GITHUB_TOKEN/GH_TOKEN or run `gh auth login`"))
            } else if e.contains("404") || e.contains("Not Found") {
                format!(" ({})", crate::i18n::t("the repository may be private: sign in with `gh auth login` or set GITHUB_TOKEN"))
            } else if e.contains("403") || e.contains("rate limit") {
                format!(" ({})", crate::i18n::t("GitHub's rate limit for anonymous requests: sign in with `gh auth login` or set GITHUB_TOKEN"))
            } else {
                String::new()
            };
            Err(format!("{}{hint}", cli_error.map_or(e.clone(), |c| format!("{c}; {e}"))))
        }
    }
}

/// curl config line that sets the `Authorization` header. Inside the quotes `\` and `"` need escaping; a token with line breaks or
/// other control characters is refused rather than letting it add config lines (headers) of its own.
fn bearer_config(token: &str) -> Result<String, String> {
    if token.chars().any(char::is_control) {
        return Err(crate::i18n::t("the GitHub token contains characters that cannot be sent").to_string());
    }
    Ok(format!("header = \"Authorization: Bearer {}\"\n", token.replace('\\', "\\\\").replace('"', "\\\"")))
}

/// Second part of a row in the list: "name · base ← head · 2d · ✓ 5 · approved".
pub fn detail(p: &Pull, now: i64) -> String {
    let mut parts: Vec<String> = vec![p.author.clone(), format!("{} ← {}", p.base, p.head)];
    let a = ago(&p.updated, now);
    if !a.is_empty() {
        parts.push(a);
    }
    if let Some(c) = &p.checks {
        parts.push(match (c.failed, c.pending) {
            (0, 0) => format!("✓ {}", c.passed),
            (0, n) => crate::i18n::tf("… {} running", &[&n]),
            (f, _) => crate::i18n::tf("✗ {} failed", &[&f]),
        });
    }
    match p.review.as_deref() {
        Some("APPROVED") => parts.push(crate::i18n::t("approved").into()),
        Some("CHANGES_REQUESTED") => parts.push(crate::i18n::t("changes requested").into()),
        Some("REVIEW_REQUIRED") => parts.push(crate::i18n::t("review needed").into()),
        _ => {}
    }
    if p.draft {
        parts.push(crate::i18n::t("draft").into());
    }
    parts.join(" · ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repo(s: &str) -> Option<String> {
        parse_remote(s).map(|r| r.slug())
    }

    #[test]
    fn remotes_in_every_common_form() {
        for url in [
            "https://github.com/rust-lang/rust",
            "https://github.com/rust-lang/rust.git",
            "https://github.com/rust-lang/rust/",
            "https://user:token@github.com/rust-lang/rust.git",
            "git@github.com:rust-lang/rust.git",
            "git@github.com:rust-lang/rust",
            "ssh://git@github.com/rust-lang/rust.git",
            "ssh://git@github.com:22/rust-lang/rust.git",
            "git://github.com/rust-lang/rust.git",
            "git@github.com-work:rust-lang/rust.git",
            "https://www.github.com/rust-lang/rust",
            "https://GitHub.com/rust-lang/rust",
        ] {
            assert_eq!(repo(url).as_deref(), Some("rust-lang/rust"), "{url}");
        }
        for url in [
            "https://gitlab.com/a/b",
            "git@bitbucket.org:a/b.git",
            "https://github.com/onlyowner",
            "https://github.com/-bad/repo",
            "https://example.com/github.com/a/b",
            "",
            "/local/path",
            "https://github.com/a b/c",
        ] {
            assert_eq!(repo(url), None, "{url}");
        }
        assert_eq!(parse_remote("https://github.com/a/b/tree/main").unwrap().slug(), "a/b", "extra path is ignored");
        assert_eq!(parse_remote("git@github.com:a/b.git").unwrap().url(), "https://github.com/a/b");
    }

    #[test]
    fn gh_json_with_checks_and_review() {
        let json = r#"[
          {"number": 12, "title": "Add feature", "author": {"login": "ada"}, "headRefName": "feat", "baseRefName": "main", "isDraft": false,
           "updatedAt": "2026-10-24T10:00:00Z", "url": "https://github.com/o/r/pull/12", "reviewDecision": "APPROVED",
           "statusCheckRollup": [{"status": "COMPLETED", "conclusion": "SUCCESS"}, {"status": "COMPLETED", "conclusion": "SKIPPED"}, {"status": "IN_PROGRESS", "conclusion": ""}, {"state": "FAILURE"}]},
          {"number": 7, "title": "WIP", "author": {"login": "bob"}, "headRefName": "wip", "baseRefName": "dev", "isDraft": true, "updatedAt": "", "url": "u", "reviewDecision": "", "statusCheckRollup": []}
        ]"#;
        let p = parse_gh(json).unwrap();
        assert_eq!(p.len(), 2);
        assert_eq!((p[0].number, p[0].author.as_str(), p[0].head.as_str()), (12, "ada", "feat"));
        assert_eq!(p[0].checks, Some(Checks { passed: 2, failed: 1, pending: 1 }));
        assert_eq!(p[0].review.as_deref(), Some("APPROVED"));
        assert!(p[1].draft && p[1].checks.is_none() && p[1].review.is_none());
        assert!(parse_gh("not json").is_err() && parse_gh("{}").is_err());
    }

    #[test]
    fn api_json_and_error_messages() {
        let json = r#"[{"number": 3, "title": "Fix", "user": {"login": "eve"}, "head": {"ref": "fix"}, "base": {"ref": "main"}, "draft": false, "updated_at": "2026-10-20T00:00:00Z", "html_url": "https://github.com/o/r/pull/3"}]"#;
        let p = parse_api(json).unwrap();
        assert_eq!((p[0].number, p[0].author.as_str(), p[0].base.as_str()), (3, "eve", "main"));
        assert_eq!(parse_api(r#"{"message": "Not Found"}"#).unwrap_err(), "Not Found");
    }

    #[test]
    fn iso_times_and_relative_ages() {
        assert_eq!(parse_iso("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(parse_iso("2000-03-01T00:00:00Z"), Some(951_868_800));
        assert_eq!(parse_iso("2026-10-24T12:30:15Z"), Some(1_792_845_015));
        assert_eq!(parse_iso("garbage"), None);
        let now = parse_iso("2026-10-24T12:00:00Z").unwrap();
        let age = |t: &str| ago(t, now);
        assert_eq!(age("2026-10-24T11:59:30Z"), "now");
        assert_eq!(age("2026-10-24T11:55:00Z"), "5m");
        assert_eq!(age("2026-10-24T09:00:00Z"), "3h");
        assert_eq!(age("2026-10-21T12:00:00Z"), "3d");
        assert_eq!(age("2026-10-03T12:00:00Z"), "3w");
        assert_eq!(age("2026-04-24T12:00:00Z"), "6mo");
        assert_eq!(age("2024-10-24T12:00:00Z"), "2y");
        assert_eq!(age(""), "");
    }

    struct Fake {
        gh_auth: bool,
        gh: Result<String, String>,
        api: Result<String, String>,
        seen: std::sync::Mutex<Vec<String>>,
        stdin: std::sync::Mutex<Vec<String>>,
    }
    impl Runner for Fake {
        fn run_with_input(&self, program: &str, args: &[String], input: &str) -> Result<String, String> {
            self.stdin.lock().unwrap().push(input.to_string());
            self.run(program, args)
        }

        fn run(&self, program: &str, args: &[String]) -> Result<String, String> {
            self.seen.lock().unwrap().push(format!("{program} {}", args.join(" ")));
            match (program, args.first().map(String::as_str)) {
                ("gh", Some("auth")) => {
                    if self.gh_auth {
                        Ok(String::new())
                    } else {
                        Err("not logged in".into())
                    }
                }
                ("gh", _) => self.gh.clone(),
                _ => self.api.clone(),
            }
        }
    }

    fn fake(gh_auth: bool, gh: Result<&str, &str>, api: Result<&str, &str>) -> Fake {
        Fake {
            gh_auth,
            gh: gh.map(String::from).map_err(String::from),
            api: api.map(String::from).map_err(String::from),
            seen: Default::default(),
            stdin: Default::default(),
        }
    }

    #[test]
    fn listing_prefers_gh_falls_back_to_the_api_and_explains_failures() {
        let r = Repo { owner: "o".into(), name: "r".into() };
        let gh_ok = r#"[{"number": 1, "title": "t", "author": {"login": "a"}, "headRefName": "h", "baseRefName": "b", "isDraft": false, "updatedAt": "", "url": "u", "reviewDecision": null, "statusCheckRollup": []}]"#;
        let api_ok = r#"[{"number": 2, "title": "t", "user": {"login": "a"}, "head": {"ref": "h"}, "base": {"ref": "b"}, "html_url": "u"}]"#;
        let (p, src) = list_pulls(&fake(true, Ok(gh_ok), Ok(api_ok)), &r, None).unwrap();
        assert_eq!((p[0].number, src), (1, Source::Cli));
        let f = fake(false, Ok(gh_ok), Ok(api_ok));
        let (p, src) = list_pulls(&f, &r, Some("tok")).unwrap();
        assert_eq!((p[0].number, src), (2, Source::Api { authenticated: true }));
        // The token is never an argument (those show up in `ps`); it travels on stdin as curl config.
        let seen = f.seen.lock().unwrap();
        assert!(seen.iter().any(|c| c.contains("--config -") && c.contains("repos/o/r/pulls")));
        assert!(!seen.iter().any(|c| c.contains("Bearer") || c.contains("tok ") || c.ends_with(" tok")), "{seen:?}");
        assert_eq!(*f.stdin.lock().unwrap(), ["header = \"Authorization: Bearer tok\"\n"]);
        drop(seen);
        let e = list_pulls(&fake(false, Ok(gh_ok), Ok(api_ok)), &r, Some("bad\nheader = \"X: y\"")).unwrap_err();
        assert!(e.contains("cannot be sent"), "{e}");
        assert_eq!(bearer_config("a\"b\\c").unwrap(), "header = \"Authorization: Bearer a\\\"b\\\\c\"\n");
        // gh fails (no access): the API is tried next.
        let (p, _) = list_pulls(&fake(true, Err("no access"), Ok(api_ok)), &r, None).unwrap();
        assert_eq!(p[0].number, 2);
        // Both fail: the message names the cause and a way out.
        let e = list_pulls(&fake(false, Err(""), Err("curl: (22) The requested URL returned error: 404")), &r, None).unwrap_err();
        assert!(e.contains("private") && e.contains("GITHUB_TOKEN"), "{e}");
        let e = list_pulls(&fake(false, Err(""), Ok(r#"{"message": "API rate limit exceeded"}"#)), &r, None).unwrap_err();
        assert!(e.contains("rate limit"), "{e}");
    }

    /// Real curl is not needed: any program that echoes its stdin shows the input arrives and the argument list stays clean.
    #[test]
    fn input_reaches_the_program_through_stdin() {
        let args = vec!["-c".to_string(), "cat".to_string()];
        assert_eq!(run_with_input("sh", &args, Some("secret"), std::time::Duration::from_secs(5)).unwrap(), "secret");
        let e = run_with_input("sleep", &["30".into()], Some("x"), std::time::Duration::from_millis(300)).unwrap_err();
        assert!(e.contains("did not answer"), "the timeout still applies: {e}");
    }

    #[test]
    fn external_commands_time_out_and_report_missing_programs() {
        let t = std::time::Instant::now();
        let e = run_with_timeout("sleep", &["30".into()], std::time::Duration::from_millis(300)).unwrap_err();
        assert!(e.contains("did not answer") && t.elapsed().as_secs() < 10, "{e}");
        let e = run_with_timeout("codeyard-no-such-program", &[], std::time::Duration::from_secs(5)).unwrap_err();
        assert!(e.contains("not installed"), "{e}");
        assert_eq!(run_with_timeout("sh", &["-c".into(), "printf hi".into()], std::time::Duration::from_secs(5)).unwrap(), "hi");
        let e = run_with_timeout("sh", &["-c".into(), "echo boom >&2; exit 3".into()], std::time::Duration::from_secs(5)).unwrap_err();
        assert_eq!(e, "boom");
        // Large output must not deadlock the pipe.
        let big = run_with_timeout("sh", &["-c".into(), "head -c 3000000 /dev/zero | tr '\\0' 'a'".into()], std::time::Duration::from_secs(20)).unwrap();
        assert_eq!(big.len(), 3_000_000);
        // The child never gets to ask questions.
        let env = run_with_timeout("sh", &["-c".into(), "printf '%s' \"$GH_PROMPT_DISABLED$GIT_TERMINAL_PROMPT\"".into()], std::time::Duration::from_secs(5))
            .unwrap();
        assert_eq!(env, "10");
    }

    #[test]
    fn rejected_tokens_and_checkout_failures_come_with_guidance() {
        let r = Repo { owner: "o".into(), name: "r".into() };
        let e = list_pulls(&fake(false, Err(""), Ok(r#"{"message": "Bad credentials"}"#)), &r, Some("old")).unwrap_err();
        assert!(e.contains("rejected") && e.contains("gh auth login"), "{e}");
        let cases = [
            ("fatal: couldn't find remote ref pull/7/head", "no head on origin"),
            ("error: Your local changes to the following files would be overwritten by checkout:\n\tsrc/a.rs", "Commit or stash"),
            ("fatal: could not read Username for 'https://github.com': terminal prompts disabled", "could not sign in"),
            ("fatal: unable to access 'https://github.com/o/r/': Could not resolve host: github.com", "network"),
            ("fatal: 'origin' does not appear to be a git repository", "not reachable"),
            ("something odd\nsecond line", "something odd"),
        ];
        for (raw, want) in cases {
            let msg = explain_checkout(raw, 7, "origin");
            assert!(msg.contains(want) && !msg.contains('\n'), "{raw} → {msg}");
        }
    }

    #[test]
    fn detail_lines_are_compact() {
        let now = parse_iso("2026-10-24T12:00:00Z").unwrap();
        let p = Pull {
            number: 1,
            title: "t".into(),
            author: "ada".into(),
            head: "feat".into(),
            base: "main".into(),
            draft: true,
            updated: "2026-10-24T09:00:00Z".into(),
            url: String::new(),
            review: Some("CHANGES_REQUESTED".into()),
            checks: Some(Checks { passed: 4, failed: 1, pending: 0 }),
        };
        assert_eq!(detail(&p, now), "ada · main ← feat · 3h · ✗ 1 failed · changes requested · draft");
        let q = Pull { draft: false, review: None, checks: Some(Checks { passed: 3, failed: 0, pending: 0 }), ..p.clone() };
        assert_eq!(detail(&q, now), "ada · main ← feat · 3h · ✓ 3");
        let r = Pull { checks: None, updated: String::new(), ..q };
        assert_eq!(detail(&r, now), "ada · main ← feat");
    }
}
