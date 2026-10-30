# Gitea Pull Requests

A pull request provider (`[[pull-requests]]`) for Codeberg, which runs Forgejo, a Gitea fork with the same API. Open a
repository whose remote is on `codeberg.org` and run **GitHub: Pull Requests…**: its open pull requests appear in the
list next to GitHub's. Picking one shows its details and comments, its changed files as diffs, adds a comment, or
checks it out.

Every call to Codeberg goes through `ctx.fetch_as` with the remote's account: Codeyard signs the request, so the
extension never sees the token. Before you run it, register an OAuth application on Codeberg (Settings › Applications)
with the redirect URI `codeyard://ext/codeyard.gitea-pulls/callback` and put its client id in `codeyard-extension.toml`.
For another Gitea or Forgejo server, add its host to `hosts` in both `[[oauth]]` and `[[pull-requests]]`.

```sh
cargo test     # unit tests, against a mock Codeyard with HTTP fixtures
cyx test       # the built extension, in the real extension host
cyx dev        # run it in Codeyard; saving a file rebuilds and reloads it
```

You need Rust and its WebAssembly target: `rustup target add wasm32-wasip2`.
