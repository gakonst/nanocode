// Startup, cold start, first frame, idle, keystroke echo, one-shot, long tool turns, concurrent fleets, session store and install size.
// One JSON line per sample in results/suite[-tag].jsonl.
// node harness/suite.mjs [--parts=startup,coldstart,firstframe,coldframe,idle,keys,oneshot,tools,fleet,store,disk] [--only=nc,fx] [--rounds=30]
import { spawn, spawnSync } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import { ROOT, WORK, RESULTS, WORKSPACE, PYTHON, fresh, startServer, runstat, now } from "./common.mjs";
import { lanes, selected } from "./lanes.mjs";
import { extra } from "./lanes-extra.mjs";
import { msBetween, usageOf } from "./analyze.mjs";

const args = Object.fromEntries(process.argv.slice(2).map((a) => a.replace(/^--/, "").split("=")));
const parts = (args.parts ?? "startup,coldstart,firstframe,coldframe,idle,keys,oneshot,tools,fleet,store,disk").split(",");
const names = selected(args.only);
const rounds = Number(args.rounds ?? 30);
const cols = Number(args.cols ?? 120), rows = Number(args.rows ?? 40);
fs.mkdirSync(RESULTS, { recursive: true });
fs.mkdirSync(WORK, { recursive: true });
const out = RESULTS + "/suite" + (args.tag ? "-" + args.tag : "") + ".jsonl";
const server = await startServer();
const ws = fresh(WORKSPACE);
let seq = 0;
const emit = (rec) => fs.appendFileSync(out, JSON.stringify({ at: new Date().toISOString(), load: os.loadavg()[0], ...rec }) + "\n");
const rot = (list, r) => list.map((_, i) => list[(i + r) % list.length]);
const megabytes = (bytes) => (bytes == null ? null : bytes / 1048576);
const kib = process.platform === "darwin" ? 1048576 : 1024;
const say = (s) => console.log(new Date().toISOString().slice(11, 19) + " " + s);


async function measured(cmd, argv, env, { timeoutMs = 120000, cwd = ws } = {}) {
  const statFile = WORK + "/rs-suite-" + process.pid + "-" + (seq++) + ".json";
  const spawnNs = now();
  const child = spawn(...runstat(statFile, cmd, argv), { cwd, env, stdio: ["ignore", "pipe", "pipe"] });
  let firstOut = null, o = "", e = "";
  child.stdout.on("data", (d) => { firstOut ??= now(); o += d; });
  child.stderr.on("data", (d) => (e += d));
  const timer = setTimeout(() => child.kill("SIGKILL"), timeoutMs);
  const code = await new Promise((r) => child.on("exit", r));
  clearTimeout(timer);
  const exitNs = now();
  let rs = {};
  try { rs = JSON.parse(fs.readFileSync(statFile, "utf8")); fs.rmSync(statFile); } catch {}
  return { code, o, e, spawnNs, firstOut, exitNs, usage: usageOf(rs) };
}

async function terminal(spec) {
  const child = spawn(PYTHON, [ROOT + "/harness/tui.py", JSON.stringify({ rows, cols, ...spec })], { stdio: ["ignore", "pipe", "pipe"] });
  let o = "", e = "";
  child.stdout.on("data", (d) => (o += d)); child.stderr.on("data", (d) => (e += d));
  await new Promise((r) => child.on("exit", r));
  try { return JSON.parse(o); } catch { return { error: o.slice(0, 300) + e.slice(-500) }; }
}
const tuiUsage = (t) => ({
  cpu_ms: t.cpu_ms, max_rss_mb: t.max_rss == null ? null : t.max_rss / kib,
  tree_cpu_ms: t.tree_cpu_ms ?? t.cpu_ms, tree_max_rss_mb: t.tree_max_rss_bytes != null ? megabytes(t.tree_max_rss_bytes) : t.max_rss == null ? null : t.max_rss / kib,
  helpers_max: t.helpers_max ?? null, helpers_left: t.helpers_left ?? null,
});

function prepared(name, tag) {
  const lane = lanes[name];
  const home = fresh(WORK + "/" + tag + "-" + name);
  lane.setup(home, server.base, ws);
  return { lane, home, env: lane.env(home, server.base) };
}

