# 0.12.9 performance comparison

Measured on the same Retina Mac with the official interleaved runner:

```sh
scripts/perf/run.sh --baseline v0.12.8 --runs 5 --mem-runs 3
scripts/perf/compare.py target/perf/baseline.json target/perf/candidate.json
```

Five timing runs capture 40 input events per interactive scenario per run. Each memory scene has three settled measurements per build. The candidate is commit `2cc789eb616b0ed7a237eea578bd61fd7bedce67`, containing the complete 0.12.9 application changes. Machine and raw sample details are in [candidate](perf-0.12.9.json) and [baseline](perf-0.12.9-baseline.json).

All blocking metrics passed. Cold extension activation is approximately 352 ms in both builds, above the 150 ms advisory target.

```text
baseline  v0.12.8 (9e292e8f5a)
candidate v0.12.8-35-g2cc789eb (2cc789eb61)

metric                           stat     baseline  candidate   change  result
startup.first_frame_ms           p95         206.1      193.0    -6.4%  ok
startup.window_open_ms           p95         167.9      154.8    -7.8%  ok
startup.extensions_ms            p95           1.3        1.2    -9.1%  ok
typing.keystroke_frame_ms        p95          25.8       24.2    -6.2%  ok
typing.keystroke_frame_words_ms  p95          23.7       22.9    -3.3%  ok
typing.keystroke_frame_decorations_ms p95          16.5       16.6    +0.2%  ok
typing.keystroke_frame_completion_ms p95          24.1       23.2    -3.9%  ok
typing.keystroke_frame_lenses_ms p95          16.6       16.7    +0.5%  ok
find.keystroke_frame_ms          p95          14.2       14.2    -0.6%  ok
views.keystroke_frame_ms         p95          16.5       16.6    +0.4%  ok
extensions.provider_overhead_ms  p95           0.9        0.4   -51.2%  ok
extensions.activation_ms         p95         351.6      351.7    +0.0%  warn: over ceiling 150
extensions.hot_reload_ms         p95         140.0      142.7    +1.9%  ok
market.search_ms                 p95           4.7        4.7    -1.0%  ok
market.search_frame_ms           p95          18.3       18.3    +0.1%  ok
memory.empty_window_mb           median       72.0       72.0    +0.0%  ok
memory.syntax_10k_mb             median      118.0      117.0    -0.8%  ok
memory.terminal_mb               median       80.0       80.0    +0.0%  ok
```
