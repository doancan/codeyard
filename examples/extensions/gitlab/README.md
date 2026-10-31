# GitLab

A pull request provider (`[[pull-requests]]`) for GitLab, in TypeScript. Open a repository whose remote is on
`gitlab.com` and run **GitHub: Pull Requests…**: its open merge requests appear in the list next to GitHub's. Picking
one shows its description, head pipeline, labels and comments, its changed files as diffs, adds a comment, or checks it
out (`refs/merge-requests/<iid>/head` into a branch `mr-<iid>`, forks included). **GitLab: Sign In** signs in ahead of
time; otherwise the first list asks.

Every call to GitLab goes through `ctx.fetchAs(remote.account, …)`: Codeyard signs the request with your GitLab
account, so the extension never sees the token, and only the hosts of the account's `[[oauth]]` table get it.

## Before you run it

Register an OAuth application on GitLab (User Settings › Applications): not confidential, scope `api` (reading merge
requests and posting comments), redirect URI `codeyard://ext/codeyard.gitlab/callback`. Put its application id in
`client-id` in `codeyard-extension.toml`. No secret is needed: Codeyard signs in with PKCE.

## A self-managed GitLab

The OAuth endpoints, the application id and the hosts are part of the manifest, so a self-managed instance is a second
provider in it. The code already registers one under the id `gitlab-self-managed`:

1. Register an OAuth application on your instance as above.
2. In `codeyard-extension.toml`, uncomment the `[[oauth]]` and `[[pull-requests]]` tables at the end, replace
   `gitlab.example.com` with your instance's host and put in the application id.
3. Add it to the permission: `auth = ["gitlab", "gitlab-self-managed"]`.
4. `cyx package`, then install the `.cyx` with **Install from File…** in the Extensions view, which shows its
   permissions first (`codeyard --install-extension` refuses a code extension that asks for any).

The API calls use the remote's host (`https://<host>/api/v4/…`), so nothing else changes. A user setting for more
hosts, without repackaging, is not there yet.

## Run it

```sh
npm install    # the SDK, its types and TypeScript
npm test       # unit tests against a mock Codeyard, with fixture responses: no network
npm run check  # type-check
cyx test       # the built extension in the real extension host (which has no network)
cyx dev        # run it in Codeyard
cyx package    # write codeyard.gitlab-0.1.0.cyx
```

Why TypeScript: the Rust path already has `gitea-pulls`, so this one exercises the TypeScript side of the provider API
(`ctx.pullRequests`, the `MockHost` pull request helpers), and GitLab's JSON maps onto it without any types of its own.
