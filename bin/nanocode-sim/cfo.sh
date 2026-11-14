#!/usr/bin/env bash
set -u
here=$(cd "$(dirname "$0")" && pwd)
if [ -f "$here/results.jsonl" ]; then
  root=${NANOCODE_CFO_ROOT:-$here}
else
  root=${NANOCODE_CFO_ROOT:-$HOME/nanocode-cfo}
fi
remote=${NANOCODE_CFO_REMOTE:-https://github.com/nanocodexyz/nanocode}
branch=${NANOCODE_CFO_BRANCH:-master}
budget_secs=${NANOCODE_CFO_BUDGET_SECS:-3600}
refresh_secs=${NANOCODE_CFO_REFRESH_SECS:-300}
job_secs_target=${NANOCODE_CFO_JOB_SECS:-60}
job_secs_max=${NANOCODE_CFO_TIMEOUT_SECS:-1800}
workers=${NANOCODE_CFO_WORKERS:-24}
fuzz_cpus=${NANOCODE_CFO_FUZZ_CPUS:-8-31}
build_cpus=${NANOCODE_CFO_BUILD_CPUS:-0-5}
build_jobs=${NANOCODE_CFO_BUILD_JOBS:-6}
canary_seeds=${NANOCODE_CFO_CANARY_SEEDS:-400}
memory_max=${NANOCODE_CFO_MEMORY_MAX:-32G}
batch_first=20
seeds_min=1
seeds_max=1000000
seed_mask=$(( (1 << 62) - 1 ))
results=$root/results.jsonl
logs=$root/failures
default_table='
16 store_content pass --scenario store
4 store_content_twice pass --scenario store --twice
16 store_slots pass --scenario store --records slots
4 store_slots_twice pass --scenario store --records slots --twice
16 agents pass --scenario agents
4 agents_twice pass --scenario agents --twice
8 turns_reference pass --scenario turns
8 turns_turnlog pass --scenario turns --policy turnlog
2 turns_turnlog_twice pass --scenario turns --policy turnlog --twice
2 turns_reference_calm pass --scenario turns --calm
2 turns_turnlog_row_limit pass --scenario turns --policy turnlog --row-limit 1024
8 runtime_memory pass --scenario runtime --store memory
8 runtime_kv pass --scenario runtime --store kv
2 runtime_memory_twice pass --scenario runtime --store memory --twice
2 runtime_kv_twice pass --scenario runtime --store kv --twice
2 runtime_kv_calm pass --scenario runtime --store kv --calm
8 runtime_kv_http pass --scenario runtime --store kv --wire http
4 runtime_memory_http pass --scenario runtime --store memory --wire http
2 runtime_kv_http_twice pass --scenario runtime --store kv --wire http --twice
8 websocket pass --scenario websocket
2 websocket_twice pass --scenario websocket --twice
8 http_server pass --scenario http_server
2 http_server_twice pass --scenario http_server --twice
1 canary_cas_ignores_expected canary --scenario store --mutation cas_ignores_expected
1 canary_record_insert_drops canary --scenario store --mutation record_insert_drops
1 canary_power_loss_drops_acked canary --scenario store --mutation power_loss_drops_acked
1 canary_record_insert_overwrites canary --scenario store --records slots --mutation record_insert_overwrites
1 canary_cache_keeps_blank canary --scenario store --mutation cache_keeps_blank
1 canary_staged_refuses_rewrite canary --scenario store --mutation staged_refuses_rewrite
1 canary_durable_seq_before_commit canary --scenario turns --mutation durable_seq_before_commit
1 canary_finish_before_row canary --scenario turns --mutation finish_before_row
1 canary_gate_releases_early canary --scenario turns --mutation gate_releases_early
1 canary_idempotent_reported_unknown canary --scenario turns --mutation idempotent_reported_unknown
1 canary_recovery_skips_clientless canary --scenario turns --mutation recovery_skips_clientless
1 canary_tick_not_atomic canary --scenario turns --mutation tick_not_atomic
1 canary_seal_skipped canary:4000 --scenario turns --live-seqs floor --mutation seal_skipped
1 canary_http_reuse_dirty canary:100 --scenario runtime --store kv --wire http --mutation http-reuse-dirty
1 canary_http_no_read_timeout canary:200 --scenario runtime --store kv --wire http --mutation http-no-read-timeout
1 canary_tool_result_twice canary:40 --scenario runtime --store kv --wire http --mutation tool-result-twice
1 canary_websocket_corrupt_payload canary --scenario websocket --mutation corrupt_payload
1 canary_websocket_drop_client_byte canary --scenario websocket --mutation drop_client_byte
1 canary_websocket_hold_eof canary --scenario websocket --mutation hold_eof
1 canary_http_server_corrupt_response_byte canary --scenario http_server --mutation corrupt_response_byte
1 canary_http_server_drop_request_byte canary --scenario http_server --mutation drop_request_byte
'
default_known='
'
discovered_weight=8
discovered_twice_weight=2

usage() {
  echo "usage: cfo.sh run | supervise | install | summary [HOURS] | failures [HOURS]" >&2
  exit 2
}

now() { date +%s; }

json_text() {
  printf '%s' "$1" | sed -e 's/\\/\\\\/g' -e 's/"/\\"/g' -e 's/\t/\\t/g' | tr -d '\r' | tr '\n' ' '
}

record() {
  printf '%s\n' "$1" >>"$results"
}

random_seed() {
  local raw
  raw=$(od -An -N8 -tu8 /dev/urandom | tr -d ' ')
  echo $(( raw & seed_mask ))
}

fetch_and_build() {
  mkdir -p "$root/bins"
  if [ ! -d "$root/nanocode/.git" ]; then
    git clone -q "$remote" "$root/nanocode" || return 1
  fi
  git -C "$root/nanocode" fetch -q origin "$branch" || return 1
  local sha
  sha=$(git -C "$root/nanocode" rev-parse FETCH_HEAD) || return 1
  if [ -x "$root/bins/$sha/nanocode-sim" ]; then
    echo "$sha"
    return 0
  fi
  git -C "$root/nanocode" checkout -q --detach "$sha" || return 1
  local started log
  started=$(now)
  log=$root/build.log
  if ! (cd "$root/nanocode" && CARGO_TARGET_DIR=$root/target CARGO_BUILD_JOBS=$build_jobs \
    taskset -c "$build_cpus" cargo build --locked --release -p nanocode-sim) >"$log" 2>&1; then
    record "{\"time\":$started,\"commit\":\"$sha\",\"scenario\":\"build\",\"ok\":false,\"wall_secs\":$(( $(now) - started )),\"message\":\"$(json_text "$(tail -20 "$log")")\"}"
    return 1
  fi
  mkdir -p "$root/bins/$sha"
  cp "$root/target/release/nanocode-sim" "$root/bins/$sha/nanocode-sim"
  record "{\"time\":$started,\"commit\":\"$sha\",\"scenario\":\"build\",\"ok\":true,\"wall_secs\":$(( $(now) - started ))}"
  echo "$sha"
}

load_table() {
  local bin=$1 table scenarios scenario
  if [ -f "$root/table" ]; then
    table=$(cat "$root/table")
  else
    table=$default_table
  fi
  scenarios=$("$bin" 2>&1 | grep -o -- '--scenario [a-z_|]*' | head -1 | sed 's/--scenario //')
  for scenario in $(echo "$scenarios" | tr '|' ' '); do
    if ! echo "$table" | grep -q -- "--scenario $scenario\( \|$\)"; then
      table="$table
$discovered_weight $scenario pass --scenario $scenario
$discovered_twice_weight ${scenario}_twice pass --scenario $scenario --twice"
    fi
  done
  names=() weights=() kinds=() flags=()
  while read -r weight name kind rest; do
    [ -z "$weight" ] && continue
    case $weight in \#*) continue ;; esac
    scenario=$(echo "$rest" | sed -n 's/.*--scenario \([a-z_]*\).*/\1/p')
    if [ -n "$scenario" ] && ! echo "|$scenarios|" | grep -q "|$scenario|"; then
      continue
    fi
    names+=("$name")
    weights+=("$weight")
    kinds+=("$kind")
    flags+=("$rest")
  done <<<"$table"
}

pick() {
  local best=-1 best_cost=0 at cost
  for at in "${!names[@]}"; do
    cost=$(( (spent[at] * 1000) / weights[at] ))
    if [ "$best" -lt 0 ] || [ "$cost" -lt "$best_cost" ]; then
      best=$at
      best_cost=$cost
    fi
  done
  echo "$best"
}

known_failure() {
  local name=$1 line=$2 known prefix pattern
  if [ -f "$root/known" ]; then
    known=$(cat "$root/known")
  else
    known=$default_known
  fi
  while read -r prefix pattern; do
    [ -z "$prefix" ] && continue
    case $prefix in \#*) continue ;; esac
    case $name in "$prefix"*) ;; *) continue ;; esac
    case $line in *"$pattern"*) return 0 ;; esac
  done <<<"$known"
  return 1
}

