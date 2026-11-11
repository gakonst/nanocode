#!/usr/bin/env bash
# Regenerate builtins/gen/ from builtins/*.ts with djs-bgen.
#   scripts/bgen.sh            write builtins/gen/prelude.js and resume-points.txt
#   scripts/bgen.sh --check    exit 1 if they are out of date (used by the gate)
#   scripts/bgen.sh --report   print the resume-point report
set -euo pipefail
cd "$(dirname "$0")/.."
exec cargo run --quiet -p djs-bgen -- "$@"
