# Web Preview

Enable **Settings > Modules > Web Preview** (also shown under Commonly Used), or set
`"modules.webPreview.enabled": true` in your user settings. It is off by default;
projects cannot enable it. Enabling the module does not start a server.

## Settings

All preview options are available in **Settings > Modules** and in user `settings.jsonc`.
Projects cannot override them. Changing an option restarts a running preview and opens its
new URL; a stopped preview stays stopped. Existing browser tabs may need to use the new URL.
A fixed port can serve only one preview at a time.

| Setting | Default | Behavior |
| --- | --- | --- |
| `webPreview.port` | `0` | Choose a free port automatically, or use a fixed port from 1 to 65535. An occupied fixed port reports an error. |
| `webPreview.host` | `"127.0.0.1"` | `127.0.0.1` and `::1` accept local IPv4/IPv6 connections. `0.0.0.0` and `::` listen on all IPv4/IPv6 interfaces for network access. |
| `webPreview.liveReload` | `true` | Inject the browser refresh script and watch served files. Disable it for manual refresh only. |
| `webPreview.reloadInterval` | `1000` | Milliseconds between browser checks, from 250 to 60000; used when live reload is enabled. |
| `webPreview.root` | `""` | Automatic root selection when empty. Otherwise, a workspace-relative path such as `public`, an absolute path, or `~/site` on the active host. |

The automatic root is the active HTML file's workspace folder, or its containing folder
when opened alone. Without an active HTML file, the first project folder is used. A configured
root takes precedence; an active HTML file outside it opens that root's `index.html` instead.

For network access, run **Web Preview: Copy URL**, then use the computer's LAN address in place
of the loopback address in that URL,
keeping the launch token until the session is established. Wildcard addresses such as `0.0.0.0`
are listening options; the editor opens a local loopback URL. Host/origin validation and
session authentication apply to both local and network connections.

## Opening a preview

Open an HTML file and click its tab's eye button, or run **Web Preview: Open in Browser**.
The built-in static server opens the saved file in your default browser. Without an active
HTML file, the command opens the project's `index.html`. No Node.js, Python or extra
web server is required. HTML, CSS, JavaScript, JSON, images, fonts and WebAssembly are
served with their appropriate content types. Folders resolve to `index.html`.

The status bar shows the allocated port and a stop button. Clicking the label opens
the preview again. **Web Preview: Stop Server** stops it; **Web Preview: Show Server Log**
opens Output. Turning the module off, changing workspace/host, disconnecting or closing
the window also stops it. A server is not restored automatically at startup.

## Live refresh and remote work

With live reload enabled, saved HTML/JavaScript and asset changes refresh the page. Linked CSS is refreshed
without reloading the document when possible; imported styles fall back to a page
refresh. Unsaved editor contents are not served. The existing agent file watcher
tracks up to 512 requested files, including changes made outside the editor.

Local, SSH and container workspaces use the same implementation: a local HTTP listener
reads files through Codeyard's active Host/agent. No listening port is opened on the
remote machine and no SSH forwarding or container port publication is needed. This
serves static files only. PHP, server-side rendering, API proxies and framework build
pipelines are not run; React/Vite projects still need their own development server.

## Serving boundaries

By default, each window's server binds an OS-selected port on `127.0.0.1`. A random launch
token establishes a session cookie; Host and browser origin checks restrict access.
The served root follows the root selection described above. Dotfiles, traversal, symlinks, special files and unsupported file types
are rejected. Files are limited to the agent's 32 MiB read limit. No directory listing
is provided. A workspace trust check precedes startup.

Each file takes one agent request, including path validation and the size check. The agent
opens path components relative to directory handles without following symlinks, then reads
the opened file; replacing a pathname during a request cannot redirect that read. The selected
root itself must also be a real directory. Redirects preserve URL query parameters while
removing the launch token once it has established the session cookie.

Previewed JavaScript runs in the browser. A restrictive page Content Security Policy
can block the injected live-refresh script; manual browser refresh still works. This
release uses the external browser, not an embedded browser or device simulator.

## Browser acceptance test

The ignored `web_preview_browser_acceptance` test starts the actual Workbench and agent
watcher with a temporary HTML/CSS/JS project. Run it in one terminal, then run the
driver in another:

```sh
cargo test --release --locked -p app --bin codeyard web_preview_browser_acceptance -- --ignored --nocapture --test-threads=1
node scripts/test-web-preview-browser.mjs
```

The driver needs a Node version with global `WebSocket` and a Chromium browser.
Windows defaults to Edge; elsewhere set `BROWSER_EXECUTABLE` to a Chromium executable.
It uses an isolated headless profile and checks real page rendering, session cookies,
root-relative assets, CSS replacement without navigation, and HTML/JS reload after
external edits. These are test tools, not Web Preview runtime dependencies.
