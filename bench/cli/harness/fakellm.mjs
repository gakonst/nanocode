import http from "node:http";
import fs from "node:fs";
import { WebSocketServer } from "ws";

const now = () => process.hrtime.bigint();
const port = Number(process.env.PORT ?? 0);
const dumpDir = process.env.DUMP ?? null;
const runs = new Map();
let requests = 0;

export function token(i) {
  return "w" + i + ((i + 1) % 40 === 0 ? "\n\n" : " ");
}

function scenario(body) {
  const matches = [...body.matchAll(/BENCH\[([A-Za-z0-9_=,.\-~\/]+)\]/g)];
  const out = { mode: "hello", run: "default" };
  if (!matches.length) return out;
  for (const pair of matches.at(-1)[1].split(",")) {
    const [k, v] = pair.split("=");
    out[k] = v ?? "1";
  }
  return out;
}

function runState(id) {
  let state = runs.get(id);
  if (!state) {
    state = { requests: [], step: 0 };
    runs.set(id, state);
  }
  return state;
}

function protocolOf(path) {
  if (path.endsWith("/responses")) return "responses";
  if (path.endsWith("/chat/completions")) return "chat";
  if (path.endsWith("/messages")) return "messages";
  if (path.endsWith("/chat")) return "fx";
  return null;
}

