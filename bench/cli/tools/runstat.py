#!/usr/bin/env python3
# Portable stand-in for tools/runstat (macOS only): runs a command and writes its resource usage as JSON.
# It reports what wait4 gives on every Unix. Peak footprint and instruction counts need tools/runstat.c on macOS
# or perf on Linux, and are null here.
import json, os, sys, time

out, cmd = sys.argv[1], sys.argv[2:]
start = time.monotonic_ns()
pid = os.fork()
if pid == 0:
    try:
        os.execvp(cmd[0], cmd)
    except OSError as error:
        sys.stderr.write("spawn %s: %s\n" % (cmd[0], error))
        os._exit(127)
_, status, usage = os.wait4(pid, 0)
end = time.monotonic_ns()
scale = 1 if sys.platform == "darwin" else 1024
with open(out, "w") as f:
    json.dump({
        "start_ns": start, "end_ns": end, "pid": pid,
        "exit": os.WEXITSTATUS(status) if os.WIFEXITED(status) else -1,
        "signal": os.WTERMSIG(status) if os.WIFSIGNALED(status) else 0,
        "user_us": int(usage.ru_utime * 1e6), "sys_us": int(usage.ru_stime * 1e6),
        "maxrss_bytes": usage.ru_maxrss * scale, "minflt": usage.ru_minflt, "majflt": usage.ru_majflt,
        "nvcsw": usage.ru_nvcsw, "nivcsw": usage.ru_nivcsw,
        "lifetime_max_footprint": None, "instructions": None,
    }, f)
sys.exit(128 + os.WTERMSIG(status) if os.WIFSIGNALED(status) else os.WEXITSTATUS(status))
