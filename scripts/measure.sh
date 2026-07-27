#!/bin/bash
set -euo pipefail

# Measures how much page text each chrome-agent command hands to a model, and
# whether `extract` actually found the right number of records.
#
# Usage:
#   ./scripts/measure.sh                  # default page set
#   ./scripts/measure.sh <url> [selector] # one page, optional ground-truth selector
#
# The selector is the honest half of this: it counts the records a human says are
# on the page, so a small output that missed half the data shows up as wrong
# rather than as a win. Size without accuracy is not a result.
#
# Token counts are chars/4, which is a rough average for English page text. They
# are there for orders of magnitude, not for billing.

BIN="${CHROME_AGENT_BIN:-./target/release/chrome-agent}"
CHARS_PER_TOKEN=4

if [ ! -x "$BIN" ]; then
  echo "Binary not found at $BIN — run: cargo build --release" >&2
  exit 1
fi

# url|label|ground-truth selector (empty = no record count to check against)
PAGES_DEFAULT=(
  "https://news.ycombinator.com|Hacker News front page|.titleline > a"
  "https://blog.rust-lang.org|Rust blog index|"
  "https://example.com|example.com|"
)

if [ $# -gt 0 ]; then
  PAGES=("$1|${1}|${2:-}")
else
  PAGES=("${PAGES_DEFAULT[@]}")
fi

tokens() { echo $(( $1 / CHARS_PER_TOKEN )); }

size_of() {
  # $1 = browser name, rest = command + args.
  # A command that fails prints an error, and counting that error as output would
  # flatter the numbers, so report it as n/a instead of measuring it.
  local br="$1"; shift
  local out status
  out=$("$BIN" --browser "$br" "$@" 2>/dev/null) && status=0 || status=$?
  if [ "$status" -ne 0 ]; then
    echo "n/a"
  else
    tokens "$(printf '%s' "$out" | wc -c | tr -d ' ')"
  fi
}

printf '| Page | extract | read | text | inspect | raw HTML | records found | expected |\n'
printf '|---|---|---|---|---|---|---|---|\n'

i=0
for entry in "${PAGES[@]}"; do
  i=$((i + 1))
  url="${entry%%|*}"
  rest="${entry#*|}"
  label="${rest%%|*}"
  selector="${rest#*|}"
  br="measure$i"

  "$BIN" --browser "$br" goto "$url" >/dev/null 2>&1 || {
    printf '| %s | navigation failed | | | | | | |\n' "$label"
    continue
  }

  # Measure a complete extraction, not the default first 10 records. Comparing a
  # truncated result against a whole page would flatter extract for no reason.
  ex=$(size_of "$br" extract --limit 500)
  rd=$(size_of "$br" read)
  tx=$(size_of "$br" text)
  ins=$(size_of "$br" inspect)
  html_chars=$("$BIN" --browser "$br" eval "document.documentElement.outerHTML.length" 2>/dev/null | tr -d ' \n"' || echo 0)
  html=$(tokens "${html_chars:-0}")

  found=$("$BIN" --browser "$br" --json extract --limit 500 2>/dev/null \
    | sed -n 's/.*"count":\([0-9]*\).*/\1/p' | head -1 || true)
  found="${found:-0}"

  if [ -n "$selector" ]; then
    expected=$("$BIN" --browser "$br" eval \
      "document.querySelectorAll('${selector}').length" 2>/dev/null | tr -d ' \n"' || echo "?")
  else
    expected="-"
  fi

  printf '| %s | %s | %s | %s | %s | %s | %s | %s |\n' \
    "$label" "$ex" "$rd" "$tx" "$ins" "$html" "$found" "$expected"

  "$BIN" --browser "$br" close --purge >/dev/null 2>&1 || true
done

echo
echo "Numbers are estimated tokens (chars/${CHARS_PER_TOKEN}). Measured with $("$BIN" --version)."
echo "Reproduce: ./scripts/measure.sh"
