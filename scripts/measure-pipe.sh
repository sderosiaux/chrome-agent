#!/bin/bash
set -euo pipefail

# Measures what `pipe` actually saves: the SAME command sequence run as one CLI process per
# command, then as one pipe session, wall clock, on local file:// fixtures.
#
# Usage:
#   cargo build --release
#   ./scripts/measure-pipe.sh          # 9 runs per mode per workload, ~1 minute
#   ./scripts/measure-pipe.sh 15       # more runs
#
# Two workloads, because they answer differently. `reads` is a stream of cheap page reads,
# where the per-invocation preamble (process start, HTTP GET, two WebSocket handshakes, the
# CDP setup round trips, the session-store lock/merge/write) is most of the cost. `actions`
# fills and clicks, where the per-command cost pipe does NOT remove — the aim probe, the
# settle window, the tree re-read — is most of the cost. A ratio from one is not a ratio for
# the other, which is the whole reason both are here.
#
# The browser is launched, and one warm-up sequence run, BEFORE any timing: a cold Chrome
# start would land on whichever mode went first and drown the effect being measured. Both
# modes then drive the same already-running browser, so what is left is per-command overhead.
# Timing is bash's own `time` (TIMEFORMAT=%3R), 1 ms resolution, around the whole sequence.
#
# Reported: median of N runs and the min-max spread. A single run is not a measurement.
#
# Run it on an otherwise idle machine. Measured on a busy one (five cargo builds in parallel) the
# same workloads read 869 ms with a 373-1161 spread where a quiet machine gives 332 ms (316-343):
# the spread is the tell, so read it before the median. A min-max wider than ~15% of the median
# means something else was running and the number is not usable.

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
BIN="${CHROME_AGENT_BIN:-$ROOT/target/release/chrome-agent}"
RUNS="${1:-9}"

if [ ! -x "$BIN" ]; then
  echo "Binary not found at $BIN — run: cargo build --release" >&2
  exit 1
fi

READ_PAGE="file://$ROOT/tests/fixtures/extract_hn_like.html"
ACTION_PAGE="file://$ROOT/tests/fixtures/multi_field_form.html"

TIMEFORMAT='%3R'

# --- the two workloads, spelled once per mode so the sequences cannot drift apart ---

reads_cli() {
  local br="$1"
  "$BIN" --browser "$br" goto "$READ_PAGE"
  "$BIN" --browser "$br" text
  "$BIN" --browser "$br" inspect
  "$BIN" --browser "$br" eval "document.title"
  "$BIN" --browser "$br" assert exists --selector "a" --min 1
  "$BIN" --browser "$br" text --selector "body"
  "$BIN" --browser "$br" extract
  "$BIN" --browser "$br" eval "document.querySelectorAll('a').length"
  "$BIN" --browser "$br" inspect
}

reads_pipe_input() {
  cat <<EOF
{"cmd":"goto","url":"$READ_PAGE"}
{"cmd":"text"}
{"cmd":"inspect"}
{"cmd":"eval","expression":"document.title"}
{"cmd":"assert","what":"exists","selector":"a","min":1}
{"cmd":"text","selector":"body"}
{"cmd":"extract"}
{"cmd":"eval","expression":"document.querySelectorAll('a').length"}
{"cmd":"inspect"}
EOF
}

# The best case pipe can have: commands that do almost nothing, so what is left is almost
# entirely the per-invocation preamble. Run at two lengths, because the ratio is not a
# constant — the one `goto` is a fixed cost both modes pay, so a longer sequence dilutes it
# and the ratio climbs. It is a ceiling, not a workload anyone runs.
TRIVIAL_N=8

trivial_cli() {
  local br="$1"
  "$BIN" --browser "$br" goto "$READ_PAGE"
  local i
  for ((i = 0; i < TRIVIAL_N; i++)); do "$BIN" --browser "$br" eval "1"; done
}

trivial_pipe_input() {
  echo "{\"cmd\":\"goto\",\"url\":\"$READ_PAGE\"}"
  local i
  for ((i = 0; i < TRIVIAL_N; i++)); do echo '{"cmd":"eval","expression":"1"}'; done
}

actions_cli() {
  local br="$1"
  "$BIN" --browser "$br" goto "$ACTION_PAGE"
  "$BIN" --browser "$br" fill --selector "#name" "Ada Lovelace"
  "$BIN" --browser "$br" fill --selector "#phone" "5551234567"
  "$BIN" --browser "$br" fill --selector "#qty" "3"
  "$BIN" --browser "$br" click --selector "#submit"
  "$BIN" --browser "$br" fill --selector "#name" "Grace Hopper"
  "$BIN" --browser "$br" fill --selector "#phone" "5559876543"
  "$BIN" --browser "$br" fill --selector "#qty" "7"
  "$BIN" --browser "$br" click --selector "#submit"
}

