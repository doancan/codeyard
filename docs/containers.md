# Optional container environments

Container integration is a user-controlled module, disabled by default:

```jsonc
"modules.containers.enabled": true
```

Change **Container Environments** at the top of Settings' **Commonly Used** view,
or under **Modules**, or in your own `settings.jsonc`. Searching for `containers`,
`Docker` or the translated description also finds it.
Project settings cannot enable it, even in trusted projects. When disabled, container
palette actions and container entries in the workspace switcher are hidden, direct
actions and restored connections are rejected, and pending module operations are
cancelled. Disconnect an active container before disabling: open files must remain
available to save. Disabling does not remove containers, images or volumes.

Enabling the module immediately adds a **Containers** button to the left activity bar.
The activity button and **View: Containers** open the full-width Overview with the sidebar closed.
Search and filters sit above the table. **Container actions** provides Connect, Create, Rebuild,
container/service logs, Output and cancellation. Opening this page never starts an engine.
Disabling the module hides the activity button and closes its page, restoring Explorer.


## Engine selection and prerequisites

In a local workspace, Connect and service-log actions first show the engines supported on this platform,
including whether their command-line tool is installed:

- macOS: Apple Container, Docker and Podman.
- Windows: WSL Containers (`wslc.exe`), Docker and Podman.
- Linux: Podman and Docker.

Docker is optional. Each adapter uses its own CLI for listing, starting, attaching
and reading logs. Engine lookup includes PATH and common GUI-installation locations,
including Homebrew, Docker Desktop, Podman and WSL installation folders. Selecting
a missing engine shows setup instructions instead of attempting to execute it.
Restored container connections perform the same prerequisite check. Engine software
and virtualization prerequisites still need to be installed on the host.

Apple's JSON array, WSLc's newline-delimited JSON and Podman's JSON array are parsed
separately. Apple's stopped state is normalized for the Start and Connect action.
No Docker compatibility shim is required for native Apple/WSLc connections.

## Live SSH resource monitor

With the module enabled, connecting to SSH automatically discovers Docker containers through the existing Codeyard agent.
Opening **Containers** shows running and stopped containers without selecting an engine or context first.
The Overview is the single container inventory. Click a row to open its detail view.
The overview shows status (including Docker health), image, CPU, memory usage/limit, network rates and
Compose project. **Columns** also offers disk read/write rates and published ports; choices last for
the connected monitoring session. The table fills the available width, including when optional columns
are hidden; Name and Image share extra space while metric columns keep readable minimum widths.
The page header combines update state, host/context and fleet counts. The initial Overview tab is
hidden until a container has been selected; navigating back to an existing selection remains available.
Long values have full-value tooltips. On narrow windows or with extra columns enabled, the table scrolls
horizontally within the page. The header actions and search/filter toolbar wrap to remain reachable.
Network directions have colored ↓ RX / ↑ TX labels and disk rates have R / W labels, with each
direction on its own aligned line and a descriptive tooltip. CPU and memory have thin usage bars;
CPU values above 100% remain visible (100% represents one core), while the bar caps at full width.
Unavailable measurements stay as dashes with empty bars. Status uses both a colored dot and text.
All/Running/Stopped and **Needs attention** filter the inventory; attention includes unhealthy, dead and
restarting containers. Search above the table filters names, images, projects and IDs; **Clear filters**
appears while a filter or query is active and also recovers an empty result. Click the **Name**, **CPU**
or **Memory** column header to sort; the active header shows the direction. Sorting stays grouped by
Compose project and captures ranking values when clicked, so live updates do not reorder rows.
Selection and table scroll survive refreshes. Stopped containers show no live resource measurements.
Click a row to select and open that container's details. Logs opens directly;
the row menu provides Open files, Terminal and Copy container ID. **Back to Containers** returns to the inventory
without losing the selected container, search, filters or scroll position. The detail area continues to show only that selected container,
including its current CPU/memory/process count and resource history.
The table is keyboard focusable: Up/Down moves its cursor and Enter opens the focused container.
While the table has focus, N sorts by name, C by CPU and M by memory; header tooltips show these keys.

Select a table row to open its full diagnostics. Resources provides 1/5/15-minute CPU/memory charts collected in memory while monitoring is active (maximum 360 samples per container, at most 15 minutes). Long gaps remain gaps; no historical measurements are fabricated before monitoring began. Disk/network rates use elapsed-time differences between cumulative counters and are unavailable after counter resets or until a second sample exists. Stopped containers do not display old rates as live data.

Ports and storage inspects health and mounted paths on selection; no volume contents are scanned. Logs shows the latest 300 timestamped entries in a dedicated monospace console, automatically refreshed alongside resource snapshots while the resource tab is active. Rows separate line numbers, UTC time, severity and message. Hover time to see the complete timestamp. JSON level/severity fields and common line prefixes determine severity; unrecognized messages remain unclassified. ANSI terminal controls are removed for display. Literal, case-insensitive search highlights message matches and also searches full timestamps. An error/warning filter and copy-visible action operate on the filtered lines; copied text retains full timestamps and messages. Line wrapping is adjustable. Scrolling upward pauses automatic following without stopping data refresh; Jump to latest and follow resumes bottom tracking. Open files attaches the editor to the selected container and opens the file explorer; Terminal attaches and opens a terminal after connection. The previous SSH connection and folder are retained while browsing container files. A persistent **Back to SSH workspace and container details** action restores that connection/folder and the selected-container tab without a new SSH login. Existing unsaved-file guards still apply. Selection changes and module shutdown cancel obsolete detail requests.

