#!/usr/bin/env python3
# Fallback for tools/runstat (runstat.c on macOS, runstat-linux.c on Linux): runs a command and writes its resource
# usage as JSON. On Linux it also becomes a child subreaper and samples /proc (RUNSTAT_INTERVAL_US, default 5000) for
# the tree's summed RSS and process count, then kills helpers left when the command exits, like runstat-linux.c.
# Instruction counts and cgroup fields are null here.
import ctypes, json, os, resource, signal, sys, threading, time

out, cmd = sys.argv[1], sys.argv[2:]
linux = sys.platform.startswith("linux")
me = os.getpid()
page = os.sysconf("SC_PAGE_SIZE")
tick = os.sysconf("SC_CLK_TCK")
interval = int(os.environ.get("RUNSTAT_INTERVAL_US", "5000")) / 1e6
if linux:
    ctypes.CDLL(None, use_errno=True).prctl(36, 1, 0, 0, 0)  # PR_SET_CHILD_SUBREAPER

def stat(pid):
    try:
        with open("/proc/%d/stat" % pid) as f: s = f.read()
    except OSError:
        return None
    f = s[s.rindex(")") + 2:].split()
    return f[0], int(f[1]), int(f[11]) + int(f[12]), int(f[19])

cls = {}
def tree():
    found = {}
    for name in os.listdir("/proc"):
        if not name.isdigit(): continue
        pid = int(name)
        if pid == me: continue
        st = stat(pid)
        if not st: continue
        chain, p = [pid], st[1]
        while p not in cls and p > 1 and p != me and len(chain) < 64:
            ps = stat(p)
            if not ps: break
            chain.append(p); p = ps[1]
        c = 2 if p == me or cls.get(p) == 2 else 1
        for q in chain: cls.setdefault(q, c)
        if cls[pid] == 2 and st[0] != "Z": found[pid] = st
    return found

seen = {}
peak = {"rss": 0, "procs": 0, "helpers": 0, "samples": 0}
lock = threading.Lock()
def rss(pid):
    try:
        with open("/proc/%d/statm" % pid) as f: return int(f.read().split()[1]) * page
    except OSError:
        return 0

def sample(main):
    t = tree()
    total = sum(rss(p) for p in t)
    with lock:
        for p, st in t.items(): seen[(p, st[3])] = st[2]
        peak["rss"] = max(peak["rss"], total)
        peak["procs"] = max(peak["procs"], len(t))
        peak["helpers"] = max(peak["helpers"], len([p for p in t if p != main]))
        peak["samples"] += 1

start = time.monotonic_ns()
pid = os.fork()
if pid == 0:
    try:
        os.setpgid(0, 0)
        os.execvp(cmd[0], cmd)
    except OSError as error:
        sys.stderr.write("spawn %s: %s\n" % (cmd[0], error))
        os._exit(127)
try: os.setpgid(pid, pid)
except OSError: pass
stop = threading.Event()
def sampler():
    while not stop.is_set():
        sample(pid); stop.wait(interval)
th = threading.Thread(target=sampler, daemon=True)
if linux and interval > 0: th.start()
_, status, usage = os.wait4(pid, 0)
end = time.monotonic_ns()
stop.set()
left = killed = sigkilled = 0
if linux:
    if th.is_alive(): th.join()
    sample(pid)
    rest = tree()
    left = len(rest)
    for p in rest:
        try: os.kill(p, signal.SIGTERM); killed += 1
        except OSError: pass
    until = time.monotonic() + 0.5
    while rest and time.monotonic() < until:
        try:
            while os.waitpid(-1, os.WNOHANG)[0] > 0: pass
        except ChildProcessError: pass
        time.sleep(0.005); rest = tree()
    for p in rest:
        try: os.kill(p, signal.SIGKILL); sigkilled += 1
        except OSError: pass
    until = time.monotonic() + 2
    while time.monotonic() < until:
        try:
            if os.waitpid(-1, os.WNOHANG)[0] > 0: continue
        except ChildProcessError: break
        if not tree(): break
        time.sleep(0.002)
ch = resource.getrusage(resource.RUSAGE_CHILDREN)
scale = 1 if sys.platform == "darwin" else 1024
row = {
    "start_ns": start, "end_ns": end, "pid": pid,
    "exit": os.WEXITSTATUS(status) if os.WIFEXITED(status) else -1,
    "signal": os.WTERMSIG(status) if os.WIFSIGNALED(status) else 0,
    "user_us": int(usage.ru_utime * 1e6), "sys_us": int(usage.ru_stime * 1e6),
    "maxrss_bytes": usage.ru_maxrss * scale, "minflt": usage.ru_minflt, "majflt": usage.ru_majflt,
    "nvcsw": usage.ru_nvcsw, "nivcsw": usage.ru_nivcsw,
    "lifetime_max_footprint": None, "instructions": None, "cgroup": None,
}
if linux:
    row.update({
        "tree_cpu_us": int((ch.ru_utime + ch.ru_stime) * 1e6), "tree_user_us": int(ch.ru_utime * 1e6), "tree_sys_us": int(ch.ru_stime * 1e6),
        "tree_cpu_sampled_us": sum(seen.values()) * 1000000 // tick if peak["samples"] else None,
        "tree_max_rss_bytes": peak["rss"] if peak["samples"] else None,
        "tree_procs_max": peak["procs"], "helpers_max": peak["helpers"], "helpers_seen": max(0, len(seen) - 1),
        "helpers_left": left, "helpers_killed": killed, "helpers_sigkilled": sigkilled,
        "samples": peak["samples"], "interval_us": int(interval * 1e6),
        "sources": {"user_us": "rusage", "maxrss_bytes": "rusage", "tree_cpu_us": "rusage-children-subreaper", "tree_cpu_sampled_us": "proc-sampler", "tree_max_rss_bytes": "proc-sampler-statm", "helpers": "proc-sampler", "instructions": None, "cgroup": None},
    })
with open(out, "w") as f:
    json.dump(row, f)
sys.exit(128 + os.WTERMSIG(status) if os.WIFSIGNALED(status) else os.WEXITSTATUS(status))