actions_pipe_input() {
  cat <<EOF
{"cmd":"goto","url":"$ACTION_PAGE"}
{"cmd":"fill","selector":"#name","value":"Ada Lovelace"}
{"cmd":"fill","selector":"#phone","value":"5551234567"}
{"cmd":"fill","selector":"#qty","value":"3"}
{"cmd":"click","selector":"#submit"}
{"cmd":"fill","selector":"#name","value":"Grace Hopper"}
{"cmd":"fill","selector":"#phone","value":"5559876543"}
{"cmd":"fill","selector":"#qty","value":"7"}
{"cmd":"click","selector":"#submit"}
EOF
}

# --- timing ---

# Wall clock of one whole sequence, in seconds with 3 decimals. Output of the sequence goes
# to /dev/null; only `time`'s own line is captured.
timed() {
  { time "$@" >/dev/null 2>&1; } 2>&1
}

run_cli() { timed "${1}_cli" "$2"; }

run_pipe() {
  local workload="$1" br="$2"
  { time "${workload}_pipe_input" | "$BIN" --browser "$br" pipe >/dev/null 2>&1; } 2>&1
}

# median + min/max of the numbers on stdin, in milliseconds.
stats() {
  sort -n | awk '
    { v[NR] = $1 * 1000 }
    END {
      if (NR == 0) { print "n/a"; exit }
      m = (NR % 2) ? v[(NR+1)/2] : (v[NR/2] + v[NR/2+1]) / 2
      printf "%.0f|%.0f|%.0f", m, v[1], v[NR]
    }'
}

measure_workload() {
  local workload="$1" label="$2"
  local br="bench-${workload}-$$"

  # Launch and warm: the first sequence pays for Chrome's start and first paint.
  "${workload}_cli" "$br" >/dev/null 2>&1 || true

  local cli_times="" pipe_times="" i
  for ((i = 0; i < RUNS; i++)); do
    cli_times+="$(run_cli "$workload" "$br")"$'\n'
    pipe_times+="$(run_pipe "$workload" "$br")"$'\n'
  done

  local c p
  c=$(printf '%s' "$cli_times" | stats)
  p=$(printf '%s' "$pipe_times" | stats)
  local cm="${c%%|*}" pm="${p%%|*}"
  local ratio
  ratio=$(awk -v a="$cm" -v b="$pm" 'BEGIN { if (b > 0) printf "%.1fx", a / b; else print "n/a" }')

  printf '| %s | %s ms (%s–%s) | %s ms (%s–%s) | **%s** |\n' \
    "$label" \
    "$cm" "$(echo "$c" | cut -d'|' -f2)" "$(echo "$c" | cut -d'|' -f3)" \
    "$pm" "$(echo "$p" | cut -d'|' -f2)" "$(echo "$p" | cut -d'|' -f3)" \
    "$ratio"

  "$BIN" --browser "$br" close --purge >/dev/null 2>&1 || true
}

# One CLI command against an already-running browser: the per-invocation floor on its own,
# which is what the "startup" row of the README comparison table is about.
measure_floor() {
  local br="bench-floor-$$"
  "$BIN" --browser "$br" goto "$READ_PAGE" >/dev/null 2>&1 || true
  "$BIN" --browser "$br" eval "1" >/dev/null 2>&1 || true

  local times="" i
  for ((i = 0; i < RUNS; i++)); do
    times+="$({ time "$BIN" --browser "$br" eval "1" >/dev/null 2>&1; } 2>&1)"$'\n'
  done
  local s
  s=$(printf '%s' "$times" | stats)
  printf '| one CLI `eval 1` on a running browser | %s ms (%s–%s) |\n' \
    "${s%%|*}" "$(echo "$s" | cut -d'|' -f2)" "$(echo "$s" | cut -d'|' -f3)"
  "$BIN" --browser "$br" close --purge >/dev/null 2>&1 || true
}

echo "Each sequence starts with one \`goto\`, which both modes pay once. $RUNS timed runs per mode, median (min–max)."
echo
printf '| Workload | one process per command | one pipe session | speed-up |\n'
printf '|---|---|---|---|\n'
measure_workload reads "reads (text/inspect/eval/assert/extract), 9 commands"
measure_workload actions "actions (fill x6, click x2), 9 commands"
TRIVIAL_N=8
measure_workload trivial "ceiling: \`eval 1\` x8, 9 commands"
TRIVIAL_N=40
measure_workload trivial "ceiling: \`eval 1\` x40, 41 commands"

echo
printf '| Per-invocation floor | wall clock |\n'
printf '|---|---|\n'
measure_floor

echo
echo "Machine: $(uname -srm), $(sysctl -n machdep.cpu.brand_string 2>/dev/null || grep -m1 'model name' /proc/cpuinfo 2>/dev/null | cut -d: -f2- || echo 'unknown CPU')"
echo "Chrome: $("$BIN" --version) driving $(/Applications/Google\ Chrome.app/Contents/MacOS/Google\ Chrome --version 2>/dev/null || google-chrome --version 2>/dev/null || echo 'unknown Chrome')"
echo "Date: $(date -u '+%Y-%m-%d')"
echo "Reproduce: ./scripts/measure-pipe.sh"
