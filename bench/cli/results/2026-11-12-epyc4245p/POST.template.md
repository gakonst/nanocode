# Seven agent CLIs on identical servers, 2026-11-12

*Draft for nanocode.xyz/experiments. Not published.*

We ran seven terminal coding agents through the same scripted work on AMD EPYC 4245P bare-metal servers (6 cores, 12 threads, 46 GB, Ubuntu 24.04, kernel 6.8) on 2026-11-12, between 03:11 and 05:22 UTC. The CLIs are nc (nanocode, master 3c16e6c8e, dist profile), nc-lite (the same SDK trimmed to what fx ships, same commit, lite profile), Vercel fx 0.0.16, Pi 1.5.2, OpenCode 1.19.5, Claude Code 2.1.322 and Codex CLI 0.167.2. No lane talks to a real model: each one points at a local fixture that streams a fixed reply, runs with its own empty HOME, and runs inside a network namespace where only loopback works. Lanes take turns in every round. Every number is a median with the sample count in parentheses, and CPU and memory cover the CLI's whole process tree. The terminal runs use a 120x40 PTY. What reached the screen is replayed through a VT emulator, so lag means time until the token is visible. The raw JSONL, the commands and the generated [REPORT.md](REPORT.md) are next to this file.

Four servers of the same model ran the work in parallel: one ran the startup, first-frame, idle, keystroke, one-shot and tool-turn suites; one ran netaudit, the cold-cache rows, fleets and install size; two ran the streaming sweeps (headless and 50,000 tok/s; 120x40 and 240x70). The session store ran on a sweep server after its sweeps. [machine.json](machine.json) records each one.

nc releases are not built like the nc lane. Since 7837aae16 they go through scripts/build-cli.sh: the dist profile without position-independent code, then laid out by BOLT from a training run. A fifth server of the same model ran those builds of nc and nc-lite at 7837aae16, with fx as a control, through the same suites ([2026-11-12-epyc4245p-lite3](../2026-11-12-epyc4245p-lite3/REPORT.md), cold rows in [-lite3-cold](../2026-11-12-epyc4245p-lite3-cold/REPORT.md)). Their rows are nc-bolt and nclite-bolt below. fx there was within 8% of fx here on wall-clock medians (first frame 1.86 against 2.01 ms, one-shot 22.9 against 23.2 ms), and its CPU medians were up to 15% lower (one-shot 1.62 against 1.87 ms, cold start 4.74 against 5.32 ms), so read CPU differences between nc-bolt and the other lanes with that margin. The BOLT nanocode binary is 82.1 MB and nc-lite's 19.1 MB (that run's disk step failed; sizes from its VERSIONS.md). That run's streaming sweeps took 6 samples, fewer than the 10 we require, so the streaming tables have no BOLT rows.

## Startup and first frame

{{Startup: --version}}

{{Cold start: --version after dropping the page cache}}

{{Time to first frame}}

{{Time to first frame, cold page cache}}

{{Memory one second after the first frame}}

## Headless runs

{{One-shot headless prompt}}

{{Long tool turn: 50 shell steps, headless}}

{{Fleet: 32 concurrent headless runs, 1,000 tokens each at 1,000 tok/s}}

## Streaming

{{Streaming, headless}}

{{Streaming, terminal interface 120x40}}

## Idle, keys, disk and network

{{Idle terminal interface}}

{{Keystroke echo}}

{{Install size}}

{{Outbound connection attempts (network blocked)}}

{{STORE}}

## The three largest differences

**Runtime startup.** `pi --version` takes 101 ms and 126 ms of CPU, and `opencode --version` 201 ms and 267 ms. The native binaries (nc, nc-lite, fx) take 2.7 ms, the same as `/usr/bin/true` under this harness. Printing a version does no work of its own, so this gap is startup cost alone. Pi runs on Node (VERSIONS.md), and OpenCode ships a 204 MB executable. The same cost shows again in every run: OpenCode takes 1,376 ms and 3.4 s of CPU to its first frame, and 997 ms for a one-word headless reply. We did not profile inside either runtime.

