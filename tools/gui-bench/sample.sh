#!/usr/bin/env bash
# Samples the memory of a macOS GUI app while you do one thing in it, and
# prints start / end / peak memory and how long it took.
#
#   tools/gui-bench/sample.sh "GitHub Desktop"     # the app's name in /Applications
#
# 1. Open the app and the repository first (the start point is "repository
#    open, history showing"). Quit other copies of the same app.
# 2. Run this, press Enter, scroll the history to the very last commit, and
#    press Enter again the moment it is there.
#
# Memory is the macOS physical footprint (what Activity Monitor calls
# "Memory"), summed over every process inside the .app (Electron apps run
# several). Samples go to stdout as CSV: seconds,MB.
set -euo pipefail

app="${1:?usage: sample.sh \"<app name>\"}"
interval="${INTERVAL:-0.5}"

footprint_mb() {
  local total=0 pid mb
  for pid in $(pgrep -f "/$app.app/Contents/" || true); do
    mb=$(footprint -p "$pid" 2>/dev/null |
      awk '/phys_footprint:/ { v=$2; if ($3=="GB") v*=1024; else if ($3=="KB") v/=1024; print v; exit }')
    [ -n "$mb" ] && total=$(awk -v a="$total" -v b="$mb" 'BEGIN { print a + b }')
  done
  echo "$total"
}

[ -n "$(pgrep -f "/$app.app/Contents/" || true)" ] || { echo "$app is not running" >&2; exit 1; }

read -r -p "Press Enter, then scroll to the last commit... " _
out="$(mktemp)"
trap 'rm -f "$out"' EXIT
t0=$(date +%s.%N)
(
  while :; do
    now=$(date +%s.%N)
    printf '%.1f,%.0f\n' "$(awk -v a="$now" -v b="$t0" 'BEGIN { print a - b }')" "$(footprint_mb)"
    sleep "$interval"
  done
) >"$out" &
sampler=$!
read -r -p "Press Enter when the last commit is on screen. " _
t1=$(date +%s.%N)
kill "$sampler" 2>/dev/null || true
wait "$sampler" 2>/dev/null || true

echo "seconds,MB"
cat "$out"
awk -F, -v secs="$(awk -v a="$t1" -v b="$t0" 'BEGIN { printf "%.1f", a - b }')" '
  NR == 1 { first = $2 }
  { last = $2; if ($2 > peak) peak = $2 }
  END { printf "\n%s\nstart %d MB, end %d MB, peak %d MB, %s s\n", "'"$app"'", first, last, peak, secs }
' "$out"
