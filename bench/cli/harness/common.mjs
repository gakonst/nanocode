import { spawn } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

export const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
export const WORK = process.env.BENCH_WORK ?? ROOT + "/work";
export const RESULTS = process.env.BENCH_RESULTS ?? ROOT + "/results";
export const WORKSPACE = process.env.BENCH_WORKSPACE ?? "/tmp/cli-bench-ws";
export const NODE = process.env.NODE_BIN ?? process.execPath;
export const PYTHON = process.env.PYTHON ?? "python3";
export const HYPERFINE = process.env.HYPERFINE ?? "hyperfine";
export const now = () => process.hrtime.bigint();

const required = (name) => {
  const value = process.env[name];
  if (!value) throw new Error(name + " is not set; see README.md for the binary each lane needs");
  return value;
};

export const bins = {
  nc: () => required("NC_BIN"),
  fx: () => required("FX_BIN"),
  pi: () => required("PI_CLI"),
  claude: () => required("CLAUDE_BIN"),
  codex: () => required("CODEX_BIN"),
  opencode: () => required("OPENCODE_BIN"),
};

export async function startServer(extra = {}) {
  const child = spawn(NODE, [ROOT + "/harness/fakellm.mjs"], { env: { ...process.env, PORT: "0", ...extra }, stdio: ["ignore", "pipe", "inherit"] });
  const line = await new Promise((resolve, reject) => {
    let buf = "";
    child.stdout.on("data", (d) => { buf += d; if (buf.includes("\n")) resolve(buf.split("\n")[0]); });
    child.once("exit", (code) => reject(new Error("server exited " + code)));
  });
  const { port } = JSON.parse(line);
  const base = "http://127.0.0.1:" + port;
  return {
    base, port, child,
    async log(run, drop = true) { return (await fetch(base + "/_log?run=" + encodeURIComponent(run) + (drop ? "&drop=1" : ""))).json(); },
    async stop() { child.kill(); },
  };
}

export function baseEnv(home) {
  const keep = ["PATH", "LANG", "TMPDIR", "USER", "LOGNAME", "SHELL"];
  const env = {};
  for (const k of keep) if (process.env[k] !== undefined) env[k] = process.env[k];
  env.TERM = process.env.BENCH_TERM ?? "xterm-256color";
  env.HOME = home;
  env.XDG_CONFIG_HOME = home + "/.config";
  env.XDG_DATA_HOME = home + "/.local/share";
  env.XDG_CACHE_HOME = home + "/.cache";
  env.XDG_STATE_HOME = home + "/.local/state";
  return env;
}

export function fresh(dir) {
  fs.rmSync(dir, { recursive: true, force: true });
  fs.mkdirSync(dir, { recursive: true });
  return dir;
}

export function stats(values) {
  const v = values.filter((x) => Number.isFinite(x)).sort((a, b) => a - b);
  if (!v.length) return null;
  const q = (f) => v[Math.min(v.length - 1, Math.ceil(v.length * f) - 1)];
  return { n: v.length, mean: v.reduce((a, b) => a + b, 0) / v.length, p50: q(0.5), p90: q(0.9), p95: q(0.95), p99: q(0.99), min: v[0], max: v.at(-1) };
}

export function runstat(statFile, cmd, args) {
  const native = ROOT + "/tools/runstat";
  return fs.existsSync(native) ? [native, [statFile, cmd, ...args]] : [PYTHON, [ROOT + "/tools/runstat.py", statFile, cmd, ...args]];
}

export function tokenTracker(row) {
  let tail = "";
  let max = -1;
  row.marks = [];
  return (text, t = now()) => {
    row.first_text_ns ??= t;
    row.events += 1;
    row.text_bytes += text.length;
    tail += text;
    const re = /w(\d+)[ \n\\]/g;
    let m, last = 0;
    while ((m = re.exec(tail))) { const i = Number(m[1]); if (i > max) max = i; last = re.lastIndex; }
    tail = tail.slice(last);
    if (row.marks.length === 0 || row.marks.at(-1)[1] !== max) row.marks.push([t, max]);
  };
}

export function newRow(label) {
  return { label, first_text_ns: null, events: 0, text_bytes: 0 };
}
