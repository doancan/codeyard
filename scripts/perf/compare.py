"""Compare a candidate's perf results with a baseline; exit 1 when a blocking metric regressed.
   python3 -I scripts/perf/compare.py BASELINE.json CANDIDATE.json [--catalogue perf/metrics.json]

A metric regresses when its statistic (median or p95) is worse than the baseline's by more than its budget, and the
change is not noise. Noise is judged on the same statistic: the candidate's value must also be above the upper end of
a 95% bootstrap interval of the baseline's value. Small samples (memory has 3) have wide intervals, so for a median
it is also a regression when every candidate sample is worse than the baseline's median by more than the budget.
A blocking metric that the baseline has no samples for (a release before the hooks) is reported, not passed, and so
is a comparison with fewer than MIN_SAMPLES on either side: one GPU texture spike is ±8 MB in a memory scene.
A metric with `ceiling_vs` has a ceiling relative to another metric of the same candidate run: that metric's statistic
plus `margin` (0.10 = 10%). Keystroke-to-frame times include up to two display refreshes, so a fixed number in
milliseconds cannot say "extensions add nothing visible over plain typing"; the same run's plain typing can."""
import json, math, random, sys

MIN_SAMPLES = 3

args = sys.argv[1:]
cat_path = "perf/metrics.json"
if "--catalogue" in args:
    i = args.index("--catalogue")
    cat_path = args[i + 1]
    del args[i:i + 2]
if len(args) != 2:
    sys.exit(__doc__)
base, cand = (json.load(open(p, encoding="utf-8")) for p in args)
catalogue = json.load(open(cat_path, encoding="utf-8"))["metrics"]


def stat(values, name):
    v = sorted(values)
    if name == "median":
        n = len(v)
        return v[n // 2] if n % 2 else (v[n // 2 - 1] + v[n // 2]) / 2
    if name == "p95":
        return v[max(0, math.ceil(0.95 * len(v)) - 1)]
    raise ValueError(name)


def upper_bound(values, name, rounds=2000):
    """97.5th percentile of the statistic over bootstrap resamples (fixed seed: the same input gives the same answer)."""
    rng = random.Random(len(values) * 7919 + int(sum(values)))
    n = len(values)
    est = sorted(stat([values[rng.randrange(n)] for _ in range(n)], name) for _ in range(rounds))
    return est[min(rounds - 1, math.ceil(0.975 * rounds) - 1)]


failed = False
print(f"baseline  {base['version']} ({base['commit'][:10]})\ncandidate {cand['version']} ({cand['commit'][:10]})")
if base.get("machine") != cand.get("machine"):
    print(f"warning: measured on different machines: {base.get('machine')} vs {cand.get('machine')}")
print(f"\n{'metric':32} {'stat':6} {'baseline':>10} {'candidate':>10} {'change':>8}  result")
for m in catalogue:
    b, c = base["runs"].get(m["id"]), cand["runs"].get(m["id"])
    blocking = m["gate"] == "block"
    if not c:
        failed |= blocking
        print(f"{m['id']:32} {m['stat']:6} {'-':>10} {'-':>10} {'':>8}  {'FAIL' if blocking else 'warn'}: no samples")
        continue
    cv = stat(c, m["stat"])
    problems, notes = [], []
    if m["ceiling"] is not None and cv > m["ceiling"]:
        problems.append(f"over ceiling {m['ceiling']}")
    if vs := m.get("ceiling_vs"):
        ref = cand["runs"].get(vs["metric"])
        if not ref:
            problems.append(f"no {vs['metric']} samples for its ceiling")
        elif cv > (limit := stat(ref, m["stat"]) * (1 + vs["margin"])):
            problems.append(f"over ceiling {limit:.1f} ({vs['metric']} {vs['margin']:+.0%})")
    change, bshow = "", "-"
    if b:
        bv = stat(b, m["stat"])
        bshow = f"{bv:.1f}"
        rel = (cv - bv) / bv if bv else 0.0
        change = f"{rel:+.1%}"
        # A metric of a few milliseconds also has an absolute budget: one cold disk read is +80% there.
        limit = max(bv * (1 + m["budget"]), bv + m.get("budget_abs", 0))
        if cv > limit and min(len(b), len(c)) < MIN_SAMPLES:
            notes.append(f"worse, but fewer than {MIN_SAMPLES} samples to judge")
        elif cv > limit:
            outside_noise = cv > upper_bound(b, m["stat"])
            all_worse = m["stat"] == "median" and min(c) > limit
            if outside_noise or all_worse:
                problems.append(f"regressed more than {m['budget']:.0%}" + (f" and {m['budget_abs']:g}" if m.get("budget_abs") else ""))
    if problems:
        failed |= blocking
        result = ("FAIL: " if blocking else "warn: ") + ", ".join(problems)
    elif notes:
        result = "warn: " + ", ".join(notes)
    elif not b:
        result = "warn: no baseline samples, not compared" if blocking else "ok (no baseline)"
    else:
        result = "ok"
    print(f"{m['id']:32} {m['stat']:6} {bshow:>10} {cv:>10.1f} {change:>8}  {result}")
sys.exit(1 if failed else 0)