function dropCaches() {
  if (process.platform !== "linux") return false;
  return spawnSync("sudo", ["-n", "sh", "-c", "sync; echo 3 > /proc/sys/vm/drop_caches"]).status === 0;
}

if (parts.includes("startup")) {
  for (let r = 0; r < rounds; r++) {
    const floor = await measured("/usr/bin/true", [], process.env);
    emit({ part: "startup", harness: "true", round: r, code: floor.code, wall_ms: msBetween(floor.spawnNs, floor.exitNs), ...floor.usage });
    for (const name of rot(names, r)) {
      const { lane, env } = prepared(name, "st");
      const res = await measured(...lane.version(), env);
      emit({ part: "startup", harness: name, round: r, code: res.code, wall_ms: msBetween(res.spawnNs, res.exitNs), ...res.usage });
    }
  }
  say("startup done");
}

if (parts.includes("coldstart")) {
  for (let r = 0; r < Number(args.coldrounds ?? 10); r++) {
    for (const name of rot(names, r)) {
      const { lane, env } = prepared(name, "cs");
      if (!dropCaches()) { emit({ part: "coldstart", harness: name, round: r, error: "cannot drop the page cache (needs Linux and passwordless sudo)" }); continue; }
      const res = await measured(...lane.version(), env);
      emit({ part: "coldstart", harness: name, round: r, code: res.code, wall_ms: msBetween(res.spawnNs, res.exitNs), ...res.usage });
    }
  }
  say("coldstart done");
}

async function frames(part, cold) {
  const n = cold ? Number(args.coldrounds ?? 10) : rounds;
  for (let r = 0; r < n; r++) {
    for (const name of rot(names, r)) {
      const { lane, home, env } = prepared(name, "ff");
      if (cold && !dropCaches()) { emit({ part, harness: name, round: r, error: "cannot drop the page cache" }); continue; }
      const t = await terminal({ ...lane.tui(home, server.base), env, cwd: ws, window: 2, timeout: 30 });
      emit({ part, harness: name, round: r, first_byte_ms: t.first_byte_ms, first_frame_ms: t.first_frame_ms, exit_after_ms: t.exit_after_ms, ...tuiUsage(t), bytes: t.bytes_total, status: t.status, error: t.error ?? (t.first_frame_ms == null ? "no first frame" : undefined) });
    }
  }
  say(part + " done");
}
if (parts.includes("firstframe")) await frames("firstframe", false);
if (parts.includes("coldframe")) await frames("coldframe", true);

if (parts.includes("idle")) {
  const windows = [3, Number(args.idlewindow ?? 23)];
  for (let r = 0; r < Number(args.idle ?? 10); r++) {
    for (const name of rot(names, r)) {
      const sample = {};
      for (const window of windows) {
        const { lane, home, env } = prepared(name, "idle");
        sample[window] = await terminal({ ...lane.tui(home, server.base), env, cwd: ws, window, timeout: window + 30 });
      }
      const [short, long] = windows.map((window) => sample[window]);
      const cpu = (t) => t.tree_cpu_ms ?? t.cpu_ms;
      emit({ part: "idle", harness: name, round: r, idle_seconds: windows[1] - windows[0], idle_cpu_ms: cpu(long) - cpu(short), idle_cpu_ms_per_second: (cpu(long) - cpu(short)) / (windows[1] - windows[0]), ...tuiUsage(long), idle_bytes: long.bytes_total - short.bytes_total, error: short.error ?? long.error });
    }
  }
  say("idle done");
}

if (parts.includes("keys")) {
  const keys = "qwertyuiopasdfghjklzxcvbnm".split("");
  for (let r = 0; r < Number(args.keyrounds ?? 10); r++) {
    for (const name of rot(names, r)) {
      const { lane, home, env } = prepared(name, "keys");
      const t = await terminal({ ...lane.tui(home, server.base), env, cwd: ws, keys, key_interval_ms: 60, timeout: 40 });
      const lat = (t.key_latency_ms ?? []).filter((x) => x != null).sort((a, b) => a - b);
      emit({ part: "keys", harness: name, round: r, keys: keys.length, echoed: lat.length, key_p50_ms: lat[lat.length >> 1] ?? null, key_max_ms: lat.at(-1) ?? null, key_latency_ms: t.key_latency_ms, ...tuiUsage(t), error: t.error ?? (lat.length < keys.length ? "echoed " + lat.length + " of " + keys.length : undefined) });
    }
  }
  say("keys done");
}

