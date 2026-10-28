# Workbench interface review

Scope: sidebar section resizing, status-bar density, terminal scrolling, Git workflows,
and the on-demand resource popover, plus the editor/search/settings defects found during
that walkthrough. Results below describe the verified scope; they do not certify every
platform or assistive-technology journey.

## Behaviour

- Outline and Timeline resize at their top edge, above their headers. Thin idle rules
  show the boundary, and shared accent feedback highlights the grabbed edge. Drags use
  painted body heights, including when the sidebar has shrunk. Section sizes persist.
- The status bar uses a 28 px height and 16 px icons. Long branch names and notices
  truncate; secondary editor details yield space in narrow or busy windows. Sync, save,
  conflict, update and transfer labels compact before they crowd out the controls.
  Branch tooltips retain the full name and problem tooltips retain both full counts.
- Terminal wheel events accumulate fractional rows. `terminal.scrollSensitivity`
  (0.1–10, default 2) applies to wheel and trackpad motion, including alternate-screen
  scrolling and mouse reports. Changing modes/gestures does not carry fractional motion
  into another program. VS Code's corresponding setting imports to this key.
- Source Control file menus preserve whether the selected row is staged. Open Changes,
  Open File, Stage/Unstage and confirmed Discard use existing operations. Conflict rows
  keep marker validation and do not offer Discard; tracked files do not offer ignore.
  Discard confirmations place Cancel first and use an explicit cancel button so Escape
  also works with translated labels.
- Long context menus keep normal row heights; keyboard navigation scrolls the selected
  item into view.
- Bottom-panel views (Problems, Output, Debug Console, Ports and Terminal) use a
  rounded selection fill and a lighter hover fill, with no active underline. Labels keep
  their translated title case instead of being forced to uppercase, using the regular
  13 px small UI type step rather than the 11 px metadata step. The final dark
  Turkish QA window displayed these labels and the selected Terminal fill correctly;
  Output selection and hover were also inspected in the preceding build.
- Settings edits preserve JSONC layout and find root keys even on a shared line or
  with escaped spelling. Nested objects and comments cannot be mistaken for root keys;
  existing duplicate occurrences are updated consistently.
- The initial Software Update prompt has one brief paragraph explaining daily checks
  and the Settings entry point. Manual and daily choices keep their existing behaviour.
- Find/Replace fills its replacement input, including blank space beside short text.
  Its fields and actions wrap inside narrow editor panes. Inline blame and line-end
  language-server annotations occupy only the space after the shaped code; clipped
  annotations keep their full text in a tooltip.

## Resource popover

Click the activity icon in the status bar to open Resources. It fades in over 160 ms.
Leaving both icon and panel closes it after a short gap-crossing grace period and a
120 ms fade. Pin keeps it open; unpin restores pointer dismissal. Close, Escape or
clicking the activity icon dismisses it. It does not open merely on hover, and the
activity icon does not display a tooltip above the bar. The refresh-interval subtitle
was also removed at the user's request. View → Resources and the command palette offer
a keyboard entry point; Tab/Shift+Tab reach Pin and Close, and Escape dismisses the panel.

The panel reports **this computer's** CPU usage (between samples), physical memory in
use, Codeyard resident memory, available disk space, logical processor count, and Unix
load averages. Native macOS, Linux and Windows APIs require no subprocesses or extra
dependencies. First CPU usage needs a second sample. Unavailable values are explicit.
Samples run every two seconds while open and stop when closed.

Local project size sums regular file lengths, includes hidden/build files, and does
not follow symlinks. It refreshes at most every 30 seconds while open. Scanning stops
at 100,000 directory entries or 1.5 seconds; access errors and limits produce a marked
partial result. A project switch discards the old size. Remote paths are not scanned
on the local filesystem; remote workspaces retain local system readings and explain
that project-size measurement is available for local folders.

## Verification

