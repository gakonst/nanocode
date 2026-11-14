// Markdown report from a results directory: one table per measurement with every lane, then where the lead lane leads and trails.
// node harness/report.mjs [--dir=results/<date>-<machine>] [--lead=nc] [--lanes=nc,fx,...] [--tie=0.05]
import fs from "node:fs";
import path from "node:path";
import { RESULTS } from "./common.mjs";

const args = Object.fromEntries(process.argv.slice(2).map((a) => a.replace(/^--/, "").split("=")));
const dir = args.dir ?? process.argv.slice(2).find((a) => !a.startsWith("--")) ?? RESULTS;
const lead = args.lead ?? "nc";
const tie = Number(args.tie ?? 0.05);
const read = (file) => fs.existsSync(file) ? fs.readFileSync(file, "utf8").split("\n").filter(Boolean).map((line) => JSON.parse(line)) : [];
const files = fs.existsSync(dir) ? fs.readdirSync(dir) : [];
const rowsOf = (re) => files.filter((name) => re.test(name)).flatMap((name) => read(path.join(dir, name)));
const sorted = (values) => values.filter((x) => typeof x === "number" && Number.isFinite(x)).sort((a, b) => a - b);
const median = (values) => { const v = sorted(values); if (!v.length) return null; const m = v.length >> 1; return v.length % 2 ? v[m] : (v[m - 1] + v[m]) / 2; };
const show = (x) => x == null ? "n/a" : Math.abs(x) >= 1000 ? Math.round(x).toLocaleString("en-US") : Math.abs(x) >= 100 ? x.toFixed(0) : Math.abs(x) >= 10 ? x.toFixed(1) : Math.abs(x) >= 1 ? x.toFixed(2) : x.toFixed(3);
const table = (head, rows) => ["| " + head.join(" | ") + " |", "|" + head.map((_, i) => (i ? "---:" : "---")).join("|") + "|", ...rows.map((row) => "| " + row.join(" | ") + " |")].join("\n");
const order = (args.lanes ?? "true,nc,ncdev,fx,pi,opencode,claude,codex").split(",");
const lanesOf = (rows) => [...new Set(rows.map((row) => row.harness))].sort((a, b) => (order.indexOf(a) + 99 * (order.indexOf(a) < 0)) - (order.indexOf(b) + 99 * (order.indexOf(b) < 0)) || a.localeCompare(b));
const good = (row) => !row.error && (row.code === undefined || row.code === 0);
const out = [];
const say = (s = "") => out.push(s);
const verdicts = [];

function cell(mine, key, spread) {
  const ok = mine.filter(good);
  if (!ok.length) return mine.length ? "failed (" + mine.length + ")" : "n/a";
  const v = sorted(ok.map((row) => row[key]));
  if (!v.length) return "n/a";
  const m = median(v);
  return show(m) + (spread && v.length > 1 ? " [" + show(v[0]) + "–" + show(v.at(-1)) + "]" : "") + " (" + v.length + ")";
}
function judge(section, label, rows, key, lower = true) {
  const meds = {};
  for (const lane of lanesOf(rows)) { if (lane === "true") continue; const ok = rows.filter((row) => row.harness === lane && good(row)); const m = median(ok.map((row) => row[key])); if (m != null && ok.length) meds[lane] = { m, n: ok.length }; }
  if (!meds[lead]) { if (rows.some((row) => row.harness === lead)) verdicts.push({ kind: "trails", text: section + ", " + label + ": " + lead + " failed" }); return; }
  const others = Object.entries(meds).filter(([lane]) => lane !== lead && !(lead.startsWith("nc") && lane.startsWith("nc")));
  if (!others.length) return;
  const best = others.reduce((a, b) => (lower ? (b[1].m < a[1].m ? b : a) : (b[1].m > a[1].m ? b : a)));
  const mine = meds[lead].m, theirs = best[1].m;
  const ratio = lower ? mine / theirs : theirs / mine;
  const kind = Math.abs(mine - theirs) <= tie * Math.max(Math.abs(mine), Math.abs(theirs)) || (Math.abs(mine - theirs) < 0.05 && key.endsWith("_ms")) ? "ties" : (lower ? mine < theirs : mine > theirs) ? "leads" : "trails";
  verdicts.push({ kind, ratio, text: section + ", " + label + ": " + lead + " " + show(mine) + " against " + best[0] + " " + show(theirs) + (kind === "ties" ? "" : " (" + (ratio < 1 ? (1 / ratio).toFixed(2) + "x better" : ratio.toFixed(2) + "x worse") + ")") });
}
function section(title, rows, columns, note) {
  if (!rows.length) return;
  say("## " + title + "\n");
  if (note) say(note + "\n");
  say(table(["lane", "samples", "errors", ...columns.map((c) => c[1])], lanesOf(rows).map((lane) => {
    const mine = rows.filter((row) => row.harness === lane);
    return [lane, String(mine.filter(good).length), String(mine.length - mine.filter(good).length), ...columns.map(([key, , spread]) => cell(mine, key, spread))];
  })));
  const errors = lanesOf(rows).map((lane) => [lane, rows.find((row) => row.harness === lane && !good(row))]).filter(([, row]) => row);
  if (errors.length) { say(""); for (const [lane, row] of errors) say("- " + lane + " error: " + String(row.error ?? row.stderr_tail ?? "exit " + row.code).replace(/\s+/g, " ").slice(0, 240)); }
  say("");
  for (const [key, label, , judged = true, lower = true] of columns) if (judged) judge(title, label, rows, key, lower);
}

