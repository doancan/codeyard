# Releasing

The `release.yml` workflow builds and publishes a release when a `vX.Y.Z` tag is pushed. It builds the agents separately and then runs the platform builds in parallel:

1. `verify` checks that the tag, the Cargo version and the changelog agree, then runs `arch-check`, `docs-check`, `licenses.sh`,
   the signing self-test and a probe of the `UPDATE_SIGNING_KEY` secret (see [Signing](#signing-and-the-update-key)).
2. `agents-linux` and `agent-macos` build the SSH agents independently. `macos` and `windows` wait for these and `verify`, then start in parallel.
3. `macos` builds the app, DMG and tar.gz and uploads them with the release notes as artifacts.
4. `windows` builds the ZIP and the setup EXE from those agents and uploads them as an artifact.
5. `publish` merges the checksums, adds `RELEASE_NOTES.md`, verifies every file against them, signs `SHA256SUMS.txt`, checks the
   signature and creates the release in one step.

**Build caches.** Every build job restores a `rust-cache` of the dependencies, so a release compiles only Codeyard's own crates (a few
minutes) instead of GPUI and everything under it (about 10). A tag can read the caches of `main` but its own run is the first to see
a changed `Cargo.lock`, so the workflow also runs as a dry run on every push to `main` that changes `Cargo.lock`, `vendor/` or the
workflow itself, and saves the caches there. After such a change, wait for that run to finish before you tag; if you tag sooner the
release is just as correct, only slower.

You can run the whole workflow without a tag (Actions, release, Run workflow): it builds and checks everything, keeps the files
as artifacts and creates no release (it signs too, so the signing path is exercised). Do that before the first tag of a new platform or a changed script.

Nothing is published before all of them succeed, so a failed Windows build leaves no half release: fix it and push the tag
again (delete the tag first with `git push origin :refs/tags/vX.Y.Z`; no release exists yet to remove). The steps below are
the same procedure by hand, for when the workflow cannot be used.

Release jobs cache Rust dependencies. Changes to Cargo.lock, vendor files or the release workflow on main also run a dry run
so tag builds can restore the default branch cache. App and agent jobs use separate cache keys.

## Windows cross-compilation benchmark

`.github/workflows/benchmark-windows.yml` compares native `windows-2022` with Linux `cargo-xwin` using the same commit,
release profile, x64 MSVC target, embedded Linux agents and FXC-generated shader bytes. Each job starts without a target
cache, measures a cold build, removes only the app package, and measures a build with compiled dependencies retained.
Linux uses LLVM 19 MSVC driver aliases and cargo-xwin 0.23.1; its x64 SDK cache is saved before compilation, even if a later
build fails. Only SDK files are restored: compiled Rust targets are never restored for this benchmark. The optional
`host=linux` workflow input reruns only the Linux arm when investigating cross toolchains. `host=prepare` checks shader/agent
preparation and upload/download roundtrips without compiling either app. Release and benchmark checkout/artifact/cache actions
use commit-pinned Node.js 24 versions, so their jobs do not rely on the retired Node.js 20 runtime.
SDK/tool installation and dependency downloads are separate steps, outside the compiler timer; include them when comparing
whole job latency. Runner CPU counts and Rust versions are recorded in `benchmark.json`. These are runner comparisons,
not a controlled comparison on identical hardware. Both executables undergo native Windows standalone checks.

The Linux path needs `GPUI_PRECOMPILED_SHADERS` pointing to `shaders_bytes.rs` generated from this checkout's vendored
GPUI HLSL by FXC. The benchmark generates it with the vendored build helper on Windows. GPUI's Windows manifest and the
app icon are still compiled into the cross-built EXE. This benchmark does not publish artifacts as a release or replace
the production Windows ZIP/installer job; installer packaging and interactive UI validation remain separate.

### Measured on 2026-10-26

Both hosted runners had 4 CPUs and Rust 1.99.1. The measured commits differ only in benchmark/workflow setup; the
Cargo manifests/lockfile, crates and vendor source trees have the same SHA-256, and both embedded agents and shader bytes
were verified identical. Raw results and hashes: [windows-msvc-2026-10-26.json](benchmarks/windows-msvc-2026-10-26.json).

| Compilation | Native Windows 2022 | Linux → Windows MSVC | Linux time reduction |
| --- | ---: | ---: | ---: |
| Cold Rust target directory | 674.359 s (11:14) | 375.218 s (6:15) | 44.4% |
| Dependencies retained, app rebuilt | 297.110 s (4:57) | 149.485 s (2:29) | 49.7% |

The [native Windows job](https://github.com/doancan/codeyard/actions/runs/40294887424/job/121478806732) passed; that overall
run failed in its initial Linux arm, so only its successful native job supplies the Windows measurements. The final
[Linux run](https://github.com/doancan/codeyard/actions/runs/40301532106) passed both builds and validated the cross-built
EXE on real Windows (GUI subsystem, application icon, embedded agents). Interactive rendering was not tested.

These compiler times exclude downloads and tool installation. The first SDK download took 12:04; the successful Linux
run restored the SDK cache and installed tools in 28 seconds. Its build job, including both compilations, took 9:48 plus
18 seconds for native Windows verification; the native Windows build job took 17:28 including its standalone check.
Neither compiler benchmark includes ZIP/installer packaging. These single samples compare hosted runners, not identical
hardware, and do not guarantee a fixed speedup on other runs.

The production release continues to package on Windows while macOS and Windows run in parallel. A full
[Node.js 24 release dry run](https://github.com/doancan/codeyard/actions/runs/40299040677) passed in 9:44 with dependency
caches, including DMG, Windows ZIP/setup, merged checksums and throw-away-key signature verification. No release was
published. The [preparation check](https://github.com/doancan/codeyard/actions/runs/40299036873) passed with zero Node.js 20
warnings in `shaders` and `agents`; all release jobs also had zero such warnings.

## Steps

1. Run all gates (`scripts/check.sh --strict --budgets`, see `testing.md`), including `scripts/docs-check.sh` (it fails if the changelog has no section for the new version).
2. Bump the version: `version` in the root `Cargo.toml` and every path dependency's `version = "x.y.z"` in
   `crates/*/Cargo.toml` (`cargo-deny` rejects wildcard dependencies):
   `sed -i '' 's/version = "OLD", path/version = "NEW", path/' crates/*/Cargo.toml` (macOS `sed`; GNU `sed` takes `-i` without
   the `''`).
3. Add a `## [x.y.z] - date` section to `CHANGELOG.md` (move the "Unreleased" entries into it).
4. Commit, tag `vX.Y.Z` (annotated), push `main` and the tag.
5. Build the agents: `rustup target add x86_64-unknown-linux-musl aarch64-unknown-linux-musl`, then for each of the two targets
   `CARGO_TARGET_<TRIPLE>_LINKER="$(dirname "$(rustc --print target-libdir)")/bin/rust-lld" cargo build --locked --profile release-agent -p agent --target <triple>`
   (the variable name is the triple in capitals with `_` for `-`; no zig needed), plus
   `cargo build --profile release-agent -p agent` for macOS; copy them to `agents/codeyard-agent-<triple>`.
   On Windows, `./scripts/build-windows.ps1` builds the two static Linux agents using Rust's musl targets and bundled
   linker (no WSL or Docker needed), then embeds them in the application. Use `-Profile Release` for an optimized build.
6. `AGENTS_DIR="$PWD/agents" scripts/bundle-macos.sh` builds the app, the bundle and the DMG.
7. Collect `dist/`: `Codeyard-X.Y.Z-macos-arm64.dmg`, `codeyard-X.Y.Z-macos-arm64.tar.gz` (binary, `agents/` and `licenses/`).
8. Release notes: the CHANGELOG section plus the install paragraph (the app is not notarized yet: right-click →
   Open, or `xattr -cr /Applications/Codeyard.app`). Save them as `dist/RELEASE_NOTES.md`: the same file is the release body
   and the signed notes the app shows.
9. Checksums and signature: `(cd dist && sha256sum * > ../SHA256SUMS.txt && mv ../SHA256SUMS.txt .)` (every asset except
   `SHA256SUMS.txt` and its `.sig`; add the Windows files first if you have them), then
   `UPDATE_SIGNING_KEY="$(cat ~/.codeyard-release-key/update-signing-key.pem)" scripts/sign-release.sh dist` and
   `scripts/verify-release.sh --dir dist`. Use Homebrew OpenSSL 3 (`/opt/homebrew/bin`) on macOS: the system LibreSSL cannot sign.
10. `gh release create vX.Y.Z dist/* --title "vX.Y.Z" --notes-file dist/RELEASE_NOTES.md --verify-tag`. The title style is `v0.1.0`.
11. Delete the untracked `agents/` and `dist/`.
12. **Windows files.** The Windows ZIP and setup EXE are only built by the `windows` job of `release.yml`, which needs a Windows
    machine. The macOS steps above publish **no** Windows assets, so without the workflow the README's Windows download
    links point at an older release or at nothing. Either run the job's steps on a Windows PC
    (copy `agents/` there, `$env:AGENTS_DIR = (Resolve-Path agents).Path; ./scripts/bundle-windows.ps1`, then copy the ZIP and
    the setup EXE into `dist/` before step 9, so they are in the signed `SHA256SUMS.txt`), or say in the
    release notes that this version has no Windows build.

The Windows packaging script builds both Linux agents automatically unless `AGENTS_DIR` supplies a prebuilt bundle
(release CI supplies the macOS agents too). It validates their ELF architecture and embeds the agents in the EXE:
users do not need a separate `agents/` directory. Windows release builds fail if either Linux agent is missing.
Before packaging, a smoke test copies only the EXE to an empty directory and runs `--check-agent-bundle` without
starting the UI, reading user state or contacting a server. App updates therefore update the embedded agents together.

## Licence texts

Each release artifact ships the files listed in `scripts/licenses.sh` (the project licences, `THIRD_PARTY_NOTICES.md`,
`THIRD_PARTY_CRATES.md` and the licences of icons, grammars, dictionary and vendored crates): in `Contents/Resources/licenses/`
and the DMG's `Licenses/` on macOS, and in a `licenses/` folder in the tar.gz, the Windows ZIP and the installer. The script
fails when a file is missing or the Windows script or installer lacks one. After a dependency change run
`scripts/crate-licenses.py` to refresh `THIRD_PARTY_CRATES.md`.

## Windows installer

`installer/windows/codeyard.iss` (Inno Setup) installs per user without admin rights, optionally adds PATH, a desktop
shortcut and "Open with Codeyard". `scripts/bundle-windows.ps1` builds the setup executable when Inno Setup is
installed. The standalone EXE check and ZIP/setup generation have been run natively on Windows; interactive installation
is a separate manual check.

## Apple signing on GitHub Actions

Tag releases require Developer ID signing and Apple notarization in the macOS job. A normal manual/main-branch dry run
stays ad-hoc signed and receives no Apple credentials. To test the real Apple path without publishing, run `release` on
`main` or `develop` with the **notarize** input enabled. A missing secret fails verification before the platform app builds.
The first real Apple-signed run is still pending credential configuration; the scripts have been checked with mocked
Apple tools, not a live notarization submission.

Configure these under **GitHub repository Settings → Secrets and variables → Actions → New repository secret**:

| Secret | Value |
| --- | --- |
| `MACOS_CERTIFICATE_P12` | Base64-encoded `.p12` containing only the Developer ID Application certificate and its private key |
| `MACOS_CERTIFICATE_PASSWORD` | Password protecting that `.p12` |
| `APPLE_ID` | Apple account email used for notarization |
| `APPLE_APP_SPECIFIC_PASSWORD` | An app-specific password for that account; not the account login password |
| `APPLE_TEAM_ID` | Team identifier matching the Developer ID Application certificate |

Export only the selected **Developer ID Application** identity from Keychain Access, including its private key, to a
password-protected `.p12` **outside the repository**. Use GitHub's Secrets UI, or stream the base64 value directly:

```sh
base64 -i /secure/path/DeveloperID.p12 | tr -d '\n' | gh secret set MACOS_CERTIFICATE_P12 --repo doancan/codeyard
```

Enter the remaining values using the Secrets UI or `gh secret set NAME --repo doancan/codeyard` interactively. Do not put
secret values in shell command arguments/history, chat, source files or workflow YAML. Do not export a whole keychain.
The local signing identity is not automatically exported or uploaded by these scripts.

`macos-signing-keychain.sh` imports the identity into a temporary keychain under `RUNNER_TEMP` with a random password,
checks that exactly one Developer ID Application identity matches the Team ID, and stores notarization credentials in
that keychain. The `.p12` is removed even if setup fails. The workflow's `always()` cleanup restores the keychain search
list and deletes the temporary keychain. Credentials are not written under the checkout, included in the cargo cache,
or uploaded as artifacts. Only the identity fingerprint and temporary keychain path are exported to subsequent steps.
Credential-file extensions are ignored by Git; the existing update **public** key remains tracked.

The build signs nested macOS agents before the app, with hardened runtime and secure timestamps, and signs the DMG.
The portable raw binary and its macOS agents are signed separately. `notarize-macos.sh` submits the DMG and a temporary
ZIP of the exact portable directory; it requires `Accepted` for both, including when a rejected submission exits zero.
It saves submission IDs/logs under ignored `target/notary-logs` for diagnosis, staples and validates the DMG, and checks
the DMG and app with Gatekeeper. Raw executable tickets cannot be stapled and require Apple's online ticket lookup.
Only after success does the workflow create the portable tar.gz and calculate checksums. The separate Ed25519 update
signature is added by `publish` afterwards. A failed signing/notarization step prevents publication of every platform.

## Signing and the update key

Every release carries two extra assets next to the downloads:

- `SHA256SUMS.txt` lists the SHA-256 of every other asset, including `RELEASE_NOTES.md` (it does not list itself or its signature);
- `SHA256SUMS.txt.sig` is the raw 64-byte Ed25519 signature over the exact bytes of `SHA256SUMS.txt`.

The in-app updater trusts only that signature. It fetches `SHA256SUMS.txt` and its `.sig` from `releases/latest/download/`, checks
the signature against the public keys compiled into the app (`TRUSTED_KEYS`), then fetches `RELEASE_NOTES.md` and, when the user
agrees, the installer from `releases/download/v<version>/` (so the tag must be `v` plus the version in the file names) and checks the
hash of each.

The list has a strict format, checked by the app and by `scripts/verify-release.sh` alike: `<64 lowercase hex>`, two spaces, a name of
`A-Za-z0-9._-`, LF line ends, no name twice; the versions in the `Codeyard-<x.y.z>-…` names are one version; an installer (the `.dmg`
or the setup `.exe`) and `RELEASE_NOTES.md` are listed. A release that is published by hand (a Windows-only release, say) is fine as
long as its list follows these rules and is signed with `scripts/sign-release.sh`. `RELEASE_NOTES.md` is the file used as the release body, so the app shows what the release page
shows.

The public key is in the repo and the app: `scripts/update-public-key.pem`, raw 32 bytes in hex
`47bc1afc0c2c70a4a72ffd0b5ba535397bd9ca83551dfc30a12d64d7cf96d67c`. The private key is a PKCS#8 PEM in the repository secret
`UPDATE_SIGNING_KEY`. Only the signing step of `publish` (and the probe in `verify`) gets it, through that step's `env`, and only in a
run for a tag: a manual dry run signs with a key made on the spot and verifies against its public half, because files signed with the
real key are accepted by every installed app, whichever branch built them;
`scripts/sign-release.sh` writes it to a mode 0600 temp file for one `openssl` call and deletes it on exit. Never print it, commit
it or put it in an artifact.

`scripts/sign-release.sh <dir>` signs and verifies with the repo public key, so a wrong secret fails the job before anything is
published. A tag build fails when the secret is empty or does not match the public key; the `verify` job checks that first, so the
failure comes in a minute and not after the builds. OpenSSL 3 is required (`pkeyutl -rawin`); `ubuntu-latest` has it, macOS
`/usr/bin/openssl` is LibreSSL and does not.

### Verify a download

    scripts/verify-release.sh v0.2.0                 # downloads the assets with gh, checks signature and every hash
    scripts/verify-release.sh --dir ~/Downloads/rel  # the same for a folder you already have

It checks the signature with `scripts/update-public-key.pem` (`--key <pem>` trusts another key; the tests use it), then every hash,
and fails if the folder holds a file that `SHA256SUMS.txt` does not list. `scripts/verify-release.sh --self-test` runs in CI on every
push: it signs a made-up release with a throw-away key and checks that a good release passes and that a changed asset, a changed
`SHA256SUMS.txt`, another key's signature, a missing or truncated `.sig`, a missing or unlisted asset, a path outside the folder, a
list with mixed versions, a `*` marker, one or three spaces, upper-case hex, CRLF, a duplicate, a name with a space, no installer or
no release notes all fail. By hand, without the script:
`openssl pkeyutl -verify -pubin -inkey scripts/update-public-key.pem -rawin -in SHA256SUMS.txt -sigfile SHA256SUMS.txt.sig`.

### Where the key lives, and the backup

The maintainer's copy is `~/.codeyard-release-key/update-signing-key.pem` (mode 0600; the public key and its hex are next to it).
**Store a copy in a password manager now.** If the key is lost, no new release can be signed with a key the installed apps trust:
they stop updating and users must download and install a new version by hand. The GitHub secret cannot be read back, so it is not
a backup.

### Key rotation

1. Generate a new pair: `openssl genpkey -algorithm ed25519 -out new.pem`, `openssl pkey -in new.pem -pubout -out new-pub.pem`.
2. Add the new public key to `TRUSTED_KEYS` in the app and ship that in a normal release, still signed with the old key.
3. Wait until users have had time to update. Apps without the new key cannot follow the switch.
4. Replace the secret `UPDATE_SIGNING_KEY` with the new key and `scripts/update-public-key.pem` with the new public key (the
   workflow checks the signature against that file), and release again. Keep the old key in `TRUSTED_KEYS` for a transition.
5. In a later release remove the old key from `TRUSTED_KEYS` and destroy the old private key.

If the old key may be compromised, skip the wait: ship the new key and the switch together, say so in the release notes and in
`SECURITY.md`, and tell users to reinstall from a download they have verified.