function encoder(protocol, sink) {
  let seq = 0;
  requests += 1;
  const rid = "resp_" + requests;
  const msg = "msg_" + requests;
  const usage = { input_tokens: 10, input_tokens_details: { cached_tokens: 0 }, output_tokens: 1, output_tokens_details: { reasoning_tokens: 0 }, total_tokens: 11 };
  const chunk = (delta, finish = null) => ({ id: rid, object: "chat.completion.chunk", created: 1, model: "bench", choices: [{ index: 0, delta, finish_reason: finish }] });
  const usageChunk = () => ({ id: rid, object: "chat.completion.chunk", created: 1, model: "bench", choices: [], usage: { prompt_tokens: 10, completion_tokens: 1, total_tokens: 11 } });
  const fxFinish = (reason) => ["finish", { type: "finish", finishReason: { unified: reason, raw: reason }, usage: { inputTokens: { total: 10 }, outputTokens: { total: 1 } } }];
  return {
    begin(kind) {
      if (protocol === "responses") {
        const out = [{ type: "response.created", response: { id: rid, status: "in_progress", output: [] } }];
        if (kind === "text") {
          out.push({ type: "response.output_item.added", output_index: 0, item: { type: "message", id: msg, role: "assistant", status: "in_progress", content: [] } });
          out.push({ type: "response.content_part.added", item_id: msg, output_index: 0, content_index: 0, part: { type: "output_text", text: "", annotations: [] } });
        }
        return sink.frames(out.map((e) => [e.type, e]));
      }
      if (protocol === "messages") {
        const out = [["message_start", { type: "message_start", message: { id: msg, type: "message", role: "assistant", model: "bench", content: [], stop_reason: null, stop_sequence: null, usage: { input_tokens: 10, output_tokens: 0 } } }]];
        if (kind === "text") out.push(["content_block_start", { type: "content_block_start", index: 0, content_block: { type: "text", text: "" } }]);
        return sink.frames(out);
      }
      if (protocol === "chat") return sink.frames([["chunk", chunk({ role: "assistant", content: "" })]]);
      if (protocol === "fx" && kind === "text") return sink.frames([["text-start", { type: "text-start", id: "0" }]]);
      return null;
    },
    deltas(texts) {
      const frames = texts.map((text) => {
        if (protocol === "responses") return ["response.output_text.delta", { type: "response.output_text.delta", item_id: msg, output_index: 0, content_index: 0, delta: text, sequence_number: ++seq }];
        if (protocol === "messages") return ["content_block_delta", { type: "content_block_delta", index: 0, delta: { type: "text_delta", text } }];
        if (protocol === "chat") return ["chunk", chunk({ content: text })];
        return ["text-delta", { type: "text-delta", id: "0", delta: text }];
      });
      return sink.frames(frames);
    },
    endText(full) {
      if (protocol === "responses") {
        const item = { type: "message", id: msg, role: "assistant", status: "completed", content: [{ type: "output_text", text: full, annotations: [] }] };
        return sink.frames([
          ["response.output_text.done", { type: "response.output_text.done", item_id: msg, output_index: 0, content_index: 0, text: full }],
          ["response.content_part.done", { type: "response.content_part.done", item_id: msg, output_index: 0, content_index: 0, part: item.content[0] }],
          ["response.output_item.done", { type: "response.output_item.done", output_index: 0, item }],
          ["response.completed", { type: "response.completed", response: { id: rid, status: "completed", output: [item], usage } }],
        ], true);
      }
      if (protocol === "messages") {
        return sink.frames([
          ["content_block_stop", { type: "content_block_stop", index: 0 }],
          ["message_delta", { type: "message_delta", delta: { stop_reason: "end_turn", stop_sequence: null }, usage: { output_tokens: 1 } }],
          ["message_stop", { type: "message_stop" }],
        ], true);
      }
      if (protocol === "chat") return sink.frames([["chunk", chunk({}, "stop")], ["chunk", usageChunk()]], true, "data: [DONE]\n\n");
      return sink.frames([["text-end", { type: "text-end", id: "0" }], fxFinish("stop")], true, "data: [DONE]\n\n");
    },
    toolCalls(calls) {
      if (protocol === "responses") {
        const items = calls.map((call) => ({ type: "function_call", id: "fc_" + call.id, call_id: call.id, name: call.name, arguments: JSON.stringify(call.input), status: "completed" }));
        const frames = [];
        items.forEach((item, i) => {
          frames.push(["response.output_item.added", { type: "response.output_item.added", output_index: i, item: { ...item, arguments: "", status: "in_progress" } }]);
          frames.push(["response.function_call_arguments.delta", { type: "response.function_call_arguments.delta", item_id: item.id, output_index: i, delta: item.arguments }]);
          frames.push(["response.output_item.done", { type: "response.output_item.done", output_index: i, item }]);
        });
        frames.push(["response.completed", { type: "response.completed", response: { id: rid, status: "completed", output: items, usage } }]);
        return sink.frames(frames, true);
      }
      return sink.frames([...calls.map((call) => ["tool-call", { type: "tool-call", toolCallId: call.id, toolName: call.name, input: call.input }]), fxFinish("tool-calls")], true, "data: [DONE]\n\n");
    },
    toolCall(call) {
      const id = call.id;
      if (protocol === "responses") {
        const item = call.custom
          ? { type: "custom_tool_call", id: "ctc_" + id, call_id: id, name: call.name, input: call.input, status: "completed" }
          : { type: "function_call", id: "fc_" + id, call_id: id, name: call.name, arguments: JSON.stringify(call.input), status: "completed" };
        const added = call.custom ? { ...item, input: "", status: "in_progress" } : { ...item, arguments: "", status: "in_progress" };
        const delta = call.custom
          ? ["response.custom_tool_call_input.delta", { type: "response.custom_tool_call_input.delta", item_id: item.id, output_index: 0, delta: call.input }]
          : ["response.function_call_arguments.delta", { type: "response.function_call_arguments.delta", item_id: item.id, output_index: 0, delta: item.arguments }];
        return sink.frames([
          ["response.output_item.added", { type: "response.output_item.added", output_index: 0, item: added }],
          delta,
          ["response.output_item.done", { type: "response.output_item.done", output_index: 0, item }],
          ["response.completed", { type: "response.completed", response: { id: rid, status: "completed", output: [item], usage } }],
        ], true);
      }
      if (protocol === "messages") {
        return sink.frames([
          ["content_block_start", { type: "content_block_start", index: 0, content_block: { type: "tool_use", id, name: call.name, input: {} } }],
          ["content_block_delta", { type: "content_block_delta", index: 0, delta: { type: "input_json_delta", partial_json: JSON.stringify(call.input) } }],
          ["content_block_stop", { type: "content_block_stop", index: 0 }],
          ["message_delta", { type: "message_delta", delta: { stop_reason: "tool_use", stop_sequence: null }, usage: { output_tokens: 1 } }],
          ["message_stop", { type: "message_stop" }],
        ], true);
      }
      if (protocol === "chat") {
        return sink.frames([
          ["chunk", chunk({ tool_calls: [{ index: 0, id, type: "function", function: { name: call.name, arguments: JSON.stringify(call.input) } }] })],
          ["chunk", chunk({}, "tool_calls")],
          ["chunk", usageChunk()],
        ], true, "data: [DONE]\n\n");
      }
      return sink.frames([
        ["tool-call", { type: "tool-call", toolCallId: id, toolName: call.name, input: call.input }],
        fxFinish("tool-calls"),
      ], true, "data: [DONE]\n\n");
    },
  };
}

