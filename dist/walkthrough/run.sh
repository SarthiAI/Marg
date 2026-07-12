#!/usr/bin/env bash
#
# Marg end-to-end walkthrough driver.
#
# Walks the operator through every scenario in dist/walkthrough/, prompts
# them to run each scenario's documented steps, and records pass/fail per
# scenario. Outputs a results dir under dist/walkthrough/results/<rfc3339>/
# that contains one Markdown file per scenario plus a `summary.md`.
#
# This is deliberately operator-driven (not fully scripted): per the project
# rule "no tests, yes manual end-to-end walkthroughs", the human running this
# is in the loop. The script's job is to enforce that every scenario is
# *touched* and *recorded*, not to replace the human's judgment.
#
# Usage:
#   ./dist/walkthrough/run.sh                       # walk every scenario
#   ./dist/walkthrough/run.sh 03-providers          # one scenario only
#   MARG_WT_KEEP_ARTIFACTS=1 ./dist/walkthrough/run.sh
#
# Requires: bash, mktemp, date. Each scenario itself documents the extra
# tools (curl, jq, docker) it needs.

set -euo pipefail

WT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$WT_DIR/../.." && pwd)"

SCENARIOS=(
  "01-boot-and-ops"
  "02-auth-and-budgets"
  "03-providers"
  "04-storage-tiers"
  "05-write-batcher"
  "06-admin-api"
  "07-console"
  "08-cli"
  "09-kavach-observe-enforce"
  "10-kavach-invariant"
  "11-kavach-permit-signing"
  "12-kavach-drift"
  "13-kavach-audit-chain"
  "14-hot-reload"
  "15-mode-flip"
  "16-bench-repass"
  "17-cluster-invalidation"
)

FILTER="${1:-}"
TS="$(date -u +%Y-%m-%dT%H-%M-%SZ)"
RESULTS_DIR="$WT_DIR/results/$TS"
mkdir -p "$RESULTS_DIR"
SUMMARY="$RESULTS_DIR/summary.md"

{
  echo "# Walkthrough results, $TS"
  echo
  echo "Box: $(uname -a)"
  echo "Marg binary: ${MARG_BIN:-$ROOT/target/release/marg}"
  echo "Stub binary: ${MARG_STUB_BIN:-$ROOT/target/release/marg-provider-stub}"
  echo
  echo "| Scenario | Result | Notes |"
  echo "|---|---|---|"
} > "$SUMMARY"

run_one() {
  local name="$1"
  local file="$WT_DIR/$name.md"
  if [[ ! -f "$file" ]]; then
    echo "Walkthrough file not found: $file" >&2
    return 2
  fi
  local outfile="$RESULTS_DIR/$name.md"
  echo
  echo "============================================================"
  echo "Scenario: $name"
  echo "Source:   $file"
  echo "Output:   $outfile"
  echo "============================================================"
  echo

  # Show the scenario contents (paginated if `less` is available).
  if command -v less >/dev/null 2>&1; then
    less -RFX "$file" || true
  else
    cat "$file"
  fi

  echo
  echo "Did this scenario PASS, FAIL, SKIP, or PARTIAL?"
  read -r -p "  result> " result
  result_upper="$(echo "$result" | tr '[:lower:]' '[:upper:]')"
  case "$result_upper" in
    PASS|FAIL|SKIP|PARTIAL) ;;
    *) echo "  unrecognised result, recording as PARTIAL"; result_upper="PARTIAL" ;;
  esac
  echo "Notes (one line, blank to skip):"
  read -r -p "  notes> " notes

  {
    echo "# $name"
    echo
    echo "- Result: $result_upper"
    echo "- Recorded at: $(date -u +%Y-%m-%dT%H:%M:%SZ)"
    echo "- Notes: ${notes:-(none)}"
    echo
    echo "## Scenario file"
    echo
    cat "$file"
  } > "$outfile"

  echo "| $name | $result_upper | ${notes:-} |" >> "$SUMMARY"

  if [[ "$result_upper" == "FAIL" ]]; then
    return 1
  fi
}

OVERALL=0

if [[ -z "$FILTER" ]]; then
  for s in "${SCENARIOS[@]}"; do
    if ! run_one "$s"; then
      OVERALL=1
    fi
  done
else
  if ! run_one "$FILTER"; then
    OVERALL=1
  fi
fi

{
  echo
  echo "Overall exit: $OVERALL"
} >> "$SUMMARY"

echo
echo "Summary written to $SUMMARY"
echo "Per-scenario results under $RESULTS_DIR"

exit "$OVERALL"
