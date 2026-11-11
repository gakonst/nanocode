#!/usr/bin/env python3
import bisect
import os
import pty
import re
import select
import subprocess
import sys
import tempfile

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
BINARY = os.path.join(ROOT, "target", "lite", "nc-lite")
READY = re.compile(rb"/help.*\xe2\x80\xba", re.S)
ANSWERED = re.compile(rb"\r\n.*\xe2\x80\xba", re.S)
END_OF_INPUT = b"\x04"


def traced(log, args):
    return ["valgrind", "--tool=lackey", "--trace-superblocks=yes", "--log-file=" + log, BINARY, *args]


def read_until(fd, pattern):
    seen = b""
    while not pattern.search(seen):
        select.select([fd], [], [])
        seen += os.read(fd, 4096)


def interactive(command, env, prompt):
    pid, fd = pty.fork()
    if pid == 0:
        os.execvpe(command[0], command, env)
    read_until(fd, READY)
    os.write(fd, prompt.encode() + b"\r")
    read_until(fd, ANSWERED)
    os.write(fd, END_OF_INPUT)
    _, status = os.waitpid(pid, 0)
    if status:
        sys.exit("the terminal run exited with status %d" % status)


def symbols(map_path):
    found = {}
    with open(map_path) as lines:
        for line in lines:
            fields = line.split()
            if len(fields) == 5 and ":(" not in fields[4] and not fields[4].startswith("."):
                found.setdefault(int(fields[0], 16), []).append(fields[4])
    starts = sorted(found)
    return starts, [found[start] for start in starts]


def build(**link):
    env = {name: value for name, value in os.environ.items() if not name.startswith("NC_LITE_")}
    env.update(link)
    subprocess.run(["cargo", "clean", "--profile", "lite", "-p", "nc-lite"], cwd=ROOT, env=env, check=True)
    subprocess.run(["cargo", "build", "--locked", "--profile", "lite", "-p", "nc-lite"], cwd=ROOT, env=env, check=True)


def main():
    if len(sys.argv) != 3:
        sys.exit("usage: nc-lite-symbol-order.py BASE_URL PROMPT")
    base, prompt = sys.argv[1:]
    with tempfile.TemporaryDirectory() as work:
        map_path = os.path.join(work, "map")
        build(NC_LITE_LINK_MAP=map_path)
        home = os.path.join(work, "home")
        os.mkdir(home)
        env = dict(os.environ, HOME=home)
        env.setdefault("AI_GATEWAY_API_KEY", "symbol-order")
        flags = ["--ephemeral", "--home", os.path.join(home, ".nc-lite"), "--base-url", base]
        logs = [os.path.join(work, "terminal"), os.path.join(work, "exec")]
        interactive(traced(logs[0], flags), env, prompt)
        subprocess.run(traced(logs[1], ["exec", *flags, prompt]), env=env, check=True, stdout=subprocess.DEVNULL)
        starts, names = symbols(map_path)
        ordered = {}
        for log in logs:
            with open(log) as lines:
                for line in lines:
                    if line.startswith("SB "):
                        index = bisect.bisect_right(starts, int(line.split()[1], 16)) - 1
                        if index >= 0:
                            ordered.update(dict.fromkeys(names[index]))
        order = os.path.join(work, "order")
        with open(order, "w") as out:
            out.writelines(name + "\n" for name in ordered)
        build(NC_LITE_SYMBOL_ORDER=order)
    print("%s is linked with %d symbols in first-run order" % (BINARY, len(ordered)))


main()