if (parts.includes("oneshot")) {
  for (let r = 0; r < rounds; r++) {
    for (const name of rot(names, r)) {
      const { lane, home, env } = prepared(name, "os");
      const run = "os-" + name + "-" + seq++;
      const res = await measured(...lane.headless(home, server.base, "Reply BENCH[run=" + run + "]"), env);
      const records = await server.log(run);
      const first = records.find((record) => record.mode !== "side") ?? records[0];
      emit({ part: "oneshot", harness: name, round: r, code: res.code, requests: records.length, side_requests: records.filter((record) => record.mode === "side").length, spawn_to_request_ms: msBetween(res.spawnNs, first?.arrive_ns), spawn_to_first_stdout_ms: msBetween(res.spawnNs, res.firstOut), request_to_exit_ms: msBetween(first?.done_ns, res.exitNs), wall_ms: msBetween(res.spawnNs, res.exitNs), request_bytes: first?.request_bytes, ...res.usage, stderr_tail: res.code === 0 ? undefined : res.e.slice(-400) });
    }
  }
  say("oneshot done");
}

if (parts.includes("tools")) {
  for (const steps of (args.steps ?? "10,50").split(",").map(Number)) {
    for (let r = 0; r < Number(args.toolrounds ?? 10); r++) {
      for (const name of rot(names, r)) {
        const x = extra[name];
        if (!x?.toolHeadless) { emit({ part: "tools", harness: name, steps, round: r, error: "no tool lane" }); continue; }
        const { home, env } = prepared(name, "tools");
        const run = "tl-" + name + "-" + seq++;
        const res = await measured(...x.toolHeadless(home, server.base, "Run ls, step by step BENCH[run=" + run + ",mode=steps,steps=" + steps + ",kind=shell,h=" + name + "]"), env, { timeoutMs: 300000 });
        const records = (await server.log(run)).filter((record) => record.mode !== "side");
        const busy = records.reduce((sum, record) => sum + (msBetween(record.arrive_ns, record.done_ns) ?? 0), 0);
        const gaps = records.slice(1).map((record, i) => msBetween(records[i].done_ns, record.arrive_ns)).filter((g) => g != null).sort((a, b) => a - b);
        const ok = res.code === 0 && records.length === steps + 1 && res.o.includes("LONG_TURN_COMPLETE");
        emit({ part: "tools", harness: name, steps, round: r, code: res.code, requests: records.length, wall_ms: msBetween(res.spawnNs, res.exitNs), server_ms: busy,
          step_gap_p50_ms: gaps[gaps.length >> 1] ?? null, step_gap_max_ms: gaps.at(-1) ?? null, last_request_bytes: records.at(-1)?.request_bytes, first_request_bytes: records[0]?.request_bytes,
          cpu_per_step_ms: res.usage.tree_cpu_ms / steps, ...res.usage, error: ok ? undefined : "exit " + res.code + ", " + records.length + " requests, done " + res.o.includes("LONG_TURN_COMPLETE") + ": " + res.e.slice(-300) });
      }
    }
  }
  say("tools done");
}

if (parts.includes("fleet")) {
  for (const k of (args.fleet ?? "1,8,32").split(",").map(Number)) {
    for (let r = 0; r < Number(args.fleetrounds ?? 10); r++) {
      for (const name of rot(names, r)) {
        const started = now();
        const runs = await Promise.all(Array.from({ length: k }, async (_, i) => {
          const { lane, home, env } = prepared(name, "fleet" + i);
          const run = "fl-" + name + "-" + k + "-" + seq++;
          const res = await measured(...lane.headless(home, server.base, "Stream it BENCH[run=" + run + ",mode=stream,tokens=1000,rate=1000]"), env, { timeoutMs: 120000 });
          const records = (await server.log(run)).filter((record) => record.mode === "stream");
          return { res, ok: res.code === 0 && records.length === 1 && /w999\b/.test(res.o), record: records[0] };
        }));
        const wall = msBetween(started, now());
        const sum = (f) => runs.reduce((s, x) => s + (f(x) ?? 0), 0);
        const lags = runs.map((x) => msBetween(x.record?.done_ns, x.res.exitNs)).filter((v) => v != null).sort((a, b) => a - b);
        emit({ part: "fleet", harness: name, k, round: r, ok: runs.filter((x) => x.ok).length, wall_ms: wall, cpu_ms_total: sum((x) => x.res.usage.tree_cpu_ms), cpu_ms_per_agent: sum((x) => x.res.usage.tree_cpu_ms) / k,
          rss_mb_sum_of_peaks: sum((x) => x.res.usage.tree_max_rss_mb), rss_mb_per_agent: sum((x) => x.res.usage.tree_max_rss_mb) / k, exit_after_stream_p50_ms: lags[lags.length >> 1] ?? null, exit_after_stream_max_ms: lags.at(-1) ?? null,
          error: runs.every((x) => x.ok) ? undefined : runs.find((x) => !x.ok)?.res.e.slice(-300) || "missing tokens" });
      }
    }
  }
  say("fleet done");
}