run_job() {
  local at=$1 sha=$2 bin=$3 seed=$4 count=$5
  local name=${names[$at]} kind=${kinds[$at]} flags=${flags[$at]}
  local end=$(( seed + count )) out started status wall ok passed failed message
  out=$(mktemp "$root/tmp/job.XXXXXX")
  started=$(now)
  timeout --signal=KILL "$job_secs_max" taskset -c "$fuzz_cpus" "$bin" --seeds "$seed..$end" $flags >"$out" 2>&1
  status=$?
  wall=$(( $(now) - started ))
  passed=$(grep '^RESULT' "$out" | sed -n 's/.* passed=\([0-9]*\).*/\1/p' | tail -1)
  failed=$(grep '^RESULT' "$out" | sed -n 's/.* failed=\([0-9]*\).*/\1/p' | tail -1)
  passed=${passed:-0}
  failed=${failed:-0}
  ok=false
  message=""
  case $kind in
    pass) [ "$status" -eq 0 ] && ok=true ;;
    canary*) [ "$status" -eq 0 ] && grep -q '^MUTATION .* caught by' "$out" && ok=true ;;
  esac
  if [ "$status" -eq 137 ] && ! grep -q '^RESULT' "$out"; then
    message="killed: timeout after ${job_secs_max}s or out of memory"
  elif [ "$ok" = false ]; then
    message=$(grep -m1 '^FAIL\|^MUTATION\|panicked\|^error' "$out")
    [ -z "$message" ] && message="exit $status: $(tail -3 "$out")"
  fi
  local replay="$bin --seeds $seed..$end $flags" known=false line
  if [ "$ok" = false ] && [ "$kind" = pass ] && [ "$failed" -gt 0 ] && grep -q '^FAIL' "$out"; then
    known=true
    while read -r line; do
      known_failure "$name" "$line" || { known=false; break; }
    done < <(grep '^FAIL' "$out")
  fi
  record "{\"time\":$started,\"commit\":\"$sha\",\"scenario\":\"$name\",\"kind\":\"$kind\",\"seed\":$seed,\"count\":$count,\"ok\":$ok,\"known\":$known,\"exit\":$status,\"passed\":$passed,\"failed\":$failed,\"wall_secs\":$wall,\"replay\":\"$(json_text "$replay")\",\"message\":\"$(json_text "$message")\"}"
  if [ "$kind" = pass ]; then
    grep '^FAIL' "$out" | head -32 | while read -r line; do
      local failing command seen=false
      known_failure "$name" "$line" && seen=true
      failing=$(echo "$line" | sed -n 's/^FAIL seed=\([0-9]*\).*/\1/p')
      command=$(echo "$line" | sed -n 's/.*:: replay: nanocode-sim//p')
      record "{\"time\":$started,\"commit\":\"$sha\",\"scenario\":\"$name\",\"kind\":\"seed\",\"seed\":${failing:-0},\"count\":1,\"ok\":false,\"known\":$seen,\"replay\":\"$(json_text "$bin$command")\",\"message\":\"$(json_text "$line")\"}"
    done
  fi
  if [ "$ok" = false ]; then
    mkdir -p "$logs"
    cp "$out" "$logs/$started-$name-$seed.log"
  fi
  rm -f "$out"
  echo "$wall"
}