function sseSink(res, protocol, record) {
  return {
    frames(frames, end = false, trailer = "") {
      let text = "";
      for (const [type, data] of frames) text += protocol === "fx" || protocol === "chat" ? "data: " + JSON.stringify(data) + "\n\n" : "event: " + type + "\ndata: " + JSON.stringify(data) + "\n\n";
      text += trailer;
      record.bytes_out += Buffer.byteLength(text);
      if (end) { res.end(text); return null; }
      if (res.write(text)) return null;
      return new Promise((resolve) => res.once("drain", () => { record.drains += 1; resolve(); }));
    },
    closed: () => res.destroyed || res.writableEnded,
  };
}

function wsSink(socket, record) {
  return {
    frames(frames) {
      for (const [, data] of frames) {
        const text = JSON.stringify(data);
        socket.send(text);
        record.bytes_out += Buffer.byteLength(text);
      }
      if (socket._socket && socket._socket.writableNeedDrain) return new Promise((resolve) => socket._socket.once("drain", () => { record.drains += 1; resolve(); }));
      return null;
    },
    closed: () => socket.readyState !== 1,
  };
}

const sleepUntil = (targetNs) => new Promise((resolve) => {
  const tick = () => {
    const left = Number(targetNs - now()) / 1e6;
    if (left <= 0) return resolve();
    if (left >= 1.5) setTimeout(tick, Math.max(1, Math.floor(left - 0.5)));
    else setImmediate(tick);
  };
  tick();
});

function toolFor(harness, kind, step) {
  const id = "call_" + step;
  if (kind === "read") {
    if (harness === "fx") return { id, name: "read_file", input: { path: "evidence.txt" } };
    if (harness === "pi") return { id, name: "read", input: { path: "evidence.txt" } };
    if (harness === "nc") return { id, name: "exec", custom: true, input: "const r = await tools.exec_command({ cmd: 'cat evidence.txt' }); text(r.output ?? JSON.stringify(r))" };
  }
  if (kind === "shell") {
    if (harness === "fx") return { id, name: "shell", input: { request: { action: "run", command: "ls", yield_time_ms: 30000 } } };
    if (harness === "pi") return { id, name: "bash", input: { command: "ls" } };
    if (harness === "nc") return { id, name: "exec", custom: true, input: "const r = await tools.exec_command({ cmd: 'ls' }); text(r.output ?? JSON.stringify(r))" };
    if (harness === "ncdirect") return { id, name: "exec_command", input: { cmd: "ls" } };
  }
  if (kind === "bridge") {
    if (harness === "ncexec") return { id, name: "exec", custom: true, input: "text(JSON.stringify(await tools.bridge_echo({ value: '" + step + "' })))" };
    return { id, name: "bridge_echo", input: { value: String(step) } };
  }
  throw new Error("no tool for " + harness + " " + kind);
}

function sideRequest(body) {
  try {
    const parsed = JSON.parse(body);
    return { side: !(Array.isArray(parsed.tools) && parsed.tools.length > 0), stream: parsed.stream === true };
  } catch {
    return { side: false, stream: true };
  }
}

