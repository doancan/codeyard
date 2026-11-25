# Agent mode

Agent mode puts every open project in **one window**: a list of the projects on the left, the selected project's terminals in the
middle, and its source control, diff, pull requests and files on the right. It is for the person who works in many projects at
once, locally and over SSH, with several agent command-line tools (Claude Code, Codex, Antigravity) and several accounts of each,
and who needs to see which project is waiting for them without opening a dozen windows.

Switch to it with the terminal button at the top of the activity bar, or *View: Toggle Agent Mode*. It replaces the editor area
and the side bars of that window; *Back to editor* (in the list), the same button, or any view's button in the activity bar returns.

## What it shows

```
┌ [All|Local|SSH] ──┬ Claude | zsh | make verify | +      [Claude Code · Work ▾] ┬ Git Diff PRs Files ┐
│ Attention      2  │                                                            │ billing-api  main  │
│ ● codeyard        │   the selected terminal                                    │  ● feat: ttl …     │
│ ● data-etl  gpu   │                                                            │  ● fix: empty …    │
│ This computer     │ ┌ Work reached its limit. [Continue with Personal] ┐       │                    │
│ ◉ billing-api  +2 │                                                            │                    │
│ SSH host prod-eu  │ ● Running · feature/pdf-speed                  3 terminals │                    │
│ ◉ infra           │                                                            │                    │
│ Claude Code       │                                                            │                    │
│ Personal  412k ▇▇ │                                                            │                    │
│ Work  Limit · 3pm │ Back to editor                                             │                    │
└───────────────────┴────────────────────────────────────────────────────────────┴────────────────────┘
```

**Project list.** One row per open project window, grouped *Attention* (a program waits for an answer, or an account is out of
quota), *This computer*, then one group per SSH host. A row shows a status dot, the project, the last line of its main terminal and
how many more terminals it has. *All / Local / SSH* filters the list, and each window has its own filter, so one window can show
only the SSH projects.

**Status** is read from the project's main terminal (the agent launcher's session if it has one, else the first terminal):

| State | Meaning | Where it comes from |
|---|---|---|
| Needs you | a program asks for the person | the bell or an OSC 9/777 notification, which Claude Code and Codex send when they wait (`terminal_marks.rs`) |
| Limit reached | the account is out of quota | the tool's limit message on screen (`agent_limits.rs`), or *Mark Account as Limited* |
| Running | a command or the agent session runs | shell integration, or the launcher's terminal being alive |
| Done | the launcher's session ended | its exit |
| Idle | at the prompt, or no terminal | |

Showing a project's terminal answers its request, so *Needs you* clears when you look. A terminal without shell integration that
runs `claude` by hand shows *Idle* while it works; *Needs you* and *Limit reached* still come through.

**Terminals.** Tabs for the project's terminals, the main one first; `+` opens a plain shell in the project (on its SSH host when
it has one). Switching project never stops anything: the terminal is the project window's own and keeps running. The window that
shows a terminal decides its size while it does; it goes back to the project's own window when agent mode shows something else.

**Right panel** (for the selected project): *Git* is the Source Control view with its graph, changes, stashes and commit box;
*Diff* shows the diff of the file picked in *Git*; *PRs* lists the repository's open pull requests with their check state and
their actions (open on GitHub, copy link, copy branch, check out); *Files* is the Explorer. They are the project window's own
views, drawn here; menus and pickers they open appear in this window, and a diff opens under *Diff*, not in the project's window.

**Accounts.** The button at the right of the tabs names the account of the project's agent session and lists the others;
choosing one asks first, as the Agents panel does. At the foot of the list every account of the tools in use is shown: *Available*
or *Limit reached* with the reset time the tool printed, and for Claude Code accounts the **tokens spent in the last five hours**
with a bar relative to the busiest account.

- There is no percentage of the plan: the tools give no such number, and the session files hold no quota. The token count comes
  from the account's own session files, counts input, output and cache writes, and covers private history only (shared history is one
  folder for every account and cannot be attributed). Codex and Antigravity show their state but no count. If you know what an
  account may spend in five hours (your plan, or the count shown when a limit hit), set `agents.claudeTokenBudget`: the bar is then a
  share of it and the text gives the percentage; at `0` (the default) the bar is relative to the busiest account.
- A limit is believed for five hours after it was first seen, unless the person ends it. *Agents: Mark Account as Available*
  ends it and ignores a message still on screen for ten minutes; *Agents: Mark Account as Limited* sets it by hand.
- When the project's main terminal shows a limit message, a bar above the status line offers the account that is free and least
  used, and continues the conversation there (*Continue with <account>*, the Agents panel's hand-off: it needs private history
  and a conversation; otherwise it switches the account).

