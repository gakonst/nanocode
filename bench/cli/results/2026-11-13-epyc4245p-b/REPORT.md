# CLI benchmark: 2026-11-13-epyc4245p-b

Machine: AMD EPYC 4245P 6-Core Processor, 12 threads, 46 GB, Ubuntu 24.04.5 LTS, 6.8.0-149-generic. 

# Installed CLIs

Written by tools/install.sh on Linux x86_64. The size is the main binary's; BENCH_INSTALL_<LANE> in env.sh is the whole install directory.

| Lane | Version | Install | Date (UTC) | Main binary | Runtime not counted |
| --- | --- | --- | --- | --- | --- |
| nclite | nc-lite 0.7.0 | PROFILE=lite scripts/build-cli.sh nc-lite nc-lite at https://github.com/nanocodexyz/nanocode 7837aae16afe1e46fbf72471c902f5cf2e683270 | 2026-11-13T16:05Z | `tools/install/nclite/nc-lite` (18.2 MiB) | none |
| fx | 0.0.16 | curl https://releases.fx.sh/$(latest.txt)/fx-linux-x86_64.tar.gz (the archive https://fx.sh/setup.sh fetches; binary only) | 2026-11-13T16:05Z | `tools/install/fx/fx` (12.2 MiB) | none |
| pi | 1.5.2 | npm install --prefix tools/install/pi @earendil-works/pi-coding-agent@latest | 2026-11-13T16:05Z | `tools/install/pi/node_modules/@earendil-works/pi-coding-agent/dist/bundle/cli.js` (0.0 MiB) | Node v22.23.5 |
| opencode | 1.19.5 | npm install --prefix tools/install/opencode opencode-ai@latest (the package's bin is the native binary from its platform package) | 2026-11-13T16:05Z | `tools/install/opencode/node_modules/opencode-ai/bin/opencode.exe` (204.5 MiB) | none |
| claude | 2.1.323 (Claude Code) | npm install --prefix tools/install/claude @anthropic-ai/claude-code@latest (the package's bin is the native binary from its platform package) | 2026-11-13T16:05Z | `tools/install/claude/node_modules/@anthropic-ai/claude-code/bin/claude.exe` (345.6 MiB) | none |
| codex | codex-cli 0.169.0 | npm install --prefix tools/install/codex @openai/codex@latest (CODEX_BIN is the platform binary: the npm codex command is a Node launcher that execs it) | 2026-11-13T16:05Z | `tools/install/codex/node_modules/@openai/codex-linux-x64/vendor/x86_64-unknown-linux-musl/bin/codex` (280.6 MiB) | none (the npm launcher needs Node) |

Medians with the sample count in parentheses and [min–max] where the spread matters. CPU and memory are the whole process tree (cgroup or /proc sampling; rusage when neither is available). Lower is better everywhere.

## Startup: --version

| lane | samples | errors | wall ms | CPU ms | peak RSS MB | instructions |
|---|---:|---:|---:|---:|---:|---:|
| true | 100 | 0 | 2.79 [2.70–98.9] (100) | 0.188 (100) | 1.25 (100) | n/a |
| nc | 100 | 0 | 2.96 [2.64–3.23] (100) | 0.573 (100) | 5.00 (100) | n/a |
| fx | 100 | 0 | 3.00 [2.61–3.19] (100) | 0.145 (100) | 1.38 (100) | n/a |
| pi | 100 | 0 | 98.9 [96.8–103] (100) | 122 (100) | 96.9 (100) | n/a |
| opencode | 100 | 0 | 201 [197–205] (100) | 268 (100) | 130 (100) | n/a |
| claude | 100 | 0 | 4.72 [4.59–6.79] (100) | 4.18 (100) | 25.6 (100) | n/a |
| codex | 100 | 0 | 4.71 [4.64–4.79] (100) | 3.14 (100) | 16.5 (100) | n/a |
| nclite | 100 | 0 | 2.74 [2.66–2.99] (100) | 0.336 (100) | 3.38 (100) | n/a |

## Cold start: --version after dropping the page cache

| lane | samples | errors | wall ms | CPU ms | peak RSS MB |
|---|---:|---:|---:|---:|---:|
| nc | 5 | 0 | 7.21 [7.06–9.21] (5) | 1.40 (5) | 4.75 (5) |
| fx | 5 | 0 | 5.26 [5.23–6.56] (5) | 0.602 (5) | 1.38 (5) |
| pi | 5 | 0 | 115 [115–119] (5) | 126 (5) | 97.1 (5) |
| opencode | 5 | 0 | 239 [238–240] (5) | 283 (5) | 130 (5) |
| claude | 5 | 0 | 33.9 [33.7–36.0] (5) | 15.3 (5) | 24.9 (5) |
| codex | 5 | 0 | 25.9 [23.9–26.0] (5) | 7.34 (5) | 16.0 (5) |
| nclite | 5 | 0 | 7.38 [7.36–7.53] (5) | 0.826 (5) | 3.25 (5) |

## Time to first frame

| lane | samples | errors | first byte ms | first frame ms | CPU ms | peak RSS MB | processes |
|---|---:|---:|---:|---:|---:|---:|---:|
| nc | 20 | 0 | 1.35 (20) | 1.78 [1.71–2.09] (20) | 4.17 (20) | 14.5 (20) | 0.000 (20) |
| fx | 20 | 0 | 1.03 (20) | 2.03 [1.93–2.39] (20) | 4.02 (20) | 9.09 (20) | 0.000 (20) |
| pi | 20 | 0 | 123 (20) | 135 [127–141] (20) | 210 (20) | 115 (20) | 0.000 (20) |
| opencode | 20 | 0 | 285 (20) | 1,396 [1,357–1,423] (20) | 3,494 (20) | 925 (20) | 0.000 (20) |
| claude | 20 | 0 | 91.5 (20) | 163 [161–166] (20) | 423 (20) | 230 (20) | 4.00 (20) |
| codex | 20 | 0 | 4.68 (20) | 133 [128–141] (20) | 188 (20) | 172 (20) | 6.00 (20) |
| nclite | 20 | 0 | 0.914 (20) | 1.04 [0.908–1.38] (20) | 0.966 (20) | 5.26 (20) | 0.000 (20) |

## Memory one second after the first frame

Summed over the CLI's process tree from /proc/<pid>/smaps_rollup. RSS counts the executable's file pages, which every instance shares; PSS splits shared pages between the processes that map them; USS is memory only this tree holds.

| lane | samples | errors | RSS MB | PSS MB | private (USS) MB | anonymous MB |
|---|---:|---:|---:|---:|---:|---:|
| nc | 20 | 0 | 14.5 (20) | 12.4 (20) | 12.0 (20) | 2.98 (20) |
| fx | 20 | 0 | 8.92 (20) | 8.91 (20) | 8.90 (20) | 1.25 (20) |
| pi | 20 | 0 | 111 (20) | 73.8 (20) | 61.2 (20) | 59.9 (20) |
| opencode | 20 | 0 | n/a | n/a | n/a | n/a |
| claude | 20 | 0 | 219 (20) | 217 (20) | 217 (20) | 136 (20) |
| codex | 20 | 0 | 170 (20) | 170 (20) | 170 (20) | 34.2 (20) |
| nclite | 20 | 0 | 5.26 (20) | 3.27 (20) | 2.89 (20) | 0.348 (20) |

## Time to first frame, cold page cache

| lane | samples | errors | first frame ms | CPU ms |
|---|---:|---:|---:|---:|
| nc | 5 | 0 | 9.34 [9.09–9.70] (5) | 6.48 (5) |
| fx | 5 | 0 | 9.43 [9.20–9.77] (5) | 5.72 (5) |
| pi | 5 | 0 | 151 [143–152] (5) | 214 (5) |
| opencode | 5 | 0 | 1,434 [1,417–1,442] (5) | 3,422 (5) |
| claude | 5 | 0 | 229 [227–231] (5) | 450 (5) |
| codex | 5 | 0 | 240 [237–242] (5) | 228 (5) |
| nclite | 5 | 0 | 4.21 [3.99–4.44] (5) | 1.77 (5) |

## Idle terminal interface

| lane | samples | errors | idle CPU ms/s | bytes written while idle | peak RSS MB | processes |
|---|---:|---:|---:|---:|---:|---:|
| nc | 10 | 0 | -0.009 [-0.142–0.003] (10) | 0.000 (10) | 14.4 (10) | 0.000 (10) |
| fx | 10 | 0 | 0.527 [0.434–0.634] (10) | 0.000 (10) | 9.36 (10) | 0.000 (10) |
| pi | 10 | 0 | 1.85 [1.20–2.32] (10) | 0.000 (10) | 114 (10) | 0.000 (10) |
| opencode | 10 | 0 | 22.7 [14.1–25.9] (10) | 0.000 (10) | 911 (10) | 0.000 (10) |
| claude | 10 | 0 | 4.47 [3.93–5.54] (10) | 21.5 (10) | 230 (10) | 4.00 (10) |
| codex | 10 | 0 | 0.343 [-1.49–0.715] (10) | -0.500 (10) | 167 (10) | 5.50 (10) |
| nclite | 10 | 0 | -0.002 [-0.051–0.001] (10) | 0.000 (10) | 5.23 (10) | 0.000 (10) |

## Keystroke echo

| lane | samples | errors | echo p50 ms | echo max ms | keys echoed of 26 |
|---|---:|---:|---:|---:|---:|
| nc | 10 | 0 | 0.054 [0.034–0.056] (10) | 0.083 (10) | 26.0 (10) |
| fx | 10 | 0 | 0.321 [0.169–0.325] (10) | 113 (10) | 26.0 (10) |
| pi | 10 | 0 | 0.361 [0.323–0.380] (10) | 2.38 (10) | 26.0 (10) |
| opencode | 10 | 0 | 1.48 [1.41–1.53] (10) | 25.2 (10) | 26.0 (10) |
| claude | 10 | 0 | 1.38 [1.19–1.46] (10) | 10.1 (10) | 26.0 (10) |
| codex | 10 | 0 | 21.1 [20.8–21.1] (10) | 22.2 (10) | 26.0 (10) |
| nclite | 10 | 0 | 0.012 [0.009–0.013] (10) | 0.028 (10) | 26.0 (10) |

## One-shot headless prompt

| lane | samples | errors | wall ms | spawn to request ms | reply end to exit ms | CPU ms | peak RSS MB | first request bytes | side requests | processes left behind |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| nc | 20 | 0 | 5.16 [4.87–5.41] (20) | 3.22 (20) | 1.69 (20) | 4.05 (20) | 14.5 (20) | 62,620 (20) | 0.000 (20) | 0.000 (20) |
| fx | 20 | 0 | 23.2 [23.1–25.9] (20) | 12.5 (20) | 10.5 (20) | 1.81 (20) | 7.62 (20) | 26,281 (20) | 0.000 (20) | 0.000 (20) |
| pi | 20 | 0 | 149 [147–153] (20) | 135 (20) | 13.5 (20) | 194 (20) | 111 (20) | 5,444 (20) | 0.000 (20) | 0.000 (20) |
| opencode | 20 | 0 | 1,001 [987–1,035] (20) | 967 (20) | 33.2 (20) | 1,846 (20) | 636 (20) | 30,705 (20) | 1.00 (20) | 0.000 (20) |
| claude | 20 | 0 | 173 [167–181] (20) | 152 (20) | 20.7 (20) | 195 (20) | 197 (20) | 46,411 (20) | 0.000 (20) | 0.000 (20) |
| codex | 20 | 0 | 65.3 [60.9–69.3] (20) | 53.1 (20) | 11.8 (20) | 78.6 (20) | 143 (20) | 39,556 (20) | 0.000 (20) | 0.000 (20) |
| nclite | 20 | 0 | 2.88 [2.81–5.41] (20) | 2.02 (20) | 0.695 (20) | 2.02 (20) | 7.50 (20) | 23,942 (20) | 0.000 (20) | 0.000 (20) |

## Long tool turn: 10 shell steps, headless

| lane | samples | errors | wall ms | tool result to next request p50 ms | CPU per step ms | peak RSS MB | last request bytes |
|---|---:|---:|---:|---:|---:|---:|---:|
| nc | 10 | 0 | 14.9 [12.9–15.0] (10) | 0.304 (10) | 1.17 (10) | 21.4 (10) | 66,923 (10) |
| fx | 10 | 0 | 231 [229–241] (10) | 20.4 (10) | 2.51 (10) | 15.8 (10) | 33,006 (10) |
| pi | 10 | 0 | 190 [187–193] (10) | 3.75 (10) | 27.5 (10) | 126 (10) | 7,337 (10) |
| opencode | 10 | 0 | 1,166 [1,151–1,191] (10) | 11.7 (10) | 220 (10) | 771 (10) | 33,163 (10) |
| claude | 10 | 0 | 291 [285–297] (10) | 10.1 (10) | 37.3 (10) | 254 (10) | 49,682 (10) |
| codex | 10 | 0 | 463 [461–467] (10) | 38.6 (10) | 27.4 (10) | 167 (10) | 43,145 (10) |
| nclite | 10 | 0 | 16.9 [14.9–20.0] (10) | 0.819 (10) | 3.21 (10) | 22.3 (10) | 27,531 (10) |

## Long tool turn: 50 shell steps, headless

| lane | samples | errors | wall ms | tool result to next request p50 ms | CPU per step ms | peak RSS MB | last request bytes |
|---|---:|---:|---:|---:|---:|---:|---:|
| nc | 10 | 0 | 33.0 [31.0–37.0] (10) | 0.264 (10) | 0.466 (10) | 20.2 (10) | 84,044 (10) |
| fx | 10 | 0 | 1,059 [1,057–1,069] (10) | 20.5 (10) | 2.12 (10) | 18.0 (10) | 59,767 (10) |
| pi | 10 | 0 | 297 [293–299] (10) | 2.58 (10) | 8.25 (10) | 126 (10) | 14,818 (10) |
| opencode | 10 | 0 | 1,608 [1,593–1,625] (10) | 10.4 (10) | 60.0 (10) | 1,399 (10) | 42,844 (10) |
| claude | 10 | 0 | 708 [699–715] (10) | 9.32 (10) | 19.7 (10) | 302 (10) | 60,630 (10) |
| codex | 10 | 0 | 2,017 [2,011–2,027] (10) | 38.5 (10) | 20.2 (10) | 302 (10) | 57,386 (10) |
| nclite | 10 | 0 | 53.1 [53.0–55.1] (10) | 0.776 (10) | 2.71 (10) | 23.9 (10) | 41,772 (10) |

## Fleet: 1 concurrent headless runs, 1,000 tokens each at 1,000 tok/s

| lane | samples | errors | wall ms | CPU per run ms | peak RSS per run MB | stream end to exit p50 ms |
|---|---:|---:|---:|---:|---:|---:|
| nc | 10 | 0 | 1,006 [1,006–1,007] (10) | 8.65 (10) | 15.2 (10) | 2.71 (10) |
| fx | 10 | 0 | 1,020 [1,020–1,035] (10) | 5.12 (10) | 7.62 (10) | 7.44 (10) |
| pi | 10 | 0 | 1,145 [1,142–1,147] (10) | 290 (10) | 124 (10) | 8.75 (10) |
| opencode | 10 | 0 | 2,006 [1,989–2,039] (10) | 2,055 (10) | 605 (10) | 30.6 (10) |
| claude | 10 | 0 | 1,173 [1,173–1,177] (10) | 333 (10) | 227 (10) | 19.4 (10) |
| codex | 10 | 0 | 1,068 [1,067–1,070] (10) | 103 (10) | 144 (10) | 12.9 (10) |
| nclite | 10 | 0 | 1,004 [1,004–1,005] (10) | 9.41 (10) | 7.75 (10) | 1.63 (10) |

## Fleet: 8 concurrent headless runs, 1,000 tokens each at 1,000 tok/s

| lane | samples | errors | wall ms | CPU per run ms | peak RSS per run MB | stream end to exit p50 ms |
|---|---:|---:|---:|---:|---:|---:|
| nc | 10 | 0 | 1,010 [1,009–1,011] (10) | 9.59 (10) | 15.1 (10) | 2.47 (10) |
| fx | 10 | 0 | 1,025 [1,024–1,031] (10) | 5.35 (10) | 7.61 (10) | 7.83 (10) |
| pi | 10 | 0 | 1,267 [1,252–1,282] (10) | 465 (10) | 124 (10) | 9.33 (10) |
| opencode | 10 | 0 | 3,572 [3,521–3,612] (10) | 3,593 (10) | 628 (10) | 37.4 (10) |
| claude | 10 | 0 | 1,312 [1,309–1,321] (10) | 540 (10) | 227 (10) | 22.2 (10) |
| codex | 10 | 0 | 1,130 [1,125–1,138] (10) | 150 (10) | 145 (10) | 14.5 (10) |
| nclite | 10 | 0 | 1,008 [1,007–1,009] (10) | 10.8 (10) | 7.57 (10) | 1.94 (10) |

## Fleet: 32 concurrent headless runs, 1,000 tokens each at 1,000 tok/s

| lane | samples | errors | wall ms | CPU per run ms | peak RSS per run MB | stream end to exit p50 ms |
|---|---:|---:|---:|---:|---:|---:|
| nc | 9 | 1 | 1,033 [1,031–1,035] (9) | 10.9 (9) | 15.2 (9) | 2.33 (9) |
| fx | 10 | 0 | 1,044 [1,043–1,048] (10) | 6.04 (10) | 7.59 (10) | 7.49 (10) |
| pi | 10 | 0 | 2,365 [2,277–2,462] (10) | 599 (10) | 125 (10) | 11.5 (10) |
| opencode | 10 | 0 | 14,013 [13,804–14,104] (10) | 4,417 (10) | 581 (10) | 214 (10) |
| claude | 10 | 0 | 3,251 [3,112–3,438] (10) | 874 (10) | 235 (10) | 28.1 (10) |
| codex | 10 | 0 | 1,575 [1,546–1,590] (10) | 203 (10) | 176 (10) | 14.3 (10) |
| nclite | 10 | 0 | 1,024 [1,023–1,026] (10) | 12.5 (10) | 7.63 (10) | 1.77 (10) |

- nc error: nanocode: computer use unavailable, continuing without it: cua-driver is not installed: run nanocode cua install, or set CUA_DRIVER_PATH 

## Session store: 1,000 one-turn sessions

| lane | samples | errors | store KB | KB per session | files | CPU per saved session ms |
|---|---:|---:|---:|---:|---:|---:|
| nc | 1 | 0 | 4,336 (1) | 4.34 (1) | 1,006 (1) | 3.91 (1) |
| fx | 1 | 0 | 12,776 (1) | 12.8 (1) | 2,005 (1) | 1.60 (1) |
| pi | 1 | 0 | 8,148 (1) | 8.15 (1) | 1,004 (1) | 196 (1) |
| opencode | 1 | 0 | 16,628 (1) | 16.6 (1) | 2.00 (1) | 1,770 (1) |
| claude | 1 | 0 | 132,124 (1) | 132 (1) | 1,004 (1) | 190 (1) |
| codex | 1 | 0 | 65,652 (1) | 65.7 (1) | 2,072 (1) | 63.9 (1) |
| nclite | 1 | 0 | 4,220 (1) | 4.22 (1) | 1,003 (1) | 10.4 (1) |

## Listing 1,000 sessions

| lane | samples | errors | wall ms | CPU ms | peak RSS MB |
|---|---:|---:|---:|---:|---:|
| nc | 20 | 0 | 6.92 [6.84–8.96] (20) | 5.95 (20) | 13.0 (20) |
| fx | 20 | 0 | 6.82 [6.77–6.96] (20) | 5.04 (20) | 6.23 (20) |
| opencode | 20 | 0 | 343 [339–352] (20) | 594 (20) | 252 (20) |
| nclite | 20 | 0 | 8.88 [8.84–9.11] (20) | 6.93 (20) | 7.83 (20) |

## Continuing the latest of 1,000 sessions

| lane | samples | errors | wall ms | spawn to request ms | CPU ms | peak RSS MB |
|---|---:|---:|---:|---:|---:|---:|
| nc | 20 | 0 | 10.9 [10.8–12.8] (20) | 7.43 (20) | 9.13 (20) | 20.7 (20) |
| fx | 20 | 0 | 28.8 [26.8–28.9] (20) | 16.4 (20) | 6.19 (20) | 8.86 (20) |
| pi | 20 | 0 | 153 [151–157] (20) | 142 (20) | 200 (20) | 110 (20) |
| opencode | 20 | 0 | 924 [915–945] (20) | 885 (20) | 1,755 (20) | 654 (20) |
| claude | 20 | 0 | 171 [167–179] (20) | 155 (20) | 197 (20) | 205 (20) |
| codex | 20 | 0 | 61.4 [59.4–63.4] (20) | 49.7 (20) | 78.8 (20) | 149 (20) |
| nclite | 20 | 0 | 24.9 [24.8–26.9] (20) | 8.78 (20) | 22.9 (20) | 13.2 (20) |

## Install size

Runtimes a lane needs but does not install (Node for npm packages) are not counted; VERSIONS.md lists them.

| lane | samples | errors | install MB | main binary MB |
|---|---:|---:|---:|---:|
| nc | 1 | 0 | 69.8 (1) | 69.8 (1) |
| fx | 1 | 0 | 12.2 (1) | 12.2 (1) |
| pi | 1 | 0 | 159 (1) | 0.000 (1) |
| opencode | 1 | 0 | 409 (1) | 204 (1) |
| claude | 1 | 0 | 346 (1) | 346 (1) |
| codex | 1 | 0 | 403 (1) | 281 (1) |
| nclite | 1 | 0 | 18.2 (1) | 18.2 (1) |

## Streaming, terminal interface 240x70

**on-screen lag p50 ms**

| lane | 10,000 tok/s |
|---|---:|
| nc | 3.67 (3) |
| fx | 22.8 (2) |
| pi | 38.5 (2) |
| opencode | 13.4 (2) |
| claude | 56.5 (2) |
| codex | 490 (2) |
| nclite | 0.011 (2) |

**on-screen lag p99 ms**

| lane | 10,000 tok/s |
|---|---:|
| nc | 8.79 (3) |
| fx | 39.3 (2) |
| pi | 250 (2) |
| opencode | 32.3 (2) |
| claude | 108 (2) |
| codex | 1,111 (2) |
| nclite | 0.111 (2) |

**stream end to last token on screen ms**

| lane | 10,000 tok/s |
|---|---:|
| nc | 3.45 (3) |
| fx | 31.9 (2) |
| pi | 102 (2) |
| opencode | 13.3 (2) |
| claude | 57.5 (2) |
| codex | 1,163 (2) |
| nclite | -0.206 (2) |

**Enter to request ms**

| lane | 10,000 tok/s |
|---|---:|
| nc | 0.933 (3) |
| fx | 4.37 (2) |
| pi | 18.1 (2) |
| opencode | 493 (2) |
| claude | 34.7 (2) |
| codex | 27.0 (2) |
| nclite | 1.83 (2) |

**CPU, whole session ms**

| lane | 10,000 tok/s |
|---|---:|
| nc | 340 (3) |
| fx | 2,626 (2) |
| pi | 4,756 (2) |
| opencode | 9,409 (2) |
| claude | 1,929 (2) |
| codex | 8,840 (2) |
| nclite | 371 (2) |

**peak RSS MB**

| lane | 10,000 tok/s |
|---|---:|
| nc | 25.6 (3) |
| fx | 32.0 (2) |
| pi | 315 (2) |
| opencode | 981 (2) |
| claude | 350 (2) |
| codex | 222 (2) |
| nclite | 11.6 (2) |

**terminal bytes per token**

| lane | 10,000 tok/s |
|---|---:|
| nc | 144 (3) |
| fx | 55.6 (2) |
| pi | 64.0 (2) |
| opencode | 132 (2) |
| claude | 6.52 (2) |
| codex | 140 (2) |
| nclite | 6.86 (2) |

**tokens never on screen**

| lane | 10,000 tok/s |
|---|---:|
| nc | 0.000 (3) |
| fx | 0.000 (2) |
| pi | 0.000 (2) |
| opencode | 0.000 (2) |
| claude | 0.000 (2) |
| codex | 0.000 (2) |
| nclite | 0.000 (2) |

## Where nc leads

- Idle terminal interface, idle CPU ms/s: nc -0.009 against codex 0.343 (-37.98x better)
- Keystroke echo, echo max ms: nc 0.083 against pi 2.38 (28.59x better)
- Long tool turn: 10 shell steps, headless, wall ms: nc 14.9 against pi 190 (12.71x better)
- Long tool turn: 10 shell steps, headless, tool result to next request p50 ms: nc 0.304 against pi 3.75 (12.36x better)
- Long tool turn: 50 shell steps, headless, tool result to next request p50 ms: nc 0.264 against pi 2.58 (9.76x better)
- Long tool turn: 50 shell steps, headless, wall ms: nc 33.0 against pi 297 (9.01x better)
- One-shot headless prompt, reply end to exit ms: nc 1.69 against fx 10.5 (6.19x better)
- Keystroke echo, echo p50 ms: nc 0.054 against fx 0.321 (5.92x better)
- Streaming TUI 240x70 at 10,000 tok/s, CPU, whole session ms: nc 340 against claude 1,929 (5.67x better)
- Streaming TUI 240x70 at 10,000 tok/s, Enter to request ms: nc 0.933 against fx 4.37 (4.68x better)
- Long tool turn: 50 shell steps, headless, CPU per step ms: nc 0.466 against fx 2.12 (4.56x better)
- One-shot headless prompt, wall ms: nc 5.16 against fx 23.2 (4.50x better)
- One-shot headless prompt, spawn to request ms: nc 3.22 against fx 12.5 (3.89x better)
- Streaming TUI 240x70 at 10,000 tok/s, stream end to last token on screen ms: nc 3.45 against opencode 13.3 (3.86x better)
- Streaming TUI 240x70 at 10,000 tok/s, on-screen lag p99 ms: nc 8.79 against opencode 32.3 (3.67x better)
- Streaming TUI 240x70 at 10,000 tok/s, on-screen lag p50 ms: nc 3.67 against opencode 13.4 (3.65x better)
- Fleet: 32 concurrent headless runs, 1,000 tokens each at 1,000 tok/s, stream end to exit p50 ms: nc 2.33 against fx 7.49 (3.21x better)
- Fleet: 8 concurrent headless runs, 1,000 tokens each at 1,000 tok/s, stream end to exit p50 ms: nc 2.47 against fx 7.83 (3.16x better)
- Fleet: 1 concurrent headless runs, 1,000 tokens each at 1,000 tok/s, stream end to exit p50 ms: nc 2.71 against fx 7.44 (2.75x better)
- Continuing the latest of 1,000 sessions, wall ms: nc 10.9 against fx 28.8 (2.65x better)
- Continuing the latest of 1,000 sessions, spawn to request ms: nc 7.43 against fx 16.4 (2.20x better)
- Long tool turn: 10 shell steps, headless, CPU per step ms: nc 1.17 against fx 2.51 (2.15x better)
- Session store: 1,000 one-turn sessions, store KB: nc 4,336 against pi 8,148 (1.88x better)
- Session store: 1,000 one-turn sessions, KB per session: nc 4.34 against pi 8.15 (1.88x better)
- Streaming TUI 240x70 at 10,000 tok/s, peak RSS MB: nc 25.6 against fx 32.0 (1.25x better)
- Time to first frame, first frame ms: nc 1.78 against fx 2.03 (1.14x better)

## Where nc trails

- Install size, main binary MB: nc 69.8 against pi 0.000 (457485.70x worse)
- Session store: 1,000 one-turn sessions, files: nc 1,006 against opencode 2.00 (503.00x worse)
- Streaming TUI 240x70 at 10,000 tok/s, terminal bytes per token: nc 144 against claude 6.52 (22.12x worse)
- One-shot headless prompt, first request bytes: nc 62,620 against pi 5,444 (11.50x worse)
- Long tool turn: 10 shell steps, headless, last request bytes: nc 66,923 against pi 7,337 (9.12x worse)
- Install size, install MB: nc 69.8 against fx 12.2 (5.71x worse)
- Long tool turn: 50 shell steps, headless, last request bytes: nc 84,044 against pi 14,818 (5.67x worse)
- Startup: --version, CPU ms: nc 0.573 against fx 0.145 (3.94x worse)
- Startup: --version, peak RSS MB: nc 5.00 against fx 1.38 (3.64x worse)
- Cold start: --version after dropping the page cache, peak RSS MB: nc 4.75 against fx 1.38 (3.45x worse)
- Session store: 1,000 one-turn sessions, CPU per saved session ms: nc 3.91 against fx 1.60 (2.44x worse)
- Memory one second after the first frame, anonymous MB: nc 2.98 against fx 1.25 (2.38x worse)
- Continuing the latest of 1,000 sessions, peak RSS MB: nc 20.7 against fx 8.86 (2.33x worse)
- Cold start: --version after dropping the page cache, CPU ms: nc 1.40 against fx 0.602 (2.33x worse)
- One-shot headless prompt, CPU ms: nc 4.05 against fx 1.81 (2.24x worse)
- Listing 1,000 sessions, peak RSS MB: nc 13.0 against fx 6.23 (2.09x worse)
- Fleet: 32 concurrent headless runs, 1,000 tokens each at 1,000 tok/s, peak RSS per run MB: nc 15.2 against fx 7.59 (2.00x worse)
- Fleet: 1 concurrent headless runs, 1,000 tokens each at 1,000 tok/s, peak RSS per run MB: nc 15.2 against fx 7.62 (2.00x worse)
- Fleet: 8 concurrent headless runs, 1,000 tokens each at 1,000 tok/s, peak RSS per run MB: nc 15.1 against fx 7.61 (1.98x worse)
- One-shot headless prompt, peak RSS MB: nc 14.5 against fx 7.62 (1.90x worse)
- Fleet: 32 concurrent headless runs, 1,000 tokens each at 1,000 tok/s, CPU per run ms: nc 10.9 against fx 6.04 (1.80x worse)
- Fleet: 8 concurrent headless runs, 1,000 tokens each at 1,000 tok/s, CPU per run ms: nc 9.59 against fx 5.35 (1.79x worse)
- Fleet: 1 concurrent headless runs, 1,000 tokens each at 1,000 tok/s, CPU per run ms: nc 8.65 against fx 5.12 (1.69x worse)
- Memory one second after the first frame, RSS MB: nc 14.5 against fx 8.92 (1.62x worse)
- Time to first frame, peak RSS MB: nc 14.5 against fx 9.09 (1.59x worse)
- Idle terminal interface, peak RSS MB: nc 14.4 against fx 9.36 (1.54x worse)
- Continuing the latest of 1,000 sessions, CPU ms: nc 9.13 against fx 6.19 (1.48x worse)
- Memory one second after the first frame, PSS MB: nc 12.4 against fx 8.91 (1.39x worse)
- Cold start: --version after dropping the page cache, wall ms: nc 7.21 against fx 5.26 (1.37x worse)
- Long tool turn: 10 shell steps, headless, peak RSS MB: nc 21.4 against fx 15.8 (1.35x worse)
- Memory one second after the first frame, private (USS) MB: nc 12.0 against fx 8.90 (1.35x worse)
- Time to first frame, first byte ms: nc 1.35 against fx 1.03 (1.30x worse)
- Listing 1,000 sessions, CPU ms: nc 5.95 against fx 5.04 (1.18x worse)
- Time to first frame, cold page cache, CPU ms: nc 6.48 against fx 5.72 (1.13x worse)
- Long tool turn: 50 shell steps, headless, peak RSS MB: nc 20.2 against fx 18.0 (1.12x worse)
- Idle terminal interface, bytes written while idle: nc 0.000 against codex -0.500 (-Infinityx better)

## Ties (within 5%)

- Startup: --version, wall ms: nc 2.96 against fx 3.00
- Time to first frame, CPU ms: nc 4.17 against fx 4.02
- Time to first frame, cold page cache, first frame ms: nc 9.34 against fx 9.43
- One-shot headless prompt, side requests: nc 0.000 against fx 0.000
- One-shot headless prompt, processes left behind: nc 0.000 against fx 0.000
- Fleet: 1 concurrent headless runs, 1,000 tokens each at 1,000 tok/s, wall ms: nc 1,006 against fx 1,020
- Fleet: 8 concurrent headless runs, 1,000 tokens each at 1,000 tok/s, wall ms: nc 1,010 against fx 1,025
- Fleet: 32 concurrent headless runs, 1,000 tokens each at 1,000 tok/s, wall ms: nc 1,033 against fx 1,044
- Listing 1,000 sessions, wall ms: nc 6.92 against fx 6.82
- Streaming TUI 240x70 at 10,000 tok/s, tokens never on screen: nc 0.000 against fx 0.000
