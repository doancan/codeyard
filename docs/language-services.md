# Managed language services

Run **Languages: Manage Language Services** from the command palette to install JavaScript/TypeScript,
HTML and CSS services on the current file host. For SSH workspaces the installer, runtime, packages and
servers run on that SSH host. The page shows the host, destination, exact service versions, current
stage and a bounded log before and during installation. Nothing is downloaded merely by opening a file
or this page.

Click Install, or press Enter while the page has focus. Escape or Cancel stops an installation before
activation; closing the page also requests cancellation. Cancellation waits for the running process
to exit before deleting its staging files. Activation finishes as a single publication step. Existing
working services remain available during installation and after a failed or cancelled attempt.

The bundled catalogue pins TypeScript 6.0.3, TypeScript Language Server 6.0.2 and
`vscode-langservers-extracted` 4.10.0. Every transitive package is locked to its npm registry URL and
SHA-512 integrity value. `npm ci` runs with lifecycle scripts, audit and funding requests disabled,
an isolated cache, and separate empty user/global npm configuration files. It never changes global
packages or workspace files.

A compatible Node.js runtime (22.22.2 or newer) with npm beside it is reused. Otherwise the installer
downloads the pinned Node.js 22.23.4 archive for macOS, Linux or Windows on x64/ARM64. The archive's
SHA-256 is verified against the bundled official checksum before extraction or execution. Downloads
require curl, tar and a SHA-256 tool (shasum on macOS, sha256sum on Linux, certutil on Windows). The
official Linux archives need glibc; musl hosts should provide a compatible Node.js installation with npm.

Installations live below `~/.codeyard/language-services` on the selected host. With a local
`CODEYARD_STATE_DIR` override they live in that profile’s `language-services` subdirectory;
remote installations always use the remote host’s own home. Each attempt gets an
unpredictable private staging directory. A completed generation is published under a unique name,
then `active.json` is replaced atomically. Earlier generations are retained. A failed generation
never replaces the active record. If the connection is lost or process cancellation cannot be
acknowledged, staging is retained for inspection rather than deleted underneath a possibly live writer.

Default server commands resolve to the active managed generation on startup. Explicit `lsp.servers`
commands and disabled languages keep their existing behavior. Installation does not grant workspace
trust; servers still start only for trusted folders. Successfully installing from the page restarts
language servers for that workspace. **Developer: Restart Language Servers** can also restart them.

When setup fails, the page shows the error and recent process output. Check the host's network,
required tools and destination permissions, then retry. After an uncertain disconnect, reconnect and
inspect the retained staging directory first. Checksum errors never activate the downloaded runtime.
No package or runtime is automatically updated in the background; catalogue changes arrive with an
application update and require another explicit installation.

## Validation

Hermetic catalogue, installer and GPUI tests cover pinned dependencies, unsafe sources, malformed
records, bounded logs, previous-generation preservation, cancellation acknowledgement, disconnects,
keyboard repair/cancellation and custom command precedence. A separate opt-in test uses actual
language servers through Codeyard's JSON-RPC client on local and disposable SSH hosts:

```sh
CODEYARD_REQUIRE_SSHD=1 \
CODEYARD_LANGUAGE_SERVICE_FIXTURE=/absolute/disposable/managed-installation \
cargo test -p app --bin codeyard managed_services_real_local_and_ssh_acceptance -- --ignored --nocapture
```

The fixture must contain a completed installation and its `active.json`. Setting
`CODEYARD_TEST_MANAGED_INSTALL=1` additionally performs a real package/runtime installation under the
owned SSH fixture before checking completion, formatting and diagnostics. This opt-in path can use
the network. Ordinary tests never download packages.
