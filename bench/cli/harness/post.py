#!/usr/bin/env python3
"""Renders the benchmark post from results directories; every number in it comes from their JSONL files.

python3 harness/post.py [--out PATH] DIR [DIR ...]

Each row takes all of its lanes from the first DIR that has nc data for it, and names that DIR's
machine and nanocode commit. PATH defaults to DIR/POST.md for the first DIR. harness/run.sh runs it
as its post step, so a full rerun on one machine renders the whole post from that run alone.
"""
import json, os, re, sys

LANES = [("nc", "nc"), ("fx", "fx"), ("nclite", "nc-lite"), ("claude", "Claude Code"), ("codex", "Codex"), ("pi", "Pi"), ("opencode", "OpenCode")]
RIVALS = ["fx", "nclite", "claude", "codex"]
CONTEXT_KEYS = {"stdout_kb"}
NAMES = dict(LANES)
NOISY_LOAD = 4.0
SCRIPT_MB = 1.0


def read_jsonl(path):
    with open(path) as f:
        return [json.loads(line) for line in f if line.strip()]


def load(d):
    rows = {"suite": [], "tui": []}
    for name in sorted(os.listdir(d)):
        if name.startswith("suite") and name.endswith(".jsonl"):
            rows["suite"] += read_jsonl(os.path.join(d, name))
        elif name.startswith("sweep-tui") and name.endswith(".jsonl"):
            rows["tui"] += read_jsonl(os.path.join(d, name))
    machine = json.load(open(os.path.join(d, "machine.json"))) if os.path.exists(os.path.join(d, "machine.json")) else {}
    commit = None
    for name in ("ncrev.txt", "ncver.txt"):
        p = os.path.join(d, name)
        if os.path.exists(p):
            m = re.search(r"\b([0-9a-f]{9})[0-9a-f]*\b", open(p).read())
            if m:
                commit = m.group(1)
                break
    versions = {}
    p = os.path.join(d, "VERSIONS.md")
    if os.path.exists(p):
        for line in open(p):
            cells = [c.strip() for c in line.strip().strip("|").split("|")]
            if len(cells) >= 6 and cells[0] in NAMES:
                versions[cells[0]] = re.sub(r"^(nc-lite|nanocode|codex-cli) | \(Claude Code\)$", "", cells[1])
    loads = [r["load"] for r in rows["suite"] + rows["tui"] if isinstance(r.get("load"), (int, float)) and r.get("part") != "fleet"]
    ats = sorted(r["at"] for r in rows["suite"] + rows["tui"] if r.get("at"))
    return {"dir": d, "rows": rows, "machine": machine, "commit": commit, "versions": versions, "loads": loads, "ats": ats}


def good(r):
    if r.get("error"):
        return False
    if r.get("part") == "fleet":
        return r.get("ok") == r.get("k")
    return r.get("code") in (None, 0)


def median(v):
    v = sorted(x for x in v if isinstance(x, (int, float)))
    if not v:
        return None
    m = len(v) // 2
    return v[m] if len(v) % 2 else (v[m - 1] + v[m]) / 2


def show(x):
    if x is None:
        return "n/a"
    a = abs(x)
    if a >= 1000:
        return f"{x:,.0f}"
    if a >= 100:
        return f"{x:.0f}"
    if a >= 10:
        return f"{x:.1f}"
    if a >= 1:
        return f"{x:.2f}"
    return f"{x:.3f}"


def suite(part, **match):
    return ("suite", lambda r: r.get("part") == part and all(r.get(k, d) == v for k, (v, d) in match.items()))


def tui(rate, cols=120, rows=40):
    return ("tui", lambda r: r.get("rate") == rate and r.get("cols", 120) == cols and r.get("rows", 40) == rows)


def single(part):
    return ("suite", lambda r: r.get("part") == part)


