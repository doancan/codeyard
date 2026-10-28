"""Compare a candidate's perf results with a baseline; exit 1 when a blocking metric regressed.
   python3 -I scripts/perf/compare.py BASELINE.json CANDIDATE.json [--catalogue perf/metrics.json]

A metric regresses when its statistic (median or p95) is worse than the baseline's by more than its budget AND the
candidate's median is worse than the baseline's p95. The second condition is the noise floor: two runs of the same
build overlap, so a shift that stays inside the baseline's own spread is not reported."""
import json, math, sys

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

failed = False
print(f"baseline  {base['version']} ({base['commit'][:10]})\ncandidate {cand['version']} ({cand['commit'][:10]})\n")
print(f"{'metric':32} {'stat':6} {'baseline':>10} {'candidate':>10} {'change':>8}  result")
for m in catalogue:
    b, c = base["runs"].get(m["id"]), cand["runs"].get(m["id"])
    if not c:
        result = "FAIL (no samples)" if m["gate"] == "block" else "warn (no samples)"
        failed |= m["gate"] == "block"
        print(f"{m['id']:32} {m['stat']:6} {'-':>10} {'-':>10} {'':>8}  {result}")
        continue
    cv = stat(c, m["stat"])
    problems = []
    if m["ceiling"] is not None and cv > m["ceiling"]:
        problems.append(f"over ceiling {m['ceiling']}")
    change = ""
    if b:
        bv = stat(b, m["stat"])
        rel = (cv - bv) / bv if bv else 0.0
        change = f"{rel:+.1%}"
        if rel > m["budget"] and stat(c, "median") > stat(b, "p95"):
            problems.append(f"regressed more than {m['budget']:.0%}")
    bshow = f"{stat(b, m['stat']):.1f}" if b else "-"
    if problems:
        result = ("FAIL: " if m["gate"] == "block" else "warn: ") + ", ".join(problems)
        failed |= m["gate"] == "block"
    else:
        result = "ok" if b else "ok (no baseline)"
    print(f"{m['id']:32} {m['stat']:6} {bshow:>10} {cv:>10.1f} {change:>8}  {result}")
sys.exit(1 if failed else 0)
