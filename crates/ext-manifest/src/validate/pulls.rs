//! `[[pull-requests]]`: pull (merge) request providers, each claiming the repository remotes on its hosts.

use std::collections::HashSet;

use super::{C, Ctx, check_host, local_id};
use crate::raw::RawManifest;
use crate::{Contributions, ExtId, Permissions, PullRequestProvider, limits};

pub(super) fn contribute(cx: &mut Ctx, raw: &RawManifest, id: &ExtId, perms: &Permissions, out: &mut Contributions) {
    cx.count("pull-requests", &raw.pull_requests);
    let mut seen = HashSet::new();
    for p in &raw.pull_requests {
        let p = p.get_ref();
        let local = local_id(cx, "pull-requests", &p.id, &mut seen);
        let label = cx.text("pull-requests label", &p.label, limits::NAME_LEN);
        if p.hosts.get_ref().is_empty() {
            cx.err(C::BadContribution, p.hosts.span(), format!("pull-requests '{local}' claims no hosts")).fix =
                Some("list the hosts of the remotes it serves, like hosts = [\"codeberg.org\"]".into());
        }
        if let Some(extra) = p.hosts.get_ref().get(limits::PERMISSION_ENTRIES) {
            cx.err(C::BadContribution, extra.span(), format!("pull-requests '{local}' has more than {} hosts", limits::PERMISSION_ENTRIES));
        }
        let mut hosts = Vec::new();
        for h in p.hosts.get_ref() {
            if let Err(e) = check_host(h.get_ref()) {
                cx.err(C::BadContribution, h.span(), format!("pull-requests host '{}' {e}", h.get_ref()));
            } else if !hosts.contains(h.get_ref()) {
                hosts.push(h.get_ref().clone());
            }
        }
        let auth = p.auth.as_ref().map(|a| {
            if !perms.auth.contains(a.get_ref()) {
                cx.err(C::NotDeclared, a.span(), format!("pull-requests auth '{}' is not in [permissions] auth", a.get_ref())).fix =
                    Some(format!("add to [permissions]:  auth = [\"{}\"]", a.get_ref()));
            }
            a.get_ref().clone()
        });
        out.pull_requests.push(PullRequestProvider { id: id.qualify(&local), label, hosts, auth });
    }
}

#[cfg(test)]
mod tests {
    use crate::{Activation, Code, Options, parse_with};

    fn parse(rest: &str) -> Result<crate::Manifest, Vec<Code>> {
        let text = format!(
            "[extension]\nid = \"a.b\"\nversion = \"1.0.0\"\nname = \"N\"\nlicense = \"MIT\"\n[code]\nentry = \"Cargo.toml\"\n\
             [permissions]\nauth = [\"gitea\"]\n[[oauth]]\nid = \"gitea\"\nname = \"Gitea\"\nauthorize-url = \"https://codeberg.org/login/oauth/authorize\"\n\
             token-url = \"https://codeberg.org/login/oauth/access_token\"\nclient-id = \"x\"\nhosts = [\"codeberg.org\"]\n{rest}"
        );
        parse_with(&text, Options { require_why: false }).map_err(|e| e.into_iter().map(|d| d.code).collect())
    }

    #[test]
    fn pull_request_providers_claim_hosts_and_start_the_extension() {
        let m = parse("[[pull-requests]]\nid = \"gitea\"\nlabel = \"Gitea\"\nhosts = [\"codeberg.org\", \"*.gitea.example.com\"]\nauth = \"gitea\"\n").unwrap();
        let p = &m.contributions.pull_requests[0];
        assert_eq!((p.id.as_str(), p.label.as_str(), p.auth.as_deref()), ("a.b.gitea", "Gitea", Some("gitea")));
        assert!(p.claims("codeberg.org") && p.claims("Git.Gitea.Example.com") && !p.claims("gitea.example.com") && !p.claims("github.com"));
        assert_eq!(m.activation, [Activation::PullRequests("a.b.gitea".into())]);
        // No auth is allowed (a public forge); a wrong one, a missing host or a bad one is not.
        assert!(parse("[[pull-requests]]\nid = \"x\"\nlabel = \"X\"\nhosts = [\"git.example.com\"]\n").is_ok());
        assert_eq!(parse("[[pull-requests]]\nid = \"x\"\nlabel = \"X\"\nhosts = [\"git.example.com\"]\nauth = \"gitlab\"\n").unwrap_err(), [Code::NotDeclared]);
        assert_eq!(parse("[[pull-requests]]\nid = \"x\"\nlabel = \"X\"\nhosts = []\n").unwrap_err(), [Code::BadContribution]);
        assert_eq!(parse("[[pull-requests]]\nid = \"x\"\nlabel = \"X\"\nhosts = [\"https://git.example.com\"]\n").unwrap_err(), [Code::BadContribution]);
        assert_eq!(parse("[[pull-requests]]\nid = \"x\"\nlabel = \"X\"\nhosts = [\"*\"]\n").unwrap_err(), [Code::BadContribution]);
        assert_eq!(
            parse("[[pull-requests]]\nid = \"x\"\nlabel = \"X\"\nhosts = [\"a.example.com\"]\n[[pull-requests]]\nid = \"x\"\nlabel = \"Y\"\nhosts = [\"b.example.com\"]\n")
                .unwrap_err(),
            [Code::DuplicateId]
        );
        assert_eq!(parse("[[pull-requests]]\nid = \"x\"\nlabel = \"X\"\nhost = \"a.example.com\"\n").unwrap_err(), [Code::Syntax]);
    }

    #[test]
    fn pull_request_providers_need_code() {
        let text = "[extension]\nid = \"a.b\"\nversion = \"1.0.0\"\nname = \"N\"\nlicense = \"MIT\"\n[[pull-requests]]\nid = \"x\"\nlabel = \"X\"\nhosts = [\"a.example.com\"]\n";
        let codes: Vec<Code> = parse_with(text, Options { require_why: false }).unwrap_err().into_iter().map(|d| d.code).collect();
        assert_eq!(codes, [Code::NeedsCode]);
    }
}
