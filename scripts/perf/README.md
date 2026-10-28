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
other apps, plug in the power, let it cool). `--runs` and `--mem-runs` change the number of runs (default 20 and 3). A
full run takes about 15 minutes. The measured windows open in the background and never take the keyboard, but they
must be visible: keep the display awake and stay on the same Space. A run that draws no frame or loses a key stops
with a message instead of producing numbers. The baseline is built once in `target/perf/worktree-<commit>` and reused.

## What is measured

| Metric | Scenario | Statistic | Budget |
| --- | --- | --- | --- |
| `startup.first_frame_ms` | start of `main` to the first drawn frame, welcome screen; one sample per run | p95 | +10% |
| `startup.window_open_ms` | start of `main` to the window being created (warn only) | p95 | +10% |
| `typing.keystroke_frame_ms` | key press to the drawn frame, 10k-line Rust file with highlighting; 40 keys per run | p95 | +10% |
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
- for a median, every candidate sample is worse than the baseline's median by more than the budget. Memory has 3
  samples, too few for an interval; one outlier must neither hide a regression nor invent one.

A value above the metric's `ceiling`, when one is set, also fails. `warn` metrics are reported and never fail. A
blocking metric with no candidate samples fails; one with no baseline samples is reported as not compared. That is the
case for timing against releases before the hooks (v0.5.0): the first gated release compares memory only, the one
after it everything. A different `machine` in the two files is printed as a warning.

## Release

Run it on the reference Mac before tagging (`docs/releasing.md`) and commit `target/perf/candidate.json` as
`docs/benchmarks/perf-<version>.json`: the record of what each release measured. The gate itself always rebuilds and
reruns the previous tag next to the candidate, so both are measured in the same session. Accepting a regression is one
line in the release notes saying which metric and why. `docs/benchmarks/perf-baseline-2026-10-28.json` is the first
record, taken before the platform work started.
