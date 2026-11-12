#!/usr/bin/env bash
set -u
from=${1:?from}
to=${2:?to}
jobs=${3:-5}
shift 3 || true
extra="$*"
here=$(cd "$(dirname "$0")" && pwd)
bin=${NANOCODE_SIM:-${CARGO_TARGET_DIR:-$here/../../target}/release/nanocode-sim}
chunk=$(( (to - from + jobs * 8 - 1) / (jobs * 8) ))
log=$(mktemp)
for ((start = from; start < to; start += chunk)); do
  end=$((start + chunk))
  ((end > to)) && end=$to
  echo "$start $end"
done | xargs -P "$jobs" -n 2 sh -c "exec \"$bin\" --seeds \"\$0..\$1\" $extra" >"$log" 2>&1
grep '^FAIL\|^CAUGHT' "$log"
awk '/^RESULT/ { for (i = 1; i <= NF; i++) { split($i, kv, "="); if (kv[1] == "passed") p += kv[2]; if (kv[1] == "failed") f += kv[2] } } END { printf "SWEEP passed=%d failed=%d\n", p, f; exit f > 0 }' "$log"
status=$?
grep -c '^panic\|panicked' "$log" | sed 's/^/panics=/'
rm -f "$log"
exit $status