async function respond(protocol, body, sink, record) {
  const sc = scenario(body);
  if (record.side) {
    record.run = sc.run;
    record.mode = "side";
    runState(sc.run).requests.push(record);
    const enc = encoder(protocol, sink);
    record.first_token_ns = record.last_token_ns = now();
    await enc.begin("text");
    await enc.deltas(["ok"]);
    await enc.endText("ok");
    record.done_ns = now();
    return;
  }
  const state = runState(sc.run);
  record.run = sc.run;
  record.mode = sc.mode;
  record.step = state.step++;
  state.requests.push(record);
  const enc = encoder(protocol, sink);
  if (sc.mode === "steps" && record.step < Number(sc.steps)) {
    if (sc.kind === "read" && sc.file) fs.writeFileSync(sc.file.replaceAll("~", "/"), (record.step + 1) + " " + "x".repeat(Number(sc.bytes ?? 1024)) + "\n");
    record.first_token_ns = record.last_token_ns = now();
    await enc.begin("tool");
    await enc.toolCall(toolFor(sc.h, sc.kind, record.step + 1));
    record.done_ns = now();
    return;
  }
  if (sc.mode === "spawn" && record.step === 0) {
    record.first_token_ns = record.last_token_ns = now();
    await enc.begin("tool");
    const k = Number(sc.k ?? 5);
    await enc.toolCall({ id: "spawn_cell", name: "exec", custom: true, input: "const k = " + k + "; const spawned = await Promise.all(Array.from({ length: k }, (_, i) => tools.spawn_agent({ task: 'child ' + i + ' BENCH[run=" + sc.run + "-c' + i + ',mode=child]', harness: null, model: null, output_contract: { kind: 'string' }, reasoning_mode: null, role: 'worker', thinking: null }))); const ids = spawned.map((s) => s.agent_id); let done = 0; const terminal = ['completed', 'failed', 'interrupted', 'closed']; while (done < k) { const r = await tools.wait_agent({ agent_ids: ids, timeout_ms: 60000 }); done = r.agents.filter((a) => terminal.includes(a.status.state)).length; } text('children ' + done)" });
    record.done_ns = now();
    return;
  }
  if (sc.mode === "child" && record.step === 0) {
    record.first_token_ns = record.last_token_ns = now();
    await enc.begin("tool");
    await enc.toolCall({ id: "submit_cell", name: "exec", custom: true, input: "const r = await tools.submit_result({ output: 'ok' }); text(JSON.stringify(r))" });
    record.done_ns = now();
    return;
  }
  if (sc.mode === "parallel" && record.step === 0) {
    record.first_token_ns = record.last_token_ns = now();
    await enc.begin("tool");
    if (sc.h === "ncexec") await enc.toolCall({ id: "par_cell", name: "exec", custom: true, input: "const r = await Promise.all(Array.from({ length: " + Number(sc.k ?? 8) + " }, (_, i) => tools.bridge_sleep({ ms: " + Number(sc.ms ?? 50) + ", index: i }))); text(JSON.stringify(r))" });
    else await enc.toolCalls(Array.from({ length: Number(sc.k ?? 8) }, (_, i) => ({ id: "par_" + i, name: "bridge_sleep", input: { ms: Number(sc.ms ?? 50), index: i } })));
    record.done_ns = now();
    return;
  }
  if (sc.mode === "bridge" && record.step % 2 === 0) {
    record.first_token_ns = record.last_token_ns = now();
    await enc.begin("tool");
    await enc.toolCall(toolFor(sc.h, "bridge", record.step / 2));
    record.done_ns = now();
    return;
  }
  const texts = sc.mode === "stream" ? Array.from({ length: Number(sc.tokens) }, (_, i) => token(i)) : sc.mode === "blob" ? Array.from({ length: Number(sc.chunks) }, () => "x".repeat(Number(sc.bytes))) : [sc.mode === "steps" ? "LONG_TURN_COMPLETE" : sc.mode === "bridge" || sc.mode === "parallel" ? "ok" : sc.text ?? "hello"];
  const rate = sc.mode === "stream" ? Number(sc.rate ?? 0) : 0;
  await enc.begin("text");
  const t0 = now();
  record.t0_ns = t0;
  record.schedule = [];
  let sent = 0;
  while (sent < texts.length) {
    if (sink.closed()) { record.aborted = true; break; }
    const t = now();
    const due = rate > 0 ? Math.min(texts.length, Math.floor((Number(t - t0) / 1e9) * rate) + 1) : texts.length;
    if (due > sent) {
      record.first_token_ns ??= t;
      const wait = enc.deltas(texts.slice(sent, due));
      sent = due;
      record.schedule.push([Number(now() - t0) / 1e6, sent]);
      if (wait) await wait;
    }
    if (sent < texts.length) await sleepUntil(t0 + BigInt(Math.ceil((sent / rate) * 1e9)));
  }
  record.last_token_ns = now();
  if (!sink.closed()) await enc.endText(texts.join(""));
  record.done_ns = now();
}

const models = { object: "list", data: ["openai/gpt-5.5", "fake/model", "bench/model"].map((id) => ({ id, type: "language", tags: ["tool-use"], context_window: 1000000, max_tokens: 32000, released: 1 })) };
const big = (k, v) => (typeof v === "bigint" ? v.toString() : v);