if (parts.includes("store")) {
  const total = Number(args.sessions ?? 1000);
  for (const name of names) {
    const x = extra[name];
    if (!x?.saveHeadless) { emit({ part: "store", harness: name, error: "no session store lane" }); continue; }
    const { home, env } = prepared(name, "store");
    let failures = 0, cpu = 0;
    const began = now();
    for (let i = 0; i < total; i++) {
      const run = "st-" + name + "-" + seq++;
      const res = await measured(...x.saveHeadless(home, server.base, "Remember " + i + " BENCH[run=" + run + "]"), env);
      if (res.code !== 0) failures++;
      cpu += res.usage.tree_cpu_ms ?? 0;
      await server.log(run);
    }
    const create_ms = msBetween(began, now());
    const kb = Number(spawnSync("du", ["-sk", x.store(home)], { encoding: "utf8" }).stdout.split("\t")[0]);
    const store_files = Number(spawnSync("sh", ["-c", "find \"$1\" -type f | wc -l", "-", x.store(home)], { encoding: "utf8" }).stdout.trim());
    emit({ part: "store", harness: name, sessions: total, failures, store_kb: kb, store_kb_per_session: kb / total, store_files, create_ms, create_cpu_ms_per_session: cpu / total });
    for (let r = 0; r < Number(args.storerounds ?? 20); r++) {
      if (x.list) {
        const res = await measured(...x.list(home, server.base), env);
        emit({ part: "store-list", harness: name, round: r, sessions: total, code: res.code, wall_ms: msBetween(res.spawnNs, res.exitNs), stdout_bytes: res.o.length, ...res.usage, error: res.code === 0 ? undefined : res.e.slice(-300) });
      }
      if (x.resume) {
        const run = "rs-" + name + "-" + seq++;
        const res = await measured(...x.resume(home, server.base, "Again BENCH[run=" + run + "]"), env);
        const records = (await server.log(run)).filter((record) => record.mode !== "side");
        emit({ part: "store-resume", harness: name, round: r, sessions: total, code: res.code, wall_ms: msBetween(res.spawnNs, res.exitNs), spawn_to_request_ms: msBetween(res.spawnNs, records[0]?.arrive_ns), request_bytes: records[0]?.request_bytes, ...res.usage, error: res.code === 0 && records.length ? undefined : "exit " + res.code + " requests " + records.length + ": " + res.e.slice(-300) });
      }
    }
  }
  say("store done");
}

if (parts.includes("disk")) {
  for (const name of names) {
    const paths = (process.env["BENCH_INSTALL_" + name.toUpperCase()] ?? "").split(":").filter(Boolean);
    if (!paths.length) { emit({ part: "disk", harness: name, error: "set BENCH_INSTALL_" + name.toUpperCase() + " to the paths the install added, separated by colons" }); continue; }
    const sizes = paths.map((path) => ({ path, kb: Number(spawnSync("du", ["-skL", path], { encoding: "utf8" }).stdout.split("\t")[0]) }));
    const [cmd, argv] = lanes[name].version();
    const main = cmd === process.execPath || /\/node$/.test(cmd) ? argv[0] : cmd;
    let binary_mb = null;
    try { binary_mb = fs.statSync(fs.realpathSync(main)).size / 1048576; } catch {}
    emit({ part: "disk", harness: name, install_mb: sizes.reduce((sum, entry) => sum + entry.kb, 0) / 1024, binary_mb, paths: sizes });
  }
  say("disk done");
}

await server.stop();