refill() {
  local at
  for at in "${!names[@]}"; do
    spent[at]=${spent[at]:-0}
    batch[at]=$(cat "$root/state/${names[$at]}.batch" 2>/dev/null || echo "$batch_first")
  done
}

run() {
  mkdir -p "$root/tmp" "$root/state"
  local started sha bin last_refresh deadline building="" at
  started=$(now)
  deadline=$(( started + budget_secs ))
  sha=$(fetch_and_build) || sha=$(ls -t "$root/bins" 2>/dev/null | head -1)
  [ -z "$sha" ] && { echo "no nanocode-sim binary built" >&2; return 1; }
  bin=$root/bins/$sha/nanocode-sim
  spent=() batch=()
  load_table "$bin"
  refill
  declare -A running=()
  last_refresh=$started
  while :; do
    local t
    t=$(now)
    if [ -z "$building" ] && [ $(( t - last_refresh )) -ge "$refresh_secs" ] && [ "$t" -lt "$deadline" ]; then
      last_refresh=$t
      ( fetch_and_build >"$root/tmp/build.sha" ) &
      building=$!
    fi
    while [ "${#running[@]}" -lt "$workers" ] && [ "$(now)" -lt "$deadline" ]; do
      at=$(pick)
      local count=${batch[$at]}
      case ${kinds[$at]} in
        canary) count=$canary_seeds ;;
        canary:*) count=${kinds[$at]#canary:} ;;
      esac
      local seed
      seed=$(random_seed)
      spent[at]=$(( spent[at] + job_secs_target ))
      ( wall=$(run_job "$at" "$sha" "$bin" "$seed" "$count"); echo "$at $count $wall" >"$root/tmp/done.$BASHPID" ) &
      running[$!]=$at
    done
    [ "${#running[@]}" -eq 0 ] && [ -z "$building" ] && break
    local finished=""
    wait -n -p finished
    if [ -n "$building" ] && [ "$finished" = "$building" ]; then
      local next
      next=$(cat "$root/tmp/build.sha" 2>/dev/null)
      if [ -n "$next" ] && [ "$next" != "$sha" ] && [ -x "$root/bins/$next/nanocode-sim" ]; then
        sha=$next
        bin=$root/bins/$sha/nanocode-sim
        load_table "$bin"
        refill
      fi
      building=""
      continue
    fi
    if [ -n "$finished" ] && [ -n "${running[$finished]+x}" ]; then
      unset "running[$finished]"
      local done_file=$root/tmp/done.$finished
      if [ -f "$done_file" ]; then
        local job_at job_count job_wall scaled
        read -r job_at job_count job_wall <"$done_file"
        rm -f "$done_file"
        spent[job_at]=$(( spent[job_at] - job_secs_target + job_wall ))
        scaled=$(( job_count * job_secs_target / (job_wall > 0 ? job_wall : 1) ))
        [ "$scaled" -gt $(( job_count * 2 )) ] && scaled=$(( job_count * 2 ))
        [ "$scaled" -lt "$seeds_min" ] && scaled=$seeds_min
        [ "$scaled" -gt "$seeds_max" ] && scaled=$seeds_max
        batch[job_at]=$scaled
        echo "$scaled" >"$root/state/${names[$job_at]}.batch"
      fi
    fi
  done
}