Regression coverage includes real dispatched section drags, fractional terminal motion,
mode/gesture transitions, native resource sampling, bounded/symlink-safe project sizes,
pointer dismissal and pin state, cancellation on close, long menus and keyboard scrolling,
file menu stage/unstage against a real Git index, and narrow status bars with long text.

The isolated macOS QA window uses a throw-away profile and repository under
`/tmp/codeyard-ui-audit`; it never exercises destructive Git actions in the user's
repository. Real-window and full-suite results are recorded after the final run.

## Local verification (2026-10-27)

- `CODEYARD_REQUIRE_SSHD=1 CODEYARD_TEST_TIMEOUT_SCALE=3 RUST_TEST_THREADS=4 scripts/check.sh --strict`
  passed: formatting, Clippy with warnings denied, architecture/docs/licence checks,
  cargo-deny, cargo-audit and the workspace tests. **1,574 tests passed, zero failed,
  three documented ignores**; the app contributed 1,225 passes and two ignores.
  This run includes the final discard and annotation fixes. The later typography-only
  adjustment separately passed a fresh build and `scripts/check.sh --quick --strict`.
- Regressions first reproduced the dense status bar extending past a 480 px window,
  blank replacement-input space focusing the search field, narrow Find/Replace controls
  overflowing, inline blame overlapping code, compact JSONC settings gaining a duplicate
  theme key, and discard choosing a destructive first button. Their fixes passed the
  corresponding tests. Status controls were checked at 480/640/800/1100/1280/1920 px;
  Find/Replace controls at 480/320/240 px. Annotation space disappeared in a narrow editor
  and returned when widened. All 24 settings tests and all 71 update UI tests passed.
- The native macOS QA bundle used isolated profiles and a disposable repository.
  Outline and Timeline grew from their top lines; Timeline also shrank. Source Control
  Stage and Unstage changed the real index, Open Changes showed HEAD versus Staged,
  and blank commit submission gave an actionable message. File menus stayed unobstructed
  after fixing a lingering tooltip. The Turkish discard warning no longer highlighted
  Discard as the default, and Escape cancelled it. Automated cancellation also preserved
  tracked edits, untracked files and the index before testing explicit discard.
- Resources displayed real CPU, memory, disk and local project measurements. Pin kept
  the panel open with the pointer outside; unpin restored pointer dismissal. The removed
  subtitle was absent in dark English and light Turkish windows. Rendered pointer events
  separately verified crossing into the card and leaving it, including polling shutdown;
  keyboard tests cover opening, Tab/Shift+Tab, pin, Escape and focus restoration.
- The light Turkish terminal printed 400 numbered rows. One upward wheel page moved
  from rows 391–400 to 303–313; a downward page returned to the end. Fractional-pixel
  deltas and terminal-program mode transitions have separate regressions.
- Search found the expected match, and Settings filtering found
  `terminal.scrollSensitivity` with default 2. Debug and Testing empty states and the
  local Ports explanation rendered in Turkish. Remote Explorer listed configured hosts
  without connecting to them. The updated theme picker successfully wrote dark mode.
- Native Find/Replace changed `production` to `release`; Undo restored the text and
  cleared the dirty indicator. Split panes kept Close and Replace All visible. Inline
  blame no longer covered code in split panes, and returned beside code in a wide pane.
- The first-run Software Update window displayed the shortened Turkish explanation
  using `CODEYARD_DEV_UPDATE=uptodate` (debug builds normally skip startup scheduling).
  Choosing daily updated the isolated profile through the scripted backend.
- After user review, panel labels increased from 11 px to 13 px. The rebuilt dark Turkish
  window showed the larger labels in bottom and right positions, with rounded Terminal
  and Output selection fills. Labels retain translated title case.
- The new resource sampler passed standalone Rust metadata compilation for both
  installed Linux musl targets, aarch64 and x86_64, using existing libc artifacts.
  This establishes Linux API type compatibility, not native Linux runtime/UI coverage.
