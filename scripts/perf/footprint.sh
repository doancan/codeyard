# Settled macOS footprint of a process, shared by scripts/memcheck.sh and scripts/perf/run.sh.
#   . scripts/perf/footprint.sh; settled_mb PID
# Measures `footprint` (what Activity Monitor shows) once it settles: at startup Metal spikes to ~180 MB and drops
# within seconds, by a variable amount of time. Sampled once a second; the value is taken when 3 consecutive samples
# agree within 2 MB (MAX_WAIT seconds at most, default 30).
MAX_WAIT=${MAX_WAIT:-30}

fp_mb() {
  footprint "$1" 2>/dev/null | awk '/Footprint:/ {
    for (i = 1; i <= NF; i++) if ($i == "Footprint:") { v = $(i+1); u = $(i+2) }
    if (u == "KB") v /= 1024; else if (u == "GB") v *= 1024; printf "%.0f", v; exit }'
}

# Sample until the value settles.
settled_mb() {
  local pid=$1 a=0 b=0 c=0 i
  # The startup peak (~195 MB) usually lasts 2-3 s, but 6-9 s while a language server
  # indexes on all cores, and it stayed flat for 3 samples so it looked settled.
  # 2026-10-22: it lasted 12 s even with the server idle (199 -> 78 MB). Wait 20 s first.
  sleep 20
  for i in $(seq 1 "$MAX_WAIT"); do
    a=$b; b=$c; c=$(fp_mb "$pid")
    if [ "$i" -ge 3 ] && [ $((a > c ? a - c : c - a)) -le 2 ] && [ $((b > c ? b - c : c - b)) -le 2 ]; then
      break
    fi
    sleep 1
  done
  echo "$c"
}