# (group, label, selector, key, samples shown); lower is better in every row.
ROWS = [
    ("Starting up", "First frame, ms", suite("firstframe"), "first_frame_ms", True),
    ("Starting up", "CPU to the first frame, ms", suite("firstframe"), "tree_cpu_ms", True),
    ("Starting up", "Memory 1 s after the first frame, RSS MB", suite("firstframe"), "mem_rss_mb", True),
    ("Starting up", "--version, ms", suite("startup"), "wall_ms", True),
    ("Starting up", "--version, CPU ms", suite("startup"), "tree_cpu_ms", True),
    ("Starting up", "--version, cold page cache, ms", suite("coldstart"), "wall_ms", True),
    ("Starting up", "First frame, cold page cache, ms", suite("coldframe"), "first_frame_ms", True),
    ("Using it", "Keystroke echo p50, ms", suite("keys"), "key_p50_ms", True),
    ("Using it", "Keystroke echo, slowest of 26, ms", suite("keys"), "key_max_ms", True),
    ("Using it", "Idle CPU, ms per s", suite("idle"), "idle_cpu_ms_per_second", True),
    ("Headless", "One-word reply, ms", suite("oneshot"), "wall_ms", True),
    ("Headless", "CPU for that reply, ms", suite("oneshot"), "tree_cpu_ms", True),
    ("Headless", "50 shell steps in one turn, ms", suite("tools", steps=(50, None)), "wall_ms", True),
    ("Headless", "Tool result to next request p50, ms", suite("tools", steps=(50, None)), "step_gap_p50_ms", True),
    ("Headless", "32 runs at once, peak RSS MB each", suite("fleet", k=(32, None)), "rss_mb_per_agent", True),
    ("Headless", "32 runs at once, CPU ms each", suite("fleet", k=(32, None)), "cpu_ms_per_agent", True),
    ("Streaming", "Token on screen at 100 tok/s, p50 ms", tui(100), "lag_p50_ms", True),
    ("Streaming", "Token on screen at 1,000 tok/s, p50 ms", tui(1000), "lag_p50_ms", True),
    ("Streaming", "Token on screen at 1,000 tok/s, p99 ms", tui(1000), "lag_p99_ms", True),
    ("Streaming", "240x70 at 10,000 tok/s, p50 ms", tui(10000, 240, 70), "lag_p50_ms", True),
    ("Streaming", "240x70 at 10,000 tok/s, p99 ms", tui(10000, 240, 70), "lag_p99_ms", True),
    ("Streaming", "240x70 at 10,000 tok/s, CPU ms", tui(10000, 240, 70), "tree_cpu_ms", True),
    ("Streaming", "240x70 at 10,000 tok/s, peak RSS MB", tui(10000, 240, 70), "tree_max_rss_mb", True),
    ("Streaming", "50,000 tok/s, p50 ms", tui(50000), "lag_p50_ms", True),
    ("Streaming", "50,000 tok/s, p99 ms", tui(50000), "lag_p99_ms", True),
    ("Streaming", "50,000 tok/s, CPU ms", tui(50000), "tree_cpu_ms", True),
    ("Streaming", "50,000 tok/s, peak RSS MB", tui(50000), "tree_max_rss_mb", True),
    ("Sessions", "1,000 sessions stored, KB per session", suite("store", sessions=(1000, 1000)), "store_kb_per_session", False),
    ("Sessions", "CPU per saved session, ms", suite("store", sessions=(1000, 1000)), "create_cpu_ms_per_session", False),
    ("Sessions", "Listing 1,000 sessions, ms", suite("store-list", sessions=(1000, 1000)), "wall_ms", True),
    ("Sessions", "Continuing the latest of 1,000, ms", suite("store-resume", sessions=(1000, 1000)), "wall_ms", True),
    ("Sessions", "Listing 14,600 sessions, ms", suite("store-list", sessions=(14600, 1000)), "wall_ms", True),
    ("Sessions", "Listing 14,600 sessions, task-clock ms", suite("store-list", sessions=(14600, 1000)), "task_clock_ms", True),
    ("Sessions", "Listing 14,600 sessions, instructions (user+kernel), M", suite("store-list", sessions=(14600, 1000)), "instructions_m", True),
    ("Sessions", "Listing 14,600 sessions, KB printed (each CLI's default listing)", suite("store-list", sessions=(14600, 1000)), "stdout_kb", True),
    ("Sessions", "Continuing the latest of 14,600, ms", suite("store-resume", sessions=(14600, 1000)), "wall_ms", True),
    ("Sessions", "Continuing the latest of 14,600, task-clock ms", suite("store-resume", sessions=(14600, 1000)), "task_clock_ms", True),
    ("Sessions", "Continuing the latest of 14,600, instructions (user+kernel), M", suite("store-resume", sessions=(14600, 1000)), "instructions_m", True),
    ("Size", "Install, MB", single("disk"), "install_mb", False),
    ("Size", "Main binary, MB", single("disk"), "binary_mb", False),
]


