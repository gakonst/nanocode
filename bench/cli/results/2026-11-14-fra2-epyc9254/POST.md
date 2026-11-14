# Seven agent CLIs, measured, 2026-11-14

*Draft for nanocode.xyz/experiments. Not published. Generated from the results' JSONL by `python3 harness/post.py --out results/2026-11-14-fra2-epyc9254/POST.md results/2026-11-14-fra2-epyc9254 results/2026-11-13-epyc4245p results/2026-11-13-epyc4245p-b results/2026-11-13-epyc4245p-c results/2026-11-14-dev-georgios-store14600` (run from bench/cli); edit harness/post.py, not this file.*

Seven terminal coding agents ran the same scripted work: nc (nanocode), nc-lite (the same SDK cut down to what fx ships), Vercel fx, Pi, OpenCode, Claude Code and Codex CLI. No lane talks to a real model: each points at a local fixture that streams a fixed reply, runs with its own empty HOME, inside a network namespace where only loopback works. The lanes take turns in every round. Every number is a median with the sample count in parentheses; CPU and memory cover the CLI's whole process tree. Lower is better in every row except the KB-printed context row.

## Where each number comes from

Each row below names its source. A row takes all of its lanes from one source, so numbers in a row are always from the same machine, run and nanocode commit.

| Source | Machine | When (UTC) | Load average (fleet rows aside, which load the box themselves) | nanocode commit | Data |
|---|---|---|---|---|---|
| A | AMD EPYC 9254 24-Core Processor, 48 threads, 377 GB, Ubuntu 24.04.5 LTS, kernel 7.0.0-43-generic, governor performance | 2026-11-14 06:50 to 08:21 | 0.1 to 2.2 | 41dcbc7c4 | [2026-11-14-fra2-epyc9254](./) |
| B | AMD EPYC 4245P 6-Core Processor, 12 threads, 46 GB, Ubuntu 24.04.5 LTS, kernel 6.8.0-149-generic, governor powersave | 2026-11-13 11:52 to 12:52 | 0.0 to 1.0 | 4be51211a | [2026-11-13-epyc4245p](../2026-11-13-epyc4245p/) |
| C | AMD EPYC 4245P 6-Core Processor, 12 threads, 46 GB, Ubuntu 24.04.5 LTS, kernel 6.8.0-149-generic, governor powersave | 2026-11-13 16:15 to 17:46 | 0.0 to 1.8 | 223cb39ba | [2026-11-13-epyc4245p-b](../2026-11-13-epyc4245p-b/) |
| D | AMD EPYC 4244P 6-Core Processor, 12 threads, 62 GB, Ubuntu 24.04.5 LTS, kernel 6.8.0-149-generic, governor powersave | 2026-11-13 23:26 to 00:30 | 0.9 to 1.5 | de77f76e8 | [2026-11-13-epyc4245p-c](../2026-11-13-epyc4245p-c/) |
| E | AMD EPYC 4585PX 16-Core Processor, 32 threads, 62 GB, Ubuntu 24.04.5 LTS, kernel 6.8.0-149-generic, governor powersave, dev-georgios (shared development server); shared and loaded, so compare lanes within a row, not machines | 2026-11-14 04:24 to 04:37 | 75.1 to 221.9 | 62a0936e4 | [2026-11-14-dev-georgios-store14600](../2026-11-14-dev-georgios-store14600/) |

Versions, from each source's VERSIONS.md: nc 0.7.0; fx 0.0.16; nc-lite 0.7.0; Claude Code 2.1.323, 2.1.325; Codex 0.169.0; Pi 1.5.2; OpenCode 1.19.5. nc is built as releases are (scripts/build-cli.sh: dist profile, non-PIE, BOLT); the source table gives its commit.

## Starting up

| | nc | fx | nc-lite | Claude Code | Codex | Pi | OpenCode | Src |
|---|---:|---:|---:|---:|---:|---:|---:|---|
| First frame, ms | 3.94 (20) | 5.24 (20) | 2.71 (20) | 247 (20) | 220 (20) | 208 (20) | 2,046 (20) | A |
| CPU to the first frame, ms | 6.95 (20) | 13.1 (20) | 2.14 (20) | 660 (20) | 348 (20) | 351 (20) | 3,787 (20) | A |
| Memory 1 s after the first frame, RSS MB | 14.7 (20) | 9.21 (20) | 5.57 (20) | 216 (20) | 171 (20) | 113 (20) | n/a | A |
| --version, ms | 7.11 (100) | 7.09 (100) | 6.82 (100) | 10.4 (100) | 10.3 (100) | 163 (100) | 301 (100) | A |
| --version, CPU ms | 0.873 (100) | 0.411 (100) | 0.744 (100) | 8.51 (100) | 13.3 (100) | 201 (100) | 406 (100) | A |
| --version, cold page cache, ms | 7.21 (5) | 5.26 (5) | 7.38 (5) | 33.9 (5) | 25.9 (5) | 115 (5) | 239 (5) | C |
| First frame, cold page cache, ms | 9.34 (5) | 9.43 (5) | 4.21 (5) | 229 (5) | 240 (5) | 151 (5) | 1,434 (5) | C |