CPU, memory usage/limit, disk read/write totals, network receive/transmit totals and process counts use
[`docker stats --no-stream --no-trunc --format`](https://docs.docker.com/reference/cli/docker/container/stats/).
CPU can exceed 100% on multicore hosts. Stopped containers show unavailable metrics rather than fabricated zeroes.
The current Docker context is discovered automatically. Snapshots refresh three seconds after the previous one finishes,
while the container page is visible. **Pause updates** stops subsequent samples; **Refresh** resumes them.
Discovery also runs once on connection and when the module is enabled in an existing SSH workspace.

Disk size comes from [`docker ps --size`](https://docs.docker.com/reference/cli/docker/container/ls/#show-disk-usage-by-container---size),
sampled at most once every 30 seconds. It reports the writable layer and virtual image size; it excludes volumes,
bind mounts and container logs. Disk and network I/O values are cumulative byte totals, not storage capacity or rates.
No cleanup, sudo, container creation or installation runs automatically. Missing engines, denied daemon access and
partial statistics failures appear in the panel. Failed refreshes retain the last successful inventory and mark it stale.
Command output is capped at 8 MB with a 20-second timeout per remote command; module shutdown and host changes
cancel sampling and discard obsolete callbacks. Monitoring currently targets Docker over SSH; native local engine
connect/start/log workflows remain available through the engine picker.

## Docker over an existing SSH connection

In an SSH workspace, Connect and service logs use Docker on the connected server.
No local Docker CLI is required. Docker context discovery, engine checks, list/start/log
commands and the container agent bootstrap all run through SSH. The saved container
identity pins both the SSH host and Docker context, so reconnects do not fall back to
the local machine. Output identifies the SSH host. The SSH user must have Docker
installed on its remote PATH and permission to access the remote engine; Codeyard
does not add sudo or change daemon permissions. Dev Container project creation on
an SSH source folder remains unsupported.

## Docker and Dev Container workflows

- **Remote: Connect to Dev Container…** selects an engine, then a container; Docker
  additionally selects a Docker context.
  Contexts are pinned in connection identities; switching the global Docker context
  does not redirect an existing connection. Remote contexts can attach to existing
  Linux containers. Stopped containers can be started after an explicit prompt.
- **Docker: Show Containers and Services…** shows running and stopped containers,
  images, status/health and published ports. Selecting a row opens its last 200 log
  lines in Output. This is a snapshot, not continuous monitoring.
- **Docker: Open Project in Dev Container…** reads `.devcontainer/devcontainer.json`
  or `.devcontainer.json` from the open local folder. The review shows image/build/
  Compose mode, user, ports, mount count and lifecycle command names. Build and Open
  uses the separately installed [reference Dev Container CLI](https://github.com/devcontainers/cli).
  No CLI, Node runtime or Docker engine is installed or downloaded automatically.
- The CLI handles image, Dockerfile and Compose setup, features and lifecycle hooks.
  Subsequent agent commands use `devcontainer exec`, preserving the configured
  `remoteUser`, `remoteEnv` and environment probing. Extensions written for other editors are not loaded.
- **Docker: Rebuild Dev Container…** rereads the source configuration and reviews it
  again before replacing the container. Save open changes first. Named volumes are
  retained; files installed only in the container's writable layer are not preserved.
- **Docker: Cancel Operation** stops the current CLI process tree. Resources already
  created by the engine remain for inspection. A daemon-side build can outlive its
  client; cancellation is not a rollback of Docker resources.

Discovery is explicit, with a 30-second timeout. Builds have a 30-minute timeout.
Both output streams are drained; command stdout is capped at 8 MB and UI log queues
are bounded. Completion callbacks are invalidated on cancellation or module shutdown.
The editor switches to the new environment only if the original workspace is still
active and no unsaved changes appeared during the build.

Creating Dev Containers requires a local Docker, Podman or WSL Containers engine,
the separately installed reference CLI and a trusted source folder. Podman Compose
projects additionally require `podman-compose`. WSL Containers Compose projects are
rejected before setup; native Apple Container currently supports existing-container
workflows, not Dev Container project creation. Remote Docker creation, repository cloning into volumes, multiple named
Dev Container configurations, automatic port forwarding and a Windows guest agent
are not implemented. Containers used for editing must provide a POSIX shell and a
supported Linux architecture. Windows-container engines are detected before editing
is attempted; their status and logs can still be viewed.

## Native runtime research (2026-10-26)

The module setting deliberately says **containers**, not Docker. Additional runtimes
need separate command adapters and capability checks; OCI image compatibility does
not imply compatible APIs, command flags or Dev Container lifecycle support.

| Runtime | Platform and guest | Official evidence | Codeyard status |
| --- | --- | --- | --- |
| Docker Engine / Desktop | Linux guests on supported local or remote engines | [Contexts](https://docs.docker.com/engine/manage-resources/contexts/), [Dev Containers](https://code.visualstudio.com/docs/devcontainers/create-dev-container) | Implemented; automated tests, physical engine verification pending |
| Apple Container | Apple silicon, macOS 26; Linux containers in lightweight VMs | [Apple README](https://github.com/apple/container), [CLI reference](https://github.com/apple/container/blob/main/docs/command-reference.md) | Native list/start/attach/log adapter implemented; fixture-tested, physical verification pending |
| WSL Containers | Windows; Linux containers through `wslc.exe`; WSL 2.9.3+ per current overview | [Microsoft overview](https://github.com/MicrosoftDocs/wsl/blob/main/WSL/wsl-container.md), [GA announcement, 2026-09-29](https://blogs.windows.com/windowsdeveloper/2026/09/29/wsl-containers-now-generally-available/) | Native adapter and reference-CLI driver implemented; fixture-tested, physical verification pending |
| Windows containers | Windows guests, process or Hyper-V isolation | [Microsoft isolation modes](https://learn.microsoft.com/en-us/virtualization/windowscontainers/manage-containers/hyperv-container) | Docker status/logs only; editing requires a new Windows guest agent |
| Podman | Linux engine; VM on macOS/Windows | [Podman installation](https://podman.io/docs/installation) | Native adapter and reference-CLI driver implemented; fixture-tested, physical verification pending |

### Apple Container direction

Use Apple's own `container` CLI, rather than requiring a Docker compatibility shim.
Its `list --format json`, `exec --interactive`, `inspect`, `logs -n` and resource
commands provide native integration points. Check Apple silicon, OS version, CLI
version and service readiness only after the module/runtime is selected. Existing
Linux ARM64 agents can potentially be reused; this needs a real attach/save/terminal
test on macOS 26. Full Dev Container and Compose support must be proved separately,
not inferred from support for OCI images.

### WSL Containers direction

Use the explicit `wslc.exe` executable, avoiding the ambiguous `container.exe` alias.
This runtime is distinct from opening a WSL distribution and from running a Windows
container. Microsoft's GA announcement includes health checks, events, mount support
and configurable storage, while Compose is explicitly future work in that announcement.
Version and capability probes must determine which project configurations are usable;
Compose projects must not be offered as supported without a verified implementation.
Start with discovery, attach and lifecycle integration, then evaluate the native API
if it removes a measured limitation of the CLI.

### Windows guest direction

Do not bootstrap Linux ELF agents with `sh` inside a Windows guest. A Windows agent
needs native process creation, Windows paths, permissions, file watching and ConPTY,
plus host/image compatibility checks for the selected isolation mode. This is a
separate implementation and validation project.

### Shared acceptance criteria for future adapters

- Runtime, host, guest OS and architecture are visible before connecting.
- No background probing or auto-installation when the module is off.
- An unsupported capability is explained before starting a build or bootstrap.
- Changing global runtime context cannot redirect an established session.
- Cancel, engine pause, restart, connection loss and failed lifecycle commands leave
  recoverable editor state and bounded process/output resources.
- Validate non-root users, Alpine, Unicode/space-containing paths, remote environments,
  unsaved edits during setup and rebuilding with persistent application data.
- Compare cold/warm attach time, editor memory, container memory and recovery time
  on the same workload before making performance or superiority claims.

Resource details pair current CPU and memory readings with time-scaled history and grid guides. Disk and network rates are separate from cumulative totals. Ports and storage shows individual IPv4/IPv6 server bindings, exposed-only ports, mount sources and destinations, and read/write access. Copy controls retain full identifiers and paths, with horizontal scrolling for long values. Refresh details reloads inspect data. Published addresses refer to the SSH server, not the local machine.

Overview filters offer all, running and stopped states, multi-word name/image/project/ID search, a result count and a clear action. CPU/memory ordering uses values captured when the sort button is clicked; click again to re-rank. Routine polling retains the table scroll handle and does not insert refresh/loading messages into populated views.

### Detail layout

The selected container name becomes the page title, with Open files and Terminal beside
the refresh controls. CPU and memory cards share a row when space allows and stack in
narrow windows. Each card keeps its current reading above the history plot; disk and
network activity use separate columns when space permits. Resource, port/storage and log
tabs remain available below the back navigation. Log search uses the same framed field
as the overview; clicking the search icon focuses the input.

Ports and storage use two columns on wider windows, stacking on narrow windows. Each
port card distinguishes published server bindings from exposed-only ports. Mount cards
show access mode, server source, container destination and optional volume name; long
values scroll horizontally without pushing Copy outside the card. Log entries place
line number, timestamp and severity above the message, preserving the full message
width. Search occupies its own row and the snapshot limit appears in the console footer.