const machine = fs.existsSync(path.join(dir, "machine.json")) ? JSON.parse(fs.readFileSync(path.join(dir, "machine.json"), "utf8")) : null;
say("# CLI benchmark: " + path.basename(path.resolve(dir)) + "\n");
if (machine) say("Machine: " + [machine.cpu, machine.cores + " threads", machine.memory_gb + " GB", machine.os, machine.kernel].filter(Boolean).join(", ") + ". " + (machine.note ?? "") + "\n");
if (fs.existsSync(path.join(dir, "VERSIONS.md"))) say(fs.readFileSync(path.join(dir, "VERSIONS.md"), "utf8").trim() + "\n");
say("Medians with the sample count in parentheses and [min–max] where the spread matters. CPU and memory are the whole process tree (cgroup or /proc sampling; rusage when neither is available). Lower is better everywhere.\n");

const suite = rowsOf(/^suite.*\.jsonl$/);
const part = (p) => suite.filter((row) => row.part === p);
section("Startup: --version", part("startup"), [["wall_ms", "wall ms", true], ["tree_cpu_ms", "CPU ms"], ["tree_max_rss_mb", "peak RSS MB"], ["instructions", "instructions", false, false]]);
section("Cold start: --version after dropping the page cache", part("coldstart"), [["wall_ms", "wall ms", true], ["tree_cpu_ms", "CPU ms"], ["tree_max_rss_mb", "peak RSS MB"]]);
section("Time to first frame", part("firstframe"), [["first_byte_ms", "first byte ms"], ["first_frame_ms", "first frame ms", true], ["tree_cpu_ms", "CPU ms"], ["tree_max_rss_mb", "peak RSS MB"], ["helpers_max", "processes", false, false]]);
section("Memory one second after the first frame", part("firstframe"), [["mem_rss_mb", "RSS MB"], ["mem_pss_mb", "PSS MB"], ["mem_uss_mb", "private (USS) MB"], ["mem_anon_mb", "anonymous MB"]], "Summed over the CLI's process tree from /proc/<pid>/smaps_rollup. RSS counts the executable's file pages, which every instance shares; PSS splits shared pages between the processes that map them; USS is memory only this tree holds.");
section("Time to first frame, cold page cache", part("coldframe"), [["first_frame_ms", "first frame ms", true], ["tree_cpu_ms", "CPU ms"]]);
section("Idle terminal interface", part("idle"), [["idle_cpu_ms_per_second", "idle CPU ms/s", true], ["idle_bytes", "bytes written while idle"], ["tree_max_rss_mb", "peak RSS MB"], ["helpers_max", "processes", false, false]]);
section("Keystroke echo", part("keys"), [["key_p50_ms", "echo p50 ms", true], ["key_max_ms", "echo max ms"], ["echoed", "keys echoed of 26", false, false]]);
section("One-shot headless prompt", part("oneshot"), [["wall_ms", "wall ms", true], ["spawn_to_request_ms", "spawn to request ms"], ["request_to_exit_ms", "reply end to exit ms"], ["tree_cpu_ms", "CPU ms"], ["tree_max_rss_mb", "peak RSS MB"], ["request_bytes", "first request bytes"], ["side_requests", "side requests"], ["helpers_left", "processes left behind", false]]);
for (const steps of [...new Set(part("tools").map((row) => row.steps))].sort((a, b) => a - b))
  section("Long tool turn: " + steps + " shell steps, headless", part("tools").filter((row) => row.steps === steps), [["wall_ms", "wall ms", true], ["step_gap_p50_ms", "tool result to next request p50 ms"], ["cpu_per_step_ms", "CPU per step ms"], ["tree_max_rss_mb", "peak RSS MB"], ["last_request_bytes", "last request bytes"]]);
for (const k of [...new Set(part("fleet").map((row) => row.k))].sort((a, b) => a - b))
  section("Fleet: " + k + " concurrent headless runs, 1,000 tokens each at 1,000 tok/s", part("fleet").filter((row) => row.k === k).map((row) => ({ ...row, error: row.ok === row.k ? undefined : row.error ?? "failed" })), [["wall_ms", "wall ms", true], ["cpu_ms_per_agent", "CPU per run ms"], ["rss_mb_per_agent", "peak RSS per run MB"], ["exit_after_stream_p50_ms", "stream end to exit p50 ms"]]);
