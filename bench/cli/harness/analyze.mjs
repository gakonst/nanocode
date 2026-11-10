export function serverTimes(rec, n) {
  const t0 = BigInt(rec.t0_ns);
  const out = new Float64Array(n);
  let j = 0;
  const sched = rec.schedule ?? [];
  for (let i = 0; i < n; i++) {
    while (j < sched.length && sched[j][1] <= i) j++;
    out[i] = j < sched.length ? Number(t0) / 1e6 + sched[j][0] : NaN;
  }
  return out;
}

export function clientTimes(marks, n) {
  const out = new Float64Array(n).fill(NaN);
  let i = 0;
  for (const [t, max] of marks) {
    const tm = Number(BigInt(t)) / 1e6;
    while (i <= max && i < n) out[i++] = tm;
  }
  return out;
}

export function q(values, f) {
  const v = values.filter(Number.isFinite).sort((a, b) => a - b);
  if (!v.length) return null;
  return v[Math.min(v.length - 1, Math.max(0, Math.ceil(v.length * f) - 1))];
}

export function streamMetrics(rec, marks, n, extra = {}) {
  const s = serverTimes(rec, n);
  const c = clientTimes(marks, n);
  const lags = [];
  let missing = 0;
  for (let i = 0; i < n; i++) {
    if (Number.isFinite(c[i]) && Number.isFinite(s[i])) lags.push(c[i] - s[i]);
    else missing++;
  }
  const gaps = [];
  for (let i = 1; i < marks.length; i++) gaps.push(Number(BigInt(marks[i][0]) - BigInt(marks[i - 1][0])) / 1e6);
  const lastClient = c[n - 1];
  const firstClient = c[0];
  const out = {
    tokens: n,
    missing,
    ttft_overhead_ms: Number.isFinite(firstClient) ? firstClient - s[0] : null,
    lag_p50_ms: q(lags, 0.5), lag_p90_ms: q(lags, 0.9), lag_p99_ms: q(lags, 0.99), lag_max_ms: lags.length ? Math.max(...lags) : null,
    lag_mean_ms: lags.length ? lags.reduce((a, b) => a + b, 0) / lags.length : null,
    last_token_lag_ms: Number.isFinite(lastClient) ? lastClient - s[n - 1] : null,
    observed_tok_s: Number.isFinite(lastClient) && lastClient > firstClient ? (n - 1) / ((lastClient - firstClient) / 1000) : null,
    server_tok_s: rec.schedule?.length ? (n - 1) / ((s[n - 1] - s[0]) / 1000) : null,
    updates: marks.length,
    max_update_gap_ms: gaps.length ? Math.max(...gaps) : null,
    server_drains: rec.drains,
    server_bytes_out: rec.bytes_out,
    request_bytes: rec.request_bytes,
    ...extra,
  };
  return out;
}

export const msBetween = (a, b) => (a == null || b == null ? null : Number(BigInt(b) - BigInt(a)) / 1e6);
