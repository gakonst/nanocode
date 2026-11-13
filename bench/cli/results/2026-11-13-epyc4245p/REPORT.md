# CLI benchmark: 2026-11-13-epyc4245p

Machine: AMD EPYC 4245P 6-Core Processor, 12 threads, 46 GB, Ubuntu 24.04.5 LTS, 6.8.0-149-generic. 

# Installed CLIs

Written by tools/install.sh on Linux x86_64. The size is the main binary's; BENCH_INSTALL_<LANE> in env.sh is the whole install directory.

| Lane | Version | Install | Date (UTC) | Main binary | Runtime not counted |
| --- | --- | --- | --- | --- | --- |
| nclite | nc-lite 0.7.0 | PROFILE=lite scripts/build-cli.sh nc-lite nc-lite at https://github.com/nanocodexyz/nanocode 7837aae16afe1e46fbf72471c902f5cf2e683270 | 2026-11-13T11:49Z | `tools/install/nclite/nc-lite` (18.2 MiB) | none |
| fx | 0.0.16 | curl https://releases.fx.sh/$(latest.txt)/fx-linux-x86_64.tar.gz (the archive https://fx.sh/setup.sh fetches; binary only) | 2026-11-13T11:24Z | `tools/install/fx/fx` (12.2 MiB) | none |
| pi | 1.5.2 | npm install --prefix tools/install/pi @earendil-works/pi-coding-agent@latest | 2026-11-13T11:24Z | `tools/install/pi/node_modules/@earendil-works/pi-coding-agent/dist/bundle/cli.js` (0.0 MiB) | Node v22.23.5 |
| opencode | 1.19.5 | npm install --prefix tools/install/opencode opencode-ai@latest (the package's bin is the native binary from its platform package) | 2026-11-13T11:24Z | `tools/install/opencode/node_modules/opencode-ai/bin/opencode.exe` (204.5 MiB) | none |
| claude | 2.1.323 (Claude Code) | npm install --prefix tools/install/claude @anthropic-ai/claude-code@latest (the package's bin is the native binary from its platform package) | 2026-11-13T11:24Z | `tools/install/claude/node_modules/@anthropic-ai/claude-code/bin/claude.exe` (345.6 MiB) | none |
| codex | codex-cli 0.169.0 | npm install --prefix tools/install/codex @openai/codex@latest (CODEX_BIN is the platform binary: the npm codex command is a Node launcher that execs it) | 2026-11-13T11:24Z | `tools/install/codex/node_modules/@openai/codex-linux-x64/vendor/x86_64-unknown-linux-musl/bin/codex` (280.6 MiB) | none (the npm launcher needs Node) |

Medians with the sample count in parentheses and [min–max] where the spread matters. CPU and memory are the whole process tree (cgroup or /proc sampling; rusage when neither is available). Lower is better everywhere.

## Startup: --version

| lane | samples | errors | wall ms | CPU ms | peak RSS MB | instructions |
|---|---:|---:|---:|---:|---:|---:|
| true | 100 | 0 | 2.83 [2.67–99.6] (100) | 0.186 (100) | 1.25 (100) | n/a |
| nc | 100 | 0 | 2.73 [2.66–2.96] (100) | 0.494 (100) | 4.63 (100) | n/a |
| fx | 100 | 0 | 2.66 [1.19–2.74] (100) | 0.115 (100) | 1.38 (100) | n/a |
| pi | 100 | 0 | 98.8 [96.8–103] (100) | 121 (100) | 96.9 (100) | n/a |
| opencode | 100 | 0 | 201 [199–205] (100) | 269 (100) | 130 (100) | n/a |
| claude | 100 | 0 | 5.04 [4.69–7.13] (100) | 4.25 (100) | 25.6 (100) | n/a |
| codex | 100 | 0 | 4.78 [4.68–5.15] (100) | 3.19 (100) | 16.5 (100) | n/a |
| nclite | 100 | 0 | 2.69 [2.64–2.76] (100) | 0.327 (100) | 3.25 (100) | n/a |

## Time to first frame

| lane | samples | errors | first byte ms | first frame ms | CPU ms | peak RSS MB | processes |
|---|---:|---:|---:|---:|---:|---:|---:|
| nc | 20 | 0 | 1.28 (20) | 1.73 [1.64–2.20] (20) | 5.48 (20) | 14.2 (20) | 0.000 (20) |
| fx | 20 | 0 | 0.944 (20) | 1.87 [1.75–1.98] (20) | 3.78 (20) | 8.98 (20) | 0.000 (20) |
| pi | 20 | 0 | 124 (20) | 136 [127–140] (20) | 210 (20) | 114 (20) | 0.000 (20) |
| opencode | 20 | 0 | 288 (20) | 1,407 [1,348–1,431] (20) | 3,519 (20) | 922 (20) | 0.000 (20) |
| claude | 20 | 0 | 91.8 (20) | 164 [162–165] (20) | 427 (20) | 230 (20) | 4.00 (20) |
| codex | 20 | 0 | 4.78 (20) | 136 [128–143] (20) | 192 (20) | 172 (20) | 6.00 (20) |
| nclite | 20 | 0 | 0.908 (20) | 1.04 [0.999–1.27] (20) | 0.992 (20) | 5.29 (20) | 0.000 (20) |

## Memory one second after the first frame

Summed over the CLI's process tree from /proc/<pid>/smaps_rollup. RSS counts the executable's file pages, which every instance shares; PSS splits shared pages between the processes that map them; USS is memory only this tree holds.

| lane | samples | errors | RSS MB | PSS MB | private (USS) MB | anonymous MB |
|---|---:|---:|---:|---:|---:|---:|
| nc | 20 | 0 | 14.2 (20) | 12.1 (20) | 11.7 (20) | 3.00 (20) |
| fx | 20 | 0 | 8.92 (20) | 8.90 (20) | 8.90 (20) | 1.25 (20) |
| pi | 20 | 0 | 111 (20) | 78.2 (20) | 61.2 (20) | 59.9 (20) |
| opencode | 20 | 0 | n/a | n/a | n/a | n/a |
| claude | 20 | 0 | 220 (20) | 218 (20) | 217 (20) | 137 (20) |
| codex | 20 | 0 | 170 (20) | 170 (20) | 170 (20) | 34.3 (20) |
| nclite | 20 | 0 | 5.29 (20) | 3.32 (20) | 2.93 (20) | 0.348 (20) |

## Idle terminal interface

| lane | samples | errors | idle CPU ms/s | bytes written while idle | peak RSS MB | processes |
|---|---:|---:|---:|---:|---:|---:|
| nc | 10 | 0 | -0.008 [-0.017–0.009] (10) | 0.000 (10) | 14.2 (10) | 0.000 (10) |
| fx | 10 | 0 | 0.485 [0.414–0.594] (10) | 0.000 (10) | 9.47 (10) | 0.000 (10) |
| pi | 10 | 0 | 2.08 [1.73–2.22] (10) | 0.000 (10) | 114 (10) | 0.000 (10) |
| opencode | 10 | 0 | 20.7 [15.6–30.3] (10) | 0.000 (10) | 919 (10) | 0.000 (10) |
| claude | 10 | 0 | 4.96 [3.54–6.69] (10) | 206 (10) | 230 (10) | 4.00 (10) |
| codex | 10 | 0 | 0.487 [0.128–0.706] (10) | 0.000 (10) | 170 (10) | 6.00 (10) |
| nclite | 10 | 0 | -0.002 [-0.013–0.005] (10) | 0.000 (10) | 5.30 (10) | 0.000 (10) |

## One-shot headless prompt

| lane | samples | errors | wall ms | spawn to request ms | reply end to exit ms | CPU ms | peak RSS MB | first request bytes | side requests | processes left behind |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| nc | 20 | 0 | 5.12 [5.07–7.76] (20) | 3.00 (20) | 1.83 (20) | 5.37 (20) | 14.8 (20) | 62,620 (20) | 0.000 (20) | 0.000 (20) |
| fx | 20 | 0 | 22.9 [22.8–24.9] (20) | 12.2 (20) | 10.5 (20) | 1.67 (20) | 7.63 (20) | 26,281 (20) | 0.000 (20) | 0.000 (20) |
| pi | 20 | 0 | 149 [147–153] (20) | 135 (20) | 13.3 (20) | 196 (20) | 111 (20) | 5,444 (20) | 0.000 (20) | 0.000 (20) |
| opencode | 20 | 0 | 1,023 [1,011–1,047] (20) | 983 (20) | 41.1 (20) | 1,893 (20) | 675 (20) | 30,705 (20) | 1.00 (20) | 0.000 (20) |
| claude | 20 | 0 | 173 [169–177] (20) | 154 (20) | 19.3 (20) | 197 (20) | 198 (20) | 46,411 (20) | 0.000 (20) | 0.000 (20) |
| codex | 20 | 0 | 64.3 [63.2–67.3] (20) | 53.1 (20) | 10.3 (20) | 77.4 (20) | 145 (20) | 39,556 (20) | 0.000 (20) | 0.000 (20) |
| nclite | 20 | 0 | 2.89 [2.86–5.14] (20) | 2.08 (20) | 0.675 (20) | 2.07 (20) | 7.56 (20) | 23,942 (20) | 0.000 (20) | 0.000 (20) |

## Long tool turn: 10 shell steps, headless

| lane | samples | errors | wall ms | tool result to next request p50 ms | CPU per step ms | peak RSS MB | last request bytes |
|---|---:|---:|---:|---:|---:|---:|---:|
| nc | 10 | 0 | 15.1 [13.1–19.9] (10) | 0.314 (10) | 1.34 (10) | 21.4 (10) | 66,923 (10) |
| fx | 10 | 0 | 229 [229–231] (10) | 20.4 (10) | 2.41 (10) | 15.3 (10) | 33,006 (10) |
| pi | 10 | 0 | 191 [189–197] (10) | 3.82 (10) | 27.7 (10) | 126 (10) | 7,337 (10) |
| opencode | 10 | 0 | 1,180 [1,169–1,193] (10) | 12.2 (10) | 221 (10) | 743 (10) | 33,163 (10) |
| claude | 10 | 0 | 293 [285–300] (10) | 9.93 (10) | 37.3 (10) | 255 (10) | 49,682 (10) |
| codex | 10 | 0 | 461 [459–463] (10) | 38.6 (10) | 27.3 (10) | 168 (10) | 43,145 (10) |
| nclite | 10 | 0 | 17.1 [17.0–20.2] (10) | 0.822 (10) | 3.27 (10) | 22.4 (10) | 27,531 (10) |

## Long tool turn: 50 shell steps, headless

| lane | samples | errors | wall ms | tool result to next request p50 ms | CPU per step ms | peak RSS MB | last request bytes |
|---|---:|---:|---:|---:|---:|---:|---:|
| nc | 10 | 0 | 33.1 [31.1–35.1] (10) | 0.276 (10) | 0.506 (10) | 20.7 (10) | 84,044 (10) |
| fx | 10 | 0 | 1,059 [1,059–1,063] (10) | 20.5 (10) | 2.12 (10) | 18.3 (10) | 59,767 (10) |
| pi | 10 | 0 | 299 [299–305] (10) | 2.60 (10) | 8.39 (10) | 126 (10) | 14,818 (10) |
| opencode | 10 | 0 | 1,632 [1,619–1,659] (10) | 10.6 (10) | 61.0 (10) | 1,304 (10) | 42,844 (10) |
| claude | 10 | 0 | 707 [697–717] (10) | 9.34 (10) | 19.8 (10) | 350 (10) | 60,630 (10) |
| codex | 10 | 0 | 2,012 [2,003–2,019] (10) | 38.4 (10) | 20.1 (10) | 172 (10) | 57,386 (10) |
| nclite | 10 | 0 | 55.2 [53.2–62.4] (10) | 0.790 (10) | 2.77 (10) | 26.0 (10) | 41,772 (10) |

## Fleet: 1 concurrent headless runs, 1,000 tokens each at 1,000 tok/s

| lane | samples | errors | wall ms | CPU per run ms | peak RSS per run MB | stream end to exit p50 ms |
|---|---:|---:|---:|---:|---:|---:|
| nc | 10 | 0 | 1,006 [1,006–1,024] (10) | 10.2 (10) | 15.3 (10) | 2.83 (10) |
| fx | 10 | 0 | 1,020 [1,020–1,030] (10) | 5.23 (10) | 7.88 (10) | 7.33 (10) |
| pi | 10 | 0 | 1,147 [1,144–1,149] (10) | 294 (10) | 124 (10) | 9.17 (10) |
| opencode | 10 | 0 | 2,020 [2,011–2,053] (10) | 2,084 (10) | 672 (10) | 38.1 (10) |
| claude | 10 | 0 | 1,174 [1,171–1,177] (10) | 335 (10) | 227 (10) | 18.4 (10) |
| codex | 10 | 0 | 1,068 [1,066–1,069] (10) | 102 (10) | 146 (10) | 12.3 (10) |
| nclite | 10 | 0 | 1,004 [1,004–1,005] (10) | 9.45 (10) | 7.88 (10) | 1.60 (10) |

## Fleet: 8 concurrent headless runs, 1,000 tokens each at 1,000 tok/s

| lane | samples | errors | wall ms | CPU per run ms | peak RSS per run MB | stream end to exit p50 ms |
|---|---:|---:|---:|---:|---:|---:|
| nc | 10 | 0 | 1,012 [1,011–1,015] (10) | 12.0 (10) | 15.4 (10) | 2.28 (10) |
| fx | 10 | 0 | 1,025 [1,023–1,031] (10) | 5.34 (10) | 7.70 (10) | 7.79 (10) |
| pi | 10 | 0 | 1,276 [1,256–1,295] (10) | 474 (10) | 124 (10) | 9.57 (10) |
| opencode | 10 | 0 | 3,598 [3,547–3,627] (10) | 3,645 (10) | 631 (10) | 40.4 (10) |
| claude | 10 | 0 | 1,315 [1,309–1,325] (10) | 545 (10) | 228 (10) | 23.4 (10) |
| codex | 10 | 0 | 1,131 [1,122–1,134] (10) | 149 (10) | 149 (10) | 13.4 (10) |
| nclite | 10 | 0 | 1,008 [1,008–1,008] (10) | 10.9 (10) | 7.68 (10) | 1.80 (10) |

## Fleet: 32 concurrent headless runs, 1,000 tokens each at 1,000 tok/s

| lane | samples | errors | wall ms | CPU per run ms | peak RSS per run MB | stream end to exit p50 ms |
|---|---:|---:|---:|---:|---:|---:|
| nc | 10 | 0 | 1,041 [1,038–1,044] (10) | 13.5 (10) | 15.5 (10) | 2.20 (10) |
| fx | 10 | 0 | 1,045 [1,040–1,056] (10) | 6.17 (10) | 7.69 (10) | 7.50 (10) |
| pi | 10 | 0 | 2,392 [2,323–2,426] (10) | 606 (10) | 125 (10) | 11.5 (10) |
| opencode | 10 | 0 | 14,054 [13,928–14,230] (10) | 4,438 (10) | 583 (10) | 265 (10) |
| claude | 10 | 0 | 3,238 [3,027–3,375] (10) | 872 (10) | 238 (10) | 27.1 (10) |
| codex | 10 | 0 | 1,583 [1,557–1,590] (10) | 203 (10) | 179 (10) | 13.3 (10) |
| nclite | 9 | 1 | 1,024 [1,022–1,027] (9) | 12.5 (9) | 7.72 (9) | 1.71 (9) |

- nclite error: missing tokens

## Install size

Runtimes a lane needs but does not install (Node for npm packages) are not counted; VERSIONS.md lists them.

| lane | samples | errors | install MB | main binary MB |
|---|---:|---:|---:|---:|
| nc | 0 | 1 | failed (1) | failed (1) |
| fx | 1 | 0 | 12.2 (1) | 12.2 (1) |
| pi | 1 | 0 | 159 (1) | 0.000 (1) |
| opencode | 1 | 0 | 409 (1) | 204 (1) |
| claude | 1 | 0 | 346 (1) | 346 (1) |
| codex | 1 | 0 | 403 (1) | 281 (1) |
| nclite | 1 | 0 | 18.2 (1) | 18.2 (1) |

- nc error: set BENCH_INSTALL_NC to the paths the install added, separated by colons

## Streaming, terminal interface 120x40

**on-screen lag p50 ms**

| lane | 100 tok/s | 1,000 tok/s |
|---|---:|---:|
| nc | 0.064 (5) | 4.04 (5) |
| fx | 200 (5) | 32.7 (5) |
| pi | 7.68 (5) | 7.86 (5) |
| opencode | 19.6 (5) | 17.6 (5) |
| claude | 151 (5) | 63.3 (5) |
| codex | 1.50 (5) | 5.52 (5) |
| nclite | 0.017 (5) | 0.012 (5) |

**on-screen lag p99 ms**

| lane | 100 tok/s | 1,000 tok/s |
|---|---:|---:|
| nc | 0.184 (5) | 8.16 (5) |
| fx | 403 (5) | 54.2 (5) |
| pi | 15.7 (5) | 15.3 (5) |
| opencode | 32.7 (5) | 32.2 (5) |
| claude | 331 (5) | 118 (5) |
| codex | 11.5 (5) | 15.5 (5) |
| nclite | 0.033 (5) | 0.015 (5) |

**stream end to last token on screen ms**

| lane | 100 tok/s | 1,000 tok/s |
|---|---:|---:|
| nc | 0.154 (5) | 0.522 (5) |
| fx | 15.8 (5) | 12.2 (5) |
| pi | 13.0 (5) | 13.8 (5) |
| opencode | 19.3 (5) | 19.7 (5) |
| claude | 19.3 (5) | 22.5 (5) |
| codex | 2.28 (5) | 11.2 (5) |
| nclite | 0.004 (5) | 0.002 (5) |

**Enter to request ms**

| lane | 100 tok/s | 1,000 tok/s |
|---|---:|---:|
| nc | 1.39 (5) | 1.25 (5) |
| fx | 3.32 (5) | 3.18 (5) |
| pi | 17.6 (5) | 16.7 (5) |
| opencode | 515 (5) | 500 (5) |
| claude | 33.7 (5) | 33.6 (5) |
| codex | 19.0 (5) | 27.6 (5) |
| nclite | 1.30 (5) | 1.40 (5) |

**CPU, whole session ms**

| lane | 100 tok/s | 1,000 tok/s |
|---|---:|---:|
| nc | 33.6 (5) | 57.0 (5) |
| fx | 49.3 (5) | 240 (5) |
| pi | 542 (5) | 904 (5) |
| opencode | 5,134 (5) | 5,802 (5) |
| claude | 811 (5) | 1,042 (5) |
| codex | 535 (5) | 1,398 (5) |
| nclite | 8.10 (5) | 39.6 (5) |

**peak RSS MB**

| lane | 100 tok/s | 1,000 tok/s |
|---|---:|---:|
| nc | 17.2 (5) | 17.9 (5) |
| fx | 12.7 (5) | 16.3 (5) |
| pi | 138 (5) | 145 (5) |
| opencode | 983 (5) | 956 (5) |
| claude | 267 (5) | 286 (5) |
| codex | 196 (5) | 199 (5) |
| nclite | 8.09 (5) | 8.44 (5) |

**terminal bytes per token**

| lane | 100 tok/s | 1,000 tok/s |
|---|---:|---:|
| nc | 190 (5) | 314 (5) |
| fx | 39.4 (5) | 73.8 (5) |
| pi | 240 (5) | 254 (5) |
| opencode | 232 (5) | 421 (5) |
| claude | 93.2 (5) | 36.9 (5) |
| codex | 368 (5) | 394 (5) |
| nclite | 5.44 (5) | 5.89 (5) |

**tokens never on screen**

| lane | 100 tok/s | 1,000 tok/s |
|---|---:|---:|
| nc | 0.000 (5) | 0.000 (5) |
| fx | 0.000 (5) | 0.000 (5) |
| pi | 0.000 (5) | 0.000 (5) |
| opencode | 0.000 (5) | 0.000 (5) |
| claude | 0.000 (5) | 0.000 (5) |
| codex | 0.000 (5) | 0.000 (5) |
| nclite | 0.000 (5) | 0.000 (5) |

## Where nc leads

- Idle terminal interface, idle CPU ms/s: nc -0.008 against fx 0.485 (-61.21x better)
- Streaming TUI 120x40 at 100 tok/s, on-screen lag p99 ms: nc 0.184 against codex 11.5 (62.68x better)
- Streaming TUI 120x40 at 100 tok/s, on-screen lag p50 ms: nc 0.064 against codex 1.50 (23.59x better)
- Streaming TUI 120x40 at 1,000 tok/s, stream end to last token on screen ms: nc 0.522 against codex 11.2 (21.38x better)
- Streaming TUI 120x40 at 100 tok/s, stream end to last token on screen ms: nc 0.154 against codex 2.28 (14.86x better)
- Long tool turn: 10 shell steps, headless, wall ms: nc 15.1 against pi 191 (12.64x better)
- Long tool turn: 10 shell steps, headless, tool result to next request p50 ms: nc 0.314 against pi 3.82 (12.16x better)
- Long tool turn: 50 shell steps, headless, tool result to next request p50 ms: nc 0.276 against pi 2.60 (9.43x better)
- Long tool turn: 50 shell steps, headless, wall ms: nc 33.1 against pi 299 (9.03x better)
- One-shot headless prompt, reply end to exit ms: nc 1.83 against codex 10.3 (5.62x better)
- One-shot headless prompt, wall ms: nc 5.12 against fx 22.9 (4.47x better)
- Streaming TUI 120x40 at 1,000 tok/s, CPU, whole session ms: nc 57.0 against fx 240 (4.21x better)
- Long tool turn: 50 shell steps, headless, CPU per step ms: nc 0.506 against fx 2.12 (4.20x better)
- One-shot headless prompt, spawn to request ms: nc 3.00 against fx 12.2 (4.08x better)
- Fleet: 8 concurrent headless runs, 1,000 tokens each at 1,000 tok/s, stream end to exit p50 ms: nc 2.28 against fx 7.79 (3.42x better)
- Fleet: 32 concurrent headless runs, 1,000 tokens each at 1,000 tok/s, stream end to exit p50 ms: nc 2.20 against fx 7.50 (3.41x better)
- Fleet: 1 concurrent headless runs, 1,000 tokens each at 1,000 tok/s, stream end to exit p50 ms: nc 2.83 against fx 7.33 (2.59x better)
- Streaming TUI 120x40 at 1,000 tok/s, Enter to request ms: nc 1.25 against fx 3.18 (2.54x better)
- Streaming TUI 120x40 at 100 tok/s, Enter to request ms: nc 1.39 against fx 3.32 (2.38x better)
- Streaming TUI 120x40 at 1,000 tok/s, on-screen lag p99 ms: nc 8.16 against pi 15.3 (1.87x better)
- Long tool turn: 10 shell steps, headless, CPU per step ms: nc 1.34 against fx 2.41 (1.80x better)
- Streaming TUI 120x40 at 100 tok/s, CPU, whole session ms: nc 33.6 against fx 49.3 (1.46x better)
- Streaming TUI 120x40 at 1,000 tok/s, on-screen lag p50 ms: nc 4.04 against codex 5.52 (1.37x better)
- Time to first frame, first frame ms: nc 1.73 against fx 1.87 (1.08x better)

## Where nc trails

- Install size, install MB: nc failed
- Install size, main binary MB: nc failed
- One-shot headless prompt, first request bytes: nc 62,620 against pi 5,444 (11.50x worse)
- Long tool turn: 10 shell steps, headless, last request bytes: nc 66,923 against pi 7,337 (9.12x worse)
- Streaming TUI 120x40 at 1,000 tok/s, terminal bytes per token: nc 314 against claude 36.9 (8.52x worse)
- Long tool turn: 50 shell steps, headless, last request bytes: nc 84,044 against pi 14,818 (5.67x worse)
- Streaming TUI 120x40 at 100 tok/s, terminal bytes per token: nc 190 against fx 39.4 (4.84x worse)
- Startup: --version, CPU ms: nc 0.494 against fx 0.115 (4.32x worse)
- Startup: --version, peak RSS MB: nc 4.63 against fx 1.38 (3.36x worse)
- One-shot headless prompt, CPU ms: nc 5.37 against fx 1.67 (3.21x worse)
- Memory one second after the first frame, anonymous MB: nc 3.00 against fx 1.25 (2.40x worse)
- Fleet: 8 concurrent headless runs, 1,000 tokens each at 1,000 tok/s, CPU per run ms: nc 12.0 against fx 5.34 (2.24x worse)
- Fleet: 32 concurrent headless runs, 1,000 tokens each at 1,000 tok/s, CPU per run ms: nc 13.5 against fx 6.17 (2.19x worse)
- Fleet: 32 concurrent headless runs, 1,000 tokens each at 1,000 tok/s, peak RSS per run MB: nc 15.5 against fx 7.69 (2.02x worse)
- Fleet: 8 concurrent headless runs, 1,000 tokens each at 1,000 tok/s, peak RSS per run MB: nc 15.4 against fx 7.70 (2.00x worse)
- Fleet: 1 concurrent headless runs, 1,000 tokens each at 1,000 tok/s, CPU per run ms: nc 10.2 against fx 5.23 (1.96x worse)
- Fleet: 1 concurrent headless runs, 1,000 tokens each at 1,000 tok/s, peak RSS per run MB: nc 15.3 against fx 7.88 (1.94x worse)
- One-shot headless prompt, peak RSS MB: nc 14.8 against fx 7.63 (1.93x worse)
- Memory one second after the first frame, RSS MB: nc 14.2 against fx 8.92 (1.59x worse)
- Time to first frame, peak RSS MB: nc 14.2 against fx 8.98 (1.59x worse)
- Idle terminal interface, peak RSS MB: nc 14.2 against fx 9.47 (1.50x worse)
- Time to first frame, CPU ms: nc 5.48 against fx 3.78 (1.45x worse)
- Long tool turn: 10 shell steps, headless, peak RSS MB: nc 21.4 against fx 15.3 (1.40x worse)
- Memory one second after the first frame, PSS MB: nc 12.1 against fx 8.90 (1.36x worse)
- Time to first frame, first byte ms: nc 1.28 against fx 0.944 (1.36x worse)
- Streaming TUI 120x40 at 100 tok/s, peak RSS MB: nc 17.2 against fx 12.7 (1.35x worse)
- Memory one second after the first frame, private (USS) MB: nc 11.7 against fx 8.90 (1.32x worse)
- Long tool turn: 50 shell steps, headless, peak RSS MB: nc 20.7 against fx 18.3 (1.13x worse)
- Streaming TUI 120x40 at 1,000 tok/s, peak RSS MB: nc 17.9 against fx 16.3 (1.10x worse)

## Ties (within 5%)

- Startup: --version, wall ms: nc 2.73 against fx 2.66
- Idle terminal interface, bytes written while idle: nc 0.000 against fx 0.000
- One-shot headless prompt, side requests: nc 0.000 against fx 0.000
- One-shot headless prompt, processes left behind: nc 0.000 against fx 0.000
- Fleet: 1 concurrent headless runs, 1,000 tokens each at 1,000 tok/s, wall ms: nc 1,006 against fx 1,020
- Fleet: 8 concurrent headless runs, 1,000 tokens each at 1,000 tok/s, wall ms: nc 1,012 against fx 1,025
- Fleet: 32 concurrent headless runs, 1,000 tokens each at 1,000 tok/s, wall ms: nc 1,041 against fx 1,045
- Streaming TUI 120x40 at 100 tok/s, tokens never on screen: nc 0.000 against fx 0.000
- Streaming TUI 120x40 at 1,000 tok/s, tokens never on screen: nc 0.000 against fx 0.000
