# Globs

The globs of `read`, `write`, `activation.workspace-contains` and `workspace.find-files`. Codeyard, the mock hosts and `cyx test` match them with the same code (`crates/ext-manifest/src/glob.rs`), so a glob means the same in a test as in Codeyard.

- A glob is matched against the whole path, relative to the workspace folder and written with `/`. `main.rs` is the file at the top of the folder; for one in any folder, write `**/main.rs`.
- `*` is any run of characters inside one part of the path, empty too, and a leading `.` too. `?` is one character.
- `**` as a whole part is any number of parts, none too: `src/**` is `src` and everything under it.
- Everything else stands for itself, `[` and `]` included: `pages/[id].tsx` is that one file.
- `.env*` files and the `.git/` folder are reached only by a glob that names them in one of its parts (`.env`, `**/.env*`, `.git/config`), never by `**`.
- A path is resolved before it is matched: `src/../../etc/passwd` and `src/%2e%2e/…` are outside the folder and refused, whatever the globs say.

`cyx check` refuses what has no meaning here, instead of letting it match nothing: `{a,b}` alternatives (list each glob), a leading `!` (globs only allow), `**` inside a part (`src/**.rs`; write `src/**/*.rs`), and `.`, `..` or empty parts. The one exception is a leading `./`, which means the same as no prefix (`./main.rs` is `main.rs`); a `.` anywhere else (`src/./a.rs`) is refused.

| Glob | Path | Matches |
| --- | --- | --- |
| `main.rs` | `main.rs` | yes |
| `main.rs` | `src/main.rs` | no |
| `**/main.rs` | `src/main.rs` | yes |
| `**/main.rs` | `main.rs` | yes |
| `src/*.rs` | `src/lib.rs` | yes |
| `src/*.rs` | `src/a/lib.rs` | no |
| `src/**/*.rs` | `src/a/b/lib.rs` | yes |
| `src/**` | `src` | yes |
| `*.json` | `.eslintrc.json` | yes |
| `?.md` | `ab.md` | no |
| `pages/[id].tsx` | `pages/[id].tsx` | yes |