**Codex waits five seconds after a headless reply.** Its one-shot run takes 5,045 ms, and 4,993 ms of that comes after the last byte of the reply. Its fleet and long-turn rows have the same 5 s floor. In the same runs, netaudit records four blocked connection attempts from headless Codex, to github.com, api.github.com and chatgpt.com. That matches the curated-plugin sync its lane note describes, which this setup cannot reach. The data shows the wait and the attempts. We did not trace whether Codex waits on those connections.

**When tokens reach the screen.** At 100 tok/s, fx shows a token 200 ms after the fixture sends it (p99 403 ms), and Claude Code after 151 ms (p99 331 ms). nc shows it after 0.081 ms and nc-lite after 0.018 ms. At 100 tok/s fx's lag is spread evenly from 0 to about 400 ms, which is what a redraw every 400 ms or so would give, and it falls as the rate rises (21.6 ms at 10,000 tok/s). We did not profile fx. nc pays for its low lag in bytes: it writes 190 terminal bytes per token at 100 tok/s against fx's 39.5 and nc-lite's 5.43, so the extra bytes come from how nc's interface repaints, not from the SDK under it. At 10,000 tok/s and above Claude Code writes the fewest bytes per token.

## Where nc is behind

nc is the heaviest of the native binaries. As released (nc-bolt) it holds 19.0 MB resident one second after the first frame against fx's 8.91 MB, needs 4.93 ms of CPU for a one-word reply (fx: 1.62 ms), and starts `--version` from a cold page cache in 8.84 ms (fx: 4.74 ms). The plain dist build is heavier still: 30.9 MB, 7.36 ms and 11.1 ms. A profile of that build's TUI on the same machine (smaps, massif) attributes its memory as follows. About 25 MB is resident pages of the executable's code; Linux maps code a folio at a time, so this counts every page near code that ran, which is what BOLT's layout shrinks. About 9 MB is read-only data, of which about 6 MB is relocation tables and data a position-independent executable fixes up at load, which the release build no longer has. The allocator's heap is about 7 MB for under 2 MB of live allocations. About 6 MB, and most of a one-shot run's remaining instructions, belong to the built-in Tempo tools: a payment proxy thread that loads and re-encodes the system's trusted roots at start, and their MCP clients. Those tools also make four outbound connection attempts at every start (netaudit), which fx and nc-lite do not make.

## What changed in nc today

Three changes landed on nanocode master on 2026-11-12. Each column adds one change to the column before it. All four builds ran interleaved in the same rounds (release builds, same server model, 30 rounds):

| metric | before (fe57c2ff3) | allocator leaves huge pages alone | trust roots read at first TLS check | syntaxes load at first turn |
|---|---:|---:|---:|---:|
| first frame ms | 7.25 | 5.76 | 4.02 | 3.86 |
| first-frame CPU ms | 29.2 | 12.9 | 11.4 | 10.1 |
| RSS 1 s after the frame MB | 63.5 | 41.9 | 41.1 | 40.2 |
| anonymous MB | 29.6 | 8.12 | 7.49 | 7.01 |
| one-shot wall ms | 11.0 | 11.0 | 8.96 | 8.92 |
| one-shot CPU ms | 12.7 | 10.7 | 9.16 | 9.06 |
| spawn to request ms | 8.22 | 7.51 | 5.85 | 5.72 |

These are release-profile builds, so their absolute numbers are higher than the dist and BOLT rows above.

## What we could not measure

- Session store for OpenCode and Codex. Each headless OpenCode run takes about 1 s and each Codex run about 5 s, so writing 1,000 sessions would have taken 17 and 83 minutes, more than the servers had left.
- Listing sessions for Pi and Claude Code: harness/lanes.mjs defines a listing command only for nc, nc-lite and fx.
- Headless per-token lag for OpenCode and Codex. Neither prints the reply until it is complete (about 2.5 s median lag), so their per-token rows measure that wait, not streaming.
- Memory after the first frame for OpenCode: the smaps sample one second after its first frame came back empty in every round. Its peak RSS from rusage (about 960 MB) is in the first-frame table.
- Instructions retired: on Linux the harness falls back to tools/runstat.py, which reports no instruction counts, so that column says n/a.
- Pi's main binary size: its package entry point is a JavaScript bundle, and the Node runtime it needs is not counted.

