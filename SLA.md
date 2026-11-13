# Latency SLA

What nanocode adds between a person and a model, measured, with the bound each release keeps. Nanocode's overhead is the time it spends that is not the provider's: from Enter to the request reaching the provider, plus from a token arriving to it being on screen. The harness is nanocode-ts `bench/sla` (a local streaming mock of the Anthropic Messages and OpenAI Responses APIs, and the live providers); its reports go to nanocode-ts `output/bench/sla`.

## Measures

| Measure | From | To |
|---|---|---|
| E2E first token | Enter written to the terminal | the first token of the reply drawn on screen |
| Overhead | E2E minus the provider's own time | Enter to request at provider, plus token sent to token drawn |
| Request out | Enter | request headers at the provider |
| Render | token sent by the provider | token drawn |
| Rendered tok/s | first token drawn | last token drawn, against the provider's own rate |
| Tail | provider's last token sent | last token drawn |
| First tool call | provider ends a tool call | the request carrying its result |
| Code Mode cell | provider ends an exec call | the next request (one host tool inside the cell) |
| Ready | process spawn | composer accepts input |
| Resume after crash | spawn after `kill -9` mid-stream | transcript restored on screen, and the resumed request at the provider |

## Targets

p50 / p99 on the mock, one Linux x86_64 host, per surface. A change that moves a measure past its bound needs a DECISIONS.md entry that says why.

| Measure | native CLI | nanocode-ts Node | nanocode-ts Workers/DO |
|---|---|---|---|
| Overhead, warm turn | 5 / 10 ms | 5 / 10 ms | 20 / 60 ms in local workerd |
| Overhead, first turn of a session | 10 / 20 ms | 10 / 80 ms | 130 / 220 ms cold isolate in local workerd |
| Render (token at provider to token drawn, or to the `?live=1` client) | 2 / 5 ms | n/a (no renderer) | 5 / 10 ms |
| Rendered tok/s | equal to the provider's up to 100k tok/s | n/a | equal to the provider's (deltas at most once per `EVENT_COALESCE_MS`) |
| Tail | 10 / 20 ms | n/a | 10 / 40 ms warm |
| First tool call / Code Mode cell | 20 / 40 ms | 20 / 40 ms | 40 / 80 ms (est., not measured yet) |
| Ready | 30 / 60 ms | n/a | n/a |
| Resume after crash, restored | 50 / 100 ms | n/a | n/a |
| New connection on a turn after the first | none | none | none |
| First request's connection opened | before Enter | before dispatch | before dispatch |

Live providers add their own time; nanocode's share is the same overhead, and the connection to the provider is opened when the session starts (and again when typing starts), so a first request pays no DNS, TCP or TLS.

## Measured

2026-11-13, nanocode with the connection changes of that day (the mock lanes on a build of them over 1616c2d47, the live lanes on 5f299dbce), dev-georgios (AMD EPYC 4585PX, 32 cores, load around 45 from fuzzers), PTY 120x40, mock replying at once with 200 tokens sent as fast as it can, 10 samples of 3 turns per lane (6 single-turn samples for the tool call). Claude Code 2.1.318, Codex CLI 0.166.0 against the same mock.

| p50 / p99 ms | nanocode (Responses) | nanocode (Claude) | Claude Code | Codex CLI |
|---|---|---|---|---|
| Overhead, turn 1 | 7.30 / 10.27 | 3.78 / 10.78 | 190.2 / 232.4 | 73.1 / 246.5 |
| Overhead, turn 2 | 3.02 / 7.38 | 3.55 / 5.29 | 84.7 / 130.9 | 55.4 / 67.9 |
| Overhead, turn 3 | 3.61 / 9.09 | 4.22 / 7.31 | 83.2 / 134.0 | 52.8 / 81.2 |
| Render | 1.5 | 1.5 | 58 | 11 |
| First tool call | 15.19 / 23.92 (Code Mode) | 11.45 / 14.11 (Code Mode) | 97.8 / 202.8 (Bash) | 94.7 / 114.7 |
| Ready | 15 | 15 | 451 | 819 |
| `exec` / `-p`: spawn to first token | 9.22 / 16.10 | 9.76 / 11.23 | 265.6 / 319.7 | |

Throughput: 20,000 tokens unpaced, the mock sent 535,069 tok/s and nanocode drew 458,619 tok/s with a 7.0 ms tail (Claude Code tail 333 ms; Codex drew about 1,168 tok/s, tail 17.0 s). With 5,000 tokens at 1,000 tok/s nanocode draws 1,000 tok/s, render 0.31 ms, tail 2.1 ms (Claude Code 38.9 ms, Codex 22.6 ms).

