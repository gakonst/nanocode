# CLI benchmark: 2026-11-14-fra2-epyc9254

Machine: AMD EPYC 9254 24-Core Processor, 48 threads, 377 GB, Ubuntu 24.04.5 LTS, 7.0.0-43-generic. 

# Installed CLIs

Written by tools/install.sh on Linux x86_64. The size is the main binary's; BENCH_INSTALL_<LANE> in env.sh is the whole install directory.

| Lane | Version | Install | Date (UTC) | Main binary | Runtime not counted |
| --- | --- | --- | --- | --- | --- |
| nc | nanocode 0.7.0 | scripts/build-cli.sh nanocode-cli nanocode at https://github.com/nanocodexyz/nanocode 41dcbc7c483536ae5ed6c8da34f16177462156ed (dist, non-PIE, BOLT 19) | 2026-11-14T06:47Z | `tools/install/nc/nanocode` (45.2 MiB) | none |
| nclite | nc-lite 0.7.0 | PROFILE=lite scripts/build-cli.sh nc-lite nc-lite at https://github.com/nanocodexyz/nanocode 7837aae16afe1e46fbf72471c902f5cf2e683270 | 2026-11-14T06:46Z | `tools/install/nclite/nc-lite` (18.2 MiB) | none |
| fx | 0.0.16 | curl https://releases.fx.sh/$(latest.txt)/fx-linux-x86_64.tar.gz (the archive https://fx.sh/setup.sh fetches; binary only) | 2026-11-14T06:46Z | `tools/install/fx/fx` (12.2 MiB) | none |
| pi | 1.5.2 | npm install --prefix tools/install/pi @earendil-works/pi-coding-agent@latest | 2026-11-14T06:46Z | `tools/install/pi/node_modules/@earendil-works/pi-coding-agent/dist/bundle/cli.js` (0.0 MiB) | Node v22.23.5 |
| opencode | 1.19.5 | npm install --prefix tools/install/opencode opencode-ai@latest (the package's bin is the native binary from its platform package) | 2026-11-14T06:46Z | `tools/install/opencode/node_modules/opencode-ai/bin/opencode.exe` (204.5 MiB) | none |
| claude | 2.1.325 (Claude Code) | npm install --prefix tools/install/claude @anthropic-ai/claude-code@latest (the package's bin is the native binary from its platform package) | 2026-11-14T06:46Z | `tools/install/claude/node_modules/@anthropic-ai/claude-code/bin/claude.exe` (348.9 MiB) | none |
| codex | codex-cli 0.169.0 | npm install --prefix tools/install/codex @openai/codex@latest (CODEX_BIN is the platform binary: the npm codex command is a Node launcher that execs it) | 2026-11-14T06:46Z | `tools/install/codex/node_modules/@openai/codex-linux-x64/vendor/x86_64-unknown-linux-musl/bin/codex` (280.6 MiB) | none (the npm launcher needs Node) |

Medians with the sample count in parentheses and [min–max] where the spread matters. CPU and memory are the whole process tree (cgroup or /proc sampling; rusage when neither is available). Lower is better everywhere.

## Startup: --version

| lane | samples | errors | wall ms | CPU ms | peak RSS MB | instructions |
|---|---:|---:|---:|---:|---:|---:|
| true | 100 | 0 | 6.87 [6.13–152] (100) | 0.470 (100) | 1.17 (100) | n/a |
| nc | 100 | 0 | 7.11 [6.01–7.45] (100) | 0.873 (100) | 3.60 (100) | n/a |
| fx | 100 | 0 | 7.09 [5.95–7.55] (100) | 0.411 (100) | 1.14 (100) | n/a |
| pi | 100 | 0 | 163 [160–167] (100) | 201 (100) | 97.0 (100) | n/a |
| opencode | 100 | 0 | 301 [297–305] (100) | 406 (100) | 130 (100) | n/a |
| claude | 100 | 0 | 10.4 [10.1–12.5] (100) | 8.51 (100) | 22.7 (100) | n/a |
| codex | 100 | 0 | 10.3 [10.1–10.6] (100) | 13.3 (100) | 18.7 (100) | n/a |
| nclite | 100 | 0 | 6.82 [5.95–7.41] (100) | 0.744 (100) | 3.64 (100) | n/a |

## Cold start: --version after dropping the page cache

| lane | samples | errors | wall ms | CPU ms | peak RSS MB |
|---|---:|---:|---:|---:|---:|
| nc | 0 | 10 | failed (10) | failed (10) | failed (10) |
| fx | 0 | 10 | failed (10) | failed (10) | failed (10) |
| pi | 0 | 10 | failed (10) | failed (10) | failed (10) |
| opencode | 0 | 10 | failed (10) | failed (10) | failed (10) |
| claude | 0 | 10 | failed (10) | failed (10) | failed (10) |
| codex | 0 | 10 | failed (10) | failed (10) | failed (10) |
| nclite | 0 | 10 | failed (10) | failed (10) | failed (10) |

- nc error: cannot drop the page cache (needs Linux and passwordless sudo)
- fx error: cannot drop the page cache (needs Linux and passwordless sudo)
- pi error: cannot drop the page cache (needs Linux and passwordless sudo)
- opencode error: cannot drop the page cache (needs Linux and passwordless sudo)
- claude error: cannot drop the page cache (needs Linux and passwordless sudo)
- codex error: cannot drop the page cache (needs Linux and passwordless sudo)
- nclite error: cannot drop the page cache (needs Linux and passwordless sudo)

## Time to first frame

| lane | samples | errors | first byte ms | first frame ms | CPU ms | peak RSS MB | processes |
|---|---:|---:|---:|---:|---:|---:|---:|
| nc | 20 | 0 | 3.05 (20) | 3.94 [3.83–4.01] (20) | 6.95 (20) | 14.7 (20) | 0.000 (20) |
| fx | 20 | 0 | 2.88 (20) | 5.24 [5.02–5.51] (20) | 13.1 (20) | 9.85 (20) | 0.000 (20) |
| pi | 20 | 0 | 200 (20) | 208 [206–225] (20) | 351 (20) | 115 (20) | 1.00 (20) |
| opencode | 20 | 0 | 415 (20) | 2,046 [2,006–2,076] (20) | 3,787 (20) | 722 (20) | 0.000 (20) |
| claude | 20 | 0 | 142 (20) | 247 [245–251] (20) | 660 (20) | 233 (20) | 6.00 (20) |
| codex | 20 | 0 | 11.3 (20) | 220 [214–226] (20) | 348 (20) | 176 (20) | 7.00 (20) |
| nclite | 20 | 0 | 2.71 (20) | 2.71 [2.63–2.85] (20) | 2.14 (20) | 5.58 (20) | 0.000 (20) |

## Memory one second after the first frame

Summed over the CLI's process tree from /proc/<pid>/smaps_rollup. RSS counts the executable's file pages, which every instance shares; PSS splits shared pages between the processes that map them; USS is memory only this tree holds.

| lane | samples | errors | RSS MB | PSS MB | private (USS) MB | anonymous MB |
|---|---:|---:|---:|---:|---:|---:|
| nc | 20 | 0 | 14.7 (20) | 12.6 (20) | 12.1 (20) | 2.18 (20) |
| fx | 20 | 0 | 9.21 (20) | 9.20 (20) | 9.19 (20) | 1.50 (20) |
| pi | 20 | 0 | 113 (20) | 75.4 (20) | 62.8 (20) | 61.5 (20) |
| opencode | 20 | 0 | n/a | n/a | n/a | n/a |
| claude | 20 | 0 | 216 (20) | 214 (20) | 214 (20) | 137 (20) |
| codex | 20 | 0 | 171 (20) | 171 (20) | 171 (20) | 35.2 (20) |
| nclite | 20 | 0 | 5.57 (20) | 3.49 (20) | 3.11 (20) | 0.348 (20) |

## Time to first frame, cold page cache

| lane | samples | errors | first frame ms | CPU ms |
|---|---:|---:|---:|---:|
| nc | 0 | 10 | failed (10) | failed (10) |
| fx | 0 | 10 | failed (10) | failed (10) |
| pi | 0 | 10 | failed (10) | failed (10) |
| opencode | 0 | 10 | failed (10) | failed (10) |
| claude | 0 | 10 | failed (10) | failed (10) |
| codex | 0 | 10 | failed (10) | failed (10) |
| nclite | 0 | 10 | failed (10) | failed (10) |

- nc error: cannot drop the page cache
- fx error: cannot drop the page cache
- pi error: cannot drop the page cache
- opencode error: cannot drop the page cache
- claude error: cannot drop the page cache
- codex error: cannot drop the page cache
- nclite error: cannot drop the page cache

## Keystroke echo

| lane | samples | errors | echo p50 ms | echo max ms | keys echoed of 26 |
|---|---:|---:|---:|---:|---:|
| nc | 10 | 0 | 0.218 [0.209–0.233] (10) | 0.320 (10) | 26.0 (10) |
| fx | 10 | 0 | 0.634 [0.606–0.639] (10) | 114 (10) | 26.0 (10) |
| pi | 10 | 0 | 0.668 [0.634–0.692] (10) | 3.44 (10) | 26.0 (10) |
| opencode | 10 | 0 | 2.76 [2.67–2.83] (10) | 21.5 (10) | 26.0 (10) |
| claude | 10 | 0 | 2.33 [2.24–2.39] (10) | 13.5 (10) | 26.0 (10) |
| codex | 10 | 0 | 21.5 [21.5–21.5] (10) | 22.6 (10) | 26.0 (10) |
| nclite | 10 | 0 | 0.153 [0.139–0.167] (10) | 0.185 (10) | 26.0 (10) |

## One-shot headless prompt

| lane | samples | errors | wall ms | spawn to request ms | reply end to exit ms | CPU ms | peak RSS MB | first request bytes | side requests | processes left behind |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| nc | 20 | 0 | 7.40 [7.05–7.93] (20) | 5.43 (20) | 1.64 (20) | 5.63 (20) | 14.9 (20) | 63,169 (20) | 0.000 (20) | 0.000 (20) |
| fx | 20 | 0 | 27.3 [26.9–29.4] (20) | 15.8 (20) | 11.2 (20) | 4.20 (20) | 7.70 (20) | 26,280 (20) | 0.000 (20) | 0.000 (20) |
| pi | 20 | 0 | 243 [241–247] (20) | 219 (20) | 24.3 (20) | 321 (20) | 111 (20) | 5,444 (20) | 0.000 (20) | 0.000 (20) |
| opencode | 20 | 0 | 1,544 [1,507–1,569] (20) | 1,466 (20) | 76.8 (20) | 2,574 (20) | 596 (20) | 30,705 (20) | 1.00 (20) | 0.000 (20) |
| claude | 20 | 0 | 267 [250–269] (20) | 232 (20) | 34.7 (20) | 308 (20) | 193 (20) | 46,419 (20) | 0.000 (20) | 0.000 (20) |
| codex | 20 | 0 | 130 [129–131] (20) | 109 (20) | 20.7 (20) | 181 (20) | 148 (20) | 39,556 (20) | 0.000 (20) | 0.000 (20) |
| nclite | 20 | 0 | 7.17 [6.45–7.46] (20) | 4.26 (20) | 2.63 (20) | 3.68 (20) | 8.30 (20) | 23,942 (20) | 0.000 (20) | 0.000 (20) |

## Long tool turn: 10 shell steps, headless

| lane | samples | errors | wall ms | tool result to next request p50 ms | CPU per step ms | peak RSS MB | last request bytes |
|---|---:|---:|---:|---:|---:|---:|---:|
| nc | 10 | 0 | 27.2 [27.2–28.6] (10) | 0.558 (10) | 2.26 (10) | 24.4 (10) | 67,472 (10) |
| fx | 10 | 0 | 250 [250–252] (10) | 20.8 (10) | 7.50 (10) | 17.2 (10) | 33,005 (10) |
| pi | 10 | 0 | 322 [319–324] (10) | 7.46 (10) | 46.5 (10) | 140 (10) | 7,337 (10) |
| opencode | 10 | 0 | 1,766 [1,742–1,780] (10) | 18.0 (10) | 308 (10) | 1,203 (10) | 33,163 (10) |
| claude | 10 | 0 | 438 [433–443] (10) | 13.4 (10) | 55.7 (10) | 252 (10) | 49,690 (10) |
| codex | 10 | 0 | 734 [731–738] (10) | 58.9 (10) | 59.8 (10) | 171 (10) | 43,145 (10) |
| nclite | 10 | 0 | 37.3 [36.8–37.4] (10) | 2.03 (10) | 10.3 (10) | 32.5 (10) | 27,531 (10) |

## Long tool turn: 50 shell steps, headless

| lane | samples | errors | wall ms | tool result to next request p50 ms | CPU per step ms | peak RSS MB | last request bytes |
|---|---:|---:|---:|---:|---:|---:|---:|
| nc | 10 | 0 | 65.5 [61.2–69.5] (10) | 0.509 (10) | 0.914 (10) | 24.4 (10) | 84,593 (10) |
| fx | 10 | 0 | 1,102 [1,101–1,104] (10) | 21.0 (10) | 6.32 (10) | 19.8 (10) | 59,766 (10) |
| pi | 10 | 0 | 546 [542–550] (10) | 5.51 (10) | 14.8 (10) | 138 (10) | 14,818 (10) |
| opencode | 10 | 0 | 2,433 [2,394–2,448] (10) | 15.1 (10) | 84.6 (10) | 1,229 (10) | 42,844 (10) |
| claude | 10 | 0 | 1,022 [994–1,046] (10) | 13.1 (10) | 27.7 (10) | 574 (10) | 60,638 (10) |
| codex | 10 | 0 | 3,092 [3,080–3,128] (10) | 58.5 (10) | 43.6 (10) | 308 (10) | 57,386 (10) |
| nclite | 10 | 0 | 132 [114–140] (10) | 1.69 (10) | 8.91 (10) | 34.7 (10) | 41,772 (10) |

## Fleet: 1 concurrent headless runs, 1,000 tokens each at 1,000 tok/s

| lane | samples | errors | wall ms | CPU per run ms | peak RSS per run MB | stream end to exit p50 ms |
|---|---:|---:|---:|---:|---:|---:|
| nc | 10 | 0 | 1,011 [1,011–1,012] (10) | 19.0 (10) | 16.2 (10) | 4.24 (10) |
| fx | 10 | 0 | 1,027 [1,027–1,049] (10) | 18.8 (10) | 7.74 (10) | 10.2 (10) |
| pi | 10 | 0 | 1,235 [1,233–1,239] (10) | 494 (10) | 124 (10) | 15.6 (10) |
| opencode | 10 | 0 | 2,535 [2,530–2,548] (10) | 2,896 (10) | 613 (10) | 68.8 (10) |
| claude | 10 | 0 | 1,266 [1,262–1,268] (10) | 521 (10) | 227 (10) | 32.0 (10) |
| codex | 10 | 0 | 1,135 [1,132–1,136] (10) | 237 (10) | 148 (10) | 22.1 (10) |
| nclite | 10 | 0 | 1,009 [1,009–1,009] (10) | 29.5 (10) | 8.43 (10) | 3.34 (10) |

## Fleet: 8 concurrent headless runs, 1,000 tokens each at 1,000 tok/s

| lane | samples | errors | wall ms | CPU per run ms | peak RSS per run MB | stream end to exit p50 ms |
|---|---:|---:|---:|---:|---:|---:|
| nc | 10 | 0 | 1,021 [1,017–1,024] (10) | 18.1 (10) | 16.1 (10) | 2.53 (10) |
| fx | 10 | 0 | 1,038 [1,033–1,039] (10) | 18.1 (10) | 7.77 (10) | 9.90 (10) |
| pi | 10 | 0 | 1,256 [1,251–1,266] (10) | 509 (10) | 124 (10) | 16.6 (10) |
| opencode | 10 | 0 | 2,738 [2,719–2,779] (10) | 3,232 (10) | 592 (10) | 76.6 (10) |
| claude | 10 | 0 | 1,302 [1,301–1,306] (10) | 560 (10) | 226 (10) | 44.4 (10) |
| codex | 10 | 0 | 1,179 [1,162–1,185] (10) | 260 (10) | 149 (10) | 21.6 (10) |
| nclite | 10 | 0 | 1,020 [1,017–1,021] (10) | 28.0 (10) | 8.18 (10) | 3.42 (10) |

## Fleet: 32 concurrent headless runs, 1,000 tokens each at 1,000 tok/s

| lane | samples | errors | wall ms | CPU per run ms | peak RSS per run MB | stream end to exit p50 ms |
|---|---:|---:|---:|---:|---:|---:|
| nc | 10 | 0 | 1,070 [1,060–1,075] (10) | 19.2 (10) | 16.3 (10) | 2.51 (10) |
| fx | 10 | 0 | 1,087 [1,074–1,098] (10) | 19.1 (10) | 7.74 (10) | 8.43 (10) |
| pi | 10 | 0 | 1,516 [1,491–1,537] (10) | 700 (10) | 125 (10) | 17.7 (10) |
| opencode | 10 | 0 | 5,114 [5,047–5,227] (10) | 4,799 (10) | 599 (10) | 86.9 (10) |
| claude | 10 | 0 | 1,562 [1,557–1,575] (10) | 793 (10) | 227 (10) | 49.8 (10) |
| codex | 10 | 0 | 1,488 [1,404–1,504] (10) | 332 (10) | 152 (10) | 21.1 (10) |
| nclite | 9 | 1 | 1,066 [1,061–1,068] (9) | 29.7 (9) | 8.15 (9) | 3.40 (9) |

- nclite error: missing tokens

## Session store: 1,000 one-turn sessions

| lane | samples | errors | store KB | KB per session | files | CPU per saved session ms |
|---|---:|---:|---:|---:|---:|---:|
| nc | 1 | 0 | 5,000 (1) | 5.00 (1) | 1,007 (1) | 6.79 (1) |
| fx | 1 | 0 | 12,776 (1) | 12.8 (1) | 2,005 (1) | 4.42 (1) |
| pi | 1 | 0 | 8,152 (1) | 8.15 (1) | 1,004 (1) | 322 (1) |
| opencode | 1 | 0 | 4,524 (1) | 4.52 (1) | 2.00 (1) | 568 (1) |
| claude | 1 | 0 | 132,120 (1) | 132 (1) | 1,004 (1) | 297 (1) |
| codex | 1 | 0 | 65,604 (1) | 65.6 (1) | 2,072 (1) | 147 (1) |
| nclite | 1 | 0 | 4,216 (1) | 4.22 (1) | 1,003 (1) | 18.8 (1) |

## Session store: 14,600 one-turn sessions

| lane | samples | errors | store KB | KB per session | files | CPU per saved session ms |
|---|---:|---:|---:|---:|---:|---:|
| nc | 1 | 0 | 71,796 (1) | 4.92 (1) | 14,607 (1) | 9.16 (1) |
| fx | 1 | 0 | 180,964 (1) | 12.4 (1) | 29,205 (1) | 19.7 (1) |

## Listing 1,000 sessions

| lane | samples | errors | wall ms | CPU ms | peak RSS MB | task-clock ms | user instructions | kernel instructions |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| nc | 20 | 0 | 13.5 [11.4–15.6] (20) | 8.85 (20) | 12.9 (20) | n/a | n/a | n/a |
| fx | 20 | 0 | 13.1 [12.9–13.5] (20) | 9.61 (20) | 6.41 (20) | n/a | n/a | n/a |
| opencode | 0 | 20 | failed (20) | failed (20) | failed (20) | failed (20) | failed (20) | failed (20) |
| nclite | 20 | 0 | 17.2 [17.0–17.6] (20) | 14.3 (20) | 8.50 (20) | n/a | n/a | n/a |

- opencode error: 

## Continuing the latest of 1,000 sessions

| lane | samples | errors | wall ms | spawn to request ms | CPU ms | peak RSS MB | task-clock ms | user instructions | kernel instructions |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| nc | 20 | 0 | 18.2 [17.0–21.2] (20) | 13.7 (20) | 15.1 (20) | 22.5 (20) | n/a | n/a | n/a |
| fx | 20 | 0 | 37.2 [35.3–37.5] (20) | 23.8 (20) | 12.7 (20) | 9.26 (20) | n/a | n/a | n/a |
| pi | 20 | 0 | 252 [248–254] (20) | 231 (20) | 330 (20) | 111 (20) | n/a | n/a | n/a |
| opencode | 0 | 20 | failed (20) | failed (20) | failed (20) | failed (20) | failed (20) | failed (20) | failed (20) |
| claude | 20 | 0 | 264 [260–268] (20) | 234 (20) | 309 (20) | 202 (20) | n/a | n/a | n/a |
| codex | 20 | 0 | 124 [122–130] (20) | 104 (20) | 182 (20) | 154 (20) | n/a | n/a | n/a |
| nclite | 20 | 0 | 45.3 [43.2–47.5] (20) | 18.1 (20) | 41.0 (20) | 14.3 (20) | n/a | n/a | n/a |

- opencode error: exit 1 requests 0: 

## Listing 14,600 sessions

| lane | samples | errors | wall ms | CPU ms | peak RSS MB | task-clock ms | user instructions | kernel instructions |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| nc | 10 | 0 | 588 [582–608] (10) | 582 (10) | 112 (10) | 556 (10) | 6,285,627,224 (10) | 215,650,442 (10) |
| fx | 10 | 0 | 89.7 [87.8–89.9] (10) | 85.2 (10) | 94.0 (10) | 63.7 (10) | 494,162,054 (10) | 104,156,911 (10) |

## Continuing the latest of 14,600 sessions

| lane | samples | errors | wall ms | spawn to request ms | CPU ms | peak RSS MB | task-clock ms | user instructions | kernel instructions |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| nc | 10 | 0 | 596 [588–616] (10) | 584 (10) | 592 (10) | 115 (10) | 566 (10) | 6,299,096,749 (10) | 223,470,969 (10) |
| fx | 10 | 0 | 113 [112–114] (10) | 96.8 (10) | 88.9 (10) | 95.2 (10) | 66.8 (10) | 497,724,247 (10) | 109,468,711 (10) |

## Install size

Runtimes a lane needs but does not install (Node for npm packages) are not counted; VERSIONS.md lists them.

| lane | samples | errors | install MB | main binary MB |
|---|---:|---:|---:|---:|
| nc | 1 | 0 | 61.9 (1) | 45.2 (1) |
| fx | 1 | 0 | 12.2 (1) | 12.2 (1) |
| pi | 1 | 0 | 159 (1) | 0.000 (1) |
| opencode | 1 | 0 | 832 (1) | 204 (1) |
| claude | 1 | 0 | 685 (1) | 349 (1) |
| codex | 1 | 0 | 403 (1) | 281 (1) |
| nclite | 1 | 0 | 18.2 (1) | 18.2 (1) |

## Streaming, terminal interface 120x40

**on-screen lag p50 ms**

| lane | 100 tok/s | 1,000 tok/s | 10,000 tok/s |
|---|---:|---:|---:|
| nc | 0.192 (10) | 4.13 (10) | 3.22 (5) |
| fx | 200 (10) | 33.1 (10) | 26.9 (6) |
| pi | 7.90 (10) | 8.47 (10) | 564 (5) |
| opencode | 19.9 (10) | 17.5 (10) | 15.8 (6) |
| claude | 152 (10) | 64.0 (10) | 58.5 (5) |
| codex | 1.90 (10) | 5.73 (10) | 2,168 (6) |
| nclite | 0.143 (10) | 0.041 (10) | 0.030 (5) |

**on-screen lag p99 ms**

| lane | 100 tok/s | 1,000 tok/s | 10,000 tok/s |
|---|---:|---:|---:|
| nc | 0.452 (10) | 8.35 (10) | 8.53 (5) |
| fx | 402 (10) | 54.7 (10) | 50.8 (6) |
| pi | 15.9 (10) | 16.5 (10) | 1,956 (5) |
| opencode | 33.2 (10) | 32.8 (10) | 35.9 (6) |
| claude | 333 (10) | 119 (10) | 117 (5) |
| codex | 11.8 (10) | 16.7 (10) | 4,117 (6) |
| nclite | 0.162 (10) | 0.055 (10) | 0.131 (5) |

**stream end to last token on screen ms**

| lane | 100 tok/s | 1,000 tok/s | 10,000 tok/s |
|---|---:|---:|---:|
| nc | 0.139 (10) | 0.283 (10) | 2.03 (5) |
| fx | 18.7 (10) | 16.5 (10) | 51.6 (6) |
| pi | 4.65 (10) | 10.2 (10) | 1,288 (5) |
| opencode | 25.4 (10) | 19.3 (10) | 21.0 (6) |
| claude | 24.9 (10) | 29.0 (10) | 83.3 (5) |
| codex | 1.99 (10) | 12.2 (10) | 4,341 (6) |
| nclite | 0.101 (10) | 0.043 (10) | -0.080 (5) |

**Enter to request ms**

| lane | 100 tok/s | 1,000 tok/s | 10,000 tok/s |
|---|---:|---:|---:|
| nc | 1.21 (10) | 1.23 (10) | 1.23 (5) |
| fx | 5.25 (10) | 5.15 (10) | 5.31 (6) |
| pi | 26.2 (10) | 26.2 (10) | 36.2 (5) |
| opencode | 960 (10) | 954 (10) | 951 (6) |
| claude | 49.3 (10) | 48.9 (10) | 49.3 (5) |
| codex | 41.5 (10) | 32.9 (10) | 36.2 (6) |
| nclite | 2.15 (10) | 2.21 (10) | 2.26 (5) |

**CPU, whole session ms**

| lane | 100 tok/s | 1,000 tok/s | 10,000 tok/s |
|---|---:|---:|---:|
| nc | 69.3 (10) | 143 (10) | 667 (5) |
| fx | 120 (10) | 408 (10) | 3,402 (6) |
| pi | 816 (10) | 1,457 (10) | 6,806 (5) |
| opencode | 7,211 (10) | 8,125 (10) | 13,647 (6) |
| claude | 1,244 (10) | 1,536 (10) | 3,217 (5) |
| codex | 981 (10) | 2,433 (10) | 14,689 (6) |
| nclite | 35.1 (10) | 125 (10) | 997 (5) |

**peak RSS MB**

| lane | 100 tok/s | 1,000 tok/s | 10,000 tok/s |
|---|---:|---:|---:|
| nc | 19.0 (10) | 19.8 (10) | 26.0 (5) |
| fx | 13.7 (10) | 18.9 (10) | 26.8 (6) |
| pi | 137 (10) | 144 (10) | 257 (5) |
| opencode | 973 (10) | 970 (10) | 1,005 (6) |
| claude | 261 (10) | 286 (10) | 356 (5) |
| codex | 200 (10) | 201 (10) | 244 (6) |
| nclite | 8.52 (10) | 9.11 (10) | 12.4 (5) |

**terminal bytes per token**

| lane | 100 tok/s | 1,000 tok/s | 10,000 tok/s |
|---|---:|---:|---:|
| nc | 191 (10) | 391 (10) | 55.7 (5) |
| fx | 39.4 (10) | 73.8 (10) | 26.4 (6) |
| pi | 238 (10) | 257 (10) | 25.1 (5) |
| opencode | 233 (10) | 418 (10) | 49.7 (6) |
| claude | 85.8 (10) | 37.5 (10) | 2.82 (5) |
| codex | 364 (10) | 400 (10) | 81.6 (6) |
| nclite | 5.44 (10) | 5.89 (10) | 6.86 (5) |

**tokens never on screen**

| lane | 100 tok/s | 1,000 tok/s | 10,000 tok/s |
|---|---:|---:|---:|
| nc | 0.000 (10) | 0.000 (10) | 0.000 (5) |
| fx | 0.000 (10) | 0.000 (10) | 0.000 (6) |
| pi | 0.000 (10) | 0.000 (10) | 0.000 (5) |
| opencode | 0.000 (10) | 0.000 (10) | 0.000 (6) |
| claude | 0.000 (10) | 0.000 (10) | 0.000 (5) |
| codex | 0.000 (10) | 0.000 (10) | 0.000 (6) |
| nclite | 0.000 (10) | 0.000 (10) | 0.000 (5) |

## Where nc leads

- Streaming TUI 120x40 at 1,000 tok/s, stream end to last token on screen ms: nc 0.283 against pi 10.2 (35.99x better)
- Streaming TUI 120x40 at 100 tok/s, on-screen lag p99 ms: nc 0.452 against codex 11.8 (26.00x better)
- Streaming TUI 120x40 at 100 tok/s, stream end to last token on screen ms: nc 0.139 against codex 1.99 (14.36x better)
- Long tool turn: 10 shell steps, headless, tool result to next request p50 ms: nc 0.558 against pi 7.46 (13.37x better)
- Long tool turn: 50 shell steps, headless, tool result to next request p50 ms: nc 0.509 against pi 5.51 (10.83x better)
- Keystroke echo, echo max ms: nc 0.320 against pi 3.44 (10.77x better)
- Streaming TUI 120x40 at 10,000 tok/s, stream end to last token on screen ms: nc 2.03 against opencode 21.0 (10.38x better)
- Streaming TUI 120x40 at 100 tok/s, on-screen lag p50 ms: nc 0.192 against codex 1.90 (9.91x better)
- Long tool turn: 10 shell steps, headless, wall ms: nc 27.2 against fx 250 (9.17x better)
- Long tool turn: 50 shell steps, headless, wall ms: nc 65.5 against pi 546 (8.33x better)
- Long tool turn: 50 shell steps, headless, CPU per step ms: nc 0.914 against fx 6.32 (6.92x better)
- One-shot headless prompt, reply end to exit ms: nc 1.64 against fx 11.2 (6.83x better)
- Streaming TUI 120x40 at 10,000 tok/s, on-screen lag p50 ms: nc 3.22 against opencode 15.8 (4.89x better)
- Streaming TUI 120x40 at 10,000 tok/s, CPU, whole session ms: nc 667 against claude 3,217 (4.82x better)
- Streaming TUI 120x40 at 100 tok/s, Enter to request ms: nc 1.21 against fx 5.25 (4.33x better)
- Streaming TUI 120x40 at 10,000 tok/s, Enter to request ms: nc 1.23 against fx 5.31 (4.33x better)
- Streaming TUI 120x40 at 10,000 tok/s, on-screen lag p99 ms: nc 8.53 against opencode 35.9 (4.20x better)
- Streaming TUI 120x40 at 1,000 tok/s, Enter to request ms: nc 1.23 against fx 5.15 (4.17x better)
- Fleet: 8 concurrent headless runs, 1,000 tokens each at 1,000 tok/s, stream end to exit p50 ms: nc 2.53 against fx 9.90 (3.92x better)
- One-shot headless prompt, wall ms: nc 7.40 against fx 27.3 (3.69x better)
- Fleet: 32 concurrent headless runs, 1,000 tokens each at 1,000 tok/s, stream end to exit p50 ms: nc 2.51 against fx 8.43 (3.36x better)
- Long tool turn: 10 shell steps, headless, CPU per step ms: nc 2.26 against fx 7.50 (3.32x better)
- Keystroke echo, echo p50 ms: nc 0.218 against fx 0.634 (2.91x better)
- One-shot headless prompt, spawn to request ms: nc 5.43 against fx 15.8 (2.90x better)
- Streaming TUI 120x40 at 1,000 tok/s, CPU, whole session ms: nc 143 against fx 408 (2.86x better)
- Session store: 14,600 one-turn sessions, store KB: nc 71,796 against fx 180,964 (2.52x better)
- Session store: 14,600 one-turn sessions, KB per session: nc 4.92 against fx 12.4 (2.52x better)
- Fleet: 1 concurrent headless runs, 1,000 tokens each at 1,000 tok/s, stream end to exit p50 ms: nc 4.24 against fx 10.2 (2.40x better)
- Session store: 14,600 one-turn sessions, CPU per saved session ms: nc 9.16 against fx 19.7 (2.15x better)
- Continuing the latest of 1,000 sessions, wall ms: nc 18.2 against fx 37.2 (2.05x better)
- Session store: 14,600 one-turn sessions, files: nc 14,607 against fx 29,205 (2.00x better)
- Streaming TUI 120x40 at 1,000 tok/s, on-screen lag p99 ms: nc 8.35 against pi 16.5 (1.97x better)
- Time to first frame, CPU ms: nc 6.95 against fx 13.1 (1.88x better)
- Continuing the latest of 1,000 sessions, spawn to request ms: nc 13.7 against fx 23.8 (1.73x better)
- Streaming TUI 120x40 at 100 tok/s, CPU, whole session ms: nc 69.3 against fx 120 (1.73x better)
- Streaming TUI 120x40 at 1,000 tok/s, on-screen lag p50 ms: nc 4.13 against codex 5.73 (1.39x better)
- Time to first frame, first frame ms: nc 3.94 against fx 5.24 (1.33x better)
- Listing 1,000 sessions, CPU ms: nc 8.85 against fx 9.61 (1.09x better)

## Where nc trails

- Install size, main binary MB: nc 45.2 against pi 0.000 (296106.20x worse)
- Session store: 1,000 one-turn sessions, files: nc 1,007 against opencode 2.00 (503.50x worse)
- Cold start: --version after dropping the page cache, wall ms: nc failed
- Cold start: --version after dropping the page cache, CPU ms: nc failed
- Cold start: --version after dropping the page cache, peak RSS MB: nc failed
- Time to first frame, cold page cache, first frame ms: nc failed
- Time to first frame, cold page cache, CPU ms: nc failed
- Listing 1,000 sessions, task-clock ms: nc failed
- Listing 1,000 sessions, user instructions: nc failed
- Listing 1,000 sessions, kernel instructions: nc failed
- Continuing the latest of 1,000 sessions, task-clock ms: nc failed
- Continuing the latest of 1,000 sessions, user instructions: nc failed
- Continuing the latest of 1,000 sessions, kernel instructions: nc failed
- Streaming TUI 120x40 at 10,000 tok/s, terminal bytes per token: nc 55.7 against claude 2.82 (19.77x worse)
- Listing 14,600 sessions, user instructions: nc 6,285,627,224 against fx 494,162,054 (12.72x worse)
- Continuing the latest of 14,600 sessions, user instructions: nc 6,299,096,749 against fx 497,724,247 (12.66x worse)
- One-shot headless prompt, first request bytes: nc 63,169 against pi 5,444 (11.60x worse)
- Streaming TUI 120x40 at 1,000 tok/s, terminal bytes per token: nc 391 against claude 37.5 (10.45x worse)
- Long tool turn: 10 shell steps, headless, last request bytes: nc 67,472 against pi 7,337 (9.20x worse)
- Listing 14,600 sessions, task-clock ms: nc 556 against fx 63.7 (8.73x worse)
- Continuing the latest of 14,600 sessions, task-clock ms: nc 566 against fx 66.8 (8.47x worse)
- Listing 14,600 sessions, CPU ms: nc 582 against fx 85.2 (6.83x worse)
- Continuing the latest of 14,600 sessions, CPU ms: nc 592 against fx 88.9 (6.66x worse)
- Listing 14,600 sessions, wall ms: nc 588 against fx 89.7 (6.55x worse)
- Continuing the latest of 14,600 sessions, spawn to request ms: nc 584 against fx 96.8 (6.03x worse)
- Long tool turn: 50 shell steps, headless, last request bytes: nc 84,593 against pi 14,818 (5.71x worse)
- Continuing the latest of 14,600 sessions, wall ms: nc 596 against fx 113 (5.29x worse)
- Install size, install MB: nc 61.9 against fx 12.2 (5.06x worse)
- Streaming TUI 120x40 at 100 tok/s, terminal bytes per token: nc 191 against fx 39.4 (4.84x worse)
- Startup: --version, peak RSS MB: nc 3.60 against fx 1.14 (3.15x worse)
- Continuing the latest of 1,000 sessions, peak RSS MB: nc 22.5 against fx 9.26 (2.43x worse)
- Startup: --version, CPU ms: nc 0.873 against fx 0.411 (2.12x worse)
- Fleet: 32 concurrent headless runs, 1,000 tokens each at 1,000 tok/s, peak RSS per run MB: nc 16.3 against fx 7.74 (2.10x worse)
- Fleet: 1 concurrent headless runs, 1,000 tokens each at 1,000 tok/s, peak RSS per run MB: nc 16.2 against fx 7.74 (2.09x worse)
- Fleet: 8 concurrent headless runs, 1,000 tokens each at 1,000 tok/s, peak RSS per run MB: nc 16.1 against fx 7.77 (2.08x worse)
- Listing 14,600 sessions, kernel instructions: nc 215,650,442 against fx 104,156,911 (2.07x worse)
- Continuing the latest of 14,600 sessions, kernel instructions: nc 223,470,969 against fx 109,468,711 (2.04x worse)
- Listing 1,000 sessions, peak RSS MB: nc 12.9 against fx 6.41 (2.01x worse)
- One-shot headless prompt, peak RSS MB: nc 14.9 against fx 7.70 (1.94x worse)
- Memory one second after the first frame, RSS MB: nc 14.7 against fx 9.21 (1.59x worse)
- Session store: 1,000 one-turn sessions, CPU per saved session ms: nc 6.79 against fx 4.42 (1.53x worse)
- Time to first frame, peak RSS MB: nc 14.7 against fx 9.85 (1.49x worse)
- Memory one second after the first frame, anonymous MB: nc 2.18 against fx 1.50 (1.45x worse)
- Long tool turn: 10 shell steps, headless, peak RSS MB: nc 24.4 against fx 17.2 (1.42x worse)
- Streaming TUI 120x40 at 100 tok/s, peak RSS MB: nc 19.0 against fx 13.7 (1.39x worse)
- Memory one second after the first frame, PSS MB: nc 12.6 against fx 9.20 (1.36x worse)
- One-shot headless prompt, CPU ms: nc 5.63 against fx 4.20 (1.34x worse)
- Memory one second after the first frame, private (USS) MB: nc 12.1 against fx 9.19 (1.32x worse)
- Long tool turn: 50 shell steps, headless, peak RSS MB: nc 24.4 against fx 19.8 (1.23x worse)
- Continuing the latest of 14,600 sessions, peak RSS MB: nc 115 against fx 95.2 (1.21x worse)
- Listing 14,600 sessions, peak RSS MB: nc 112 against fx 94.0 (1.19x worse)
- Continuing the latest of 1,000 sessions, CPU ms: nc 15.1 against fx 12.7 (1.19x worse)
- Session store: 1,000 one-turn sessions, store KB: nc 5,000 against opencode 4,524 (1.11x worse)
- Session store: 1,000 one-turn sessions, KB per session: nc 5.00 against opencode 4.52 (1.11x worse)
- Time to first frame, first byte ms: nc 3.05 against fx 2.88 (1.06x worse)

## Ties (within 5%)

- Startup: --version, wall ms: nc 7.11 against fx 7.09
- One-shot headless prompt, side requests: nc 0.000 against fx 0.000
- One-shot headless prompt, processes left behind: nc 0.000 against fx 0.000
- Fleet: 1 concurrent headless runs, 1,000 tokens each at 1,000 tok/s, wall ms: nc 1,011 against fx 1,027
- Fleet: 1 concurrent headless runs, 1,000 tokens each at 1,000 tok/s, CPU per run ms: nc 19.0 against fx 18.8
- Fleet: 8 concurrent headless runs, 1,000 tokens each at 1,000 tok/s, wall ms: nc 1,021 against fx 1,038
- Fleet: 8 concurrent headless runs, 1,000 tokens each at 1,000 tok/s, CPU per run ms: nc 18.1 against fx 18.1
- Fleet: 32 concurrent headless runs, 1,000 tokens each at 1,000 tok/s, wall ms: nc 1,070 against fx 1,087
- Fleet: 32 concurrent headless runs, 1,000 tokens each at 1,000 tok/s, CPU per run ms: nc 19.2 against fx 19.1
- Listing 1,000 sessions, wall ms: nc 13.5 against fx 13.1
- Streaming TUI 120x40 at 1,000 tok/s, peak RSS MB: nc 19.8 against fx 18.9
- Streaming TUI 120x40 at 10,000 tok/s, peak RSS MB: nc 26.0 against fx 26.8
- Streaming TUI 120x40 at 100 tok/s, tokens never on screen: nc 0.000 against fx 0.000
- Streaming TUI 120x40 at 1,000 tok/s, tokens never on screen: nc 0.000 against fx 0.000
- Streaming TUI 120x40 at 10,000 tok/s, tokens never on screen: nc 0.000 against fx 0.000
