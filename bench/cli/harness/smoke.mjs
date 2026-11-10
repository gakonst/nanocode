// Runs one short streamed prompt through each lane, headless and in its terminal interface, and prints what happened.
// Use it to make a lane work before measuring it: node harness/smoke.mjs --only=claude [--dump=/tmp/dump]
import { spawn } from "node:child_process";
import fs from "node:fs";
import { ROOT, WORK, WORKSPACE, PYTHON, fresh, startServer } from "./common.mjs";
import { lanes, selected } from "./lanes.mjs";

const args = Object.fromEntries(process.argv.slice(2).map((a) => a.replace(/^--/, "").split("=")));
const server = await startServer(args.dump ? { DUMP: fresh(args.dump) } : {});
const ws = fresh(WORKSPACE);
const run = (cmd, argv, env) => new Promise((resolve) => {
  const child = spawn(cmd, argv, { cwd: ws, env, stdio: ["ignore", "pipe", "pipe"] });
  let out = "", err = "";
  child.stdout.on("data", (d) => (out += d));
  child.stderr.on("data", (d) => (err += d));
  const timer = setTimeout(() => child.kill("SIGKILL"), 60000);
  child.on("exit", (code) => { clearTimeout(timer); resolve({ code, out, err }); });
});

for (const name of selected(args.only)) {
  const lane = lanes[name];
  const home = fresh(WORK + "/smoke-" + name);
  lane.setup(home, server.base, ws);
  const env = lane.env(home, server.base);
  const version = await run(...lane.version(), env);
  const id = "smoke-" + name + "-" + process.pid;
  const [cmd, argv] = lane.headless(home, server.base, "Stream it BENCH[run=" + id + ",mode=stream,tokens=200,rate=200]");
  const headless = await run(cmd, argv, env);
  const records = await server.log(id);
  const streamed = records.filter((r) => r.mode === "stream");
  console.log(JSON.stringify({ lane: name, verified: lane.verified, version: version.out.trim().split("\n")[0], headless: { exit: headless.code, requests: records.map((r) => r.mode + ":" + r.protocol + ":" + r.transport), saw_last_token: /w199/.test(headless.out), stdout_bytes: headless.out.length, stderr_tail: headless.code === 0 && streamed.length ? undefined : headless.err.slice(-600) } }));

  const tui = lane.tui(home, server.base);
  const tid = "smoke-tui-" + name + "-" + process.pid;
  const spec = { ...tui, env, cwd: ws, send: ["Stream it BENCH[run=" + tid + ",mode=stream,tokens=200,rate=200]", "\r"], tokens: 200, quiet_ms: 600, min_ms: 3000, max_ms: 20000, timeout: 40, rows: 40, cols: 120, dump: WORK + "/smoke-tui-" + name + ".bin" };
  const child = spawn(PYTHON, [ROOT + "/harness/tui.py", JSON.stringify(spec)], { stdio: ["ignore", "pipe", "pipe"] });
  let out = "", err = "";
  child.stdout.on("data", (d) => (out += d));
  child.stderr.on("data", (d) => (err += d));
  await new Promise((resolve) => child.on("exit", resolve));
  let result;
  try { result = JSON.parse(out); } catch { result = { error: out.slice(0, 300) + err.slice(-300) }; }
  const trecords = await server.log(tid);
  console.log(JSON.stringify({ lane: name, tui: { first_frame_ms: result.first_frame_ms, first_byte_ms: result.first_byte_ms, max_token: result.max_token, requests: trecords.map((r) => r.mode), exit_status: result.status, error: result.error, screen_dump: spec.dump } }));
}
await server.stop();