supervise() {
  mkdir -p "$root"
  trap 'kill 0' EXIT INT TERM
  while :; do
    if [ -f "$root/nanocode/bin/nanocode-sim/cfo.sh" ]; then
      cp "$root/nanocode/bin/nanocode-sim/cfo.sh" "$root/cfo.run.sh"
    else
      cp "$here/cfo.sh" "$root/cfo.run.sh"
    fi
    bash "$root/cfo.run.sh" run || sleep 10
  done
}

install() {
  mkdir -p "$HOME/.config/systemd/user" "$root"
  cp "$here/cfo.sh" "$root/cfo.sh"
  cat >"$HOME/.config/systemd/user/nanocode-cfo.service" <<EOF
[Unit]
Description=nanocode-sim continuous fuzzing of nanocode $branch

[Service]
Environment=NANOCODE_CFO_ROOT=$root
Environment=PATH=$HOME/.cargo/bin:/usr/local/bin:/usr/bin:/bin
ExecStart=/bin/bash $root/cfo.sh supervise
Restart=always
RestartSec=10
MemoryMax=$memory_max
KillMode=control-group
Nice=10

[Install]
WantedBy=default.target
EOF
  systemctl --user daemon-reload
  systemctl --user enable --now nanocode-cfo.service
}

field_awk='
function field(name,   pattern, value) {
  pattern = "\"" name "\":(\"([^\"\\\\]|\\\\.)*\"|[^,}]*)"
  if (!match($0, pattern)) return ""
  value = substr($0, RSTART + length(name) + 3, RLENGTH - length(name) - 3)
  gsub(/^"|"$/, "", value)
  return value
}'