const server = http.createServer((req, res) => {
  const arrive = now();
  const url = new URL(req.url, "http://x");
  if (url.pathname === "/_log") {
    const id = url.searchParams.get("run");
    const state = runs.get(id);
    const out = JSON.stringify(state ? state.requests : [], big);
    if (url.searchParams.has("drop")) runs.delete(id);
    res.writeHead(200, { "content-type": "application/json" });
    return res.end(out);
  }
  if (url.pathname === "/_quit") { res.end("bye"); setTimeout(() => process.exit(0), 10); return; }
  if (req.method === "GET") {
    req.resume();
    res.writeHead(200, { "content-type": "application/json" });
    return res.end(JSON.stringify(models));
  }
  const protocol = protocolOf(url.pathname);
  const chunks = [];
  req.on("data", (c) => chunks.push(c));
  req.on("end", async () => {
    const body = Buffer.concat(chunks).toString("utf8");
    if (dumpDir) fs.writeFileSync(dumpDir + "/" + Date.now() + "-" + requests + "-" + (protocol ?? "unknown") + ".json", url.pathname + "\n" + body);
    if (url.pathname.endsWith("/count_tokens")) {
      res.writeHead(200, { "content-type": "application/json" });
      return res.end(JSON.stringify({ input_tokens: 10 }));
    }
    if (!protocol) {
      res.writeHead(404);
      return res.end("unknown endpoint " + url.pathname);
    }
    const record = { protocol, transport: "http", path: url.pathname, arrive_ns: arrive, body_end_ns: now(), request_bytes: Buffer.byteLength(body), bytes_out: 0, drains: 0 };
    if (url.pathname.startsWith("/sa/")) {
      const kind = sideRequest(body);
      record.side = kind.side;
      if (kind.side && !kind.stream && protocol === "messages") {
        const sc = scenario(body);
        record.run = sc.run;
        record.mode = "side";
        record.done_ns = now();
        runState(sc.run).requests.push(record);
        res.writeHead(200, { "content-type": "application/json" });
        return res.end(JSON.stringify({ id: "msg_side", type: "message", role: "assistant", model: "bench", content: [{ type: "text", text: "ok" }], stop_reason: "end_turn", stop_sequence: null, usage: { input_tokens: 10, output_tokens: 1 } }));
      }
    }
    res.writeHead(200, { "content-type": "text/event-stream", "cache-control": "no-cache", "x-fake-request-id": String(requests + 1) });
    res.flushHeaders();
    record.headers_ns = now();
    try { await respond(protocol, body, sseSink(res, protocol, record), record); } catch (e) { record.error = String(e && e.stack || e); res.destroy(); }
  });
});
server.keepAliveTimeout = 120000;
const wss = new WebSocketServer({ noServer: true, perMessageDeflate: false });
server.on("upgrade", (req, socket, head) => {
  const url = new URL(req.url, "http://x");
  if (!url.pathname.endsWith("/responses") || process.env.NO_WS) {
    socket.end("HTTP/1.1 426 Upgrade Required\r\nconnection: close\r\ncontent-length: 0\r\n\r\n");
    return;
  }
  wss.handleUpgrade(req, socket, head, (ws) => {
    ws._socket.setNoDelay(true);
    let queue = Promise.resolve();
    ws.on("message", (data) => {
      const arrive = now();
      queue = queue.then(async () => {
        const body = data.toString("utf8");
        let parsed = null;
        try { parsed = JSON.parse(body); } catch {}
        if (dumpDir) fs.writeFileSync(dumpDir + "/" + Date.now() + "-" + requests + "-ws.json", url.pathname + "\n" + body);
        if (!parsed || parsed.type !== "response.create") return;
        const record = { protocol: "responses", transport: "ws", path: url.pathname, arrive_ns: arrive, body_end_ns: arrive, headers_ns: arrive, request_bytes: Buffer.byteLength(body), bytes_out: 0, drains: 0 };
        if (parsed.generate === false) {
          const enc = encoder("responses", wsSink(ws, record));
          ws.send(JSON.stringify({ type: "response.created", response: { id: "warm", status: "in_progress", output: [] } }));
          ws.send(JSON.stringify({ type: "response.completed", response: { id: "warm", status: "completed", output: [], usage: { input_tokens: 1, input_tokens_details: { cached_tokens: 0 }, output_tokens: 0, output_tokens_details: { reasoning_tokens: 0 }, total_tokens: 1 } } }));
          return;
        }
        try { await respond("responses", body, wsSink(ws, record), record); } catch (e) { record.error = String(e && e.stack || e); }
      });
    });
  });
});
server.listen(port, "127.0.0.1", () => {
  process.stdout.write(JSON.stringify({ port: server.address().port, pid: process.pid }) + "\n");
});