## Using it

| | nc | fx | nc-lite | Claude Code | Codex | Pi | OpenCode | Src |
|---|---:|---:|---:|---:|---:|---:|---:|---|
| Keystroke echo p50, ms | 0.218 (10) | 0.634 (10) | 0.153 (10) | 2.33 (10) | 21.5 (10) | 0.668 (10) | 2.76 (10) | A |
| Keystroke echo, slowest of 26, ms | 0.320 (10) | 114 (10) | 0.185 (10) | 13.5 (10) | 22.6 (10) | 3.44 (10) | 21.5 (10) | A |
| Idle CPU, ms per s | -0.008 (10) | 0.485 (10) | -0.002 (10) | 4.96 (10) | 0.487 (10) | 2.08 (10) | 20.7 (10) | B |

## Headless

| | nc | fx | nc-lite | Claude Code | Codex | Pi | OpenCode | Src |
|---|---:|---:|---:|---:|---:|---:|---:|---|
| One-word reply, ms | 7.40 (20) | 27.3 (20) | 7.17 (20) | 267 (20) | 130 (20) | 243 (20) | 1,544 (20) | A |
| CPU for that reply, ms | 5.63 (20) | 4.20 (20) | 3.68 (20) | 308 (20) | 181 (20) | 321 (20) | 2,574 (20) | A |
| 50 shell steps in one turn, ms | 65.5 (10) | 1,102 (10) | 132 (10) | 1,022 (10) | 3,092 (10) | 546 (10) | 2,433 (10) | A |
| Tool result to next request p50, ms | 0.509 (10) | 21.0 (10) | 1.69 (10) | 13.1 (10) | 58.5 (10) | 5.51 (10) | 15.1 (10) | A |
| 32 runs at once, peak RSS MB each | 16.3 (10) | 7.74 (10) | 8.15 (9) | 227 (10) | 152 (10) | 125 (10) | 599 (10) | A |
| 32 runs at once, CPU ms each | 19.2 (10) | 19.1 (10) | 29.7 (9) | 793 (10) | 332 (10) | 700 (10) | 4,799 (10) | A |

## Streaming

| | nc | fx | nc-lite | Claude Code | Codex | Pi | OpenCode | Src |
|---|---:|---:|---:|---:|---:|---:|---:|---|
| Token on screen at 100 tok/s, p50 ms | 0.192 (10) | 200 (10) | 0.143 (10) | 152 (10) | 1.90 (10) | 7.90 (10) | 19.9 (10) | A |
| Token on screen at 1,000 tok/s, p50 ms | 4.13 (10) | 33.1 (10) | 0.041 (10) | 64.0 (10) | 5.73 (10) | 8.47 (10) | 17.5 (10) | A |
| Token on screen at 1,000 tok/s, p99 ms | 8.35 (10) | 54.7 (10) | 0.055 (10) | 119 (10) | 16.7 (10) | 16.5 (10) | 32.8 (10) | A |
| 240x70 at 10,000 tok/s, p50 ms | 3.67 (3) | 22.8 (2) | 0.011 (2) | 56.5 (2) | 490 (2) | 38.5 (2) | 13.4 (2) | C |
| 240x70 at 10,000 tok/s, p99 ms | 8.79 (3) | 39.3 (2) | 0.111 (2) | 108 (2) | 1,111 (2) | 250 (2) | 32.3 (2) | C |
| 240x70 at 10,000 tok/s, CPU ms | 340 (3) | 2,626 (2) | 371 (2) | 1,929 (2) | 8,840 (2) | 4,756 (2) | 9,409 (2) | C |
| 240x70 at 10,000 tok/s, peak RSS MB | 25.6 (3) | 32.0 (2) | 11.6 (2) | 350 (2) | 222 (2) | 315 (2) | 981 (2) | C |
| 50,000 tok/s, p50 ms | 3.25 (10) | 72.7 (5) | 0.023 (5) | 107 (5) | 10,798 (5) | 2,141 (5) | 1,864 (5) | D |
| 50,000 tok/s, p99 ms | 8.48 (10) | 349 (5) | 0.044 (5) | 2,432 (5) | 21,517 (5) | 7,932 (5) | 3,697 (5) | D |
| 50,000 tok/s, CPU ms | 2,983 (10) | 6,364 (5) | 3,682 (5) | 8,051 (5) | 42,117 (5) | 10,163 (5) | 25,251 (5) | D |
| 50,000 tok/s, peak RSS MB | 48.6 (10) | 110 (5) | 24.9 (5) | 526 (5) | 469 (5) | 350 (5) | 1,692 (5) | D |

