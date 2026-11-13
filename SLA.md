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
| Overhead, warm turn | 5 / 10 ms | 5 / 10 ms | 10 / 25 ms (est., not measured yet) |
| Overhead, first turn of a session | 10 / 20 ms | 10 / 80 ms | cold start excluded |
| Render | 2 / 5 ms | n/a (no renderer) | n/a |
| Rendered tok/s | equal to the provider's up to 100k tok/s | n/a | n/a |
| Tail | 10 / 20 ms | n/a | n/a |
| First tool call / Code Mode cell | 20 / 40 ms | 20 / 40 ms | 40 / 80 ms (est.) |
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

## Not measured yet

- nanocode-ts on Workers and Durable Objects: no local workerd on the host; the deployed endpoint (`bench/targets/cloudflare.ts`) is the next lane.
- Live Claude (no Anthropic key or Claude login on the host), DeepSeek and GLM (reachable through `NANOCODE_ANTHROPIC_BASE_URL` with their Anthropic-compatible endpoints).
- The first turn's request build (about 4 to 7 ms against 1.5 ms on later turns: the environment context and tool catalog are built lazily on it), and request compression on a blocking thread.

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
node bench/sla/ts.ts --samples=10 --turns=3
```
