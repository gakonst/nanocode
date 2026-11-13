# Seven agent CLIs on one quiet server, 2026-11-13

*Draft for nanocode.xyz/experiments. Not published.*

We ran seven terminal coding agents through the same scripted work on one AMD EPYC 4245P bare-metal server (6 cores, 12 threads, 46 GB, Ubuntu 24.04, kernel 6.8, load average under 1 at every step) on 2026-11-13 between 11:51 and 12:52 UTC. All lanes ran on the same server, taking turns in every round, unlike the 2026-11-12 run, which split the work across five servers of this model.

The CLIs: nc (nanocode master 4be51211a, built as releases are, with scripts/build-cli.sh: dist profile, no position-independent code, laid out by BOLT 19 from a training run), nc-lite (the same SDK cut down to what fx ships, at 7837aae16, lite profile, BOLT), Vercel fx 0.0.16, Pi 1.5.2, OpenCode 1.19.5, Claude Code 2.1.323 and Codex CLI 0.169.0. No lane talks to a real model: each points at a local fixture that streams a fixed reply, runs with its own empty HOME, inside a network namespace where only loopback works. Every number is a median with the sample count in parentheses. CPU and memory cover the CLI's whole process tree. The generated [REPORT.md](REPORT.md), the raw JSONL and [COMMANDS.md](COMMANDS.md) are next to this file.

This run took 20 rounds (10 for idle, tool turns and fleets) where the 2026-11-12 run took 30, and 5 streaming samples per rate where we usually require 10, so the streaming rows are indicative. Cold-cache rows, the 240x70 sweep, the session store and nc's install size come from a second pass, below. The 50,000 tok/s sweep is still missing.

## Headline

| | nc | fx | Pi | OpenCode | Claude Code | Codex | nc-lite |
|---|---:|---:|---:|---:|---:|---:|---:|
| First frame, ms | 1.73 (20) | 1.87 (20) | 136 (20) | 1,407 (20) | 164 (20) | 136 (20) | 1.04 (20) |
| Memory 1 s after it, RSS MB | 14.2 (20) | 8.92 (20) | 111 (20) | n/a | 220 (20) | 170 (20) | 5.29 (20) |
| One-word reply, ms | 5.12 (20) | 22.9 (20) | 149 (20) | 1,023 (20) | 173 (20) | 64.3 (20) | 2.89 (20) |
| CPU for that reply, ms | 5.37 (20) | 1.67 (20) | 196 (20) | 1,893 (20) | 197 (20) | 77.4 (20) | 2.07 (20) |
| 50 shell steps in one turn, ms | 33.1 (10) | 1,059 (10) | 299 (10) | 1,632 (10) | 707 (10) | 2,012 (10) | 55.2 (10) |
| Tool result to next request, ms | 0.276 (10) | 20.5 (10) | 2.60 (10) | 10.6 (10) | 9.34 (10) | 38.4 (10) | 0.790 (10) |
| Idle CPU, ms per s | 0.00 (10) | 0.485 (10) | 2.08 (10) | 20.7 (10) | 4.96 (10) | 0.487 (10) | 0.00 (10) |
| 32 runs at once, RSS MB each | 15.5 (10) | 7.69 (10) | 125 (10) | 583 (10) | 238 (10) | 179 (10) | 7.72 (9) |
| Token on screen at 100 tok/s, p50 ms | 0.064 (5) | 200 (5) | 7.68 (5) | 19.6 (5) | 151 (5) | 1.50 (5) | 0.017 (5) |
| Main binary, MB | 72.7 | 12.2 | Node bundle | 204 | 346 | 281 | 18.2 |

OpenCode's memory sample one second after its first frame came back empty in every round; its peak was 922 MB. nc's install size is in the second pass (69.8 MB at 223cb39ba). Pi's install (159 MB) does not count the Node runtime.

## What changed in nc since 2026-11-12

The 2026-11-12 run measured the BOLT release build at 7837aae16 (nc-bolt). Against it, on a server of the same model:

| nc | 2026-11-12 (7837aae16) | 2026-11-13 (4be51211a) |
|---|---:|---:|
| First frame, ms | 2.02 (30) | 1.73 (20) |
| Memory 1 s after the first frame, RSS MB | 19.0 (30) | 14.2 (20) |
| CPU to the first frame, ms | 6.84 (30) | 5.48 (20) |
| 50 shell steps in one turn, ms | 63.1 (10) | 33.1 (10) |
| --version, CPU ms | 0.444 (100) | 0.494 (100) |

Three changes on the CLI track account for most of the memory: the terminal interface runs one Tokio worker as exec does (95aee2808, 35 threads to 4 on a 32-core host), the trust store decodes each root certificate once per source instead of four times (d3ec97b99, off the first-frame path), and BOLT now trains with the Tempo tools on, as a user's nc starts (4be51211a), so the startup code that connects them lands in BOLT's hot section and the kernel maps far fewer cold neighbors. The faster tool loop comes from work on other tracks between the two commits.

## Where nc trails

