// Streams a scripted reply at fixed token rates through each lane and measures how far the consumer lags the server.
// node harness/sweep.mjs --mode=cli|tui [--rates=1000,10000] [--seconds=5] [--samples=3] [--only=nc,claude] [--tag=x]
import { spawn, spawnSync } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import { ROOT, WORK, RESULTS, WORKSPACE, PYTHON, fresh, startServer, runstat, tokenTracker, newRow, now } from "./common.mjs";
import { lanes, selected } from "./lanes.mjs";
import { streamMetrics, msBetween, usageOf } from "./analyze.mjs";

const args = Object.fromEntries(process.argv.slice(2).map((a) => a.replace(/^--/, "").split("=")));
const mode = args.mode ?? "cli";
const rates = (args.rates ?? "100,1000,10000").split(",").map(Number);
const samples = Number(args.samples ?? 3);
const seconds = Number(args.seconds ?? 5);
const names = selected(args.only);
const cols = Number(args.cols ?? 120), rows = Number(args.rows ?? 40);
fs.mkdirSync(RESULTS, { recursive: true });
fs.mkdirSync(WORK, { recursive: true });
const outFile = RESULTS + "/sweep-" + mode + (args.tag ? "-" + args.tag : "") + ".jsonl";
const kib = process.platform === "darwin" ? 1048576 : 1024;
const server = await startServer();
const ws = fresh(WORKSPACE);
let seq = 0;
const runId = (h, rate) => "sw-" + mode + "-" + h + "-" + rate + "-" + (seq++) + "-" + process.pid;
const prompt = (run, n, rate) => "Stream it BENCH[run=" + run + ",mode=stream,tokens=" + n + ",rate=" + rate + "]";

async function cliSample(h, rate, n) {
  const lane = lanes[h];
  const home = fresh(WORK + "/cli-" + h);
  lane.setup(home, server.base, ws);
  const run = runId(h, rate);
  const [cmd, argv] = lane.headless(home, server.base, prompt(run, n, rate));
  const statFile = WORK + "/rs-cli-" + h + ".json";
  const row = newRow("cli");
  const track = tokenTracker(row);
  const spawnNs = now();
  const child = spawn(...runstat(statFile, cmd, argv), { cwd: ws, env: lane.env(home, server.base), stdio: ["ignore", "pipe", "pipe"] });
  let firstOut = null, outBytes = 0, err = "";
  child.stdout.on("data", (d) => { const t = now(); firstOut ??= t; outBytes += d.length; track(d.toString("utf8"), t); });
  child.stderr.on("data", (d) => (err += d));
  const code = await new Promise((r) => child.on("exit", r));
  const exitNs = now();
  const rs = JSON.parse(fs.readFileSync(statFile, "utf8"));
  const all = await server.log(run);
  const recs = all.filter((r) => r.mode === "stream");
  if (code !== 0 || !recs.length) return { error: "exit " + code + " stream requests " + recs.length + ": " + err.slice(-600) };
  const rec = recs[0];
  return streamMetrics(rec, row.marks, n, {
    spawn_to_request_ms: msBetween(spawnNs, rec.arrive_ns),
    spawn_to_first_stdout_ms: msBetween(spawnNs, firstOut),
    tail_ms: msBetween(rec.done_ns, exitNs),
    wall_ms: msBetween(spawnNs, exitNs),
    stdout_bytes: outBytes,
    requests: all.length, side_requests: all.filter((r) => r.mode === "side").length,
    ...usageOf(rs),
    cpu_us_per_token: (usageOf(rs).tree_cpu_ms * 1000) / n,
  });
}