- The final release repeat passed all ten memory/size budgets: Terminal 79 MB, Welcome
  73 MB, Rust file 96 MB, plain text 87 MB, highlighted text 117 MB, first-run question
  75 MB, idle agent 2 MB, app binary 60 MB and Linux agents 1,568/1,360 KB.
  The preceding run failed Terminal at 309 MB while its other nine budgets passed.
  Profiling the repeat captured the same process at 309 MB during startup, with 227 MB
  attributed to unmapped graphics memory, before the benchmark sampled it at 79 MB.
  A separate identical Terminal launch settled at 78 MB. The graphics allocation's cause
  and release timing remain unestablished; the passing repeat does not erase the first
  budget failure. Earlier checks also saw a similar 308 MB Terminal measurement.

An intermediate full run had a dead translation key and two SSH connection timeouts.
The key was removed; the final passing full run exercised both SSH regressions without
recurrence. No Git workflow failed in that intermediate run.

## Verification limits

Physical Windows interaction, native Linux UI/runtime behaviour and VoiceOver journeys
were not verified locally. Remote project-size measurement is intentionally unavailable;
remote workspaces display local computer resources with that explanation. Background
macOS captures sometimes required raising or resizing the QA window before displaying
fresh pixels; the cause and automatic redraw timing are not established. New Turkish
resource labels were authored by Codex; no independent native-speaker review is recorded.
Release validation should retain these limits alongside the successful local checks.


## Container Overview layout (2026-10-28)

The inventory now stretches to the table viewport instead of stopping at the sum of fixed column widths.
Name and Image absorb spare space; metric columns keep minimum widths and align their headings with their
values. The table uses the standard small UI text size rather than the smaller metadata step. Header and
row actions share one right-aligned column, and the table has the same side gutters as the page controls.
Search and filters share a wrapping toolbar. Header actions wrap below the title when necessary. Clear
filters is shown only for an active query/status/attention filter, including an empty result.

Name, CPU and Memory headers now sort directly and indicate the current direction. With the table focused,
N/C/M provide the same actions; header tooltips show the keys. Ranking values remain frozen until another
explicit sort, preserving row order and scroll position during live refreshes.

The original regression measured a 1,330-point row inside a 1,872-point viewport at a 1,920-point window,
leaving about 542 points unused. GPUI layout tests now require rows to reach the right table border within
2 points at 1,280, 1,920 and 2,560 points, with both default columns and all optional columns hidden.
Other cases cover long names/images, English/Turkish controls at 480/800/1280 points, sorting without
opening details, clearing an empty search, and horizontal scrolling to row actions with every column enabled.
The existing live-refresh/navigation regression also uses the new column-header sorting control.

Native window inspection was completed after Screen Recording access became available. Disposable
container snapshots in isolated profiles were checked at 480, 1280 and 1920 points, in English/light
and Turkish/dark variants. The revised header puts update state beside the title, keeps host/context
and fleet counts together, and places a framed search field beside grouped status filters. Duplicate
counters and the redundant initial Overview tab are removed. The hint is one wrapping flex row rather
than three stacked text nodes, and the update label no longer collapses into an ellipsis.

Header regressions require a readable update label, a single-line desktop hint and table content within
210 points of the page top at 1280 points. Clicking the search icon focuses the text field; typing is
verified through dispatched input events. Initial discovery does not report zero fleet counts before
a successful snapshot. Stale-data errors remain explicit below the header. This is native rendering
and interaction evidence with sample data, not a live-server or screen-reader certification.

Container detail polish was checked in the native macOS app with isolated sample data:
resource cards at 1280 px and the log console at 480 px. Automated layout coverage checks
chart wrapping, header action bounds and long selected-container names at both widths.
These checks do not constitute a live remote-engine acceptance run.

The revised port/storage cards were inspected in a native 1280 px window, and the
full-width log message layout at 1280 and 480 px using isolated sample data. A GPUI
regression covers long mount paths, port-copy IPv6 formatting and wrapped log bounds.
