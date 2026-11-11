#!/usr/bin/env bash
# The 5% regression gate: the base tree's benchmarks and the change's, built
# apart, run alternately for several rounds on one machine; report.py fails
# when a sys-lib benchmark is more than 5% slower in every round.
#
#   bench/gate.sh <base-checkout> <change-checkout> [rounds]
set -euo pipefail
base="$(cd "${1:?base checkout}" && pwd)"
change="$(cd "${2:?change checkout}" && pwd)"
rounds="${3:-3}"
here="$(cd "$(dirname "$0")" && pwd)"
export CRITERION_HOME="${CRITERION_HOME:-$PWD/criterion-gate}"
rm -rf -- "${CRITERION_HOME:?}"
filter='sys-lib|reports'

build() {
  (cd "$1/bench" && CARGO_TARGET_DIR="$2" cargo bench --no-run --bench readings --message-format=json 2>/dev/null) |
    python3 -c 'import json,sys
for line in sys.stdin:
    m = json.loads(line)
    if m.get("executable") and m.get("target", {}).get("name") == "readings":
        print(m["executable"])'
}
b="$(build "$base" "$PWD/target-base")"
c="$(build "$change" "$PWD/target-change")"
# The order alternates, so load that rises or falls during the job does not
# always land on the same side.
for n in $(seq 1 "$rounds"); do
  if [ $((n % 2)) -eq 1 ]; then
    "$b" --bench "$filter" --save-baseline "base-$n" --noplot >/dev/null
    "$c" --bench "$filter" --save-baseline "pr-$n" --noplot >/dev/null
  else
    "$c" --bench "$filter" --save-baseline "pr-$n" --noplot >/dev/null
    "$b" --bench "$filter" --save-baseline "base-$n" --noplot >/dev/null
  fi
done
python3 "$here/report.py" gate "$CRITERION_HOME" "$rounds"
