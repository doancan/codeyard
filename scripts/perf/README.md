# Performance gate

Every release is compared with the previous one on the same Mac. A metric blocks the release when it gets worse than
its budget allows and the change is larger than the noise of the measurement. There is no app binary size budget; the
memory limits in `scripts/memcheck.sh` and the agent size limit stay.

## Run it

```sh
scripts/perf/run.sh --baseline v0.5.0        # builds v0.5.0 in target/perf, runs both builds interleaved
scripts/perf/compare.py target/perf/baseline.json target/perf/candidate.json
```

`--aa` runs the working tree against itself instead: every row must pass, otherwise the machine is too noisy (close
other apps, plug in the power, let it cool). `--runs` and `--mem-runs` change the number of runs (default 20 and 5). A
full run takes about 35 minutes. The measured windows never take the keyboard: the app is not activated, so its window
would open behind the windows of the app in front, and macOS draws nothing in a window that is covered entirely.
`CODEYARD_PERF_ON_TOP` keeps the measured window above normal windows and on every Space instead (`keep_drawn` in
`crates/app/src/perf.rs`); keep the display awake. A baseline from before that variable has no such hook: the run warns,
and nothing may cover its window. Every build that has a scenario's hook must give all of that scenario's samples, and
every memory scene a value; otherwise the run stops with a message instead of producing numbers (a baseline from before
a scenario is not compared on it). The baseline is built once in `target/perf/worktree-<commit>` and reused, with
its own `codeyard-exthost` when it has one (`CODEYARD_EXTHOST`): an app and an exthost of different commits need not
agree on every call.

## What is measured

| Metric | Scenario | Statistic | Budget |
| --- | --- | --- | --- |
| `startup.first_frame_ms` | start of `main` to the first drawn frame, welcome screen; one sample per run | p95 | +10% |
| `startup.window_open_ms` | start of `main` to the window being created (warn only) | p95 | +10% |
| `startup.extensions_ms` | loading 100 installed, inactive extensions at startup (50 themes made by `cyx new`, 50 code extensions packaged with `cyx package --no-build`); the run fails if one of them starts | p95 | +10% and +1 ms; ceiling 5 ms |
| `typing.keystroke_frame_ms` | key press to the drawn frame, 10k-line Rust file with highlighting; 40 keys per run | p95 | +10% |
| `typing.keystroke_frame_words_ms` | as `typing.keystroke_frame_ms`, with the keys of `typing.keystroke_frame_completion_ms` (`a`, `b`, space) and no extension: the reference for that row. Words typed at the top of the file make tree-sitter recover from the error across the whole file, so a reparse often runs past its 4 ms budget and finishes in the background; the redraw that follows can land just before the next key, which then waits for it. Plain typing (one long word) never does that; 40 keys per run | p95 | +10% |
| `typing.keystroke_frame_decorations_ms` | as `typing.keystroke_frame_ms`, with 2 installed extensions decorating the file (the decoration provider `crates/ext-host/testdata/decorator.wasm`, about 1000 marks each); 40 keys per run | p95 | +10%; ceiling: plain typing +10% |
| `typing.keystroke_frame_completion_ms` | as `typing.keystroke_frame_ms`, with 1 installed extension completing Rust (the same provider, `crates/ext-host/testdata/decorator.wasm`, answering 50 items at once); a new word every third key, so it is asked again and again (its 50 items, `perf_item_<n>`, never match the typed words, so no list is drawn); 40 keys per run | p95 | +10%; ceiling: `typing.keystroke_frame_words_ms` +10% |
| `typing.keystroke_frame_lenses_ms` | as `typing.keystroke_frame_ms`, with 1 installed extension giving code lenses and inlay hints for the lines in view (`decorator.wasm` again: a lens on every fifth line and a hint on every line, asked again after each pause in typing); 40 keys per run | p95 | +10%; ceiling: plain typing +10% |
| `views.keystroke_frame_ms` | key press in an extension view to the drawn frame: PageDown in a sidebar view with a list of 10 000 rows (the view of `crates/ext-host/testdata/viewlist.wasm`); 40 keys per run | p95 | +10%; ceiling: plain typing +10% |
| `extensions.provider_overhead_ms` | an extension provider request (format, code actions, tasks) to a running extension, sent to its answer back in the window, with a provider that answers at once: the same two extensions asked for their tasks, 40 rounds per run | p95 | +10%; ceiling 3 ms |
| `extensions.hot_reload_ms` | `cyx dev`: a new `extension.wasm` noticed to the first request answered by the new build (the old exthost stopped, the new build compiled with the incremental cache and started), 2 MB Rust component started with `--extension-dev` (`crates/ext-host/testdata/reload-a.wasm` and `reload-b.wasm`, two builds that differ in one constant, swapped in place by turns); 10 reloads per run after one that fills the incremental cache | p95 | +15%; ceiling 500 ms |
| `market.search_ms` | key press in the Market's search to the results in the view (searched off the UI thread), 10 000 extensions (`cyx yard fixture`, served on this machine); 40 keys per run | p95 | +10% and +1 ms; ceiling 16 ms |
| `market.search_frame_ms` | the same keys, to the drawn frame showing the results (warn only: like typing, it includes up to two display refreshes) | p95 | +10% |
| `memory.empty_window_mb` | settled footprint, welcome screen | median | +5% |
| `memory.syntax_10k_mb` | settled footprint, 10k-line Rust file | median | +5% |
| `memory.terminal_mb` | settled footprint, terminal open | median | +5% |