def cell(key, v, n, counted):
    if key == "binary_mb" and v < SCRIPT_MB:
        return "Node script"
    return show(v) + (f" ({n})" if counted else "")


def value(r, key):
    if key == "stdout_kb":
        return r["stdout_bytes"] / 1024 if isinstance(r.get("stdout_bytes"), (int, float)) else None
    if key == "instructions_m":
        u, k = r.get("instructions_user"), r.get("instructions_kernel")
        return (u + k) / 1e6 if isinstance(u, (int, float)) and isinstance(k, (int, float)) else None
    return r.get(key)


def measure(src, sel, key):
    kind, pred = sel
    out = {}
    for lane, _ in LANES:
        vals = [value(r, key) for r in src["rows"][kind] if r.get("harness") == lane and pred(r) and good(r)]
        vals = [v for v in vals if isinstance(v, (int, float))]
        if vals:
            out[lane] = (median(vals), len(vals))
    return out


def main(argv):
    out = None
    if argv[:1] == ["--out"]:
        out, argv = argv[1], argv[2:]
    if not argv:
        sys.exit("usage: python3 harness/post.py [--out PATH] DIR [DIR ...]")
    dirs = [os.path.abspath(d) for d in argv]
    out = out or os.path.join(dirs[0], "POST.md")
    root = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
    command = "python3 harness/post.py " + " ".join(["--out", os.path.relpath(os.path.abspath(out), root)] + [os.path.relpath(d, root) for d in dirs])
    srcs = [load(d) for d in dirs if os.path.isdir(d)]
    letters = {id(s): chr(ord("A") + i) for i, s in enumerate(srcs)}
    key_of = lambda label: next(k for _, l, _, k, _ in ROWS if l == label)
    table = []
    for group, label, sel, key, counted in ROWS:
        for s in srcs:
            m = measure(s, sel, key)
            if "nc" in m:
                table.append((group, label, s, m, counted))
                break
    L = []
    say = L.append
    a = srcs[0]
    when = a["ats"][0][:10] if a["ats"] else "undated"
    say(f"# Seven agent CLIs, measured, {when}\n")
    say(f"*Draft for nanocode.xyz/experiments. Not published. Generated from the results' JSONL by `{command}` (run from bench/cli); edit harness/post.py, not this file.*\n")
    say("Seven terminal coding agents ran the same scripted work: nc (nanocode), nc-lite (the same SDK cut down to what fx ships), Vercel fx, Pi, OpenCode, Claude Code and Codex CLI. No lane talks to a real model: each points at a local fixture that streams a fixed reply, runs with its own empty HOME, inside a network namespace where only loopback works. The lanes take turns in every round. Every number is a median with the sample count in parentheses; CPU and memory cover the CLI's whole process tree. Lower is better in every row except the KB-printed context row.\n")
    say("## Where each number comes from\n")
    say("Each row below names its source. A row takes all of its lanes from one source, so numbers in a row are always from the same machine, run and nanocode commit.\n")
    say("| Source | Machine | When (UTC) | Load average (fleet rows aside, which load the box themselves) | nanocode commit | Data |")
    say("|---|---|---|---|---|---|")
    for s in srcs:
        mc = s["machine"]
        hw = ", ".join(x for x in [mc.get("cpu"), f"{mc['cores']} threads" if mc.get("cores") else None, f"{mc['memory_gb']} GB" if mc.get("memory_gb") else None, mc.get("os"), "kernel " + mc["kernel"] if mc.get("kernel") else None, "governor " + mc["governor"] if mc.get("governor") else None, mc.get("host")] if x)
        span = f"{s['ats'][0][:16].replace('T', ' ')} to {s['ats'][-1][11:16]}" if s["ats"] else "n/a"
        ld = f"{min(s['loads']):.1f} to {max(s['loads']):.1f}" if s["loads"] else "n/a"
        noisy = "; shared and loaded, so compare lanes within a row, not machines" if s["loads"] and median(s["loads"]) > NOISY_LOAD else ""
        rel = os.path.relpath(s["dir"], os.path.dirname(os.path.abspath(out)))
        say(f"| {letters[id(s)]} | {hw}{noisy} | {span} | {ld} | {s['commit'] or 'not recorded'} | [{os.path.basename(s['dir'])}]({rel}/) |")
    say("")
    vers = {}
    for s in srcs:
        for lane, v in s["versions"].items():
            vers.setdefault(lane, []).append((letters[id(s)], v))
    if vers:
        say("Versions, from each source's VERSIONS.md: " + "; ".join(NAMES[l] + " " + ", ".join(sorted({v for _, v in vs})) for l, vs in sorted(vers.items(), key=lambda kv: [x for x, _ in LANES].index(kv[0]))) + ". nc is built as releases are (scripts/build-cli.sh: dist profile, non-PIE, BOLT); the source table gives its commit.\n")
    head = "| | " + " | ".join(n for _, n in LANES) + " | Src |"
    rule = "|---|" + "---:|" * len(LANES) + "---|"
    for group in dict.fromkeys(g for g, *_ in table):
        say(f"## {group}\n")
        say(head)
        say(rule)
        for g, label, s, m, counted in table:
            if g != group:
                continue
            cells = [cell(key_of(label), m[l][0], m[l][1], counted) if l in m else "n/a" for l, _ in LANES]
            say(f"| {label} | " + " | ".join(cells) + f" | {letters[id(s)]} |")
        say("")
    say("## fx's own claims (unverified)\n")
    say("Vercel publishes these for fx. We have not reproduced them; the right-hand columns are our nearest measurement of the same thing, from the rows above.\n")
    find = lambda label: next(((s, m) for _, l, s, m, _ in table if l == label), (None, {}))
    def ours(label, lane):
        s, m = find(label)
        return f"{show(m[lane][0])} ({letters[id(s)]})" if lane in m else "n/a"
    claims = [
        ("Startup before terminal setup (FX_BENCH=1)", "~0.5 ms", "--version, ms", "our --version is a whole process lifetime"),
        ("CI fails any command over", "2 ms", "--version, ms", ""),
        ("Cold start", "10 µs", "--version, cold page cache, ms", "ours drops the page cache first"),
        ("Binary", "6.39 MiB", "Main binary, MB", "the linux-x86_64 release archive's binary, as installed"),
        ("Memory", "single-digit MB", "Memory 1 s after the first frame, RSS MB", ""),
        ("Terminal interactions p95", "< 15 ms", "Keystroke echo, slowest of 26, ms", "ours is the slowest of 26 echoes"),
        ("Listing 14,600 sessions", "95 s to 0.17 s", "Listing 14,600 sessions, ms", ""),
        ("--resume last", "0.5 s", "Continuing the latest of 14,600, ms", "ours includes sending the next request"),
        ("Shell calls", "27.9 ms", "Tool result to next request p50, ms", "ours is per step inside one turn"),
        ("MCP exit", "7.4 ms", None, "not measured: no lane runs an MCP server here"),
    ]
    say("| fx claim | fx says | fx, ours | nc, ours | Note |")
    say("|---|---:|---:|---:|---|")
    for claim, says, label, note in claims:
        say(f"| {claim} | {says} | {ours(label, 'fx') if label else 'n/a'} | {ours(label, 'nc') if label else 'n/a'} | {note} |")
    say("")
    say("## Where nanocode does not lead yet\n")
    say("Every row above where nc's median is not below each of fx, nc-lite, Claude Code and Codex, with the gap to the best of them.\n")
    gaps = []
    for g, label, s, m, _ in table:
        if key_of(label) in CONTEXT_KEYS:
            continue
        ncv = m["nc"][0]
        behind = [(l, m[l][0]) for l in RIVALS if l in m and m[l][0] <= ncv and not (key_of(label) == "binary_mb" and m[l][0] < SCRIPT_MB)]
        if behind:
            l, v = min(behind, key=lambda x: x[1])
            ratio = f"{ncv / v:.2f}x" if v > 0 else "n/a"
            others = ", ".join(NAMES[o] for o, _ in behind if o != l)
            gaps.append((ncv / v if v > 0 else float("inf"), f"- {label} ({letters[id(s)]}): nc {show(ncv)} against {NAMES[l]} {show(v)}, {show(ncv - v)} or {ratio}" + (f"; also behind {others}" if others else "")))
    for _, line in sorted(gaps, key=lambda x: -x[0]):
        say(line)
    if not gaps:
        say("- nowhere in this data")
    say("")
    nc_rows = []
    for g, label, sel, key, counted in ROWS:
        vals = [(letters[id(s)], measure(s, sel, key).get("nc")) for s in srcs]
        vals = [(x, v) for x, v in vals if v]
        if len(vals) > 1:
            nc_rows.append((label, dict(vals)))
    if nc_rows:
        say("## nc across sources\n")
        say("The same nc rows from every source that has them, to show what changed between nanocode commits (and machines; see the source table).\n")
        say("| nc | " + " | ".join(f"{letters[id(s)]} ({s['commit'] or '?'})" for s in srcs) + " |")
        say("|---|" + "---:|" * len(srcs))
        for label, vals in nc_rows:
            say(f"| {label} | " + " | ".join(f"{show(vals[letters[id(s)]][0])} ({vals[letters[id(s)]][1]})" if letters[id(s)] in vals else "" for s in srcs) + " |")
        say("")
    missing = [label for _, label, *_ in ROWS if label not in {l for _, l, *_ in table}]
    if missing:
        say("Not measured in these sources: " + "; ".join(missing) + ".\n")
    say("## Method\n")
    say("- First frame is the time from spawn until the terminal interface has drawn its prompt in a pseudo-terminal (harness/tui.py); memory is sampled from /proc/<pid>/smaps_rollup one second later, summed over the process tree. RSS counts the executable's file pages, which kernel fault-around maps in 64 KB blocks, so it depends on code layout as much as on what runs.")
    say("- One-word reply is a headless run from spawn to exit against a fixture that answers at once. Streaming rows replay a fixed reply at the given token rate and time each token from the fixture sending it to the pseudo-terminal showing it.")
    say("- Session rows save one-turn sessions through each CLI's own headless mode, then list them with its documented command (nc sessions, fx sessions --json) and continue the latest one (nc --continue, fx --resume last), interleaving the lanes round by round. On a loaded machine the 14,600-session rows run under perf stat, whose task-clock and instruction counts are the comparable numbers; wall times there are noise.")
    say("- Idle CPU is a 23 s session's CPU minus a 3 s session's, per second; values a few thousandths from zero are zero within measurement noise.")
    say("- Cold-cache rows drop the page cache before every sample. Install sizes leave out runtimes a lane needs but does not ship (Node for Pi).")
    say("")
    say("## Reproduce\n")
    say("Each source directory holds COMMANDS.md (the exact commands), REPORT.md (every column, PSS and USS, errors) and the raw JSONL. A full run on one machine:\n")
    say("```sh")
    say("cd bench/cli")
    say("sh tools/install.sh && LANES=nclite sh tools/install.sh && . tools/install/env.sh")
    say("export NC_BIN=<nc built with scripts/build-cli.sh nanocode-cli nanocode>")
    say("ROUNDS=20 bash harness/run.sh <machine>    # every part, including the 14,600-session store, then REPORT.md and POST.md")
    say("```")
    open(out, "w").write("\n".join(L).rstrip() + "\n")


main(sys.argv[1:])
