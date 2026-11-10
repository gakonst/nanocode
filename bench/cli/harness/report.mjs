// Prints Markdown tables from results/suite*.jsonl and results/sweep-*.jsonl: medians per lane, with the sample count.
import fs from "node:fs";
import { RESULTS } from "./common.mjs";

const read = (file) => fs.existsSync(file) ? fs.readFileSync(file, "utf8").trim().split("\n").filter(Boolean).map((line) => JSON.parse(line)) : [];
const median = (values) => { const v = values.filter((x) => typeof x === "number" && Number.isFinite(x)).sort((a, b) => a - b); if (!v.length) return null; const m = v.length >> 1; return v.length % 2 ? v[m] : (v[m - 1] + v[m]) / 2; };
const show = (x) => x == null ? "n/a" : Math.abs(x) >= 100 ? x.toFixed(0) : Math.abs(x) >= 10 ? x.toFixed(1) : x.toFixed(2);
const table = (head, rows) => ["| " + head.join(" | ") + " |", "|" + head.map((_, i) => (i ? "---:" : "---")).join("|") + "|", ...rows.map((row) => "| " + row.join(" | ") + " |")].join("\n");
const files = fs.existsSync(RESULTS) ? fs.readdirSync(RESULTS) : [];
const suite = files.filter((name) => /^suite.*\.jsonl$/.test(name)).flatMap((name) => read(RESULTS + "/" + name));
const lanesOf = (rows) => [...new Set(rows.map((row) => row.harness))];

for (const [part, columns] of [
  ["startup", [["wall_ms", "wall ms"], ["cpu_ms", "CPU ms"], ["max_rss_mb", "peak RSS MB"], ["instructions", "instructions"]]],
  ["oneshot", [["wall_ms", "wall ms"], ["spawn_to_request_ms", "spawn to request ms"], ["cpu_ms", "CPU ms"], ["max_rss_mb", "peak RSS MB"], ["request_bytes", "first request bytes"], ["side_requests", "side requests"]]],
  ["firstframe", [["first_frame_ms", "first frame ms"], ["cpu_ms", "CPU ms"], ["max_rss_mb", "peak RSS MB"]]],
  ["idle", [["idle_cpu_ms_per_second", "idle CPU ms per second"], ["max_rss_mb", "peak RSS MB"]]],
  ["disk", [["install_mb", "install MB"]]],
]) {
  const rows = suite.filter((row) => row.part === part);
  if (!rows.length) continue;
  console.log("\n## " + part + "\n");
  console.log(table(["lane", "samples", "errors", ...columns.map(([, label]) => label)], lanesOf(rows).map((lane) => {
    const mine = rows.filter((row) => row.harness === lane);
    const good = mine.filter((row) => !row.error && (row.code === undefined || row.code === 0));
    return [lane, String(good.length), String(mine.length - good.length), ...columns.map(([key]) => show(median(good.map((row) => row[key]))))];
  })));
}

for (const mode of ["cli", "tui"]) {
  const rows = files.filter((name) => name.startsWith("sweep-" + mode) && name.endsWith(".jsonl")).flatMap((name) => read(RESULTS + "/" + name));
  if (!rows.length) continue;
  const rates = [...new Set(rows.map((row) => row.rate))].sort((a, b) => a - b);
  const metrics = mode === "cli"
    ? [["lag_p50_ms", "per-token lag p50 (ms)"], ["lag_p99_ms", "per-token lag p99 (ms)"], ["tail_ms", "stream end to exit (ms)"], ["cpu_ms", "CPU, whole run (ms)"], ["cpu_us_per_token", "CPU per token (µs)"], ["max_rss_mb", "peak RSS (MB)"], ["missing", "tokens never seen on stdout"]]
    : [["lag_p50_ms", "on-screen lag p50 (ms)"], ["lag_p99_ms", "on-screen lag p99 (ms)"], ["display_done_after_server_ms", "stream end to last token on screen (ms)"], ["send_to_request_ms", "Enter to request (ms)"], ["cpu_ms_total", "CPU, whole session (ms)"], ["max_rss_mb", "peak RSS (MB)"], ["missing", "tokens never seen on screen"]];
  console.log("\n## streaming, " + (mode === "cli" ? "headless" : "terminal interface") + "\n");
  for (const [key, label] of metrics) {
    console.log("\n**" + label + "**\n");
    console.log(table(["lane", ...rates.map((rate) => rate + " tok/s")], lanesOf(rows).map((lane) => [lane, ...rates.map((rate) => { const mine = rows.filter((row) => row.harness === lane && row.rate === rate); const good = mine.filter((row) => !row.error); return good.length ? show(median(good.map((row) => row[key]))) + " (" + good.length + ")" : "failed (" + mine.length + ")"; })])));
  }
}