The catalogue is `perf/metrics.json`; a new metric is one entry there plus a `perf::sample` or `perf::mark` call in the
app (`crates/app/src/perf.rs`). Time metrics come from the app itself (`CODEYARD_PERF`, see `docs/environment.md`);
memory uses the same settled `footprint` as `scripts/memcheck.sh` (`footprint.sh`).

Times are measured until the frame after the one that shows the change starts, because GPUI runs frame callbacks
before it draws. They are upper bounds by at most one display refresh; that is the same for both builds.

## How a regression is decided

For each metric, `compare.py` takes the statistic (median or p95) of both builds. It fails when the candidate is worse
than the baseline by more than the budget **and** the change is not noise:

- the candidate's value is above the upper end of a 95% bootstrap interval of the baseline's value (the same statistic
  on resamples of the baseline), or
- for a median, every candidate sample is worse than the baseline's median by more than the budget. Memory has 5
  samples, few for an interval; one outlier must neither hide a regression nor invent one.

With fewer than 3 samples on either side a worse value is reported as a warning, never a failure: a single GPU
texture spike moves a memory scene by up to 8 MB (a 1-sample run once showed 99 MB for a scene that measures 87-90).

A value above the metric's `ceiling`, when one is set, also fails. `warn` metrics are reported and never fail. A
blocking metric with no candidate samples fails; one with no baseline samples is reported as not compared. That is the
case for timing against releases before the hooks (v0.5.0): the first gated release compares memory only, the one
after it everything. A different `machine` in the two files is printed as a warning.

The keystroke rows with extensions (`typing.keystroke_frame_decorations_ms`, `_completion_ms`, `_lenses_ms` and
`views.keystroke_frame_ms`) have a relative ceiling instead (`ceiling_vs` in the catalogue): the p95 of
`typing.keystroke_frame_ms` in the same candidate run, plus 10%; for the completion row, the p95 of
`typing.keystroke_frame_words_ms`, the same keys without the extension. A keystroke time includes up to two display
refreshes, so plain typing itself measures about 29 ms on the reference Mac and a ceiling in milliseconds would only
measure the display; what these rows must show is that extensions add nothing visible over typing the same keys
without them. 10% is above the noise: in the two reference sessions of 2026-10-31, two random sets of 20 plain typing
runs differ at p95 by at most 4.4% in 99.5% of splits (6.8% at most); with 5 runs it is up to 11%, so a quick
`--runs 5` run can cross the ceiling by noise alone.

## Release

Run it on the reference Mac before tagging (`docs/releasing.md`) and commit `target/perf/candidate.json` as
`docs/benchmarks/perf-<version>.json`: the record of what each release measured. The gate itself always rebuilds and
reruns the previous tag next to the candidate, so both are measured in the same session. Accepting a regression is one
line in the release notes saying which metric and why. `docs/benchmarks/perf-baseline-2026-10-28.json` is the first
record, taken before the platform work started. `docs/benchmarks/perf-2026-10-31.json` is the first with the extension
rows, measured against v0.6.0; it is the record the extension rows were made blocking on. `docs/benchmarks/perf-2026-11-01.json` (main at
838838e8) and `perf-2026-11-01-before-215.json` (58e18f21, main before the change for #215, which redraws after a
background parse only for new colours on screen, once typing pauses) were measured side by side on an M4 Pro, 20 runs
each, with no runtime shaders: `typing.keystroke_frame_words_ms` p95 22.6 → 22.5 ms and `typing.keystroke_frame_ms`
24.5 → 24.6 ms (`compare.py`). On this Mac words typed at the top of the file were not slower than plain typing before
the change either, so the 34.4 against 28.5 ms of the 2026-10-31 record (a runtime-shader build) did not show here. Both of its builds were
made with `--features gpui/runtime_shaders` (that Mac has no Metal compiler), which compiles the shaders at startup;
release builds do not.