fx still draws less and holds less. Its first frame needs 3.78 ms of CPU where nc needs 5.48, its one-word reply 1.67 ms where nc needs 5.37, and it holds 8.92 MB after the first frame where nc holds 14.2. nc's binary is 72.7 MB against fx's 12.2: most of nc's resident memory is pages of its own code and read-only data, and BOLT reorders code, not data. nc-lite shows the floor for this SDK at fx's feature set: 1.04 ms to the first frame and 5.29 MB.

The bundled python3 costs almost nothing until it runs: one 4 KB page of its 12,589 functions is resident after nc's first frame.

## Second pass, 15:00 to 17:49 UTC: the rows left out above

A second box of the same model (AMD EPYC 4245P) ran the suite again with nc at 223cb39ba, two commits after 4be51211a on this track: the trust store tells repeated root certificates apart by inode instead of resolving every path (18e5355b3, 800 `readlink` calls to 6 per start), and the paying proxy's certificate bundle is the system bundle file copied as-is rather than every root decoded and re-encoded (223cb39ba). The ring P-256 keys (f223811c7) came after this build. Raw data and REPORT.md are in [../2026-11-13-epyc4245p-b](../2026-11-13-epyc4245p-b/REPORT.md). The box's two-hour lifetime ran out during the 240x70 sweep (3 nc samples, 2 for the others) and before the 50,000 tok/s sweep, which is still missing. Cold-cache rows take 5 rounds.

| | nc | fx | Pi | OpenCode | Claude Code | Codex | nc-lite |
|---|---:|---:|---:|---:|---:|---:|---:|
| --version, cold page cache, ms | 7.21 (5) | 5.26 (5) | 115 (5) | 239 (5) | 33.9 (5) | 25.9 (5) | 7.38 (5) |
| First frame, cold page cache, ms | 9.34 (5) | 9.43 (5) | 151 (5) | 1,434 (5) | 229 (5) | 240 (5) | 4.21 (5) |
| 240x70 at 10,000 tok/s, on-screen lag p50 ms | 3.67 (3) | 22.8 (2) | 38.5 (2) | 13.4 (2) | 56.5 (2) | 490 (2) | 0.011 (2) |
| Same, p99 ms | 8.79 (3) | 39.3 (2) | 250 (2) | 32.3 (2) | 108 (2) | 1,111 (2) | 0.111 (2) |
| Same, CPU for the session, ms | 340 (3) | 2,626 (2) | 4,756 (2) | 9,409 (2) | 1,929 (2) | 8,840 (2) | 371 (2) |
| Same, peak RSS MB | 25.6 (3) | 32.0 (2) | 315 (2) | 981 (2) | 350 (2) | 222 (2) | 11.6 (2) |
| 1,000 sessions stored, KB per session | 4.34 | 12.8 | 8.15 | 16.6 | 132 | 65.7 | 4.22 |
| CPU per saved session, ms | 3.91 | 1.60 | 196 | 1,770 | 190 | 63.9 | 10.4 |
| Install size, MB | 69.8 | 12.2 | 159 | 409 | 346 | 403 | 18.2 |

nc against the first pass (4be51211a, same CPU model, 20 rounds each):

| nc | 4be51211a | 223cb39ba |
|---|---:|---:|
| One-word reply, ms | 5.12 (20) | 5.16 (20) |
| CPU for that reply, ms | 5.37 (20) | 4.05 (20) |
| First frame, ms | 1.73 (20) | 1.78 (20) |
| CPU to the first frame, ms | 5.48 (20) | 4.17 (20) |
| Memory 1 s after the first frame, RSS MB | 14.2 (20) | 14.5 (20) |
| 50 shell steps in one turn, ms | 33.1 (10) | 33.0 (10) |

The two commits take a quarter of nc's CPU off both the first frame and the one-word reply, but neither wall time moves: the work they remove runs on the paying proxy's own thread, beside the main thread rather than in front of it. The one-word reply is still about 0.3 ms slower than the 2026-11-12 build (4.83 ms at 7837aae16), so the trust store was not the cause. CPU to the first frame (4.17 ms) is now within 4% of fx (4.02). nc's cold --version is 2 ms behind fx's because it pages in more binary (69.8 MB against 12.2 MB). Its cold first frame ties fx.

## Method notes

- Kernel fault-around (64 KB) maps neighbors of every touched file page, so RSS depends on code layout as much as on what runs. PSS and USS are in REPORT.md.
- nc's 1 s memory sample includes its connection attempts to its Tempo tools, which the namespace refuses.
- The CPU governor was powersave, as on the earlier runs.

## Reproduce

```sh
cd bench/cli
LANES=fx,pi,opencode,claude,codex sh tools/install.sh
LANES=nclite sh tools/install.sh
. tools/install/env.sh
export NC_BIN=<nc built with scripts/build-cli.sh nanocode-cli nanocode>
ONLY=nc,nclite,fx,pi,opencode,claude,codex ROUNDS=20 RATES=100,1000 SAMPLES=5 \
  bash harness/run.sh 2026-11-13-epyc4245p smoke startup firstframe oneshot idle tools fleet disk tui report
```