Resume after `kill -9` mid-stream: ready 10.5 ms, the resumed request at the provider 14.9 ms, the restored transcript on screen 16.6 ms (1 sample).

nanocode-ts on Node 24, AgentRuntime with the Claude provider, overhead from `run` to the request at the provider plus the first event: turn 1 3.73 / 70.12 ms (the first runtime in a process compiles the wasm module and opens undici's first connection, about 28 and 29 ms), turn 2 2.86 / 5.70, turn 3 2.70 / 3.90.

Connections: before 2026-11-13 every model request opened a new connection (36 of 36 turns on the mock) and the first one opened it on the request path; now turns 2 and 3 reuse the first (0 of 24 new) and the first request arrives on a connection opened at session start, about 1.7 s earlier. Claude Code and Codex CLI open theirs on the request path (0.2 ms old on arrival). Connection setup from the same host: api.anthropic.com TLS done in 24.6 ms, api.openai.com 24.2 ms, chatgpt.com 43.7 ms, ai-gateway.vercel.sh 224 ms (201 ms of it DNS), which is what a first turn saved.

Live, from the same host (Enter to the first token drawn, so mostly the model's own time; p50 with min and max, ChatGPT login, gpt-6-luna, a one-word reply): nanocode turn 1 1,330 ms (1,071 to 1,393, 8 sessions), turn 2 1,272 (989 to 3,885), turn 3 1,332 (870 to 2,939); Codex CLI turn 1 2,204 ms (1,739 to 2,340, 4 sessions), turn 2 1,562, turn 3 1,945. Ready 8.3 ms against Codex CLI's 452 ms. Against real endpoints that refuse the key at once (a 401 is the provider's fastest answer, so Enter to the error drawn is nanocode's path plus one round trip), 6 sessions, before and after the connection changes: Vercel AI Gateway turn 1 187.9 to 38.7 ms p50 (the DNS, TCP and TLS of a first request moved to session start), turns 2 and 3 about 37 ms both; api.anthropic.com 148 to 161 ms on every turn before and after, which is the endpoint's own time to refuse.

First turn against later ones, bench/sla, Enter to request at the mock, p50 (2026-11-13). Responses: 3.34 ms at 5f299dbce, 2.33 after the first request's runtime, factory and system text were warmed at preconnect (12 sessions), and 2.59 to 2.04 ms once a tool-catalog revision lets the first turn reuse the runtime and factory built at preconnect (3954a7a46, 16 sessions; turns 2 and 3 1.14 to 1.44 ms). Claude: 1.82 to 1.70 ms (turn 2 1.19). The rest of the Responses gap, about 0.6 to 0.9 ms, is the first context capture, user input preparation and first history serialization.

nanocode-ts on Workers and Durable Objects, durable-cloudflare `scripts/measure-sla.mjs` in Miniflare's workerd with the provider in the Node process (hosted Hand, 5 cores, 6 fresh isolates of 3 turns, `EVENT_COALESCE_MS` 100), p50, Claude and Responses: 2,000 tokens at 1,000 tok/s, POST to the provider's request 11 to 15 ms warm and 93 to 128 ms cold (opening the runtime 43 ms and the first wasm entry 67 ms of it); provider's first token to the client's first delta 1.2 to 2.6 ms warm and 3.8 to 5.5 ms cold, where it was 96 to 101 ms on every turn before nanocode 61dfe4e8e stopped holding a reply's first delta for the coalescing window; POST to first delta 10 to 20 ms warm (111 to 118 before) and 98 to 121 ms cold (202 to 298 before); tail 1.9 to 3.2 ms on Responses (6.3 to 8.2 before) and 5.8 to 7.0 ms on Claude; the client sees 1,000 tok/s. 200 tokens unpaced: first delta 12 to 21 ms warm, 111 to 136 ms cold.

Live Claude, claude-haiku-4-5 with an API key from the hosted Hand, whose egress goes through a TLS-terminating proxy (DNS 0.2 to 0.4 s and TLS 0.24 to 0.27 s per new connection), 8 sessions of 3 turns each, Enter to the first token drawn, p50 (min to max): nanocode 477 ms (446 to 568), 439, 446; the same without that day's connection changes 875, 1,071, 848; Claude Code 2.1.318 1,844 ms (1,427 to 2,416), 813, 799. Ready 45.5 ms against Claude Code's 1,132.

Live Claude 5.5 models, same host, proxy and method, 6 sessions of 3 turns each, Enter to the first token drawn, p50 (min to max) for turns 1, 2 and 3:

| model | nanocode (5f299dbce) | Claude Code 2.1.318 |
|---|---|---|
| claude-haiku-5-5 | 695 (639 to 863), 1,317 (732 to 1,994), 1,183 | 1,704, 1,077, 1,079 |
| claude-sonnet-5-5 | 807 (697 to 1,401), 1,595 (670 to 1,781), 823 | 2,286, 1,538, 1,482 |
| claude-opus-5-5 | 1,639 (1,436 to 3,232), 2,233 (1,403 to 5,691), 1,478 | 2,633, 1,748, 1,739 |

nanocode is ahead on every first turn (by 1.0 to 1.5 s) and on turn 3 for Sonnet and Opus, and behind on turn 2 for all three models and on turn 3 for Haiku, with a wide spread (nanocode's turn-2 minimum is below Claude Code's p50); six sessions do not settle whether that is the egress proxy or nanocode. Ready: nanocode 36 to 43 ms, Claude Code 1,376 to 1,518 ms.

The deployed Worker, nanocode-cloudflare-durable on tempo-dev.workers.dev (durable-cloudflare 7c43bb3, nanocode-ts build-1a597dd, version b1bdc8aa, deployed 2026-11-13), measured from gak-9 (M1 Max, home network, no proxy) with a `?live=1` turn stream, a new session per round (a cold Durable Object), 2 rounds of 3 turns per provider, demo models (claude-haiku-5-5; the demo's Responses model, priority tier), a 150-number reply. Claude: POST to response headers 196 to 270 ms on warm turns (one outlier 1,079), first delta 738 to 1,574 ms, of which the provider's own response start through the AI Gateway (`host.http_start`) is 566 to 774 ms; cold turns 1,238 to 2,505 ms to headers and 2,794 to 4,160 ms to the first delta, with opening the SDK 337 to 398 ms and the first provider request 1,630 to 1,798 ms. Responses: warm first delta 1,435 to 1,695 ms (provider start 664 to 958 ms), cold 3,281 to 5,081 ms (provider start 2,227 to 3,469, SDK open 396 to 1,043). The client draws 575 to 701 characters/s on Claude and 113 to 402 on Responses, the provider's own rate. The cold turn's largest share is the first provider request from a new isolate, 1 s or more above a warm one: a connection to the gateway that a new Durable Object opens on its first request, which opening it during the SDK open would hide.

## Not measured yet

- The deployed Worker's cold first provider request (open the gateway connection while the SDK opens), more deployed samples, and on Workers: tool calls and Code Mode cells on Workers.
- DeepSeek and GLM live (reachable through `NANOCODE_ANTHROPIC_BASE_URL` with their Anthropic-compatible endpoints), and live Claude from a host without an egress proxy.
- Request compression on a blocking thread.

## Running it

From nanocode-ts, with `nanocode`, `claude` and `codex` on PATH or in `NC_BIN`, `CLAUDE_BIN`, `CODEX_BIN` (another build as `nc@NAME` with `NC_BIN_NAME`):

```
node bench/sla/run.ts --lanes=nc,nc-claude,claude,codex --samples=10 --turns=3 --label=cmp
node bench/sla/run.ts --lanes=nc,claude,codex --turns=1 --tool=1 --tokens=50 --label=tool
node bench/sla/run.ts --lanes=nc,claude,codex --turns=1 --tokens=20000 --timeout=60000 --label=throughput
node bench/sla/run.ts --lanes=nc,claude,codex --turns=1 --tokens=5000 --rate=1000 --timeout=60000 --label=paced
node bench/sla/run.ts --lanes=nc,nc-claude,claude --mode=exec --label=exec
node bench/sla/run.ts --lanes=nc --mode=resume --label=resume
node bench/sla/run.ts --live --lanes=live-nc-codex,live-codex --turns=3 --settle=3000 --timeout=30000 --label=live
node bench/sla/run.ts --live --rejected --lanes=rejected-nc-claude,rejected-nc-gateway,rejected-claude --turns=3 --settle=3000 --label=rejected
node bench/sla/run.ts --live --rejected --key --lanes=rejected-nc-claude,rejected-claude --turns=3 --settle=3000 --timeout=30000 --label=live-claude
node bench/sla/ts.ts --samples=10 --turns=3
```

From durable-cloudflare, after `npm run dry-run`:

```
node scripts/measure-sla.mjs --providers=claude,codex --rounds=6 --turns=3 --tokens=2000 --rate=1000 --label=paced
```
