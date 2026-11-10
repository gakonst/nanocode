#!/bin/sh
# Run a command where only loopback is reachable: netguard.sh [--cgroup] cmd...
# Run the whole harness under it (netguard.sh node harness/suite.mjs ...), so the fixture on 127.0.0.1 shares the
# network namespace with the CLIs. Without root it uses an unprivileged user namespace, brings lo up as the mapped
# root, then maps back to the caller's uid in a nested user namespace so CLIs do not see uid 0. Where unprivileged
# user namespaces are blocked (Ubuntu's kernel.apparmor_restrict_unprivileged_userns=1) it uses sudo unshare --net
# and drops back to the caller with setpriv (NETGUARD_MODE=sudo forces this path). Needs util-linux unshare and iproute2 (apt-get install iproute2).
# --cgroup (needs sudo) creates /sys/fs/cgroup/cli-bench, moves this process into cli-bench/harness and exports
# RUNSTAT_CGROUP so tools/runstat puts each run in its own cgroup and reports cgroup_* fields.
# NETGUARD_SINK=1 also routes every address to lo (connects are refused locally instead of unreachable), puts the
# IPv6 nameservers on lo and lets port 53 be bound, so harness/netaudit.mjs can answer DNS itself and see each name.
set -e
if [ "$1" = "--cgroup" ]; then
  shift
  G=/sys/fs/cgroup/cli-bench
  if sudo -n sh -c '
    G=$1
    mkdir -p "$G/harness"
    for c in cpu pids memory; do echo +$c > /sys/fs/cgroup/cgroup.subtree_control 2>/dev/null || true; done
    for d in "$G" "$G/harness"; do [ "$(cat "$d/cgroup.type")" != "domain invalid" ] || echo threaded > "$d/cgroup.type"; done
    for c in cpu pids memory; do echo +$c > "$G/cgroup.subtree_control" 2>/dev/null || true; done
    chown -R "$2" "$G"
    echo "$3" > "$G/harness/cgroup.procs" 2>/dev/null || echo "$3" > "$G/harness/cgroup.threads"
  ' sh "$G" "$(id -u):$(id -g)" $$; then
    export RUNSTAT_CGROUP=$G
  else
    echo "netguard: --cgroup needs sudo and cgroup v2; continuing without" >&2
  fi
fi
[ $# -gt 0 ] || { echo "usage: netguard.sh [--cgroup] cmd..." >&2; exit 2; }
IP=$(command -v ip || echo /usr/sbin/ip)
U=$(id -u) G=$(id -g)
export NETGUARD=1
UP='"$0" link set lo up'
if [ "$NETGUARD_SINK" = 1 ]; then
  UP="$UP"' && "$0" route add local 0.0.0.0/0 dev lo && { "$0" -6 route add local ::/0 dev lo 2>/dev/null || true; } && echo 53 > /proc/sys/net/ipv4/ip_unprivileged_port_start'
  for ns in $(awk '/^nameserver/ && $2 ~ /:/ {print $2}' /etc/resolv.conf); do UP="$UP"' && "$0" -6 addr add '"$ns"'/128 dev lo nodad'; done
fi
if [ "$NETGUARD_MODE" != sudo ] && unshare --user --map-root-user --net "$IP" link set lo up 2>/dev/null; then
  exec unshare --user --map-root-user --net sh -c 'u=$1 g=$2 up=$3; shift 3; eval "$up" && exec unshare --user --map-user="$u" --map-group="$g" -- "$@"' "$IP" "$U" "$G" "$UP" "$@"
fi
echo "netguard: unprivileged user namespaces unavailable, using sudo unshare --net" >&2
exec sudo -n --preserve-env env PATH="$PATH" unshare --net sh -c 'u=$1 g=$2 up=$3; shift 3; eval "$up" && exec setpriv --reuid="$u" --regid="$g" --init-groups -- "$@"' "$IP" "$U" "$G" "$UP" "$@"
