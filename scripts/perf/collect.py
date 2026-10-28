"""Turn the JSON lines written under CODEYARD_PERF into one result file (stdout).
   python3 -I scripts/perf/collect.py samples.jsonl COMMIT VERSION"""
import json, platform, subprocess, sys, time

path, commit, version = sys.argv[1:4]
runs = {}
with open(path, encoding="utf-8") as f:
    for line in f:
        line = line.strip()
        if not line:
            continue
        s = json.loads(line)
        if s["value"] is not None:
            runs.setdefault(s["metric"], []).append(s["value"])

def sysctl(key):
    try:
        return subprocess.run(["/usr/sbin/sysctl", "-n", key], capture_output=True, text=True, check=True).stdout.strip()
    except (OSError, subprocess.CalledProcessError):
        return None

json.dump({
    "date": time.strftime("%Y-%m-%d"),
    "version": version,
    "commit": commit,
    "machine": {"model": sysctl("hw.model"), "cpu": sysctl("machdep.cpu.brand_string"), "os": platform.mac_ver()[0]},
    "runs": {k: runs[k] for k in sorted(runs)},
}, sys.stdout, indent=2)
print()
