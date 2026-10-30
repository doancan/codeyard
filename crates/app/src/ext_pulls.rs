//! Pull request providers of code extensions (`[[pull-requests]]`, #145), without UI: which repository remotes a
//! provider claims, checks on what a provider answers before anything is shown or run, and the details page as
//! Markdown. The view itself is `workbench_ext_pulls.rs`.

use ext_api::value as v;
use ext_manifest::PullRequestProvider;

/// Longest title, author, branch or label shown; longer text is cut.
const TEXT_LEN: usize = 256;
/// Requests one provider may list; the rest are dropped.
pub const MAX_PULLS: usize = 500;
/// Description and comment bodies, in bytes; longer ones are cut.
const BODY_BYTES: usize = 256 * 1024;
/// Changed files shown for one request.
pub const MAX_FILES: usize = 3000;

/// The host and repository path of a remote URL: `https://host[:port]/a/b(.git)`, `ssh://[user@]host[:port]/a/b`,
/// `git://host/a/b`, `user@host:a/b` and `host:a/b`. Local paths and `file:` URLs have no host.
pub fn parse_remote(url: &str) -> Option<(String, String)> {
    let url = url.trim();
    let (rest, scp) = match ["https://", "http://", "ssh://", "git://", "git+ssh://", "ssh+git://"].iter().find_map(|p| url.strip_prefix(p)) {
        Some(r) => (r, false),
        None if url.contains("://") || url.starts_with(['/', '.', '~']) || url.as_bytes().get(1) == Some(&b':') => return None,
        None => (url, true),
    };
    let (authority, path) = if scp { rest.split_once(':')? } else { rest.split_once('/')? };
    let host = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
    let host = if scp { host } else { host.split(':').next()? };
    let host = host.to_ascii_lowercase();
    if host.is_empty() || !host.contains('.') && host != "localhost" || !host.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-') {
        return None;
    }
    let path = path.trim_matches('/');
    let path = path.strip_suffix(".git").unwrap_or(path).trim_end_matches('/');
    let parts: Vec<&str> = path.split('/').collect();
    let valid = |s: &&str| {
        !s.is_empty() && s.len() <= 100 && !s.starts_with(['-', '.']) && s.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
    };
    (parts.len() >= 2 && parts.len() <= 20 && parts.iter().all(valid)).then(|| (host, path.to_string()))
}

/// The remote as `provider` gets it, when the provider claims its host. `local` is the provider's local id.
pub fn claimed(provider: &PullRequestProvider, local: &str, name: &str, url: &str) -> Option<v::PullRemote> {
    let (host, path) = parse_remote(url)?;
    provider.claims(&host).then(|| v::PullRemote {
        provider: local.to_string(),
        name: name.to_string(),
        url: url.to_string(),
        host,
        path,
        account: provider.auth.clone(),
    })
}

/// Text from an extension as the view shows it: one line, no control or invisible characters, at most `max` characters.
pub fn shown(s: &str, max: usize) -> String {
    let one: String = s.chars().map(|c| if c.is_whitespace() { ' ' } else { c }).filter(|c| !ext_manifest::is_deceptive(*c)).collect();
    let one = one.split_whitespace().collect::<Vec<_>>().join(" ");
    match one.char_indices().nth(max) {
        Some((at, _)) => format!("{}…", &one[..at]),
        None => one,
    }
}

/// A body (Markdown) from an extension: no control or invisible characters but line breaks, at most `BODY_BYTES`.
fn body(s: &str) -> String {
    let mut out: String = s.chars().filter(|&c| c == '\n' || c == '\t' || !ext_manifest::is_deceptive(c)).collect();
    if out.len() > BODY_BYTES {
        let mut at = BODY_BYTES;
        while !out.is_char_boundary(at) {
            at -= 1;
        }
        out.truncate(at);
        out.push('…');
    }
    out
}

