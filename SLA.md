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

Live Claude 5.5 models, same host and proxy, 6 sessions of 3 turns each, every session's prompts unique (the run's id in each), Enter to the first token drawn, p50 (min to max) for turns 1, 2 and 3, interleaved lanes (2026-11-13):

| model | nanocode 6638af866 | nanocode 5f299dbce | Claude Code 2.1.318 |
|---|---|---|---|
| claude-haiku-5-5 | 858 (686 to 1,383), 639 (564 to 984), 753 (561 to 789) | 666, 1,186 (525 to 3,557), 698 (475 to 1,986) | 1,969, 1,014, 982 |
| claude-sonnet-5-5 | 952 (817 to 1,372), 956 (861 to 1,106), 901 (803 to 1,037) | 1,313, 1,375 (850 to 2,651), 1,278 | 2,249, 1,740, 1,508 |
| claude-opus-5-5 | 1,148 (652 to 1,275), 1,045 (696 to 1,320), 1,074 (738 to 1,174) | 1,242, 1,158 (1,001 to 3,589), 1,478 | 2,222, 1,656, 1,644 |

nanocode 6638af866 is ahead of Claude Code on every turn of every model: by 1.1 to 1.3 s on the first turn and 0.2 to 0.8 s on later ones, and its later turns are as fast as its first. Before it, nanocode lost later turns at random because the Claude stream dropped each response before its chunked terminator arrived, so the connection closed and the next turn paid for a new one (0.5 to 1 s here); DECISIONS.md has the trace. Ready: nanocode 33 to 46 ms, Claude Code 1,257 to 1,316 ms.