**Attention.** While agent mode is on, a project that starts waiting, hits a limit or finishes while the person is not looking at
it gets a notice in the window (*Name needs you*, *Name: limit reached*, *Name finished*) whose button, *Open Project*, shows that
project's terminal. When Codeyard is in the background (macOS, as an application bundle) the same event is also a **banner in
Notification Center** with the project's last line of output; clicking the banner brings that project up. The Dock icon shows how
many projects need the person and bounces once when one more does. What a project was when agent mode was opened is not news.
Banners need the person's permission, which macOS asks for once; run unbundled (`cargo run`) no banner is made.

## How it works

- **A project is its window.** Each project keeps its own `Workbench` and terminals. Agent mode reads them through the windows'
  `WeakEntity<Workbench>`; terminals are `Entity<TerminalView>` and are drawn in the hub as the same object (`workbench_agent_mode.rs`).
- **Views of another project** use the mechanism of views in windows of their own (`workbench_view_windows.rs`): the hub calls the
  project's `side_view` through its entity. The project knows its hub (`hub_host`), so `view_host_window` sends its menus and
  pickers there, and `open_diff` hands the diff to the hub (`agent_mode_show_diff`).
- **Terminal size** has one leader (`TerminalView::lead_size`): two windows drawing one terminal would otherwise resize it to
  their own areas in turn without end. The leader's window ending ends its lead.
- **Pull requests** are `PullsPanel` (`workbench_pulls_panel.rs`), an entity over `github_listing`, the reading the Pull Requests
  list also uses. A checkout is asked of the project's own workbench.
- **Accounts and limits** are `agent_accounts.rs` (`Limits`, an app global: who is limited, which account to offer),
  `agent_limits.rs` (the tools' messages) and `agent_usage.rs` (tokens from Claude Code's session files). The list is redrawn
  once a second; usage is read off the UI thread once a minute.
- **The Dock** is `dock_attention.rs` (`objc2-app-kit`'s `NSApplication` and `NSDockTile`; nothing on other systems). **Banners**
  are `system_notifications.rs` (`objc2-user-notifications`, the one dependency this change adds; it is in the stack table of
  [architecture.md](architecture.md)): the project travels in the request identifier, a click is queued and the next window to
  draw opens the project. `UNUserNotificationCenter` needs a bundle identifier, so without one nothing is done.

## Limits of this version

- The mode is not restored after a restart (each window would otherwise open in it); the windows themselves are.
- A project without a window is not listed: open it first (a project opened from the hub is not offered yet).
- Existing project windows are not hidden by the mode; minimise them as usual.
- Pull requests of GitHub only; extension-provided pull request sources stay in the Pull Requests list of the project's window.
- Banners are written for a signed bundle and **could not be run here** (see below); unbundled runs show the in-window notice and the Dock count only.
- A terminal drawn by the hub and by its own window at once draws correctly in the leader only; the other shows the same text at
  the leader's width.

## Not verified

The app was started with agent mode on (two windows) and looked at in screenshots. **Seen:** the project list with its groups and
filter; the *Attention* group with a project waiting (amber dot on the row and the tab, *Needs you* in the status line) and with a
project at its limit (red dot, *Limit reached*), each fed by a terminal that rang the bell or printed a limit message; the terminal
with its tab and status line; the right panel with the project's own Source Control view (commit box, changes, worktrees, graph) and
the tabs *Git*, *Diff*, *Pull requests*, *Files*. With a stand-in script for the CLI (started from the Agents panel, printing a limit
message) the screenshots also show a project with an agent session: its main terminal first among the tabs, the extra-terminal count,
the account button *Personal* of the tool, its menu with each account's state, the bar *Personal reached its limit; it resets 3pm*
with its offer of the other account, and the accounts at the foot of the list (*Limit reached · 3pm*, *0 tokens*). **Nobody has
seen** the usage bars with real token counts, what the offer does when pressed (it asks first, as the Agents panel does), the Dock
badge and bounce, the banner with its permission prompt and click, a real CLI hitting a real limit, or SSH projects. The limit
messages are the tools' wording as known today and will need upkeep.

## Tests

`workbench_agent_mode.rs` (list, status, switching without stopping, the plus, closing a project, keyboard input into another
project's shell, terminal size and its leader, the diff under *Diff*, limits and the bar, marking by hand, the Dock count),
`workbench_pulls_panel.rs`, `agent_limits.rs`, `agent_accounts.rs`, `agent_usage.rs` and `a11y_agent_mode_tests.rs` (named
buttons and presses).
