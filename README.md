<p align="center">
  <img src="crates/app/assets/macos/codeyard.svg" width="128" alt="Codeyard logo">
</p>

<h1 align="center">Codeyard</h1>

<p align="center">
  A small native code editor for local folders and remote servers over SSH.<br>
  Written in Rust on GPUI. No Electron, no Node, no telemetry.
</p>

<p align="center">
  <a href="LICENSE-MIT"><img src="https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue" alt="License: MIT or Apache-2.0"></a>
  <img src="https://img.shields.io/badge/status-beta-orange" alt="Status: beta">
  <img src="https://img.shields.io/badge/platforms-macOS%20%7C%20Windows-lightgrey" alt="Platforms: macOS and Windows">
</p>

<p align="center">
  <img src="docs/screenshots/hero.png" width="900" alt="Codeyard editing a Rust file with the explorer, outline and timeline open (dark theme)">
</p>

> **Beta.** Codeyard is used daily on macOS, but it is young software. Expect rough edges and keep your work under version control.
> Windows is released and passes its native checks, with some things not yet verified on real hardware. Linux is not released.
> Details are in [Status](#status). Bug reports are welcome; see [Troubleshooting](docs/troubleshooting.md) for how to file a useful one.

## Why Codeyard?

Codeyard is for people who want one editor that works the same on a local folder and on a server, and who want the everyday
tools built in instead of assembled from a pile of extensions.

What it is not:

- **Complete core, optional extensions.** Features that people usually install an extension for (schema validation, spell
  check, a merge editor, `.http` requests, a debugger client) are built in, and Codeyard is complete without any extension.
  Extensions are in development: they will add themes, languages, commands and integrations, run sandboxed with only the
  permissions they declare, and cost nothing until used. The current release does not load extensions yet; see the
  [design](docs/plugin-architecture.md).
- **No telemetry, no account.** Requests go out when you start them and, if you choose daily update checks, when Codeyard looks for
  a new version: at most one successful check a day, which downloads two small files from github.com (the checksum list of the
  latest release and its signature). GitHub sees your IP address and that the request comes from `curl`; there are no cookies, no
  identifier and nothing about your files (see [Updates](docs/user-guide.md#updates) and
  [what leaves your machine](docs/architecture.md#stack)).
- **No embedded web view.** The interface is drawn by GPUI. Markdown preview is drawn natively too.

How it sits next to other editors:

- **VS Code** has a very large extension ecosystem and many more languages and integrations. Codeyard's extensions follow the
  same model (one manifest, contribution points, a publish command) but run sandboxed, activate lazily and draw their UI natively
  instead of in web views. It imports VS Code settings, shortcuts and themes.
- **Zed** is also a native Rust editor built on GPUI. Codeyard is smaller in scope and has no collaboration and no
  AI features; its remote editing runs a small static agent on the server and uses your own `ssh`.
- **Helix** is a modal terminal editor. Codeyard is a graphical editor with conventional keys and mouse support.

Numbers: every release is measured against the previous one. `scripts/perf` runs both builds side by side and blocks the
release when startup, typing latency or memory regresses past its budget (see [Budgets and gates](docs/architecture.md#budgets-and-gates)).
`scripts/memcheck.sh` also keeps absolute memory limits: an empty window must stay under 85 MB, a 10k-line file with syntax
highlighting under 130 MB, measured with macOS `footprint` at 1280x800 @2x with the screen buffers included. The Linux SSH agent
stays under 1664 KB because it is copied to every server. The app binary has no size limit. Files over 32 MB open in a chunked
read-only view.

## Highlights

- **Local and SSH are the same thing.** Open a server folder like a local one: files, search, terminals, git and language servers
  run on the server through a tiny static agent; only results cross the network. It uses your own `ssh` and `~/.ssh/config`,
  survives dropped connections, and manages hosts, keys, forwarded ports and file transfers from the Remote Explorer.
- **The editor you expect.** Multiple cursors, folding, split views, minimap, snippets, Emmet, EditorConfig, more than 20 languages with
  tree-sitter highlighting, language servers (LSP), a Run and Debug view (DAP), a Testing view, a terminal with shell integration and
  profiles, tasks and problems, themes with enforced WCAG AA contrast.
- **Search by structure.** Find and rewrite code by its syntax tree, not its text: patterns are code with `$X` and `$$$X`
  placeholders (ast-grep style), for Rust, TypeScript, JavaScript, Python, Go and more, over local and SSH folders.
- **Git inside.** Source control with a one-click Sync button, branch graph, timeline and local history, blame, a three-way merge editor, GitHub pull requests.
- **Everyday tools built in.** JSON/YAML schema validation, spell check (English built in, more on demand), colour picker,
  `.http` requests, log follow and filter, Markdown preview, hex view.
- **Data tools.** Convert between JSON, YAML, TOML and `.env`, query a file with a jq-style language as you type, copy the path
  under the cursor, decode a JWT or a Unix timestamp. All local; see [Data tools](docs/user-guide.md#data-tools).
- **Turkish interface.** The whole interface is translated to Turkish; the system language is followed by default
  (see [Interface language](docs/user-guide.md#interface-language)).

## Install

**macOS (Apple silicon).** Download the DMG from [Releases](https://github.com/doancan/codeyard/releases), open it and drag
**Codeyard** to Applications. New tag releases use Developer ID signing and Apple notarization. Older releases published
before this change require approval in System Settings → Privacy & Security on first launch. A single runnable binary is also published as
`codeyard-<version>-macos-arm64.tar.gz`; keep its `agents/` folder next to it, SSH needs it.

**Windows (x64).** Download the setup EXE or the portable ZIP from [Releases](https://github.com/doancan/codeyard/releases); keep the
portable app's `agents/` folder next to the EXE, SSH needs it. Builds are unsigned. The native terminal uses ConPTY (Windows 10 1809
or newer). To build locally you need Rust's MSVC toolchain and the Visual Studio C++ Build Tools with the Windows SDK (GPUI needs
`fxc.exe`), then `cargo build --release --locked -p app`; `scripts\bundle-windows.ps1` makes the ZIP and, with Inno Setup 6, the
installer. `scripts\check-windows.ps1` runs the native checks. Windows SSH needs a key or ssh-agent and a known host entry, and
in-app port forwarding is unavailable there (use `ssh -L` / `ssh -R` in a terminal). See [platform status](docs/platforms.md).

**Linux** is not released yet (it builds; see [platform status](docs/platforms.md#linux-notes)).

## Quick start

```
codeyard [FOLDER | FILE] [--connect HOST] [--theme ID] [--terminal] [--search TEXT] [--panel NAME] [--no-extensions] [--extension-dev FOLDER] [--install-extension FILE.cyx|ID[@VERSION]] [--update-extensions] [--version]
```

- `⇧⌘P` opens the command palette, `⌘P` goes to a file, `⌃R` opens the workspace switcher (recent folders, SSH hosts).
- `⌘,` opens Settings; `⌘K ⌘T` picks a theme. Nothing needs to be configured to start.
- Coming from another editor? *Preferences: Import Settings and Shortcuts from Another Editor* converts what has an
  equivalent.

## Documentation

| | |
| --- | --- |
| [User guide](docs/user-guide.md) | every feature, keys, settings, SSH, debugging, schemas, merge editor |
| [Settings reference](docs/settings.md) | every setting with its type, default and meaning (generated from the code) |
| [Architecture](docs/architecture.md) | crates, workbench, stack choices, budgets |
| [Remote development](docs/remote.md) | SSH, agent protocol, reconnecting, askpass, transfers |
| [All technical documents](docs/README.md) | services, design tokens, accessibility, platforms, testing, releasing |
| [Troubleshooting](docs/troubleshooting.md) | SSH, language servers, schemas, `.http`, reporting a problem |
| [Changelog](CHANGELOG.md) | what changed in every version |
| [Contributing](CONTRIBUTING.md) · [Code of conduct](CODE_OF_CONDUCT.md) · [Security](SECURITY.md) | building, the quality gates, behaviour, reporting a vulnerability |
| [Accessibility](ACCESSIBILITY.md) | what works with a screen reader, keyboard and zoom, known gaps, reporting a barrier |

## Build from source

```
cargo run --release -p app -- [FOLDER | FILE]
cargo test --workspace
```

macOS needs Xcode with the Metal toolchain (for GPUI). Building the SSH agents for Linux needs `cargo-zigbuild` and `zig`.
`scripts/bundle-macos.sh` produces the app and the DMG. `CODEYARD_STATE_DIR` and `CODEYARD_CONFIG_DIR` point a run at throw-away
folders so it never touches your real profile. Full instructions and the gates a change must pass are in [CONTRIBUTING.md](CONTRIBUTING.md).

## Status

**Beta.** Codeyard is used daily on macOS. Windows is released (setup EXE and portable ZIP, unsigned) and
passes the native checks in `scripts\check-windows.ps1`, but real SSH authentication, mixed-DPI displays, IME input and the installer
itself have not been verified on a physical machine. Linux builds and passed the app test suite once in a container; it has no release
or installer yet. Screen-reader support (macOS and Windows) is partial and not yet tested with a screen reader (see [ACCESSIBILITY.md](ACCESSIBILITY.md)). Per-platform detail:
[platforms](docs/platforms.md).

## License

Licensed under either of [Apache License 2.0](LICENSE-APACHE) or [MIT license](LICENSE-MIT), at your option.
Unless you state otherwise, a contribution you submit is licensed the same way, without additional terms. The vendored GPUI
(`vendor/gpui`, Apache-2.0) and `xattr` (`vendor/xattr`, MIT or Apache-2.0) keep their own licence files.

## Third-party data

Licences and attributions for bundled third-party code and data are collected in [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md).
The built-in English spell-check dictionary is derived from [SCOWL](http://wordlist.sourceforge.net) (MIT and BSD style licences; the
full text is in `crates/spell/dicts/en/license`). Other dictionaries are downloaded only when you ask for them and keep their own
licence file next to them in the cache. The spell checker itself is [spellbook](https://github.com/helix-editor/spellbook) (MPL-2.0).
