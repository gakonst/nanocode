# CLI benchmark: 2026-11-12-epyc4245p

Machine: AMD EPYC 4245P 6-Core Processor, 12 threads, 46 GB, Ubuntu 24.04.5 LTS, 6.8.0-147-generic. 

# Installed CLIs

Written by tools/install.sh on Linux x86_64. The size is the main binary's; BENCH_INSTALL_<LANE> in env.sh is the whole install directory.

| Lane | Version | Install | Date (UTC) | Main binary | Runtime not counted |
| --- | --- | --- | --- | --- | --- |
| fx | 0.0.16 | curl https://releases.fx.sh/$(latest.txt)/fx-linux-x86_64.tar.gz (the archive https://fx.sh/setup.sh fetches; binary only) | 2026-11-12T03:11Z | `tools/install/fx/fx` (12.2 MiB) | none |
| pi | 1.5.2 | npm install --prefix tools/install/pi @earendil-works/pi-coding-agent@latest | 2026-11-12T03:11Z | `tools/install/pi/node_modules/@earendil-works/pi-coding-agent/dist/bundle/cli.js` (0.0 MiB) | Node v24.22.3 |
| opencode | 1.19.5 | npm install --prefix tools/install/opencode opencode-ai@latest (the package's bin is the native binary from its platform package) | 2026-11-12T03:11Z | `tools/install/opencode/node_modules/opencode-ai/bin/opencode.exe` (204.5 MiB) | none |
| claude | 2.1.322 (Claude Code) | npm install --prefix tools/install/claude @anthropic-ai/claude-code@latest (the package's bin is the native binary from its platform package) | 2026-11-12T03:11Z | `tools/install/claude/node_modules/@anthropic-ai/claude-code/bin/claude.exe` (326.1 MiB) | none |
| codex | codex-cli 0.167.2 | npm install --prefix tools/install/codex @openai/codex@latest (CODEX_BIN is the platform binary: the npm codex command is a Node launcher that execs it) | 2026-11-12T03:11Z | `tools/install/codex/node_modules/@openai/codex-linux-x64/vendor/x86_64-unknown-linux-musl/bin/codex` (305.4 MiB) | none (the npm launcher needs Node) |
| nc | nanocode 0.7.0 (3c16e6c8e) | cargo build --locked --profile dist -p nanocode-cli --bin nanocode at nanocodexyz/nanocode 3c16e6c8e | | `tools/install/nc/nanocode` (71.4 MiB) | none |
| nclite | nc-lite 0.7.0 (3c16e6c8e) | cargo build --locked --profile lite -p nc-lite at nanocodexyz/nanocode 3c16e6c8e | | `tools/install/nclite/nc-lite` (14.1 MiB) | none |

Medians with the sample count in parentheses and [min–max] where the spread matters. CPU and memory are the whole process tree (cgroup or /proc sampling; rusage when neither is available). Lower is better everywhere.

## Startup: --version

| lane | samples | errors | wall ms | CPU ms | peak RSS MB | instructions |
|---|---:|---:|---:|---:|---:|---:|
| true | 100 | 0 | 2.82 [1.38–137] (100) | 0.181 (100) | 1.25 (100) | n/a |
| nc | 100 | 0 | 2.68 [2.61–2.91] (100) | 1.18 (100) | 9.13 (100) | n/a |
| fx | 100 | 0 | 2.74 [2.65–2.88] (100) | 0.154 (100) | 1.38 (100) | n/a |
| pi | 100 | 0 | 101 [96.8–105] (100) | 126 (100) | 103 (100) | n/a |
| opencode | 100 | 0 | 201 [199–209] (100) | 267 (100) | 130 (100) | n/a |
| claude | 100 | 0 | 5.05 [4.68–7.05] (100) | 4.09 (100) | 21.9 (100) | n/a |
| codex | 100 | 0 | 4.78 [4.69–4.90] (100) | 4.23 (100) | 25.6 (100) | n/a |
| nclite | 100 | 0 | 2.69 [2.63–2.87] (100) | 0.548 (100) | 4.75 (100) | n/a |

## Cold start: --version after dropping the page cache

| lane | samples | errors | wall ms | CPU ms | peak RSS MB |
|---|---:|---:|---:|---:|---:|
| nc | 10 | 0 | 11.1 [11.0–11.3] (10) | 2.64 (10) | 9.13 (10) |
| fx | 10 | 0 | 5.32 [5.27–5.58] (10) | 0.622 (10) | 1.38 (10) |
| pi | 10 | 0 | 119 [115–121] (10) | 130 (10) | 103 (10) |
| opencode | 10 | 0 | 236 [236–244] (10) | 281 (10) | 130 (10) |
| claude | 10 | 0 | 29.8 [29.7–31.9] (10) | 13.0 (10) | 21.6 (10) |
| codex | 10 | 0 | 27.6 [27.5–27.7] (10) | 9.62 (10) | 25.8 (10) |
| nclite | 10 | 0 | 7.07 [6.95–7.19] (10) | 1.21 (10) | 4.63 (10) |

## Time to first frame

| lane | samples | errors | first byte ms | first frame ms | CPU ms | peak RSS MB | processes |
|---|---:|---:|---:|---:|---:|---:|---:|
| nc | 30 | 0 | 2.26 (30) | 2.89 [2.67–3.31] (30) | 8.56 (30) | 31.0 (30) | 0.000 (30) |
| fx | 30 | 0 | 1.03 (30) | 2.01 [1.89–2.36] (30) | 3.90 (30) | 8.89 (30) | 0.000 (30) |
| pi | 30 | 0 | 126 (30) | 138 [127–145] (30) | 216 (30) | 121 (30) | 0.000 (30) |
| opencode | 30 | 0 | 285 (30) | 1,376 [1,357–1,399] (30) | 3,435 (30) | 923 (30) | 0.000 (30) |
| claude | 30 | 0 | 84.8 (30) | 151 [149–152] (30) | 404 (30) | 218 (30) | 4.00 (30) |
| codex | 30 | 0 | 5.76 (30) | 126 [118–129] (30) | 190 (30) | 177 (30) | 8.00 (30) |
| nclite | 30 | 0 | 1.14 (30) | 1.25 [1.11–1.77] (30) | 1.37 (30) | 10.0 (30) | 0.000 (30) |

## Memory one second after the first frame

Summed over the CLI's process tree from /proc/<pid>/smaps_rollup. RSS counts the executable's file pages, which every instance shares; PSS splits shared pages between the processes that map them; USS is memory only this tree holds.

| lane | samples | errors | RSS MB | PSS MB | private (USS) MB | anonymous MB |
|---|---:|---:|---:|---:|---:|---:|
| nc | 30 | 0 | 30.9 (30) | 28.5 (30) | 28.3 (30) | 5.97 (30) |
| fx | 30 | 0 | 8.80 (30) | 8.79 (30) | 8.78 (30) | 1.25 (30) |
| pi | 30 | 0 | 121 (30) | 86.2 (30) | 74.8 (30) | 67.7 (30) |
| opencode | 30 | 0 | n/a | n/a | n/a | n/a |
| claude | 30 | 0 | 209 (30) | 207 (30) | 207 (30) | 133 (30) |
| codex | 30 | 0 | 175 (30) | 175 (30) | 175 (30) | 34.6 (30) |
| nclite | 30 | 0 | 10.0 (30) | 7.85 (30) | 7.64 (30) | 0.887 (30) |

## Time to first frame, cold page cache

| lane | samples | errors | first frame ms | CPU ms |
|---|---:|---:|---:|---:|
| nc | 10 | 0 | 20.6 [20.3–21.0] (10) | 14.5 (10) |
| fx | 10 | 0 | 9.48 [9.00–9.84] (10) | 5.72 (10) |
| pi | 10 | 0 | 155 [146–157] (10) | 220 (10) |
| opencode | 10 | 0 | 1,409 [1,402–1,425] (10) | 3,427 (10) |
| claude | 10 | 0 | 213 [210–215] (10) | 430 (10) |
| codex | 10 | 0 | 223 [220–226] (10) | 222 (10) |
| nclite | 10 | 0 | 4.83 [4.42–5.14] (10) | 2.92 (10) |

## Idle terminal interface

| lane | samples | errors | idle CPU ms/s | bytes written while idle | peak RSS MB | processes |
|---|---:|---:|---:|---:|---:|---:|
| nc | 10 | 0 | -0.009 [-0.073–0.036] (10) | 0.000 (10) | 30.9 (10) | 0.000 (10) |
| fx | 10 | 0 | 0.550 [0.372–0.646] (10) | 0.000 (10) | 9.54 (10) | 0.000 (10) |
| pi | 10 | 0 | -0.031 [-0.402–0.638] (10) | 0.000 (10) | 121 (10) | 0.000 (10) |
| opencode | 10 | 0 | 24.2 [20.1–27.0] (10) | 0.000 (10) | 926 (10) | 0.000 (10) |
| claude | 10 | 0 | 5.46 [3.29–6.42] (10) | 241 (10) | 219 (10) | 4.00 (10) |
| codex | 10 | 0 | 0.413 [0.249–0.705] (10) | 0.000 (10) | 176 (10) | 7.00 (10) |
| nclite | 10 | 0 | -0.002 [-0.011–0.010] (10) | 0.000 (10) | 10.1 (10) | 0.000 (10) |

## Keystroke echo

| lane | samples | errors | echo p50 ms | echo max ms | keys echoed of 26 |
|---|---:|---:|---:|---:|---:|
| nc | 10 | 0 | 0.078 [0.076–0.081] (10) | 0.100 (10) | 26.0 (10) |
| fx | 10 | 0 | 0.315 [0.308–0.319] (10) | 113 (10) | 26.0 (10) |
| pi | 10 | 0 | 0.317 [0.198–0.406] (10) | 2.18 (10) | 26.0 (10) |
| opencode | 10 | 0 | 1.50 [1.43–1.56] (10) | 21.4 (10) | 26.0 (10) |
| claude | 10 | 0 | 1.30 [0.992–1.36] (10) | 9.83 (10) | 26.0 (10) |
| codex | 10 | 0 | 20.9 [20.8–21.0] (10) | 22.0 (10) | 26.0 (10) |
| nclite | 10 | 0 | 0.013 [0.011–0.014] (10) | 0.031 (10) | 26.0 (10) |

## One-shot headless prompt

| lane | samples | errors | wall ms | spawn to request ms | reply end to exit ms | CPU ms | peak RSS MB | first request bytes | side requests | processes left behind |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| nc | 30 | 0 | 6.87 [6.79–7.04] (30) | 4.27 (30) | 2.42 (30) | 7.36 (30) | 33.6 (30) | 62,483 (30) | 0.000 (30) | 0.000 (30) |
| fx | 30 | 0 | 23.2 [23.1–33.2] (30) | 12.5 (30) | 10.5 (30) | 1.87 (30) | 7.52 (30) | 26,281 (30) | 0.000 (30) | 0.000 (30) |
| pi | 30 | 0 | 151 [149–157] (30) | 139 (30) | 12.7 (30) | 203 (30) | 118 (30) | 5,444 (30) | 0.000 (30) | 0.000 (30) |
| opencode | 30 | 0 | 997 [987–1,019] (30) | 963 (30) | 33.4 (30) | 1,853 (30) | 676 (30) | 30,705 (30) | 1.00 (30) | 0.000 (30) |
| claude | 30 | 0 | 163 [159–167] (30) | 144 (30) | 18.9 (30) | 183 (30) | 186 (30) | 46,398 (30) | 0.000 (30) | 0.000 (30) |
| codex | 30 | 0 | 5,045 [5,043–5,047] (30) | 51.5 (30) | 4,993 (30) | 90.4 (30) | 153 (30) | 39,395 (30) | 0.000 (30) | 0.000 (30) |
| nclite | 30 | 0 | 4.89 [4.79–5.19] (30) | 2.52 (30) | 2.21 (30) | 2.70 (30) | 14.1 (30) | 23,942 (30) | 0.000 (30) | 0.000 (30) |

## Long tool turn: 10 shell steps, headless

| lane | samples | errors | wall ms | tool result to next request p50 ms | CPU per step ms | peak RSS MB | last request bytes |
|---|---:|---:|---:|---:|---:|---:|---:|
| nc | 10 | 0 | 23.0 [23.0–27.0] (10) | 0.990 (10) | 4.18 (10) | 49.1 (10) | 66,786 (10) |
| fx | 10 | 0 | 231 [229–241] (10) | 20.4 (10) | 2.52 (10) | 16.2 (10) | 33,006 (10) |
| pi | 10 | 0 | 189 [185–193] (10) | 3.38 (10) | 27.8 (10) | 138 (10) | 7,337 (10) |
| opencode | 10 | 0 | 1,169 [1,147–1,189] (10) | 12.0 (10) | 218 (10) | 762 (10) | 33,163 (10) |
| claude | 10 | 0 | 275 [269–281] (10) | 9.11 (10) | 35.0 (10) | 245 (10) | 49,669 (10) |
| codex | 10 | 0 | 5,045 [5,043–5,045] (10) | 41.3 (10) | 31.0 (10) | 185 (10) | 42,984 (10) |
| nclite | 10 | 0 | 17.0 [16.9–19.0] (10) | 0.861 (10) | 3.32 (10) | 27.4 (10) | 27,531 (10) |

## Long tool turn: 50 shell steps, headless

| lane | samples | errors | wall ms | tool result to next request p50 ms | CPU per step ms | peak RSS MB | last request bytes |
|---|---:|---:|---:|---:|---:|---:|---:|
| nc | 10 | 0 | 69.1 [67.1–71.2] (10) | 0.903 (10) | 3.01 (10) | 51.7 (10) | 83,907 (10) |
| fx | 10 | 0 | 1,057 [1,049–1,061] (10) | 20.5 (10) | 2.12 (10) | 17.4 (10) | 59,767 (10) |
| pi | 10 | 0 | 299 [293–303] (10) | 2.56 (10) | 8.47 (10) | 163 (10) | 14,818 (10) |
| opencode | 10 | 0 | 1,593 [1,579–1,627] (10) | 10.3 (10) | 59.4 (10) | 770 (10) | 42,844 (10) |
| claude | 10 | 0 | 656 [653–665] (10) | 8.57 (10) | 18.4 (10) | 287 (10) | 60,617 (10) |
| codex | 10 | 0 | 5,045 [5,045–5,047] (10) | 41.0 (10) | 23.0 (10) | 319 (10) | 57,225 (10) |
| nclite | 10 | 0 | 55.1 [55.1–59.2] (10) | 0.813 (10) | 2.77 (10) | 29.8 (10) | 41,772 (10) |

## Fleet: 1 concurrent headless runs, 1,000 tokens each at 1,000 tok/s

| lane | samples | errors | wall ms | CPU per run ms | peak RSS per run MB | stream end to exit p50 ms |
|---|---:|---:|---:|---:|---:|---:|
| nc | 10 | 0 | 1,008 [1,008–1,033] (10) | 12.4 (10) | 33.1 (10) | 3.31 (10) |
| fx | 10 | 0 | 1,020 [1,020–1,051] (10) | 5.16 (10) | 7.62 (10) | 7.39 (10) |
| pi | 10 | 0 | 1,151 [1,147–1,161] (10) | 281 (10) | 138 (10) | 10.9 (10) |
| opencode | 10 | 0 | 2,001 [1,974–2,055] (10) | 2,020 (10) | 603 (10) | 28.6 (10) |
| claude | 10 | 0 | 1,163 [1,161–1,206] (10) | 314 (10) | 220 (10) | 18.3 (10) |
| codex | 10 | 0 | 5,047 [5,045–5,127] (10) | 111 (10) | 153 (10) | 3,995 (10) |
| nclite | 10 | 0 | 1,004 [1,004–1,007] (10) | 10.3 (10) | 14.1 (10) | 1.18 (10) |

## Fleet: 8 concurrent headless runs, 1,000 tokens each at 1,000 tok/s

| lane | samples | errors | wall ms | CPU per run ms | peak RSS per run MB | stream end to exit p50 ms |
|---|---:|---:|---:|---:|---:|---:|
| nc | 10 | 0 | 1,015 [1,014–1,016] (10) | 15.1 (10) | 33.1 (10) | 2.87 (10) |
| fx | 10 | 0 | 1,025 [1,025–1,035] (10) | 5.34 (10) | 7.56 (10) | 7.76 (10) |
| pi | 10 | 0 | 1,296 [1,281–1,302] (10) | 457 (10) | 136 (10) | 12.6 (10) |
| opencode | 10 | 0 | 3,555 [3,508–3,614] (10) | 3,588 (10) | 637 (10) | 39.6 (10) |
| claude | 10 | 0 | 1,295 [1,291–1,301] (10) | 513 (10) | 219 (10) | 22.3 (10) |
| codex | 10 | 0 | 5,103 [5,101–5,120] (10) | 158 (10) | 157 (10) | 3,973 (10) |
| nclite | 10 | 0 | 1,009 [1,008–1,010] (10) | 12.1 (10) | 14.1 (10) | 1.65 (10) |

## Fleet: 32 concurrent headless runs, 1,000 tokens each at 1,000 tok/s

| lane | samples | errors | wall ms | CPU per run ms | peak RSS per run MB | stream end to exit p50 ms |
|---|---:|---:|---:|---:|---:|---:|
| nc | 10 | 0 | 1,055 [1,050–1,058] (10) | 17.8 (10) | 33.2 (10) | 2.42 (10) |
| fx | 10 | 0 | 1,051 [1,048–1,063] (10) | 5.92 (10) | 7.55 (10) | 7.58 (10) |
| pi | 10 | 0 | 2,476 [2,410–2,561] (10) | 594 (10) | 135 (10) | 13.3 (10) |
| opencode | 10 | 0 | 13,997 [13,673–14,243] (10) | 4,453 (10) | 584 (10) | 275 (10) |
| claude | 10 | 0 | 3,082 [2,976–3,355] (10) | 835 (10) | 221 (10) | 26.7 (10) |
| codex | 10 | 0 | 5,569 [5,541–5,584] (10) | 210 (10) | 197 (10) | 3,873 (10) |
| nclite | 8 | 2 | 1,034 [1,030–1,035] (8) | 19.3 (8) | 14.1 (8) | 1.43 (8) |

- nclite error: missing tokens

## Session store: 1,000 one-turn sessions

| lane | samples | errors | store KB | KB per session | files | CPU per saved session ms |
|---|---:|---:|---:|---:|---:|---:|
| nc | 1 | 0 | 4,264 (1) | 4.26 (1) | 1,006 (1) | 11.4 (1) |
| fx | 1 | 0 | 12,780 (1) | 12.8 (1) | 2,005 (1) | 1.57 (1) |
| pi | 1 | 0 | 8,144 (1) | 8.14 (1) | 1,004 (1) | 203 (1) |
| claude | 1 | 0 | 132,132 (1) | 132 (1) | 1,004 (1) | 176 (1) |
| nclite | 1 | 0 | 4,216 (1) | 4.22 (1) | 1,003 (1) | 9.34 (1) |

## Listing 1,000 sessions

| lane | samples | errors | wall ms | CPU ms | peak RSS MB |
|---|---:|---:|---:|---:|---:|
| nc | 20 | 0 | 9.23 [7.11–9.31] (20) | 6.50 (20) | 20.0 (20) |
| fx | 20 | 0 | 6.88 [6.83–7.00] (20) | 4.94 (20) | 6.28 (20) |
| nclite | 20 | 0 | 8.98 [8.90–9.10] (20) | 6.72 (20) | 11.1 (20) |

## Continuing the latest of 1,000 sessions

| lane | samples | errors | wall ms | spawn to request ms | CPU ms | peak RSS MB |
|---|---:|---:|---:|---:|---:|---:|
| nc | 20 | 0 | 19.1 [17.0–23.1] (20) | 8.92 (20) | 18.7 (20) | 41.9 (20) |
| fx | 20 | 0 | 28.9 [26.8–28.9] (20) | 16.3 (20) | 6.17 (20) | 8.88 (20) |
| pi | 20 | 0 | 155 [154–160] (20) | 143 (20) | 206 (20) | 114 (20) |
| claude | 20 | 0 | 166 [162–168] (20) | 148 (20) | 187 (20) | 195 (20) |
| nclite | 20 | 0 | 22.9 [20.9–23.0] (20) | 8.77 (20) | 20.2 (20) | 18.6 (20) |

## Install size

Runtimes a lane needs but does not install (Node for npm packages) are not counted; VERSIONS.md lists them.

| lane | samples | errors | install MB | main binary MB |
|---|---:|---:|---:|---:|
| nc | 1 | 0 | 71.4 (1) | 71.4 (1) |
| fx | 1 | 0 | 12.2 (1) | 12.2 (1) |
| pi | 1 | 0 | 159 (1) | 0.000 (1) |
| opencode | 1 | 0 | 409 (1) | 204 (1) |
| claude | 1 | 0 | 326 (1) | 326 (1) |
| codex | 1 | 0 | 437 (1) | 305 (1) |
| nclite | 1 | 0 | 14.1 (1) | 14.1 (1) |

## Streaming, headless

**per-token lag p50 ms**

| lane | 100 tok/s | 1,000 tok/s | 10,000 tok/s |
|---|---:|---:|---:|
| nc | 0.026 (10) | 0.021 (10) | 0.018 (10) |
| fx | 0.023 (10) | 0.018 (10) | 0.016 (10) |
| pi | 0.098 (10) | 0.054 (10) | 0.040 (10) |
| opencode | 2,497 (10) | 2,507 (10) | 2,522 (10) |
| claude | 0.101 (10) | 0.050 (10) | 0.038 (10) |
| codex | 2,491 (10) | 2,501 (10) | 2,514 (10) |
| nclite | 0.032 (10) | 0.026 (10) | 0.023 (10) |

**per-token lag p99 ms**

| lane | 100 tok/s | 1,000 tok/s | 10,000 tok/s |
|---|---:|---:|---:|
| nc | 0.038 (10) | 0.025 (10) | 0.023 (10) |
| fx | 0.038 (10) | 0.022 (10) | 0.019 (10) |
| pi | 0.201 (10) | 0.112 (10) | 0.192 (10) |
| opencode | 4,947 (10) | 4,957 (10) | 4,966 (10) |
| claude | 0.257 (10) | 0.153 (10) | 0.876 (10) |
| codex | 4,941 (10) | 4,951 (10) | 4,962 (10) |
| nclite | 0.046 (10) | 0.030 (10) | 0.027 (10) |

**stream end to exit ms**

| lane | 100 tok/s | 1,000 tok/s | 10,000 tok/s |
|---|---:|---:|---:|
| nc | 3.00 (10) | 5.12 (10) | 9.58 (10) |
| fx | 7.31 (10) | 8.09 (10) | 9.76 (10) |
| pi | 15.1 (10) | 16.9 (10) | 49.4 (10) |
| opencode | 44.5 (10) | 46.0 (10) | 59.9 (10) |
| claude | 23.3 (10) | 24.7 (10) | 35.1 (10) |
| codex | 17.3 (10) | 18.5 (10) | 27.9 (10) |
| nclite | 3.46 (10) | 4.12 (10) | 8.99 (10) |

**CPU, whole run ms**

| lane | 100 tok/s | 1,000 tok/s | 10,000 tok/s |
|---|---:|---:|---:|
| nc | 19.7 (10) | 68.4 (10) | 546 (10) |
| fx | 10.6 (10) | 47.8 (10) | 419 (10) |
| pi | 384 (10) | 637 (10) | 2,555 (10) |
| opencode | 2,379 (10) | 3,113 (10) | 7,114 (10) |
| claude | 438 (10) | 705 (10) | 2,520 (10) |
| codex | 174 (10) | 327 (10) | 1,648 (10) |
| nclite | 14.5 (10) | 87.7 (10) | 813 (10) |

**CPU per token µs**

| lane | 100 tok/s | 1,000 tok/s | 10,000 tok/s |
|---|---:|---:|---:|
| nc | 39.4 (10) | 13.7 (10) | 10.9 (10) |
| fx | 21.2 (10) | 9.55 (10) | 8.38 (10) |
| pi | 769 (10) | 127 (10) | 51.1 (10) |
| opencode | 4,757 (10) | 623 (10) | 142 (10) |
| claude | 875 (10) | 141 (10) | 50.4 (10) |
| codex | 348 (10) | 65.5 (10) | 33.0 (10) |
| nclite | 28.9 (10) | 17.5 (10) | 16.3 (10) |

**peak RSS MB**

| lane | 100 tok/s | 1,000 tok/s | 10,000 tok/s |
|---|---:|---:|---:|
| nc | 32.9 (10) | 32.6 (10) | 39.5 (10) |
| fx | 7.44 (10) | 7.69 (10) | 10.9 (10) |
| pi | 135 (10) | 147 (10) | 272 (10) |
| opencode | 598 (10) | 617 (10) | 680 (10) |
| claude | 206 (10) | 219 (10) | 263 (10) |
| codex | 149 (10) | 150 (10) | 158 (10) |
| nclite | 13.9 (10) | 14.0 (10) | 17.0 (10) |

**tokens never on stdout**

| lane | 100 tok/s | 1,000 tok/s | 10,000 tok/s |
|---|---:|---:|---:|
| nc | 0.000 (10) | 0.000 (10) | 0.000 (10) |
| fx | 0.000 (10) | 0.000 (10) | 0.000 (10) |
| pi | 0.000 (10) | 0.000 (10) | 0.000 (10) |
| opencode | 0.000 (10) | 0.000 (10) | 0.000 (10) |
| claude | 0.000 (10) | 0.000 (10) | 0.000 (10) |
| codex | 0.000 (10) | 0.000 (10) | 0.000 (10) |
| nclite | 0.000 (10) | 0.000 (10) | 0.000 (10) |

## Streaming, terminal interface 240x70

**on-screen lag p50 ms**

| lane | 10,000 tok/s |
|---|---:|
| nc | 3.23 (10) |
| fx | 22.6 (10) |
| pi | 38.4 (10) |
| opencode | 16.5 (10) |
| claude | 56.7 (10) |
| codex | 417 (10) |
| nclite | 0.011 (10) |

**on-screen lag p99 ms**

| lane | 10,000 tok/s |
|---|---:|
| nc | 8.79 (10) |
| fx | 38.7 (10) |
| pi | 311 (10) |
| opencode | 33.7 (10) |
| claude | 107 (10) |
| codex | 861 (10) |
| nclite | 0.111 (10) |

**stream end to last token on screen ms**

| lane | 10,000 tok/s |
|---|---:|
| nc | 2.39 (10) |
| fx | 34.1 (10) |
| pi | 140 (10) |
| opencode | 22.8 (10) |
| claude | 49.9 (10) |
| codex | 908 (10) |
| nclite | -0.143 (10) |

**Enter to request ms**

| lane | 10,000 tok/s |
|---|---:|
| nc | 2.36 (10) |
| fx | 4.22 (10) |
| pi | 17.2 (10) |
| opencode | 487 (10) |
| claude | 33.3 (10) |
| codex | 19.3 (10) |
| nclite | 2.11 (10) |

**CPU, whole session ms**

| lane | 10,000 tok/s |
|---|---:|
| nc | 567 (10) |
| fx | 2,594 (10) |
| pi | 4,925 (10) |
| opencode | 9,152 (10) |
| claude | 1,893 (10) |
| codex | 8,638 (10) |
| nclite | 370 (10) |

**peak RSS MB**

| lane | 10,000 tok/s |
|---|---:|
| nc | 47.7 (10) |
| fx | 25.1 (10) |
| pi | 485 (10) |
| opencode | 980 (10) |
| claude | 337 (10) |
| codex | 227 (10) |
| nclite | 16.9 (10) |

**terminal bytes per token**

| lane | 10,000 tok/s |
|---|---:|
| nc | 139 (10) |
| fx | 56.1 (10) |
| pi | 64.9 (10) |
| opencode | 133 (10) |
| claude | 6.52 (10) |
| codex | 139 (10) |
| nclite | 6.86 (10) |

**tokens never on screen**

| lane | 10,000 tok/s |
|---|---:|
| nc | 0.000 (10) |
| fx | 0.000 (10) |
| pi | 0.000 (10) |
| opencode | 0.000 (10) |
| claude | 0.000 (10) |
| codex | 0.000 (10) |
| nclite | 0.000 (10) |

## Streaming, terminal interface 120x40

**on-screen lag p50 ms**

| lane | 100 tok/s | 1,000 tok/s | 10,000 tok/s | 50,000 tok/s |
|---|---:|---:|---:|---:|
| nc | 0.081 (10) | 5.05 (10) | 4.07 (10) | 3.94 (10) |
| fx | 200 (10) | 32.5 (10) | 21.6 (10) | 73.1 (10) |
| pi | 7.53 (10) | 7.73 (10) | 37.6 (10) | 2,159 (10) |
| opencode | 19.5 (10) | 17.7 (10) | 16.2 (10) | 1,985 (10) |
| claude | 151 (10) | 62.9 (10) | 55.6 (10) | 104 (10) |
| codex | 1.51 (10) | 5.51 (10) | 224 (10) | 10,898 (10) |
| nclite | 0.018 (10) | 0.012 (10) | 0.011 (10) | 0.023 (10) |

**on-screen lag p99 ms**

| lane | 100 tok/s | 1,000 tok/s | 10,000 tok/s | 50,000 tok/s |
|---|---:|---:|---:|---:|
| nc | 0.197 (10) | 9.20 (10) | 8.46 (10) | 8.60 (10) |
| fx | 403 (10) | 54.1 (10) | 37.5 (10) | 347 (10) |
| pi | 15.5 (10) | 15.1 (10) | 288 (10) | 8,179 (10) |
| opencode | 32.6 (10) | 32.0 (10) | 33.2 (10) | 3,529 (10) |
| claude | 331 (10) | 118 (10) | 105 (10) | 2,427 (10) |
| codex | 19.6 (10) | 15.5 (10) | 494 (10) | 21,472 (10) |
| nclite | 0.034 (10) | 0.017 (10) | 0.112 (10) | 0.044 (10) |

**stream end to last token on screen ms**

| lane | 100 tok/s | 1,000 tok/s | 10,000 tok/s | 50,000 tok/s |
|---|---:|---:|---:|---:|
| nc | 0.125 (10) | 0.794 (10) | 1.69 (10) | -0.545 (10) |
| fx | 18.6 (10) | 14.6 (10) | 31.0 (10) | 212 (10) |
| pi | 9.87 (10) | 7.56 (10) | 146 (10) | n/a |
| opencode | 18.1 (10) | 22.3 (10) | 16.9 (10) | 2,231 (10) |
| claude | 18.6 (10) | 21.7 (10) | 47.1 (10) | 241 (10) |
| codex | 2.48 (10) | 10.7 (10) | 592 (10) | n/a |
| nclite | 0.055 (10) | 0.008 (10) | -0.173 (10) | -0.538 (10) |

**Enter to request ms**

| lane | 100 tok/s | 1,000 tok/s | 10,000 tok/s | 50,000 tok/s |
|---|---:|---:|---:|---:|
| nc | 2.03 (10) | 2.26 (10) | 2.28 (10) | 2.12 (10) |
| fx | 3.31 (10) | 3.44 (10) | 3.49 (10) | 4.42 (10) |
| pi | 17.2 (10) | 17.3 (10) | 17.1 (10) | 19.8 (10) |
| opencode | 489 (10) | 483 (10) | 485 (10) | 681 (10) |
| claude | 33.2 (10) | 33.2 (10) | 33.1 (10) | 35.2 (10) |
| codex | 19.3 (10) | 22.4 (10) | 28.7 (10) | 24.9 (10) |
| nclite | 2.01 (10) | 2.02 (10) | 2.13 (10) | 1.95 (10) |

**CPU, whole session ms**

| lane | 100 tok/s | 1,000 tok/s | 10,000 tok/s | 50,000 tok/s |
|---|---:|---:|---:|---:|
| nc | 54.1 (10) | 94.0 (10) | 501 (10) | 5,575 (10) |
| fx | 48.2 (10) | 222 (10) | 2,433 (10) | 6,349 (10) |
| pi | 519 (10) | 791 (10) | 4,934 (10) | 10,380 (10) |
| opencode | 5,066 (10) | 5,603 (10) | 9,121 (10) | 25,012 (10) |
| claude | 770 (10) | 1,000 (10) | 1,960 (10) | 8,008 (10) |
| codex | 532 (10) | 1,338 (10) | 8,315 (10) | 42,817 (10) |
| nclite | 9.82 (10) | 42.5 (10) | 375 (10) | 3,733 (10) |

**peak RSS MB**

| lane | 100 tok/s | 1,000 tok/s | 10,000 tok/s | 50,000 tok/s |
|---|---:|---:|---:|---:|
| nc | 37.0 (10) | 37.8 (10) | 47.8 (10) | 76.9 (10) |
| fx | 12.0 (10) | 15.0 (10) | 23.2 (10) | 108 (10) |
| pi | 151 (10) | 157 (10) | 493 (10) | 334 (10) |
| opencode | 964 (10) | 972 (10) | 991 (10) | 1,710 (10) |
| claude | 250 (10) | 273 (10) | 340 (10) | 527 (10) |
| codex | 200 (10) | 203 (10) | 228 (10) | 473 (10) |
| nclite | 13.6 (10) | 14.0 (10) | 17.0 (10) | 31.1 (10) |

**terminal bytes per token**

| lane | 100 tok/s | 1,000 tok/s | 10,000 tok/s | 50,000 tok/s |
|---|---:|---:|---:|---:|
| nc | 190 (10) | 389 (10) | 56.8 (10) | 15.6 (10) |
| fx | 39.5 (10) | 73.7 (10) | 31.0 (10) | 9.61 (10) |
| pi | 239 (10) | 255 (10) | 29.6 (10) | 4.58 (10) |
| opencode | 232 (10) | 420 (10) | 53.3 (10) | 15.0 (10) |
| claude | 85.6 (10) | 37.6 (10) | 2.67 (10) | 0.484 (10) |
| codex | 358 (10) | 389 (10) | 49.1 (10) | 30.9 (10) |
| nclite | 5.43 (10) | 5.89 (10) | 6.86 (10) | 7.63 (10) |

**tokens never on screen**

| lane | 100 tok/s | 1,000 tok/s | 10,000 tok/s | 50,000 tok/s |
|---|---:|---:|---:|---:|
| nc | 0.000 (10) | 0.000 (10) | 0.000 (10) | 0.000 (10) |
| fx | 0.000 (10) | 0.000 (10) | 0.000 (10) | 0.000 (10) |
| pi | 0.000 (10) | 0.000 (10) | 0.000 (10) | 172,422 (10) |
| opencode | 0.000 (10) | 0.000 (10) | 0.000 (10) | 0.000 (10) |
| claude | 0.000 (10) | 0.000 (10) | 0.000 (10) | 0.000 (10) |
| codex | 0.000 (10) | 0.000 (10) | 0.000 (10) | 76,307 (10) |
| nclite | 0.000 (10) | 0.000 (10) | 0.000 (10) | 0.000 (10) |

## Outbound connection attempts (network blocked)

| lane | attempts | destinations | names |
|---|---:|---:|---:|
| nc | 8 | 3 | api.inference.boundless.network, gateway.tempo-dev.workers.dev, mercator.sh |
| fx | 0 | 0 | none |
| pi | 0 | 0 | none |
| opencode | 0 | 0 | registry.npmjs.org |
| claude | 3 | 2 | downloads.claude.ai, github.com |
| codex | 9 | 4 | api.github.com, chatgpt.com, github.com, raw.githubusercontent.com |
| nclite | 0 | 0 | none |

## Where nc leads

- Streaming TUI 120x40 at 50,000 tok/s, stream end to last token on screen ms: nc -0.545 against fx 212 (-389.61x better)
- Streaming TUI 120x40 at 100 tok/s, on-screen lag p99 ms: nc 0.197 against pi 15.5 (78.93x better)
- Streaming TUI 120x40 at 50,000 tok/s, on-screen lag p99 ms: nc 8.60 against fx 347 (40.35x better)
- Keystroke echo, echo max ms: nc 0.100 against pi 2.18 (21.68x better)
- Streaming TUI 120x40 at 100 tok/s, stream end to last token on screen ms: nc 0.125 against codex 2.48 (19.77x better)
- Streaming TUI 120x40 at 100 tok/s, on-screen lag p50 ms: nc 0.081 against codex 1.51 (18.72x better)
- Streaming TUI 120x40 at 50,000 tok/s, on-screen lag p50 ms: nc 3.94 against fx 73.1 (18.57x better)
- Streaming TUI 120x40 at 10,000 tok/s, stream end to last token on screen ms: nc 1.69 against opencode 16.9 (9.96x better)
- Streaming TUI 120x40 at 1,000 tok/s, stream end to last token on screen ms: nc 0.794 against pi 7.56 (9.52x better)
- Streaming TUI 240x70 at 10,000 tok/s, stream end to last token on screen ms: nc 2.39 against opencode 22.8 (9.51x better)
- Long tool turn: 10 shell steps, headless, wall ms: nc 23.0 against pi 189 (8.21x better)
- Streaming TUI 240x70 at 10,000 tok/s, on-screen lag p50 ms: nc 3.23 against opencode 16.5 (5.13x better)
- One-shot headless prompt, reply end to exit ms: nc 2.42 against fx 10.5 (4.35x better)
- Long tool turn: 50 shell steps, headless, wall ms: nc 69.1 against pi 299 (4.33x better)
- Keystroke echo, echo p50 ms: nc 0.078 against fx 0.315 (4.01x better)
- Streaming TUI 120x40 at 10,000 tok/s, on-screen lag p50 ms: nc 4.07 against opencode 16.2 (3.99x better)
- Streaming TUI 120x40 at 10,000 tok/s, on-screen lag p99 ms: nc 8.46 against opencode 33.2 (3.92x better)
- Streaming TUI 120x40 at 10,000 tok/s, CPU, whole session ms: nc 501 against claude 1,960 (3.91x better)
- Streaming TUI 240x70 at 10,000 tok/s, on-screen lag p99 ms: nc 8.79 against opencode 33.7 (3.83x better)
- Long tool turn: 10 shell steps, headless, tool result to next request p50 ms: nc 0.990 against pi 3.38 (3.42x better)
- One-shot headless prompt, wall ms: nc 6.87 against fx 23.2 (3.38x better)
- Streaming TUI 240x70 at 10,000 tok/s, CPU, whole session ms: nc 567 against claude 1,893 (3.34x better)
- Fleet: 32 concurrent headless runs, 1,000 tokens each at 1,000 tok/s, stream end to exit p50 ms: nc 2.42 against fx 7.58 (3.14x better)
- One-shot headless prompt, spawn to request ms: nc 4.27 against fx 12.5 (2.94x better)
- Long tool turn: 50 shell steps, headless, tool result to next request p50 ms: nc 0.903 against pi 2.56 (2.83x better)
- Fleet: 8 concurrent headless runs, 1,000 tokens each at 1,000 tok/s, stream end to exit p50 ms: nc 2.87 against fx 7.76 (2.70x better)
- Streaming headless at 100 tok/s, stream end to exit ms: nc 3.00 against fx 7.31 (2.44x better)
- Streaming TUI 120x40 at 1,000 tok/s, CPU, whole session ms: nc 94.0 against fx 222 (2.36x better)
- Fleet: 1 concurrent headless runs, 1,000 tokens each at 1,000 tok/s, stream end to exit p50 ms: nc 3.31 against fx 7.39 (2.24x better)
- Streaming TUI 120x40 at 50,000 tok/s, Enter to request ms: nc 2.12 against fx 4.42 (2.08x better)
- Session store: 1,000 one-turn sessions, store KB: nc 4,264 against pi 8,144 (1.91x better)
- Session store: 1,000 one-turn sessions, KB per session: nc 4.26 against pi 8.14 (1.91x better)
- Continuing the latest of 1,000 sessions, spawn to request ms: nc 8.92 against fx 16.3 (1.82x better)
- Streaming TUI 240x70 at 10,000 tok/s, Enter to request ms: nc 2.36 against fx 4.22 (1.79x better)
- Streaming TUI 120x40 at 1,000 tok/s, on-screen lag p99 ms: nc 9.20 against pi 15.1 (1.64x better)
- Streaming TUI 120x40 at 100 tok/s, Enter to request ms: nc 2.03 against fx 3.31 (1.63x better)
- Streaming headless at 1,000 tok/s, stream end to exit ms: nc 5.12 against fx 8.09 (1.58x better)
- Streaming TUI 120x40 at 10,000 tok/s, Enter to request ms: nc 2.28 against fx 3.49 (1.53x better)
- Streaming TUI 120x40 at 1,000 tok/s, Enter to request ms: nc 2.26 against fx 3.44 (1.52x better)
- Continuing the latest of 1,000 sessions, wall ms: nc 19.1 against fx 28.9 (1.51x better)
- Streaming TUI 120x40 at 50,000 tok/s, peak RSS MB: nc 76.9 against fx 108 (1.41x better)
- Streaming TUI 120x40 at 50,000 tok/s, CPU, whole session ms: nc 5,575 against fx 6,349 (1.14x better)
- Streaming TUI 120x40 at 1,000 tok/s, on-screen lag p50 ms: nc 5.05 against codex 5.51 (1.09x better)

## Where nc trails

- Install size, main binary MB: nc 71.4 against pi 0.000 (467710.15x worse)
- Streaming TUI 120x40 at 50,000 tok/s, terminal bytes per token: nc 15.6 against claude 0.484 (32.17x worse)
- Streaming TUI 240x70 at 10,000 tok/s, terminal bytes per token: nc 139 against claude 6.52 (21.32x worse)
- Streaming TUI 120x40 at 10,000 tok/s, terminal bytes per token: nc 56.8 against claude 2.67 (21.28x worse)
- One-shot headless prompt, first request bytes: nc 62,483 against pi 5,444 (11.48x worse)
- Streaming TUI 120x40 at 1,000 tok/s, terminal bytes per token: nc 389 against claude 37.6 (10.34x worse)
- Long tool turn: 10 shell steps, headless, last request bytes: nc 66,786 against pi 7,337 (9.10x worse)
- Startup: --version, CPU ms: nc 1.18 against fx 0.154 (7.64x worse)
- Session store: 1,000 one-turn sessions, CPU per saved session ms: nc 11.4 against fx 1.57 (7.23x worse)
- Startup: --version, peak RSS MB: nc 9.13 against fx 1.38 (6.64x worse)
- Cold start: --version after dropping the page cache, peak RSS MB: nc 9.13 against fx 1.38 (6.64x worse)
- Install size, install MB: nc 71.4 against fx 12.2 (5.84x worse)
- Long tool turn: 50 shell steps, headless, last request bytes: nc 83,907 against pi 14,818 (5.66x worse)
- Streaming TUI 120x40 at 100 tok/s, terminal bytes per token: nc 190 against fx 39.5 (4.82x worse)
- Memory one second after the first frame, anonymous MB: nc 5.97 against fx 1.25 (4.79x worse)
- Continuing the latest of 1,000 sessions, peak RSS MB: nc 41.9 against fx 8.88 (4.72x worse)
- One-shot headless prompt, peak RSS MB: nc 33.6 against fx 7.52 (4.47x worse)
- Streaming headless at 100 tok/s, peak RSS MB: nc 32.9 against fx 7.44 (4.42x worse)
- Fleet: 32 concurrent headless runs, 1,000 tokens each at 1,000 tok/s, peak RSS per run MB: nc 33.2 against fx 7.55 (4.40x worse)
- Fleet: 8 concurrent headless runs, 1,000 tokens each at 1,000 tok/s, peak RSS per run MB: nc 33.1 against fx 7.56 (4.37x worse)
- Fleet: 1 concurrent headless runs, 1,000 tokens each at 1,000 tok/s, peak RSS per run MB: nc 33.1 against fx 7.62 (4.34x worse)
- Cold start: --version after dropping the page cache, CPU ms: nc 2.64 against fx 0.622 (4.25x worse)
- Streaming headless at 1,000 tok/s, peak RSS MB: nc 32.6 against fx 7.69 (4.24x worse)
- One-shot headless prompt, CPU ms: nc 7.36 against fx 1.87 (3.94x worse)
- Streaming headless at 10,000 tok/s, peak RSS MB: nc 39.5 against fx 10.9 (3.62x worse)
- Memory one second after the first frame, RSS MB: nc 30.9 against fx 8.80 (3.51x worse)
- Time to first frame, peak RSS MB: nc 31.0 against fx 8.89 (3.49x worse)
- Memory one second after the first frame, PSS MB: nc 28.5 against fx 8.79 (3.25x worse)
- Idle terminal interface, peak RSS MB: nc 30.9 against fx 9.54 (3.24x worse)
- Memory one second after the first frame, private (USS) MB: nc 28.3 against fx 8.78 (3.22x worse)
- Listing 1,000 sessions, peak RSS MB: nc 20.0 against fx 6.28 (3.19x worse)
- Streaming TUI 120x40 at 100 tok/s, peak RSS MB: nc 37.0 against fx 12.0 (3.09x worse)
- Continuing the latest of 1,000 sessions, CPU ms: nc 18.7 against fx 6.17 (3.03x worse)
- Long tool turn: 10 shell steps, headless, peak RSS MB: nc 49.1 against fx 16.2 (3.03x worse)
- Fleet: 32 concurrent headless runs, 1,000 tokens each at 1,000 tok/s, CPU per run ms: nc 17.8 against fx 5.92 (3.00x worse)
- Long tool turn: 50 shell steps, headless, peak RSS MB: nc 51.7 against fx 17.4 (2.97x worse)
- Fleet: 8 concurrent headless runs, 1,000 tokens each at 1,000 tok/s, CPU per run ms: nc 15.1 against fx 5.34 (2.84x worse)
- Time to first frame, cold page cache, CPU ms: nc 14.5 against fx 5.72 (2.54x worse)
- Streaming TUI 120x40 at 1,000 tok/s, peak RSS MB: nc 37.8 against fx 15.0 (2.52x worse)
- Fleet: 1 concurrent headless runs, 1,000 tokens each at 1,000 tok/s, CPU per run ms: nc 12.4 against fx 5.16 (2.40x worse)
- Time to first frame, first byte ms: nc 2.26 against fx 1.03 (2.20x worse)
- Time to first frame, CPU ms: nc 8.56 against fx 3.90 (2.19x worse)
- Time to first frame, cold page cache, first frame ms: nc 20.6 against fx 9.48 (2.17x worse)
- Cold start: --version after dropping the page cache, wall ms: nc 11.1 against fx 5.32 (2.08x worse)
- Streaming TUI 120x40 at 10,000 tok/s, peak RSS MB: nc 47.8 against fx 23.2 (2.07x worse)
- Streaming TUI 240x70 at 10,000 tok/s, peak RSS MB: nc 47.7 against fx 25.1 (1.90x worse)
- Streaming headless at 100 tok/s, CPU, whole run ms: nc 19.7 against fx 10.6 (1.86x worse)
- Streaming headless at 100 tok/s, CPU per token µs: nc 39.4 against fx 21.2 (1.86x worse)
- Long tool turn: 10 shell steps, headless, CPU per step ms: nc 4.18 against fx 2.52 (1.65x worse)
- Time to first frame, first frame ms: nc 2.89 against fx 2.01 (1.44x worse)
- Streaming headless at 1,000 tok/s, CPU, whole run ms: nc 68.4 against fx 47.8 (1.43x worse)
- Streaming headless at 1,000 tok/s, CPU per token µs: nc 13.7 against fx 9.55 (1.43x worse)
- Long tool turn: 50 shell steps, headless, CPU per step ms: nc 3.01 against fx 2.12 (1.42x worse)
- Listing 1,000 sessions, wall ms: nc 9.23 against fx 6.88 (1.34x worse)
- Listing 1,000 sessions, CPU ms: nc 6.50 against fx 4.94 (1.32x worse)
- Streaming headless at 10,000 tok/s, CPU, whole run ms: nc 546 against fx 419 (1.30x worse)
- Streaming headless at 10,000 tok/s, CPU per token µs: nc 10.9 against fx 8.38 (1.30x worse)
- Streaming TUI 120x40 at 100 tok/s, CPU, whole session ms: nc 54.1 against fx 48.2 (1.12x worse)
- Idle terminal interface, idle CPU ms/s: nc -0.009 against pi -0.031 (3.61x better)

## Ties (within 5%)

- Startup: --version, wall ms: nc 2.68 against fx 2.74
- Idle terminal interface, bytes written while idle: nc 0.000 against fx 0.000
- One-shot headless prompt, side requests: nc 0.000 against fx 0.000
- One-shot headless prompt, processes left behind: nc 0.000 against fx 0.000
- Fleet: 1 concurrent headless runs, 1,000 tokens each at 1,000 tok/s, wall ms: nc 1,008 against fx 1,020
- Fleet: 8 concurrent headless runs, 1,000 tokens each at 1,000 tok/s, wall ms: nc 1,015 against fx 1,025
- Fleet: 32 concurrent headless runs, 1,000 tokens each at 1,000 tok/s, wall ms: nc 1,055 against fx 1,051
- Session store: 1,000 one-turn sessions, files: nc 1,006 against pi 1,004
- Streaming headless at 100 tok/s, per-token lag p50 ms: nc 0.026 against fx 0.023
- Streaming headless at 1,000 tok/s, per-token lag p50 ms: nc 0.021 against fx 0.018
- Streaming headless at 10,000 tok/s, per-token lag p50 ms: nc 0.018 against fx 0.016
- Streaming headless at 100 tok/s, per-token lag p99 ms: nc 0.038 against fx 0.038
- Streaming headless at 1,000 tok/s, per-token lag p99 ms: nc 0.025 against fx 0.022
- Streaming headless at 10,000 tok/s, per-token lag p99 ms: nc 0.023 against fx 0.019
- Streaming headless at 10,000 tok/s, stream end to exit ms: nc 9.58 against fx 9.76
- Streaming headless at 100 tok/s, tokens never on stdout: nc 0.000 against fx 0.000
- Streaming headless at 1,000 tok/s, tokens never on stdout: nc 0.000 against fx 0.000
- Streaming headless at 10,000 tok/s, tokens never on stdout: nc 0.000 against fx 0.000
- Streaming TUI 240x70 at 10,000 tok/s, tokens never on screen: nc 0.000 against fx 0.000
- Streaming TUI 120x40 at 100 tok/s, tokens never on screen: nc 0.000 against fx 0.000
- Streaming TUI 120x40 at 1,000 tok/s, tokens never on screen: nc 0.000 against fx 0.000
- Streaming TUI 120x40 at 10,000 tok/s, tokens never on screen: nc 0.000 against fx 0.000
- Streaming TUI 120x40 at 50,000 tok/s, tokens never on screen: nc 0.000 against fx 0.000