## Sessions

| | nc | fx | nc-lite | Claude Code | Codex | Pi | OpenCode | Src |
|---|---:|---:|---:|---:|---:|---:|---:|---|
| 1,000 sessions stored, KB per session | 5.00 | 12.8 | 4.22 | 132 | 65.6 | 8.15 | 4.52 | A |
| CPU per saved session, ms | 6.79 | 4.42 | 18.8 | 297 | 147 | 322 | 568 | A |
| Listing 1,000 sessions, ms | 13.5 (20) | 13.1 (20) | 17.2 (20) | n/a | n/a | n/a | n/a | A |
| Continuing the latest of 1,000, ms | 18.2 (20) | 37.2 (20) | 45.3 (20) | 264 (20) | 124 (20) | 252 (20) | n/a | A |
| Listing 14,600 sessions, ms | 588 (10) | 89.7 (10) | n/a | n/a | n/a | n/a | n/a | A |
| Listing 14,600 sessions, task-clock ms | 556 (10) | 63.7 (10) | n/a | n/a | n/a | n/a | n/a | A |
| Listing 14,600 sessions, instructions (user+kernel), M | 6,501 (10) | 598 (10) | n/a | n/a | n/a | n/a | n/a | A |
| Listing 14,600 sessions, KB printed (each CLI's default listing) | 10.5 (10) | 27.2 (10) | n/a | n/a | n/a | n/a | n/a | A |
| Continuing the latest of 14,600, ms | 596 (10) | 113 (10) | n/a | n/a | n/a | n/a | n/a | A |
| Continuing the latest of 14,600, task-clock ms | 566 (10) | 66.8 (10) | n/a | n/a | n/a | n/a | n/a | A |
| Continuing the latest of 14,600, instructions (user+kernel), M | 6,521 (10) | 607 (10) | n/a | n/a | n/a | n/a | n/a | A |

## Size

| | nc | fx | nc-lite | Claude Code | Codex | Pi | OpenCode | Src |
|---|---:|---:|---:|---:|---:|---:|---:|---|
| Install, MB | 61.9 | 12.2 | 18.2 | 685 | 403 | 159 | 832 | A |
| Main binary, MB | 45.2 | 12.2 | 18.2 | 349 | 281 | Node script | 204 | A |

## fx's own claims (unverified)

Vercel publishes these for fx. We have not reproduced them; the right-hand columns are our nearest measurement of the same thing, from the rows above.

| fx claim | fx says | fx, ours | nc, ours | Note |
|---|---:|---:|---:|---|
| Startup before terminal setup (FX_BENCH=1) | ~0.5 ms | 7.09 (A) | 7.11 (A) | our --version is a whole process lifetime |
| CI fails any command over | 2 ms | 7.09 (A) | 7.11 (A) |  |
| Cold start | 10 µs | 5.26 (C) | 7.21 (C) | ours drops the page cache first |
| Binary | 6.39 MiB | 12.2 (A) | 45.2 (A) | the linux-x86_64 release archive's binary, as installed |
| Memory | single-digit MB | 9.21 (A) | 14.7 (A) |  |
| Terminal interactions p95 | < 15 ms | 114 (A) | 0.320 (A) | ours is the slowest of 26 echoes |
| Listing 14,600 sessions | 95 s to 0.17 s | 89.7 (A) | 588 (A) |  |
| --resume last | 0.5 s | 113 (A) | 596 (A) | ours includes sending the next request |
| Shell calls | 27.9 ms | 21.0 (A) | 0.509 (A) | ours is per step inside one turn |
| MCP exit | 7.4 ms | n/a | n/a | not measured: no lane runs an MCP server here |

## Where nanocode does not lead yet

Every row above where nc's median is not below each of fx, nc-lite, Claude Code and Codex, with the gap to the best of them.

- 240x70 at 10,000 tok/s, p50 ms (C): nc 3.67 against nc-lite 0.011, 3.66 or 326.83x
- 50,000 tok/s, p99 ms (D): nc 8.48 against nc-lite 0.044, 8.44 or 193.43x
- Token on screen at 1,000 tok/s, p99 ms (A): nc 8.35 against nc-lite 0.055, 8.30 or 151.92x
- 50,000 tok/s, p50 ms (D): nc 3.25 against nc-lite 0.023, 3.23 or 141.09x
- Token on screen at 1,000 tok/s, p50 ms (A): nc 4.13 against nc-lite 0.041, 4.09 or 100.66x
- 240x70 at 10,000 tok/s, p99 ms (C): nc 8.79 against nc-lite 0.111, 8.68 or 79.07x
- Listing 14,600 sessions, instructions (user+kernel), M (A): nc 6,501 against fx 598, 5,903 or 10.87x
- Continuing the latest of 14,600, instructions (user+kernel), M (A): nc 6,521 against fx 607, 5,914 or 10.74x
- Listing 14,600 sessions, task-clock ms (A): nc 556 against fx 63.7, 492 or 8.73x
- Continuing the latest of 14,600, task-clock ms (A): nc 566 against fx 66.8, 499 or 8.47x
- Listing 14,600 sessions, ms (A): nc 588 against fx 89.7, 498 or 6.55x
- Continuing the latest of 14,600, ms (A): nc 596 against fx 113, 483 or 5.29x
- Install, MB (A): nc 61.9 against fx 12.2, 49.7 or 5.06x; also behind nc-lite
- Main binary, MB (A): nc 45.2 against fx 12.2, 33.0 or 3.70x; also behind nc-lite
- CPU to the first frame, ms (A): nc 6.95 against nc-lite 2.14, 4.81 or 3.25x
- Memory 1 s after the first frame, RSS MB (A): nc 14.7 against nc-lite 5.57, 9.13 or 2.64x; also behind fx
- 240x70 at 10,000 tok/s, peak RSS MB (C): nc 25.6 against nc-lite 11.6, 14.1 or 2.22x
- First frame, cold page cache, ms (C): nc 9.34 against nc-lite 4.21, 5.12 or 2.22x
- --version, CPU ms (A): nc 0.873 against fx 0.411, 0.461 or 2.12x; also behind nc-lite
- 32 runs at once, peak RSS MB each (A): nc 16.3 against fx 7.74, 8.52 or 2.10x; also behind nc-lite
- 50,000 tok/s, peak RSS MB (D): nc 48.6 against nc-lite 24.9, 23.7 or 1.95x
- Keystroke echo, slowest of 26, ms (A): nc 0.320 against nc-lite 0.185, 0.135 or 1.73x
- CPU per saved session, ms (A): nc 6.79 against fx 4.42, 2.36 or 1.53x
- CPU for that reply, ms (A): nc 5.63 against nc-lite 3.68, 1.94 or 1.53x; also behind fx
- First frame, ms (A): nc 3.94 against nc-lite 2.71, 1.23 or 1.45x
- Keystroke echo p50, ms (A): nc 0.218 against nc-lite 0.153, 0.065 or 1.43x
- --version, cold page cache, ms (C): nc 7.21 against fx 5.26, 1.95 or 1.37x
- Token on screen at 100 tok/s, p50 ms (A): nc 0.192 against nc-lite 0.143, 0.049 or 1.35x
- 1,000 sessions stored, KB per session (A): nc 5.00 against nc-lite 4.22, 0.784 or 1.19x
- --version, ms (A): nc 7.11 against nc-lite 6.82, 0.292 or 1.04x; also behind fx
- One-word reply, ms (A): nc 7.40 against nc-lite 7.17, 0.235 or 1.03x
- Listing 1,000 sessions, ms (A): nc 13.5 against fx 13.1, 0.424 or 1.03x
- 32 runs at once, CPU ms each (A): nc 19.2 against fx 19.1, 0.144 or 1.01x

## nc across sources

The same nc rows from every source that has them, to show what changed between nanocode commits (and machines; see the source table).

| nc | A (41dcbc7c4) | B (4be51211a) | C (223cb39ba) | D (de77f76e8) | E (62a0936e4) |
|---|---:|---:|---:|---:|---:|
| First frame, ms | 3.94 (20) | 1.73 (20) | 1.78 (20) |  |  |
| CPU to the first frame, ms | 6.95 (20) | 5.48 (20) | 4.17 (20) |  |  |
| Memory 1 s after the first frame, RSS MB | 14.7 (20) | 14.2 (20) | 14.5 (20) |  |  |
| --version, ms | 7.11 (100) | 2.73 (100) | 2.96 (100) |  |  |
| --version, CPU ms | 0.873 (100) | 0.494 (100) | 0.573 (100) |  |  |
| Keystroke echo p50, ms | 0.218 (10) |  | 0.054 (10) |  |  |
| Keystroke echo, slowest of 26, ms | 0.320 (10) |  | 0.083 (10) |  |  |
| Idle CPU, ms per s |  | -0.008 (10) | -0.009 (10) |  |  |
| One-word reply, ms | 7.40 (20) | 5.12 (20) | 5.16 (20) |  |  |
| CPU for that reply, ms | 5.63 (20) | 5.37 (20) | 4.05 (20) |  |  |
| 50 shell steps in one turn, ms | 65.5 (10) | 33.1 (10) | 33.0 (10) |  |  |
| Tool result to next request p50, ms | 0.509 (10) | 0.276 (10) | 0.264 (10) |  |  |
| 32 runs at once, peak RSS MB each | 16.3 (10) | 15.5 (10) | 15.2 (9) |  |  |
| 32 runs at once, CPU ms each | 19.2 (10) | 13.5 (10) | 10.9 (9) |  |  |
| Token on screen at 100 tok/s, p50 ms | 0.192 (10) | 0.064 (5) |  |  |  |
| Token on screen at 1,000 tok/s, p50 ms | 4.13 (10) | 4.04 (5) |  |  |  |
| Token on screen at 1,000 tok/s, p99 ms | 8.35 (10) | 8.16 (5) |  |  |  |
| 1,000 sessions stored, KB per session | 5.00 (1) |  | 4.34 (1) |  |  |
| CPU per saved session, ms | 6.79 (1) |  | 3.91 (1) |  |  |
| Listing 1,000 sessions, ms | 13.5 (20) |  | 6.92 (20) |  |  |
| Continuing the latest of 1,000, ms | 18.2 (20) |  | 10.9 (20) |  |  |
| Listing 14,600 sessions, ms | 588 (10) |  |  |  | 2,673 (10) |
| Listing 14,600 sessions, task-clock ms | 556 (10) |  |  |  | 932 (10) |
| Listing 14,600 sessions, instructions (user+kernel), M | 6,501 (10) |  |  |  | 5,247 (10) |
| Listing 14,600 sessions, KB printed (each CLI's default listing) | 10.5 (10) |  |  |  | 1,514 (10) |
| Continuing the latest of 14,600, ms | 596 (10) |  |  |  | 1,646 (10) |
| Continuing the latest of 14,600, task-clock ms | 566 (10) |  |  |  | 772 (10) |
| Continuing the latest of 14,600, instructions (user+kernel), M | 6,521 (10) |  |  |  | 4,984 (10) |
| Install, MB | 61.9 (1) |  | 69.8 (1) |  | 44.1 (1) |
| Main binary, MB | 45.2 (1) |  | 69.8 (1) |  | 44.1 (1) |

## Method

- First frame is the time from spawn until the terminal interface has drawn its prompt in a pseudo-terminal (harness/tui.py); memory is sampled from /proc/<pid>/smaps_rollup one second later, summed over the process tree. RSS counts the executable's file pages, which kernel fault-around maps in 64 KB blocks, so it depends on code layout as much as on what runs.
- One-word reply is a headless run from spawn to exit against a fixture that answers at once. Streaming rows replay a fixed reply at the given token rate and time each token from the fixture sending it to the pseudo-terminal showing it.
- Session rows save one-turn sessions through each CLI's own headless mode, then list them with its documented command (nc sessions, fx sessions --json) and continue the latest one (nc --continue, fx --resume last), interleaving the lanes round by round. On a loaded machine the 14,600-session rows run under perf stat, whose task-clock and instruction counts are the comparable numbers; wall times there are noise.
- Idle CPU is a 23 s session's CPU minus a 3 s session's, per second; values a few thousandths from zero are zero within measurement noise.
- Cold-cache rows drop the page cache before every sample. Install sizes leave out runtimes a lane needs but does not ship (Node for Pi).

## Reproduce

Each source directory holds COMMANDS.md (the exact commands), REPORT.md (every column, PSS and USS, errors) and the raw JSONL. A full run on one machine:

```sh
cd bench/cli
sh tools/install.sh && LANES=nclite sh tools/install.sh && . tools/install/env.sh
export NC_BIN=<nc built with scripts/build-cli.sh nanocode-cli nanocode>
ROUNDS=20 bash harness/run.sh <machine>    # every part, including the 14,600-session store, then REPORT.md and POST.md
```
