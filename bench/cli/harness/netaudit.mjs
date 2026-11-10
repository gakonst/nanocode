// Records every network destination each lane tries: one headless prompt and a 5 s terminal session under
// strace -f, inside tools/netguard.sh so nothing leaves the machine. Writes RESULTS/netaudit.jsonl and prints a table.
// Background checks race the exit of a short run, so --repeat=N runs each lane N times and the table is the union.
// node harness/netaudit.mjs [--only=nc,fx] [--tui-seconds=5] [--repeat=3]   (re-executes itself under netguard.sh when needed)
import { spawn, spawnSync } from "node:child_process";
import dgram from "node:dgram";
import fs from "node:fs";
import os from "node:os";
import { ROOT, WORK, RESULTS, WORKSPACE, PYTHON, fresh, startServer } from "./common.mjs";
import { lanes, selected } from "./lanes.mjs";

const args = Object.fromEntries(process.argv.slice(2).map((a) => a.replace(/^--/, "").split("=")));
const external = Object.entries(os.networkInterfaces()).some(([name, addrs]) => name !== "lo" && addrs.length);
if (external || process.env.NETGUARD_SINK !== "1") {
  if (process.env.NETGUARD_SINK === "1") throw new Error("NETGUARD_SINK is set but a non-loopback interface is up");
  const r = spawnSync(ROOT + "/tools/netguard.sh", [process.execPath, ...process.argv.slice(1)], { stdio: "inherit", env: { ...process.env, NETGUARD_SINK: "1" } });
  process.exit(r.status ?? 1);
}
const STRACE = process.env.STRACE ?? "strace";
const traceArgs = (file) => [STRACE, "-f", "-qq", "-yy", "-xx", "-s", "256", "-e", "signal=none", "-e", "trace=connect,sendto,sendmsg,sendmmsg,write,openat", "-o", file];

const unhex = (s) => Buffer.from(s.replace(/\\x/g, ""), "hex");
function dnsName(buf) {
  if (buf.length < 17 || buf.readUInt16BE(4) < 1 || (buf[2] & 0x80)) return null;
  const labels = [];
  for (let i = 12; i < buf.length && buf[i];) {
    const n = buf[i];
    if (n > 63 || i + 1 + n > buf.length) return null;
    labels.push(buf.subarray(i + 1, i + 1 + n).toString("latin1"));
    i += n + 1;
  }
  return labels.length && labels.every((l) => /^[A-Za-z0-9_-]+$/.test(l)) ? labels.join(".") : null;
}
// The fake resolver answers A queries with 127.1.x.y, one address per name, and AAAA with no records, so a CLI goes
// on to connect and the connect shows which host and port it wanted. Connects are then refused locally.
const fakeNames = new Map(), fakeIps = new Map(), queried = [];
function fakeIp(name) {
  if (!fakeNames.has(name)) { const k = fakeNames.size + 1, ip = "127.1." + (k >> 8) + "." + (k & 255); fakeNames.set(name, ip); fakeIps.set(ip, name); }
  return fakeNames.get(name);
}
function dnsAnswer(q) {
  const name = dnsName(q);
  if (!name) return null;
  let i = 12;
  while (q[i]) i += q[i] + 1;
  const type = q.readUInt16BE(i + 1), question = q.subarray(12, i + 5);
  queried.push({ name, type });
  const head = Buffer.from(q.subarray(0, 12));
  head.writeUInt16BE(0x8180, 2); head.writeUInt16BE(1, 4); head.writeUInt16BE(type === 1 ? 1 : 0, 6); head.writeUInt32BE(0, 8);
  if (type !== 1) return Buffer.concat([head, question]);
  const rr = Buffer.alloc(16);
  rr.writeUInt16BE(0xc00c, 0); rr.writeUInt16BE(1, 2); rr.writeUInt16BE(1, 4); rr.writeUInt32BE(1, 6); rr.writeUInt16BE(4, 10);
  fakeIp(name).split(".").forEach((b, j) => (rr[12 + j] = Number(b)));
  return Buffer.concat([head, question, rr]);
}
function startResolver() {
  const servers = [...fs.readFileSync("/etc/resolv.conf", "utf8").matchAll(/^nameserver\s+(\S+)/gm)].map((m) => m[1]);
  return Promise.all(servers.map((ip) => new Promise((resolve) => {
    const sock = dgram.createSocket(ip.includes(":") ? "udp6" : "udp4");
    sock.on("message", (msg, from) => { const a = dnsAnswer(msg); if (a) sock.send(a, from.port, from.address); });
    sock.on("error", (err) => { process.stderr.write("netaudit: no fake resolver on " + ip + ": " + err.code + "\n"); resolve(null); });
    sock.bind(53, ip, () => resolve(sock));
  })));
}
const search = (fs.readFileSync("/etc/resolv.conf", "utf8").match(/^(?:search|domain)\s+(.+)$/m)?.[1] ?? "").split(/\s+/).filter(Boolean);
const loopback = (ip) => /^127\./.test(ip) || ip === "::1" || /^::ffff:127\./.test(ip) || ip === "0.0.0.0" || ip === "::";

