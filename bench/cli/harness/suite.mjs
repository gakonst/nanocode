// Startup, one-shot, first frame, idle and install-size measurements, one JSON line per sample in results/suite.jsonl.
// node harness/suite.mjs [--parts=startup,oneshot,firstframe,idle,disk] [--only=nc,fx] [--rounds=30]
import { spawn, spawnSync } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import { ROOT, WORK, RESULTS, WORKSPACE, PYTHON, fresh, startServer, runstat, now } from "./common.mjs";
import { lanes, selected } from "./lanes.mjs";
import { msBetween } from "./analyze.mjs";

const args = Object.fromEntries(process.argv.slice(2).map((a) => a.replace(/^--/, "").split("=")));
const parts = (args.parts ?? "startup,oneshot,firstframe,idle,disk").split(",");
const names = selected(args.only);
const rounds = Number(args.rounds ?? 30);
fs.mkdirSync(RESULTS, { recursive: true });
fs.mkdirSync(WORK, { recursive: true });
const out = RESULTS + "/suite" + (args.tag ? "-" + args.tag : "") + ".jsonl";
const server = await startServer();
const ws = fresh(WORKSPACE);
let seq = 0;
const emit = (rec) => fs.appendFileSync(out, JSON.stringify({ at: new Date().toISOString(), load: os.loadavg()[0], ...rec }) + "\n");
const rot = (list, r) => list.map((_, i) => list[(i + r) % list.length]);
const megabytes = (bytes) => (bytes == null ? null : bytes / 1048576);

async function measured(cmd, argv, env, { timeoutMs = 120000 } = {}) {
  const statFile = WORK + "/rs-suite-" + process.pid + "-" + (seq++) + ".json";
  const spawnNs = now();
  const child = spawn(...runstat(statFile, cmd, argv), { cwd: ws, env, stdio: ["ignore", "pipe", "pipe"] });
  let firstOut = null, o = "", e = "";
  child.stdout.on("data", (d) => { firstOut ??= now(); o += d; });
  child.stderr.on("data", (d) => (e += d));
  const timer = setTimeout(() => child.kill("SIGKILL"), timeoutMs);
  const code = await new Promise((r) => child.on("exit", r));
  clearTimeout(timer);
  const exitNs = now();
  let rs = {};
  try { rs = JSON.parse(fs.readFileSync(statFile, "utf8")); fs.rmSync(statFile); } catch {}
  return { code, o, e, spawnNs, firstOut, exitNs, usage: { cpu_ms: (rs.user_us + rs.sys_us) / 1000, user_ms: rs.user_us / 1000, sys_ms: rs.sys_us / 1000, max_rss_mb: megabytes(rs.maxrss_bytes), max_footprint_mb: megabytes(rs.lifetime_max_footprint), instructions: rs.instructions } };
}

async function terminal(spec) {
  const child = spawn(PYTHON, [ROOT + "/harness/tui.py", JSON.stringify(spec)], { stdio: ["ignore", "pipe", "pipe"] });
  let o = "", e = "";
  child.stdout.on("data", (d) => (o += d)); child.stderr.on("data", (d) => (e += d));
  await new Promise((r) => child.on("exit", r));
  try { return JSON.parse(o); } catch { return { error: o.slice(0, 300) + e.slice(-500) }; }
}

function prepared(name, tag) {
  const lane = lanes[name];
  const home = fresh(WORK + "/" + tag + "-" + name);
  lane.setup(home, server.base, ws);
  return { lane, home, env: lane.env(home, server.base) };
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
  console.log("startup done");
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
  console.log("oneshot done");
}

if (parts.includes("firstframe")) {
  for (let r = 0; r < rounds; r++) {
    for (const name of rot(names, r)) {
      const { lane, home, env } = prepared(name, "ff");
      const t = await terminal({ ...lane.tui(home, server.base), env, cwd: ws, window: 2, timeout: 30 });
      emit({ part: "firstframe", harness: name, round: r, first_byte_ms: t.first_byte_ms, first_frame_ms: t.first_frame_ms, exit_after_ms: t.exit_after_ms, cpu_ms: t.cpu_ms, max_rss_mb: t.max_rss == null ? null : t.max_rss / (process.platform === "darwin" ? 1048576 : 1024), bytes: t.bytes_total, status: t.status, error: t.error });
    }
  }
  console.log("firstframe done");
}

if (parts.includes("idle")) {
  const windows = [3, 23];
  for (let r = 0; r < Number(args.idle ?? 5); r++) {
    for (const name of rot(names, r)) {
      const sample = {};
      for (const window of windows) {
        const { lane, home, env } = prepared(name, "idle");
        const t = await terminal({ ...lane.tui(home, server.base), env, cwd: ws, window, timeout: window + 30 });
        sample[window] = { cpu_ms: t.cpu_ms, max_rss: t.max_rss, first_frame_ms: t.first_frame_ms, error: t.error };
      }
      const [short, long] = windows.map((window) => sample[window]);
      emit({ part: "idle", harness: name, round: r, idle_seconds: windows[1] - windows[0], idle_cpu_ms: long.cpu_ms - short.cpu_ms, idle_cpu_ms_per_second: (long.cpu_ms - short.cpu_ms) / (windows[1] - windows[0]), max_rss_mb: long.max_rss == null ? null : long.max_rss / (process.platform === "darwin" ? 1048576 : 1024), error: short.error ?? long.error });
    }
  }
  console.log("idle done");
}

if (parts.includes("disk")) {
  for (const name of names) {
    const paths = (process.env["BENCH_INSTALL_" + name.toUpperCase()] ?? "").split(":").filter(Boolean);
    if (!paths.length) { emit({ part: "disk", harness: name, error: "set BENCH_INSTALL_" + name.toUpperCase() + " to the paths the install added, separated by colons" }); continue; }
    const sizes = paths.map((path) => ({ path, kb: Number(spawnSync("du", ["-skL", path], { encoding: "utf8" }).stdout.split("\t")[0]) }));
    emit({ part: "disk", harness: name, install_mb: sizes.reduce((sum, entry) => sum + entry.kb, 0) / 1024, paths: sizes });
  }
  console.log("disk done");
}

await server.stop();