const sessionCounts = [...new Set(part("store").map((row) => row.sessions ?? 1000))].sort((a, b) => a - b);
const count = (n) => Number(n ?? 1000).toLocaleString("en-US");
for (const n of sessionCounts) section("Session store: " + count(n) + " one-turn sessions", part("store").filter((row) => (row.sessions ?? 1000) === n), [["store_kb", "store KB"], ["store_kb_per_session", "KB per session"], ["store_files", "files", false], ["create_cpu_ms_per_session", "CPU per saved session ms"]]);
for (const n of sessionCounts) {
  const of = (p) => part(p).filter((row) => (row.sessions ?? 1000) === n);
  section("Listing " + count(n) + " sessions", of("store-list"), [["wall_ms", "wall ms", true], ["tree_cpu_ms", "CPU ms"], ["tree_max_rss_mb", "peak RSS MB"], ["task_clock_ms", "task-clock ms"], ["instructions_user", "user instructions", false], ["instructions_kernel", "kernel instructions", false]]);
  section("Continuing the latest of " + count(n) + " sessions", of("store-resume"), [["wall_ms", "wall ms", true], ["spawn_to_request_ms", "spawn to request ms"], ["tree_cpu_ms", "CPU ms"], ["tree_max_rss_mb", "peak RSS MB"], ["task_clock_ms", "task-clock ms"], ["instructions_user", "user instructions", false], ["instructions_kernel", "kernel instructions", false]]);
}
section("Install size", part("disk").map((row) => ({ ...row, code: undefined })), [["install_mb", "install MB"], ["binary_mb", "main binary MB"]], "Runtimes a lane needs but does not install (Node for npm packages) are not counted; VERSIONS.md lists them.");

for (const mode of ["cli", "tui"]) {
  const rows = rowsOf(new RegExp("^sweep-" + mode + ".*\\.jsonl$"));
  if (!rows.length) continue;
  const sizes = [...new Set(rows.map((row) => (row.cols ?? 120) + "x" + (row.rows ?? 40)))];
  for (const size of sizes) {
    const mine = rows.filter((row) => (row.cols ?? 120) + "x" + (row.rows ?? 40) === size);
    const rates = [...new Set(mine.map((row) => row.rate))].sort((a, b) => a - b);
    const metrics = mode === "cli"
      ? [["lag_p50_ms", "per-token lag p50 ms"], ["lag_p99_ms", "per-token lag p99 ms"], ["tail_ms", "stream end to exit ms"], ["tree_cpu_ms", "CPU, whole run ms"], ["cpu_us_per_token", "CPU per token µs"], ["tree_max_rss_mb", "peak RSS MB"], ["missing", "tokens never on stdout"]]
      : [["lag_p50_ms", "on-screen lag p50 ms"], ["lag_p99_ms", "on-screen lag p99 ms"], ["display_done_after_server_ms", "stream end to last token on screen ms"], ["send_to_request_ms", "Enter to request ms"], ["tree_cpu_ms", "CPU, whole session ms"], ["tree_max_rss_mb", "peak RSS MB"], ["term_bytes_per_token", "terminal bytes per token"], ["missing", "tokens never on screen"]];
    say("## Streaming, " + (mode === "cli" ? "headless" : "terminal interface " + size) + "\n");
    for (const [key, label] of metrics) {
      say("**" + label + "**\n");
      say(table(["lane", ...rates.map((rate) => rate.toLocaleString("en-US") + " tok/s")], lanesOf(mine).map((lane) => [lane, ...rates.map((rate) => cell(mine.filter((row) => row.harness === lane && row.rate === rate), key, false))])));
      say("");
      for (const rate of rates) judge("Streaming " + (mode === "cli" ? "headless" : "TUI " + size) + " at " + rate.toLocaleString("en-US") + " tok/s", label, mine.filter((row) => row.rate === rate), key);
    }
    const errs = mine.filter((row) => row.error);
    if (errs.length) { for (const lane of lanesOf(errs)) say("- " + lane + ": " + errs.filter((row) => row.harness === lane).length + " failed samples, e.g. " + String(errs.find((row) => row.harness === lane).error).replace(/\s+/g, " ").slice(0, 200)); say(""); }
  }
}

const audit = rowsOf(/^netaudit.*\.jsonl$/).map((row) => ({ ...row, harness: row.harness ?? row.lane }));
if (audit.length) {
  say("## Outbound connection attempts (network blocked)\n");
  say(table(["lane", "attempts", "destinations", "names"], lanesOf(audit).map((lane) => { const r = audit.filter((row) => row.harness === lane); return [lane, String(r.reduce((s, x) => s + (x.attempts ?? 0), 0)), String(new Set(r.flatMap((x) => Object.keys(x.destinations ?? {}))).size), [...new Set(r.flatMap((x) => x.names ?? []))].join(", ") || "none"]; })));
  say("");
}

say("## Where " + lead + " leads\n");
for (const v of verdicts.filter((v) => v.kind === "leads").sort((a, b) => a.ratio - b.ratio)) say("- " + v.text);
say("\n## Where " + lead + " trails\n");
const trails = verdicts.filter((v) => v.kind === "trails").sort((a, b) => (b.ratio ?? 99) - (a.ratio ?? 99));
if (!trails.length) say("- nowhere in this data");
for (const v of trails) say("- " + v.text);
say("\n## Ties (within " + Math.round(tie * 100) + "%)\n");
for (const v of verdicts.filter((v) => v.kind === "ties")) say("- " + v.text);
console.log(out.join("\n"));