/// An `https:` link from an extension, or nothing.
pub fn https_url(url: &Option<String>) -> Option<String> {
    url.as_deref()
        .map(str::trim)
        .filter(|u| u.len() <= 2048 && u.starts_with("https://") && !u.chars().any(|c| c.is_whitespace() || c.is_control()))
        .map(str::to_string)
}

/// What a provider listed, cleaned for display: text cut to size, links only `https:`, at most `MAX_PULLS`.
pub fn clean(mut list: Vec<v::PullSummary>) -> Vec<v::PullSummary> {
    list.truncate(MAX_PULLS);
    for p in &mut list {
        clean_summary(p);
    }
    list
}

fn clean_summary(p: &mut v::PullSummary) {
    for s in [&mut p.reference, &mut p.title, &mut p.author, &mut p.source_branch, &mut p.target_branch] {
        *s = shown(s, TEXT_LEN);
    }
    p.id = shown(&p.id, TEXT_LEN);
    p.url = https_url(&p.url);
}

/// How a changed file changed, for the files list.
pub fn file_detail(f: &v::ChangedFile) -> String {
    match (f.status, &f.old_path) {
        (v::FileStatus::Renamed, Some(old)) => crate::i18n::tf("renamed from {}", &[&shown(old, 512)]),
        (v::FileStatus::Renamed, None) => crate::i18n::t("renamed").to_string(),
        (v::FileStatus::Added, _) => crate::i18n::t("added").to_string(),
        (v::FileStatus::Deleted, _) => crate::i18n::t("deleted").to_string(),
        (v::FileStatus::Modified, _) => crate::i18n::t("modified").to_string(),
    }
}

pub fn state_name(s: v::PullState) -> &'static str {
    match s {
        v::PullState::Open => crate::i18n::t("open"),
        v::PullState::Draft => crate::i18n::t("draft"),
        v::PullState::Merged => crate::i18n::t("merged"),
        v::PullState::Closed => crate::i18n::t("closed"),
    }
}

/// "Relative time" of Unix milliseconds, or nothing when unknown.
fn ago(ms: u64, now_secs: i64) -> Option<String> {
    (ms > 0).then(|| crate::git::ago_phrase(&crate::git::relative_time(now_secs - (ms / 1000) as i64)))
}

/// The picker's detail line: provider, author, branches, state when not open, last change.
pub fn detail(p: &v::PullSummary, label: &str, now_secs: i64) -> String {
    let mut parts = vec![label.to_string()];
    if !p.author.is_empty() {
        parts.push(p.author.clone());
    }
    parts.push(format!("{} → {}", p.source_branch, p.target_branch));
    if p.state != v::PullState::Open {
        parts.push(state_name(p.state).to_string());
    }
    parts.extend(ago(p.updated, now_secs));
    parts.join(" · ")
}

fn check_mark(s: v::CheckState) -> &'static str {
    match s {
        v::CheckState::Success => "✓",
        v::CheckState::Failure => "✗",
        v::CheckState::Pending => "…",
        v::CheckState::Neutral => "–",
    }
}

/// Markdown can link and format, but a provider's text must not close a code span or start a heading of ours.
fn inline(s: &str) -> String {
    shown(s, TEXT_LEN).replace(['`', '*', '_', '[', ']', '<', '>'], " ")
}