function parseTrace(text) {
  const attempts = [], names = new Set(), unix = new Set(), files = {};
  for (const raw of text.split("\n")) {
    const line = raw.replace(/"((?:\\x[0-9a-f]{2})+)"/g, (m, h) => (/^"(?:\\x(?:2[0-9a-f]|3[0-9a]|[46][0-9a-f]|5[0-9a-f]|7[0-9a-e]))+"$/.test(m) && h.length < 512 ? JSON.stringify(unhex(h).toString("latin1")) : m));
    const call = line.match(/^\d+\s+(\w+)\(/)?.[1];
    if (!call) continue;
    const result = line.match(/\)\s+=\s+(-?\d+)(?:\s+(\w+))?/);
    if (call === "openat") {
      const f = line.match(/"(\/etc\/(?:hosts|resolv\.conf|nsswitch\.conf|gai\.conf))"/)?.[1];
      if (f) files[f] = (files[f] ?? 0) + 1;
      continue;
    }
    if (call === "connect") {
      const v4 = line.match(/sin_port=htons\((\d+)\), sin_addr=inet_addr\("([^"]+)"\)/);
      const v6 = line.match(/sin6_port=htons\((\d+)\).*?inet_pton\(AF_INET6, "([^"]+)"/);
      const un = line.match(/sun_path="([^"]*)"/);
      if (v4 || v6) {
        const [, port, ip] = v4 ?? v6;
        const p = Number(port);
        const host = fakeIps.get(ip.replace(/^::ffff:/, ""));
        if (host || !loopback(ip) || p === 53) attempts.push({ ip, port: p, host, family: v4 ? 4 : 6, dns: p === 53, result: result ? (result[2] ?? "ok") : "?" });
      } else if (un) unix.add(un[1].replace(/\\x([0-9a-f]{2})/g, (_, h) => String.fromCharCode(parseInt(h, 16))));
      continue;
    }
    const dnsSock = /<UDP:\[[^\]]*:53\]>|<TCP:\[[^\]]*:53\]>/.test(line) || /sin6?_port=htons\(53\)/.test(line);
    if (call === "write" && !dnsSock) continue;
    for (const m of line.matchAll(/"((?:\\x[0-9a-f]{2})+)"/g)) {
      const buf = unhex(m[1]);
      const name = dnsSock || call !== "write" ? dnsName(buf) : null;
      if (name) names.add(name);
      const json = buf.toString("latin1").match(/"(?:name|hostname)"\s*:\s*"([^"]+)"/);
      if (json && /io\.systemd\.Resolve/.test(buf.toString("latin1"))) names.add(json[1]);
    }
  }
  const plain = [...names].filter((n) => !search.some((d) => n.endsWith("." + d)));
  return { attempts, names: plain.sort(), unix: [...unix].sort(), files };
}

function run(cmd, argv, opts, timeoutMs) {
  return new Promise((resolve) => {
    const child = spawn(cmd, argv, { ...opts, stdio: ["ignore", "pipe", "pipe"] });
    let out = "", err = "";
    child.stdout.on("data", (d) => (out += d)); child.stderr.on("data", (d) => (err += d));
    const timer = setTimeout(() => child.kill("SIGKILL"), timeoutMs);
    child.on("exit", (code) => { clearTimeout(timer); resolve({ code, out, err }); });
  });
}