The deployed Worker, nanocode-cloudflare-durable on tempo-dev.workers.dev (durable-cloudflare 7c43bb3, nanocode-ts build-1a597dd, version b1bdc8aa, deployed 2026-11-13), measured from gak-9 (M1 Max, home network, no proxy) with a `?live=1` turn stream, a new session per round (a cold Durable Object), 2 rounds of 3 turns per provider, demo models (claude-haiku-5-5; the demo's Responses model, priority tier), a 150-number reply. Claude: POST to response headers 196 to 270 ms on warm turns (one outlier 1,079), first delta 738 to 1,574 ms, of which the provider's own response start through the AI Gateway (`host.http_start`) is 566 to 774 ms; cold turns 1,238 to 2,505 ms to headers and 2,794 to 4,160 ms to the first delta, with opening the SDK 337 to 398 ms and the first provider request 1,630 to 1,798 ms. Responses: warm first delta 1,435 to 1,695 ms (provider start 664 to 958 ms), cold 3,281 to 5,081 ms (provider start 2,227 to 3,469, SDK open 396 to 1,043). The client draws 575 to 701 characters/s on Claude and 113 to 402 on Responses, the provider's own rate. The cold turn's largest share is the first provider request from a new isolate, 1 s or more above a warm one: a connection to the gateway that a new Durable Object opens on its first request, which opening it during the SDK open would hide.

The deployed Worker on nanocode-ts build-4ed9bef (nanocode 6638af866, durable-cloudflare version a7616016, 2026-11-13), from gak-9, Claude warm turns: response headers 171 to 343 ms, first delta 783 to 1,435 ms. A new session's first turn depends on whether the client warmed it. Unwarmed (the probe above, an API client), the model request waits for the session's two MCP servers' handshakes and starts 427 to 1,814 ms after admission (21 to 245 ms on later turns). Warmed with `POST /warm` 2 s before, as the web client does when someone starts typing, it starts 45 to 103 ms after admission, and its provider time (817 to 1,216 ms Claude, 616 to 1,211 Codex) matches later turns'. nanocode-ts DECISIONS.md has the per-request trace (`model_requests` in a turn's result timing). With the demo's MCP listings in Workers KV (durable-cloudflare f80216e), an unwarmed session reads them in 3 to 65 ms and its model request starts 146 to 534 ms after admission.

2026-11-14, live Codex from dev-georgios (ChatGPT login, nanocode master 2367c144e, Codex CLI 0.166.0, 8 sessions of 3 turns, every prompt unique, Enter to the first token drawn, p50 / p99 for turns 1, 2 and 3). Codex CLI 0.145 is refused gpt-6 models on a ChatGPT login, and nanocode does not know the gpt-5.6 models that 0.145 allows, so 0.166 is the comparison.

| model | nanocode | Codex CLI 0.166.0 |
|---|---|---|
| gpt-6-luna | 1,022 / 1,655, 1,004 / 5,348, 1,085 / 4,005 | 1,500 / 4,234, 2,046 / 4,375, 1,301 / 1,750 |
| gpt-6.1-sol | 1,588 / 2,348, 1,583 / 4,765, 1,772 / 2,777 | 1,754 / 3,469, 1,516 / 1,652, 1,452 / 2,399 |

nanocode is ahead on every gpt-6-luna turn and on the first gpt-6.1-sol turn, and 70 to 320 ms behind Codex CLI on later gpt-6.1-sol turns at p50, which is the model's time (its p99 single outliers are 4 to 5 s on both clients). Ready: nanocode 33 / 72 ms, Codex CLI 108 / 253 ms to its composer prompt.

Throughput on the mock, same day and host, 6 samples: 20,000 tokens unpaced, nanocode draws 298,766 / 524,190 tok/s on Responses (provider 356,939 / 587,056) and 347,406 / 582,336 on Claude (provider 362,452 / 659,854), tail 11.7 / 39.3 and 5.3 / 8.0 ms; Codex CLI draws 920 / 1,347 tok/s with a 20.1 s tail and Claude Code draws the whole reply at the end (tail 365 / 604 ms). Paced at 1,000 tok/s (5,000 tokens): nanocode draws 1,000 tok/s on both providers, render 0.56 / 5.72 ms, tail 2.8 / 4.1 ms; Claude Code tail 57.5 / 182 ms, Codex CLI 24.2 / 65.8 ms.

The deployed Worker, 2026-11-14, from gak-9, a new session per round, 12 rounds of 3 turns per provider, all 72 turns completed, p50 / p99 ms. durable-cloudflare fbc9a2b with nanocode-ts build-3a5564a, whose responses carry `server-timing` from the Worker (`worker`), the app's dispatch to the session (`session_response`) and the session object (`session`):

| | Claude cold | Claude warm | Responses cold | Responses warm |
|---|---|---|---|---|
| POST to response headers | 1,081 / 2,141 | 193 / 313 | 1,038 / 2,559 | 219 / 393 |
| Worker | 986 / 1,923 | 92 / 201 | 948 / 2,035 | 93 / 158 |
| Object start (`session_response` − `session`) | 905 / 1,887 | 47 / 138 | 836 / 1,855 | 42 / 104 |
| Session to headers (admission) | 59 / 166 | 46 / 63 | 65 / 180 | 51 / 79 |
| Admission to model request | 442 / 1,129 | 46 / 63 | 175 / 1,171 | 51 / 79 |
| Admission to provider headers | 1,182 / 3,828 | 672 / 1,593 | 1,480 / 3,180 | 717 / 1,233 |
| POST to first delta | 2,787 / 6,038 | 803 / 1,781 | 3,033 / 7,435 | 1,146 / 1,913 |

A new session's extra second before response headers is the Durable Object starting (836 to 905 ms p50 against 42 to 47 warm), not session work: the object answers 59 to 65 ms after its `fetch` starts. The rest of the cold first delta is the first provider request from the new object, 0.5 to 0.8 s above a warm one at p50. Workers KV reads of the MCP listings now use `cacheTtl` (durable-cloudflare ae0aa1b). On version a9e3d78f the client drew a reply over 0.99 of the provider's own window on Claude (1.06 to 1.09 at p99, 571 to 637 characters/s) and 0.79 to 0.82 on Responses (the provider's window there includes events after the last text), so the Worker draws at the provider's rate.

2026-11-14, the full live matrix: 20 sessions of 3 turns per model and client, every prompt unique (the run's id in each), lanes interleaved, Enter to the first token drawn, p50 / p99 ms with n = 20 per cell unless noted (p99 of 20 is the slowest sample). GPT models from dev-georgios (ChatGPT login, Codex CLI 0.166.0, under `flock ~/sla/live.lock`, load 100 to 160 from other jobs); Codex CLI accepts all four on this login. Claude models from the hosted Hand (API key, Claude Code 2.1.318, TLS-terminating egress proxy). **Bold** marks a cell nanocode does not win.

| model | turn | nanocode | Codex CLI 0.166.0 |
|---|---|---|---|
| gpt-6-luna (e4779d26f) | 1 | 994 / 2,132 (n 19) | 1,474 / 2,893 |
| | 2 | 1,113 / **2,833** | 1,257 / 2,008 |
| | 3 | 936 / 1,734 | 1,199 / 1,991 |
| gpt-6.1-sol (9a9e5d038) | 1 | 1,795 / 3,352 | 2,486 / 10,622 |
| | 2 | 1,660 / 2,677 | 1,949 / 3,092 |
| | 3 | **1,798 / 3,398** | 1,705 / 2,870 |
| gpt-6-astra (9a9e5d038) | 1 | 1,524 / 6,822 | 2,439 / 7,063 |
| | 2 | 1,800 / 5,007 | 1,874 / 5,244 |
| | 3 | 1,631 / 3,667 | 1,680 / 4,128 |
| gpt-6.1-terra (9a9e5d038) | 1 | 1,283 / 3,823 | 1,808 / 4,560 |
| | 2 | 1,258 / 2,682 | 1,621 / 3,217 |
| | 3 | 1,175 / 2,165 (n 19) | 1,367 / 3,292 |

The two n = 19 cells each lost one sample to the model answering a different word, not to the client. Gaps where nanocode loses: gpt-6-luna turn 2 p99 +825 ms; gpt-6.1-sol turn 3 p50 +93 ms and p99 +528 ms. nanocode wins every gpt-6-astra and gpt-6.1-terra cell, and every turn-1 cell, by 480 to 910 ms at p50.

| model | turn | nanocode e4779d26f | Claude Code | nanocode 9a9e5d038 | Claude Code |
|---|---|---|---|---|---|
| claude-haiku-5-5 | 1 | 768 / **2,696** | 1,768 / 2,053 | 814 / **2,362** | 1,651 / 2,266 |
| | 2 | 695 / 1,657 | 1,146 / 1,659 | 732 / **2,334** | 1,049 / 1,991 |
| | 3 | 658 / 1,285 | 997 / 1,545 | 710 / 1,298 | 1,006 / 1,990 |
| claude-sonnet-5-5 | 1 | 695 / 1,345 | 1,968 / 2,300 | 1,356 / 1,543 | 2,068 / 2,503 |
| | 2 | 741 / 1,464 | 1,338 / 1,890 | 1,338 / **2,296** | 1,382 / 1,723 |
| | 3 | 775 / 1,480 | 1,314 / 1,636 | 1,356 / **1,693** | 1,386 / 1,651 |
| claude-opus-5-5 | 1 | 1,442 / 2,590 | 2,372 / 4,169 | 1,154 / 2,234 | 2,574 / 4,067 |
| | 2 | 1,243 / 1,749 | 1,666 / 6,645 | 1,155 / 1,791 | 1,649 / 3,981 |
| | 3 | 1,354 / **1,834** | 1,518 / 1,721 | 1,152 / **1,859** | 1,532 / 1,799 |

nanocode wins every Claude p50, by 0.7 to 1.4 s on turn 1 and 30 to 500 ms on later turns. It loses these p99 cells: haiku turn 1 +643 and +96 ms, haiku turn 2 +343 ms (9a9e5d038; e4779d26f wins it by 2 ms), sonnet turn 2 +573 ms, sonnet turn 3 +42 ms, opus turn 3 +113 and +60 ms. Each lost p99 is one slow sample out of 20 while Claude Code's slowest is lower in that cell; the nanocode outliers are 1.6 to 2.7 s against medians of 0.7 to 1.4 s. nanocode's sonnet median moved from 0.7 s to 1.35 s between the two runs (about 04:10 and 05:05 UTC) while Claude Code's did not; an interleaved A/B of the two builds in the later window (12 sessions each) gives e4779d26f 1,376 / 1,343 / 1,334 ms and 9a9e5d038 1,287 / 1,300 / 1,317 ms (p50, turns 1 to 3), so the move is the provider's time for nanocode's sonnet requests in that window, not the build. Request bodies of both builds are byte-identical in length (37,509 bytes) with the same adaptive thinking, high effort and betas.

## Not measured yet

- A new Worker session's Durable Object start (836 to 905 ms p50 before its `fetch` runs): how much is placement for a new name and how much is the isolate and module start, and whether a client warm before the first turn hides it. Tool calls and Code Mode cells on Workers.
- The Responses HTTP stream (gateways) may drop its connection the way the Claude one did; it was not traced live.
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