summary() {
  local since=$(( $(now) - ${1:-24} * 3600 ))
  awk -v since="$since" "$field_awk"'
    {
      time = field("time") + 0
      if (time < since) next
      kind = field("kind")
      scenario = field("scenario")
      if (kind == "seed" || scenario == "build") next
      runs[scenario]++
      seeds[scenario] += field("count")
      if (field("ok") != "true" && field("known") == "true") known[scenario]++
      else if (field("ok") != "true") bad[scenario]++
      total += field("count")
      if (time < first || first == 0) first = time
      if (time > last) last = time
      commits[field("commit")] = 1
    }
    END {
      span = last - first
      if (span <= 0) span = 1
      printf "%-36s %8s %12s %8s %8s %10s\n", "scenario", "runs", "seeds", "failed", "known", "seeds/h"
      for (s in runs) printf "%-36s %8d %12d %8d %8d %10d\n", s, runs[s], seeds[s], bad[s], known[s], seeds[s] * 3600 / span
      n = 0
      for (c in commits) n++
      printf "total %d seeds over %.2f h, %d per hour, %d commits\n", total, span / 3600, total * 3600 / span, n
    }' "$results"
}

failures() {
  local since=$(( $(now) - ${1:-24} * 3600 ))
  awk -v since="$since" "$field_awk"'
    {
      if (field("time") + 0 < since || field("ok") == "true" || field("scenario") == "build") next
      if (field("kind") == "pass" && field("failed") + 0 > 0) next
      group = field("message")
      sub(/ :: replay:.*/, "", group)
      gsub(/[0-9]+/, "N", group)
      if (field("known") == "true") group = "[known] " group
      key = field("scenario") " | " group
      count[key]++
      if (!(key in example)) example[key] = field("commit") " " field("replay")
    }
    END {
      for (key in count) printf "%6d  %s\n        %s\n", count[key], key, example[key]
    }' "$results"
}

case ${1:-} in
  run) run ;;
  supervise) supervise ;;
  install) install ;;
  summary) summary "${2:-24}" ;;
  failures) failures "${2:-24}" ;;
  *) usage ;;
esac