async function tuiSample(h, rate, n) {
  const lane = lanes[h];
  const home = fresh(WORK + "/tui-" + h);
  lane.setup(home, server.base, ws);
  const run = runId(h, rate);
  const recordPath = WORK + "/tui-rec-" + h + ".bin";
  const spec = { ...lane.tui(home, server.base), env: lane.env(home, server.base), cwd: ws, send: [prompt(run, n, rate), "\r"], tokens: n, quiet_ms: 600, min_ms: seconds * 1000 + 1500, max_ms: seconds * 1000 + 15000, timeout: seconds + 40, rows, cols, record: recordPath };
  const child = spawn(PYTHON, [ROOT + "/harness/tui.py", JSON.stringify(spec)], { stdio: ["ignore", "pipe", "pipe"] });
  let o = "", e = "";
  child.stdout.on("data", (d) => (o += d)); child.stderr.on("data", (d) => (e += d));
  await new Promise((r) => child.on("exit", r));
  let t;
  try { t = JSON.parse(o); } catch { return { error: "tui parse: " + o.slice(0, 300) + e.slice(-500) }; }
  const recs = (await server.log(run)).filter((r) => r.mode === "stream");
  if (!recs.length) return { error: "no request; first frame " + t.first_frame_ms + " max token " + t.max_token + " " + e.slice(-300) };
  const rec = recs[0];
  const rp = spawnSync(PYTHON, [ROOT + "/harness/tui-replay.py", recordPath, String(rows), String(cols), String(n)], { encoding: "utf8", maxBuffer: 256 * 1024 * 1024 });
  let replay;
  try { replay = JSON.parse(rp.stdout); } catch { return { error: "replay failed: " + rp.stderr.slice(-400) }; }
  const doneMark = replay.marks.find((m) => m[1] >= n - 1);
  return streamMetrics(rec, replay.marks, n, {
    first_frame_ms: t.first_frame_ms,
    send_to_request_ms: msBetween(t.sent_ns, rec.arrive_ns),
    display_done_after_server_ms: doneMark ? msBetween(rec.done_ns, doneMark[0]) : null,
    screen_tail: replay.screen_tail.slice(-3),
    cols, rows,
    cpu_ms_total: t.tree_cpu_ms ?? t.cpu_ms, cpu_ms: t.cpu_ms, max_rss_mb: t.max_rss / kib,
    tree_cpu_ms: t.tree_cpu_ms ?? t.cpu_ms, tree_max_rss_mb: t.tree_max_rss_bytes != null ? t.tree_max_rss_bytes / 1048576 : t.max_rss / kib, helpers_max: t.helpers_max ?? null, helpers_left: t.helpers_left ?? null,
    cpu_us_per_token: ((t.tree_cpu_ms ?? t.cpu_ms) * 1000) / n,
    live_max_token: t.max_token, replay_agrees: replay.max_token === t.max_token,
    term_bytes: t.bytes_after_send, term_bytes_per_token: t.bytes_after_send / n, term_writes: t.chunks_after_send, sync_frames: t.sync_frames_after_send,
    max_token_seen: replay.max_token,
  });
}

const fn = { cli: cliSample, tui: tuiSample }[mode];
for (const rate of rates) {
  const n = Math.max(1, Math.round(rate * seconds));
  for (let s = 0; s < samples; s++) {
    const order = names.map((_, i) => names[(i + s) % names.length]);
    for (const h of order) {
      const t = Date.now();
      let res;
      try { res = await fn(h, rate, n); } catch (err) { res = { error: String(err && err.stack || err) }; }
      fs.appendFileSync(outFile, JSON.stringify({ mode, harness: h, rate, sample: s, cols, rows, at: new Date().toISOString(), load: os.loadavg()[0], ...res }) + "\n");
      console.log(JSON.stringify({ h, rate, s, secs: ((Date.now() - t) / 1000).toFixed(1), err: res.error?.slice(0, 200), p50: res.lag_p50_ms?.toFixed?.(2), p99: res.lag_p99_ms?.toFixed?.(2), cpu: res.tree_cpu_ms?.toFixed?.(0), rss: res.tree_max_rss_mb?.toFixed?.(0), missing: res.missing }));
    }
  }
}
await server.stop();