/// The details page: title, who and where, labels, checks, the description and the comments.
pub fn details_markdown(d: &v::PullDetails, comments: &[v::PullComment], label: &str, now_secs: i64) -> String {
    let mut s = d.summary.clone();
    clean_summary(&mut s);
    let mut out = format!("# {} {}\n\n", inline(&s.reference), inline(&s.title));
    let mut who = vec![format!("**{}**", state_name(s.state)), label.to_string()];
    if !s.author.is_empty() {
        who.push(inline(&s.author));
    }
    who.push(format!("`{}` → `{}`", inline(&s.source_branch), inline(&s.target_branch)));
    who.extend(ago(s.updated, now_secs));
    out.push_str(&who.join(" · "));
    out.push_str("\n\n");
    if let Some(url) = &s.url {
        out.push_str(&format!("<{url}>\n\n"));
    }
    let labels: Vec<String> = d.labels.iter().take(50).map(|l| format!("`{}`", inline(l))).collect();
    if !labels.is_empty() {
        out.push_str(&format!("{}: {}\n\n", crate::i18n::t("Labels"), labels.join(" ")));
    }
    if !d.checks.is_empty() {
        out.push_str(&format!("## {}\n\n", crate::i18n::t("Checks")));
        for c in d.checks.iter().take(200) {
            let name = inline(&c.name);
            match https_url(&c.url) {
                Some(u) => out.push_str(&format!("- {} [{name}]({u})\n", check_mark(c.state))),
                None => out.push_str(&format!("- {} {name}\n", check_mark(c.state))),
            }
        }
        out.push('\n');
    }
    out.push_str(&format!("## {}\n\n", crate::i18n::t("Description")));
    let description = body(&d.description);
    out.push_str(if description.trim().is_empty() { crate::i18n::t("No description.") } else { description.trim_end() });
    out.push_str("\n\n");
    out.push_str(&format!("## {}\n\n", crate::i18n::tf("Comments ({})", &[&comments.len()])));
    for c in comments.iter().take(1000) {
        let mut head = vec![format!("**{}**", inline(&c.author))];
        head.extend(ago(c.created, now_secs));
        if let Some(p) = &c.path {
            head.push(match c.line {
                Some(l) => format!("`{}:{l}`", inline(p)),
                None => format!("`{}`", inline(p)),
            });
        }
        out.push_str(&head.join(" · "));
        out.push_str("\n\n");
        for line in body(&c.body).trim_end().lines() {
            out.push_str("> ");
            out.push_str(line);
            out.push('\n');
        }
        out.push('\n');
    }
    out
}

/// A ref or branch name git takes without surprises: no option, no `..`, no spaces or special characters.
fn safe_ref(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 255
        && !s.starts_with(['-', '/', '.'])
        && !s.ends_with(['/', '.'])
        && !s.ends_with(".lock")
        && !s.contains("..")
        && !s.contains("//")
        && !s.contains("@{")
        && s.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '/' | '.' | '_' | '-' | '+'))
}

/// A URL to fetch a fork from: `https://` or ssh only, so no other transport (`ext::`, `file:`, a local path) runs.
fn safe_fetch_url(u: &str) -> bool {
    if u.len() > 2048 || u.starts_with('-') || u.chars().any(|c| c.is_whitespace() || c.is_control()) || u.contains("::") {
        return false;
    }
    if let Some(rest) = u.strip_prefix("https://").or_else(|| u.strip_prefix("ssh://")) {
        return rest.split('/').next().is_some_and(|h| !h.is_empty());
    }
    // scp form: `user@host:path`.
    match u.split_once(':') {
        Some((auth, path)) => {
            let host = auth.rsplit_once('@').map_or(auth, |(_, h)| h);
            !host.is_empty()
                && host.contains('.')
                && host.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-'))
                && !path.is_empty()
                && !path.starts_with('-')
        }
        None => false,
    }
}

/// What to fetch and where, checked; `Err` says what is wrong with the provider's answer.
pub fn checkout_plan(c: &v::PullCheckout) -> Result<(), String> {
    if !safe_ref(&c.r#ref) {
        return Err(format!("the ref to fetch, {:?}, is not a plain git ref", shown(&c.r#ref, 80)));
    }
    if !safe_ref(&c.branch) || c.branch.starts_with("refs/") {
        return Err(format!("the branch {:?} is not a plain branch name", shown(&c.branch, 80)));
    }
    if let Some(u) = &c.fetch_url
        && !safe_fetch_url(u)
    {
        return Err(format!("{:?} is not an https:// or ssh URL", shown(u, 80)));
    }
    Ok(())
}

#[cfg(test)]
#[path = "ext_pulls_tests.rs"]
mod tests;
