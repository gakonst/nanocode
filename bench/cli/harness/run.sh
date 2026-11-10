#!/usr/bin/env bash
# Full benchmark run into results/<date>-<machine>/. Usage: harness/run.sh [machine-name] [steps...]
# Steps (default all): smoke netaudit startup coldstart firstframe coldframe idle keys oneshot tools fleet store disk cli tui tui50k tuisize report
# ROUNDS, SAMPLES, ONLY (comma lanes), BENCH_NC_VARIANTS and QUIET_LOAD (1-minute load to wait for, default 0.5) tune it.
set -uo pipefail
cd "$(dirname "$0")/.."
ROOT=$PWD
[ -f tools/install/env.sh ] && . tools/install/env.sh
export PYTHON=${PYTHON:-$ROOT/.venv/bin/python}
machine=${1:-$(hostname -s)}; shift || true
steps=${*:-smoke netaudit startup coldstart firstframe coldframe idle keys oneshot tools fleet store disk cli tui tui50k tuisize report}
dir=${BENCH_RESULTS:-$ROOT/results/$(date -u +%F)-$machine}
export BENCH_RESULTS=$dir
mkdir -p "$dir"
log=$dir/run.log
only=${ONLY:+--only=$ONLY}
rounds=${ROUNDS:-30}
samples=${SAMPLES:-10}
guard=""
[ "${NETGUARD:-1}" = 1 ] && [ -x tools/netguard.sh ] && guard=tools/netguard.sh

note() { echo "$(date -u +%FT%TZ) $*" | tee -a "$log"; }
quiet() {
  local limit=${QUIET_LOAD:-0.5} waited=0
  while :; do
    local load; load=$(cut -d' ' -f1 /proc/loadavg 2>/dev/null || sysctl -n vm.loadavg | awk '{print $2}')
    awk -v l="$load" -v m="$limit" 'BEGIN{exit !(l<m)}' && break
    if [ $((waited % 60)) = 0 ]; then note "waiting for load $load < $limit; busiest:"; ps -eo pcpu,pid,etime,args --sort=-pcpu | awk 'NR>1 && $1>5' | head -5 | cut -c1-160 | tee -a "$log"; fi
    sleep 5; waited=$((waited + 5))
  done
}
machine_json() {
  node -e '
    const os = require("os"), fs = require("fs"), { execSync } = require("child_process");
    const sh = (c) => { try { return execSync(c, { encoding: "utf8" }).trim(); } catch { return null; } };
    fs.writeFileSync(process.argv[1], JSON.stringify({
      name: process.argv[2], cpu: os.cpus()[0].model.trim(), cores: os.cpus().length, memory_gb: Math.round(os.totalmem() / 2 ** 30),
      os: sh(". /etc/os-release && echo $PRETTY_NAME") ?? sh("sw_vers -productName") + " " + sh("sw_vers -productVersion"), kernel: os.release(), arch: os.arch(),
      governor: sh("cat /sys/devices/system/cpu/cpu0/cpufreq/scaling_governor"), node: process.version, load_at_start: os.loadavg(),
      virtualization: sh("systemd-detect-virt"), at: new Date().toISOString(), commit: sh("git rev-parse HEAD"),
    }, null, 1));
  ' "$dir/machine.json" "$machine"
}
step() { quiet; note "start $1 (load $(cut -d' ' -f1 /proc/loadavg 2>/dev/null))"; shift; echo "+ $*" >> "$dir/COMMANDS.md"; "$@" >> "$log" 2>&1; local rc=$?; note "end rc=$rc"; }

machine_json
cp tools/install/VERSIONS.md "$dir/VERSIONS.md" 2>/dev/null || true
[ -n "${BENCH_NC_VARIANTS:-}" ] && echo "nc variants: $BENCH_NC_VARIANTS" >> "$dir/VERSIONS.md"
echo "# Commands ($(date -u +%FT%TZ), cwd bench/cli, harness commit $(git rev-parse --short HEAD))" >> "$dir/COMMANDS.md"
for s in $steps; do
  case $s in
    smoke) step smoke $guard node harness/smoke.mjs $only ;;
    netaudit) step netaudit node harness/netaudit.mjs $only --dir="$dir" ;;
    startup) step startup $guard node harness/suite.mjs --parts=startup --rounds=$((rounds > 100 ? rounds : 100)) $only ;;
    coldstart|coldframe) step $s $guard node harness/suite.mjs --parts=$s --coldrounds=${COLDROUNDS:-10} $only ;;
    firstframe|oneshot) step $s $guard node harness/suite.mjs --parts=$s --rounds=$rounds $only ;;
    idle) step idle $guard node harness/suite.mjs --parts=idle --idle=${IDLE:-10} $only ;;
    keys) step keys $guard node harness/suite.mjs --parts=keys --keyrounds=${KEYROUNDS:-10} $only ;;
    tools) step tools $guard node harness/suite.mjs --parts=tools --steps=${STEPS:-10,50} --toolrounds=${TOOLROUNDS:-10} $only ;;
    fleet) step fleet $guard node harness/suite.mjs --parts=fleet --fleet=${FLEET:-1,8,32} --fleetrounds=${FLEETROUNDS:-10} $only ;;
    store) step store $guard node harness/suite.mjs --parts=store --sessions=${SESSIONS:-1000} --storerounds=${STOREROUNDS:-20} $only ;;
    disk) step disk node harness/suite.mjs --parts=disk $only ;;
    cli) step cli $guard node harness/sweep.mjs --mode=cli --rates=${RATES:-100,1000,10000} --samples=$samples $only ;;
    tui) step tui $guard node harness/sweep.mjs --mode=tui --rates=${RATES:-100,1000,10000} --samples=$samples $only ;;
    tui50k) step tui50k $guard node harness/sweep.mjs --mode=tui --rates=50000 --samples=$samples --tag=50k $only ;;
    tuisize) step tuisize $guard node harness/sweep.mjs --mode=tui --rates=10000 --samples=$samples --cols=240 --rows=70 --tag=240x70 $only ;;
    report) node harness/report.mjs --dir="$dir" > "$dir/REPORT.md"; note "report written" ;;
    *) note "unknown step $s" ;;
  esac
done
note "done"