const tuiSeconds = Number(args["tui-seconds"] ?? 5);
fs.mkdirSync(RESULTS, { recursive: true });
const outFile = args.out ?? RESULTS + "/netaudit.jsonl";
const server = await startServer();
const resolvers = await startResolver();
const ws = fresh(WORKSPACE);
const rows = [];
for (let round = 0; round < Number(args.repeat ?? 1); round++) for (const name of selected(args.only)) {
  const lane = lanes[name];
  for (const kind of ["headless", "tui"]) {
    const home = fresh(WORK + "/netaudit-" + name);
    const trace = WORK + "/netaudit-" + name + "-" + kind + ".strace";
    let row = { lane: name, kind, round, at: new Date().toISOString() };
    try {
      lane.setup(home, server.base, ws);
      const env = lane.env(home, server.base);
      const id = "netaudit-" + name + "-" + kind + "-" + process.pid;
      const prompt = "Stream it BENCH[run=" + id + ",mode=stream,tokens=20,rate=200]";
      let res;
      if (kind === "headless") {
        const [cmd, argv] = lane.headless(home, server.base, prompt);
        res = await run(traceArgs(trace)[0], [...traceArgs(trace).slice(1), cmd, ...argv], { cwd: ws, env }, 90000);
        row.exit = res.code;
      } else {
        const tui = lane.tui(home, server.base);
        const spec = { ...tui, cmd: [...traceArgs(trace), ...tui.cmd], env, cwd: ws, timeout: tuiSeconds, runstat: false, rows: 40, cols: 120 };
        res = await run(PYTHON, [ROOT + "/harness/tui.py", JSON.stringify(spec)], {}, (tuiSeconds + 30) * 1000);
        try { const t = JSON.parse(res.out); row.first_frame_ms = t.first_frame_ms; row.exit = t.status; } catch { row.error = res.out.slice(0, 200) + res.err.slice(-300); }
      }
      row.fixture_requests = (await server.log(id)).length;
      const asked = queried.splice(0);
      const parsed = parseTrace(fs.readFileSync(trace, "utf8"));
      const dest = new Map();
      for (const a of parsed.attempts.filter((a) => !a.dns)) { const k = (a.host ?? a.ip) + ":" + a.port; dest.set(k, (dest.get(k) ?? 0) + 1); }
      const names = [...new Set([...parsed.names, ...asked.map((q) => q.name)])].filter((n) => !search.some((d) => n.endsWith("." + d))).sort();
      row = { ...row, attempts: parsed.attempts.filter((a) => !a.dns).length, dns_queries: asked.length, destinations: Object.fromEntries(dest), names, unix_sockets: parsed.unix, resolver_files: parsed.files, results: [...new Set(parsed.attempts.map((a) => a.result))] };
      if (kind === "headless" && res.code !== 0) row.stderr_tail = res.err.slice(-300);
    } catch (err) {
      row.error = String(err?.message ?? err);
    }
    rows.push(row);
    fs.appendFileSync(outFile, JSON.stringify(row) + "\n");
  }
}
await server.stop();
for (const sock of resolvers) sock?.close();
const pad = (s, n) => String(s).padEnd(n);
console.log(pad("lane", 10) + pad("kind", 10) + pad("ok", 6) + pad("dns", 6) + pad("connects", 10) + pad("distinct", 10) + "destinations (names only resolved)");
const union = new Map();
for (const r of rows) {
  const k = r.lane + " " + r.kind, u = union.get(k) ?? { lane: r.lane, kind: r.kind, dns_queries: 0, attempts: 0, destinations: {}, names: [], runs: 0, failed: 0 };
  u.runs++;
  if (r.error || r.exit) u.failed++;
  if (!r.error) { u.dns_queries += r.dns_queries; u.attempts += r.attempts; for (const [d, n] of Object.entries(r.destinations)) u.destinations[d] = (u.destinations[d] ?? 0) + n; u.names = [...new Set([...u.names, ...r.names])].sort(); }
  union.set(k, u);
}
for (const r of union.values()) {
  const dest = [...Object.keys(r.destinations), ...r.names.filter((n) => !Object.keys(r.destinations).some((d) => d.startsWith(n + ":"))).map((n) => "(" + n + ")")].join(" ") || "-";
  console.log(pad(r.lane, 10) + pad(r.kind, 10) + pad(r.runs - r.failed + "/" + r.runs, 6) + pad(r.dns_queries, 6) + pad(r.attempts, 10) + pad(Object.keys(r.destinations).length, 10) + dest);
}
console.log("wrote " + outFile);
