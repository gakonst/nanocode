# CLI benchmark: 2026-11-10-scout

Machine: AMD EPYC 4245P 6-Core Processor, 12 threads, 46 GB, Ubuntu 24.04.5 LTS, 6.8.0-147-generic. 

# Installed CLIs

Written by tools/install.sh on Linux x86_64. The size is the main binary's; BENCH_INSTALL_<LANE> in env.sh is the whole install directory.

| Lane | Version | Install | Date (UTC) | Main binary | Runtime not counted |
| --- | --- | --- | --- | --- | --- |
| nc | nanocode 0.7.0 | curl https://github.com/nanocodexyz/nanocode/releases/download/nanocode-cli/nanocode-x86_64-unknown-linux-gnu.gz (the asset https://nanocode.xyz/install fetches; its optional macOS computer-use driver is left out) | 2026-11-10T19:47Z | `tools/install/nc/nanocode` (37.5 MiB) | none |
| fx | 0.0.16 | curl https://releases.fx.sh/$(latest.txt)/fx-linux-x86_64.tar.gz (the archive https://fx.sh/setup.sh fetches; binary only) | 2026-11-10T19:47Z | `tools/install/fx/fx` (12.2 MiB) | none |
| pi | 1.5.2 | npm install --prefix tools/install/pi @earendil-works/pi-coding-agent@latest | 2026-11-10T19:47Z | `tools/install/pi/node_modules/@earendil-works/pi-coding-agent/dist/bundle/cli.js` (0.0 MiB) | Node v24.22.2 |
| opencode | 1.19.5 | npm install --prefix tools/install/opencode opencode-ai@latest (the package's bin is the native binary from its platform package) | 2026-11-10T19:47Z | `tools/install/opencode/node_modules/opencode-ai/bin/opencode.exe` (204.5 MiB) | none |
| claude | 2.1.322 (Claude Code) | npm install --prefix tools/install/claude @anthropic-ai/claude-code@latest (the package's bin is the native binary from its platform package) | 2026-11-10T19:47Z | `tools/install/claude/node_modules/@anthropic-ai/claude-code/bin/claude.exe` (326.1 MiB) | none |
| codex | codex-cli 0.167.0 | npm install --prefix tools/install/codex @openai/codex@latest (CODEX_BIN is the platform binary: the npm codex command is a Node launcher that execs it) | 2026-11-10T19:47Z | `tools/install/codex/node_modules/@openai/codex-linux-x64/vendor/x86_64-unknown-linux-musl/bin/codex` (305.7 MiB) | none (the npm launcher needs Node) |
nc variants: ncdev=/home/ubuntu/ncdev-base

Medians with the sample count in parentheses and [min–max] where the spread matters. CPU and memory are the whole process tree (cgroup or /proc sampling; rusage when neither is available). Lower is better everywhere.

## Startup: --version

| lane | samples | errors | wall ms | CPU ms | peak RSS MB | instructions |
|---|---:|---:|---:|---:|---:|---:|
| true | 100 | 0 | 2.82 [2.73–3.69] (100) | 0.194 (100) | 0.000 (100) | n/a |
| nc | 100 | 0 | 2.98 [2.63–3.82] (100) | 0.955 (100) | 6.18 (100) | n/a |
| ncdev | 100 | 0 | 3.03 [2.65–3.63] (100) | 0.913 (100) | 0.000 (100) | n/a |
| fx | 100 | 0 | 2.75 [2.65–3.00] (100) | 0.159 (100) | 0.000 (100) | n/a |
| pi | 100 | 0 | 101 [96.8–105] (100) | 125 (100) | 103 (100) | n/a |
| opencode | 100 | 0 | 201 [199–207] (100) | 267 (100) | 130 (100) | n/a |
| claude | 100 | 0 | 4.75 [4.65–6.79] (100) | 4.06 (100) | 14.5 (100) | n/a |
| codex | 100 | 0 | 4.72 [4.64–4.97] (100) | 4.15 (100) | 16.1 (100) | n/a |

## Cold start: --version after dropping the page cache

| lane | samples | errors | wall ms | CPU ms | peak RSS MB |
|---|---:|---:|---:|---:|---:|
| nc | 3 | 0 | 9.20 [9.19–9.33] (3) | 1.96 (3) | 5.75 (3) |
| ncdev | 3 | 0 | 9.06 [8.98–9.11] (3) | 1.79 (3) | 6.11 (3) |
| fx | 3 | 0 | 7.08 [5.13–8.52] (3) | 0.559 (3) | 0.367 (3) |
| pi | 3 | 0 | 117 [117–119] (3) | 129 (3) | 104 (3) |
| opencode | 3 | 0 | 238 [236–238] (3) | 282 (3) | 130 (3) |
| claude | 3 | 0 | 29.7 [29.7–29.8] (3) | 13.4 (3) | 20.4 (3) |
| codex | 3 | 0 | 27.8 [27.8–27.9] (3) | 9.89 (3) | 26.0 (3) |

## Time to first frame

| lane | samples | errors | first byte ms | first frame ms | CPU ms | peak RSS MB | processes |
|---|---:|---:|---:|---:|---:|---:|---:|
| nc | 10 | 0 | 1.99 (10) | 5.41 [5.23–31.6] (10) | 25.9 (10) | 30.1 (10) | 0.000 (10) |
| ncdev | 10 | 0 | 1.96 (10) | 6.93 [6.66–20.7] (10) | 33.2 (10) | 31.9 (10) | 0.000 (10) |
| fx | 10 | 0 | 1.01 (10) | 2.02 [1.91–9.20] (10) | 4.15 (10) | 8.83 (10) | 0.000 (10) |
| pi | 10 | 0 | 125 (10) | 137 [134–138] (10) | 215 (10) | 121 (10) | 0.000 (10) |
| opencode | 10 | 0 | 284 (10) | 1,379 [1,366–1,419] (10) | 3,491 (10) | 928 (10) | 0.000 (10) |
| claude | 10 | 0 | 85.0 (10) | 151 [150–195] (10) | 474 (10) | 270 (10) | 4.00 (10) |
| codex | 10 | 0 | 5.84 (10) | 132 [124–224] (10) | 699 (10) | 215 (10) | 8.50 (10) |

## Idle terminal interface

| lane | samples | errors | idle CPU ms/s | bytes written while idle | peak RSS MB | processes |
|---|---:|---:|---:|---:|---:|---:|
| nc | 3 | 0 | 0.081 [-0.000–0.263] (3) | 0.000 (3) | 30.3 (3) | 0.000 (3) |
| ncdev | 3 | 0 | 0.088 [0.010–0.100] (3) | 7.00 (3) | 32.3 (3) | 0.000 (3) |
| fx | 3 | 0 | 0.725 [0.717–0.770] (3) | 0.000 (3) | 9.91 (3) | 0.000 (3) |
| pi | 3 | 0 | -0.199 [-0.253–-0.031] (3) | 0.000 (3) | 121 (3) | 0.000 (3) |
| opencode | 3 | 0 | 125 [119–126] (3) | 0.000 (3) | 1,214 (3) | 0.000 (3) |
| claude | 3 | 0 | 5.41 [5.00–5.49] (3) | -96.0 (3) | 271 (3) | 4.00 (3) |
| codex | 3 | 0 | 0.314 [-0.902–7.78] (3) | 0.000 (3) | 215 (3) | 7.00 (3) |

## Keystroke echo

| lane | samples | errors | echo p50 ms | echo max ms | keys echoed of 26 |
|---|---:|---:|---:|---:|---:|
| nc | 3 | 0 | 0.054 [0.049–0.080] (3) | 0.136 (3) | 26.0 (3) |
| ncdev | 3 | 0 | 0.080 [0.080–0.082] (3) | 0.147 (3) | 26.0 (3) |
| fx | 3 | 0 | 0.325 [0.320–0.325] (3) | 113 (3) | 26.0 (3) |
| pi | 3 | 0 | 0.319 [0.280–0.329] (3) | 2.08 (3) | 26.0 (3) |
| opencode | 3 | 0 | 2.22 [1.98–2.23] (3) | 37.9 (3) | 26.0 (3) |
| claude | 3 | 0 | 1.29 [1.27–1.40] (3) | 9.76 (3) | 26.0 (3) |
| codex | 3 | 0 | 20.9 [20.8–21.0] (3) | 22.0 (3) | 26.0 (3) |

## One-shot headless prompt

| lane | samples | errors | wall ms | spawn to request ms | reply end to exit ms | CPU ms | peak RSS MB | first request bytes | side requests | processes left behind |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| nc | 10 | 0 | 19.2 [14.9–31.2] (10) | 12.4 (10) | 6.33 (10) | 16.6 (10) | 27.3 (10) | 61,070 (10) | 0.000 (10) | 0.000 (10) |
| ncdev | 10 | 0 | 24.2 [19.2–33.2] (10) | 15.4 (10) | 8.40 (10) | 20.2 (10) | 28.5 (10) | 62,183 (10) | 0.000 (10) | 0.000 (10) |
| fx | 10 | 0 | 23.0 [23.0–25.6] (10) | 12.5 (10) | 10.5 (10) | 1.86 (10) | 7.49 (10) | 26,275 (10) | 0.000 (10) | 0.000 (10) |
| pi | 10 | 0 | 150 [143–153] (10) | 138 (10) | 11.4 (10) | 201 (10) | 115 (10) | 5,428 (10) | 0.000 (10) | 0.000 (10) |
| opencode | 10 | 0 | 1,037 [1,031–1,051] (10) | 994 (10) | 40.8 (10) | 1,900 (10) | 677 (10) | 30,704 (10) | 1.00 (10) | 0.000 (10) |
| claude | 10 | 0 | 163 [157–169] (10) | 144 (10) | 19.5 (10) | 182 (10) | 185 (10) | 46,392 (10) | 0.000 (10) | 0.000 (10) |
| codex | 10 | 0 | 158 [150–190] (10) | 55.4 (10) | 101 (10) | 118 (10) | 182 (10) | 39,388 (10) | 0.000 (10) | 3.00 (10) |

## Long tool turn: 10 shell steps, headless

| lane | samples | errors | wall ms | tool result to next request p50 ms | CPU per step ms | peak RSS MB | last request bytes |
|---|---:|---:|---:|---:|---:|---:|---:|
| nc | 2 | 0 | 35.1 [33.2–37.1] (2) | 1.08 (2) | 5.55 (2) | 42.2 (2) | 65,374 (2) |
| ncdev | 0 | 2 | failed (2) | failed (2) | failed (2) | failed (2) | failed (2) |
| fx | 2 | 0 | 241 [241–242] (2) | 20.4 (2) | 2.70 (2) | 15.7 (2) | 33,001 (2) |
| pi | 2 | 0 | 190 [187–193] (2) | 3.51 (2) | 27.4 (2) | 148 (2) | 7,322 (2) |
| opencode | 2 | 0 | 1,209 [1,209–1,209] (2) | 13.4 (2) | 228 (2) | 732 (2) | 33,163 (2) |
| claude | 2 | 0 | 280 [279–281] (2) | 9.52 (2) | 35.4 (2) | 245 (2) | 49,664 (2) |
| codex | 2 | 0 | 577 [570–585] (2) | 41.8 (2) | 35.1 (2) | 207 (2) | 42,978 (2) |

- ncdev error: exit 1, 5 requests, done false: ode: computer use unavailable, continuing without it: cua-driver is not installed: run nanocode cua install, or set CUA_DRIVER_PATH session 01a1c70f-a91c-7000-9f28-8f8893df0d6a nanocode: durable operation rec

## Fleet: 8 concurrent headless runs, 1,000 tokens each at 1,000 tok/s

| lane | samples | errors | wall ms | CPU per run ms | peak RSS per run MB | stream end to exit p50 ms |
|---|---:|---:|---:|---:|---:|---:|
| nc | 2 | 0 | 1,030 [1,028–1,033] (2) | 34.6 (2) | 28.9 (2) | 3.49 (2) |
| ncdev | 2 | 0 | 1,034 [1,034–1,035] (2) | 44.1 (2) | 30.7 (2) | 3.71 (2) |
| fx | 2 | 0 | 1,039 [1,031–1,047] (2) | 6.25 (2) | 7.59 (2) | 15.7 (2) |
| pi | 2 | 0 | 1,294 [1,280–1,308] (2) | 453 (2) | 135 (2) | 11.4 (2) |
| opencode | 2 | 0 | 3,636 [3,599–3,673] (2) | 4,029 (2) | 664 (2) | 41.6 (2) |
| claude | 2 | 0 | 1,294 [1,292–1,297] (2) | 504 (2) | 219 (2) | 21.2 (2) |
| codex | 2 | 0 | 1,406 [1,165–1,646] (2) | 359 (2) | 196 (2) | 19.6 (2) |

## Session store: 1,000 one-turn sessions

| lane | samples | errors | store KB | KB per session | files | CPU per saved session ms |
|---|---:|---:|---:|---:|---:|---:|
| nc | 1 | 0 | 3,408 (1) | 68.2 (1) | 50.0 (1) | 15.5 (1) |
| ncdev | 1 | 0 | 3,408 (1) | 68.2 (1) | 50.0 (1) | 19.1 (1) |
| fx | 1 | 0 | 664 (1) | 13.3 (1) | 105 (1) | 1.71 (1) |
| pi | 1 | 0 | 432 (1) | 8.64 (1) | 54.0 (1) | 201 (1) |
| opencode | 1 | 0 | 1,032 (1) | 20.6 (1) | 2.00 (1) | 1,754 (1) |
| claude | 1 | 0 | 6,648 (1) | 133 (1) | 54.0 (1) | 177 (1) |
| codex | 1 | 0 | 12,372 (1) | 247 (1) | 172 (1) | 88.2 (1) |

## Listing 1,000 sessions

| lane | samples | errors | wall ms | CPU ms | peak RSS MB |
|---|---:|---:|---:|---:|---:|
| nc | 3 | 0 | 2.94 [2.92–30.9] (3) | 1.83 (3) | 12.4 (3) |
| ncdev | 3 | 0 | 3.00 [2.90–26.8] (3) | 1.96 (3) | 14.1 (3) |
| fx | 3 | 0 | 2.75 [2.75–2.83] (3) | 0.487 (3) | 3.38 (3) |
| opencode | 3 | 0 | 337 [335–341] (3) | 582 (3) | 250 (3) |

## Continuing the latest of 1,000 sessions

| lane | samples | errors | wall ms | spawn to request ms | CPU ms | peak RSS MB |
|---|---:|---:|---:|---:|---:|---:|
| nc | 3 | 0 | 18.9 [18.8–18.9] (3) | 12.4 (3) | 16.6 (3) | 28.8 (3) |
| ncdev | 3 | 0 | 20.9 [20.9–22.9] (3) | 15.2 (3) | 20.0 (3) | 30.3 (3) |
| fx | 3 | 0 | 24.8 [24.8–24.8] (3) | 12.1 (3) | 2.16 (3) | 8.76 (3) |
| pi | 3 | 0 | 149 [149–155] (3) | 138 (3) | 202 (3) | 117 (3) |
| opencode | 3 | 0 | 917 [915–923] (3) | 877 (3) | 1,746 (3) | 655 (3) |
| claude | 3 | 0 | 165 [163–171] (3) | 146 (3) | 184 (3) | 191 (3) |
| codex | 3 | 0 | 58.5 [58.1–62.4] (3) | 40.9 (3) | 93.9 (3) | 181 (3) |

## Install size

Runtimes a lane needs but does not install (Node for npm packages) are not counted; VERSIONS.md lists them.

| lane | samples | errors | install MB | main binary MB |
|---|---:|---:|---:|---:|
| nc | 1 | 0 | 37.5 (1) | 37.5 (1) |
| ncdev | 1 | 0 | 39.2 (1) | 39.2 (1) |
| fx | 1 | 0 | 12.2 (1) | 12.2 (1) |
| pi | 1 | 0 | 159 (1) | 0.000 (1) |
| opencode | 1 | 0 | 409 (1) | 204 (1) |
| claude | 1 | 0 | 326 (1) | 326 (1) |
| codex | 1 | 0 | 438 (1) | 306 (1) |

## Streaming, headless

**per-token lag p50 ms**

| lane | 1,000 tok/s | 10,000 tok/s |
|---|---:|---:|
| nc | 8.02 (3) | 8.31 (3) |
| fx | 0.010 (3) | 0.008 (3) |
| pi | 0.026 (3) | 0.017 (3) |
| opencode | 2,506 (3) | 2,517 (3) |
| claude | 0.024 (3) | 0.019 (3) |
| codex | 2,501 (3) | 2,509 (3) |

**per-token lag p99 ms**

| lane | 1,000 tok/s | 10,000 tok/s |
|---|---:|---:|
| nc | 16.0 (3) | 16.5 (3) |
| fx | 0.016 (3) | 0.009 (3) |
| pi | 0.088 (3) | 0.081 (3) |
| opencode | 4,956 (3) | 4,963 (3) |
| claude | 0.113 (3) | 0.792 (3) |
| codex | 4,951 (3) | 4,958 (3) |

**stream end to exit ms**

| lane | 1,000 tok/s | 10,000 tok/s |
|---|---:|---:|
| nc | 4.32 (3) | 6.24 (3) |
| fx | 7.15 (3) | 7.63 (3) |
| pi | 11.6 (3) | 37.1 (3) |
| opencode | 41.4 (3) | 46.7 (3) |
| claude | 19.3 (3) | 24.7 (3) |
| codex | 19.1 (3) | 24.6 (3) |

**CPU, whole run ms**

| lane | 1,000 tok/s | 10,000 tok/s |
|---|---:|---:|
| nc | 53.0 (3) | 259 (3) |
| fx | 21.9 (3) | 183 (3) |
| pi | 426 (3) | 1,284 (3) |
| opencode | 4,732 (3) | 7,366 (3) |
| claude | 522 (3) | 1,483 (3) |
| codex | 695 (3) | 1,449 (3) |

**CPU per token µs**

| lane | 1,000 tok/s | 10,000 tok/s |
|---|---:|---:|
| nc | 10.6 (3) | 5.18 (3) |
| fx | 4.37 (3) | 3.65 (3) |
| pi | 85.3 (3) | 25.7 (3) |
| opencode | 946 (3) | 147 (3) |
| claude | 104 (3) | 29.7 (3) |
| codex | 139 (3) | 29.0 (3) |

**peak RSS MB**

| lane | 1,000 tok/s | 10,000 tok/s |
|---|---:|---:|
| nc | 30.0 (3) | 33.5 (3) |
| fx | 7.75 (3) | 11.1 (3) |
| pi | 148 (3) | 272 (3) |
| opencode | 886 (3) | 1,116 (3) |
| claude | 220 (3) | 264 (3) |
| codex | 182 (3) | 183 (3) |

**tokens never on stdout**

| lane | 1,000 tok/s | 10,000 tok/s |
|---|---:|---:|
| nc | 0.000 (3) | 0.000 (3) |
| fx | 0.000 (3) | 0.000 (3) |
| pi | 0.000 (3) | 0.000 (3) |
| opencode | 0.000 (3) | 0.000 (3) |
| claude | 0.000 (3) | 0.000 (3) |
| codex | 0.000 (3) | 0.000 (3) |

## Streaming, terminal interface 120x40

**on-screen lag p50 ms**

| lane | 1,000 tok/s | 10,000 tok/s |
|---|---:|---:|
| nc | 8.06 (3) | 8.08 (3) |
| fx | 32.6 (3) | 21.8 (3) |
| pi | 7.73 (3) | 39.4 (3) |
| opencode | 19.5 (3) | 19.0 (3) |
| claude | 63.7 (3) | 55.8 (3) |
| codex | 5.53 (3) | 418 (3) |

**on-screen lag p99 ms**

| lane | 1,000 tok/s | 10,000 tok/s |
|---|---:|---:|
| nc | 17.1 (3) | 16.7 (3) |
| fx | 54.2 (3) | 37.8 (3) |
| pi | 15.1 (3) | 316 (3) |
| opencode | 298 (3) | 317 (3) |
| claude | 118 (3) | 105 (3) |
| codex | 15.5 (3) | 782 (3) |

**stream end to last token on screen ms**

| lane | 1,000 tok/s | 10,000 tok/s |
|---|---:|---:|
| nc | 0.949 (3) | 3.34 (3) |
| fx | 13.7 (3) | 36.2 (3) |
| pi | 13.8 (3) | 163 (3) |
| opencode | 15.5 (3) | 18.5 (3) |
| claude | 21.4 (3) | 45.7 (3) |
| codex | 12.2 (3) | 869 (3) |

**Enter to request ms**

| lane | 1,000 tok/s | 10,000 tok/s |
|---|---:|---:|
| nc | 3.04 (3) | 3.24 (3) |
| fx | 3.31 (3) | 3.33 (3) |
| pi | 16.6 (3) | 16.7 (3) |
| opencode | 494 (3) | 492 (3) |
| claude | 42.1 (3) | 42.2 (3) |
| codex | 18.6 (3) | 18.8 (3) |

**CPU, whole session ms**

| lane | 1,000 tok/s | 10,000 tok/s |
|---|---:|---:|
| nc | 105 (3) | 557 (3) |
| fx | 230 (3) | 2,489 (3) |
| pi | 808 (3) | 5,008 (3) |
| opencode | 7,818 (3) | 11,642 (3) |
| claude | 1,103 (3) | 2,071 (3) |
| codex | 1,890 (3) | 9,371 (3) |

**peak RSS MB**

| lane | 1,000 tok/s | 10,000 tok/s |
|---|---:|---:|
| nc | 36.3 (3) | 41.6 (3) |
| fx | 16.1 (3) | 22.3 (3) |
| pi | 156 (3) | 490 (3) |
| opencode | 1,164 (3) | 1,483 (3) |
| claude | 277 (3) | 348 (3) |
| codex | 235 (3) | 242 (3) |

**terminal bytes per token**

| lane | 1,000 tok/s | 10,000 tok/s |
|---|---:|---:|
| nc | 300 (3) | 33.3 (3) |
| fx | 73.7 (3) | 30.9 (3) |
| pi | 246 (3) | 29.3 (3) |
| opencode | 359 (3) | 45.4 (3) |
| claude | 37.2 (3) | 2.80 (3) |
| codex | 397 (3) | 47.5 (3) |

**tokens never on screen**

| lane | 1,000 tok/s | 10,000 tok/s |
|---|---:|---:|
| nc | 0.000 (3) | 0.000 (3) |
| fx | 0.000 (3) | 0.000 (3) |
| pi | 0.000 (3) | 0.000 (3) |
| opencode | 0.000 (3) | 0.000 (3) |
| claude | 0.000 (3) | 0.000 (3) |
| codex | 0.000 (3) | 0.000 (3) |

## Where nc leads

- Keystroke echo, echo max ms: nc 0.136 against pi 2.08 (15.31x better)
- Streaming TUI 120x40 at 1,000 tok/s, stream end to last token on screen ms: nc 0.949 against codex 12.2 (12.87x better)
- Keystroke echo, echo p50 ms: nc 0.054 against pi 0.319 (5.89x better)
- Streaming TUI 120x40 at 10,000 tok/s, stream end to last token on screen ms: nc 3.34 against opencode 18.5 (5.53x better)
- Long tool turn: 10 shell steps, headless, wall ms: nc 35.1 against pi 190 (5.41x better)
- Streaming TUI 120x40 at 10,000 tok/s, CPU, whole session ms: nc 557 against claude 2,071 (3.72x better)
- Fleet: 8 concurrent headless runs, 1,000 tokens each at 1,000 tok/s, stream end to exit p50 ms: nc 3.49 against pi 11.4 (3.25x better)
- Long tool turn: 10 shell steps, headless, tool result to next request p50 ms: nc 1.08 against pi 3.51 (3.24x better)
- Streaming TUI 120x40 at 10,000 tok/s, on-screen lag p50 ms: nc 8.08 against opencode 19.0 (2.35x better)
- Streaming TUI 120x40 at 10,000 tok/s, on-screen lag p99 ms: nc 16.7 against fx 37.8 (2.27x better)
- Streaming TUI 120x40 at 1,000 tok/s, CPU, whole session ms: nc 105 against fx 230 (2.19x better)
- Streaming headless at 1,000 tok/s, stream end to exit ms: nc 4.32 against fx 7.15 (1.66x better)
- One-shot headless prompt, reply end to exit ms: nc 6.33 against fx 10.5 (1.65x better)
- Continuing the latest of 1,000 sessions, wall ms: nc 18.9 against fx 24.8 (1.31x better)
- Streaming headless at 10,000 tok/s, stream end to exit ms: nc 6.24 against fx 7.63 (1.22x better)
- One-shot headless prompt, wall ms: nc 19.2 against fx 23.0 (1.20x better)
- Streaming TUI 120x40 at 1,000 tok/s, Enter to request ms: nc 3.04 against fx 3.31 (1.09x better)

## Where nc trails

- Startup: --version, peak RSS MB: nc 6.18 against fx 0.000 (Infinityx worse)
- Install size, main binary MB: nc 37.5 against pi 0.000 (245789.50x worse)
- Streaming headless at 10,000 tok/s, per-token lag p99 ms: nc 16.5 against fx 0.009 (1821.03x worse)
- Streaming headless at 10,000 tok/s, per-token lag p50 ms: nc 8.31 against fx 0.008 (1054.20x worse)
- Streaming headless at 1,000 tok/s, per-token lag p99 ms: nc 16.0 against fx 0.016 (973.14x worse)
- Streaming headless at 1,000 tok/s, per-token lag p50 ms: nc 8.02 against fx 0.010 (831.69x worse)
- Session store: 1,000 one-turn sessions, files: nc 50.0 against opencode 2.00 (25.00x worse)
- Cold start: --version after dropping the page cache, peak RSS MB: nc 5.75 against fx 0.367 (15.67x worse)
- Streaming TUI 120x40 at 10,000 tok/s, terminal bytes per token: nc 33.3 against claude 2.80 (11.89x worse)
- One-shot headless prompt, first request bytes: nc 61,070 against pi 5,428 (11.25x worse)
- Session store: 1,000 one-turn sessions, CPU per saved session ms: nc 15.5 against fx 1.71 (9.07x worse)
- One-shot headless prompt, CPU ms: nc 16.6 against fx 1.86 (8.93x worse)
- Long tool turn: 10 shell steps, headless, last request bytes: nc 65,374 against pi 7,322 (8.93x worse)
- Streaming TUI 120x40 at 1,000 tok/s, terminal bytes per token: nc 300 against claude 37.2 (8.06x worse)
- Session store: 1,000 one-turn sessions, store KB: nc 3,408 against pi 432 (7.89x worse)
- Session store: 1,000 one-turn sessions, KB per session: nc 68.2 against pi 8.64 (7.89x worse)
- Continuing the latest of 1,000 sessions, CPU ms: nc 16.6 against fx 2.16 (7.70x worse)
- Time to first frame, CPU ms: nc 25.9 against fx 4.15 (6.23x worse)
- Startup: --version, CPU ms: nc 0.955 against fx 0.159 (6.01x worse)
- Fleet: 8 concurrent headless runs, 1,000 tokens each at 1,000 tok/s, CPU per run ms: nc 34.6 against fx 6.25 (5.54x worse)
- Streaming headless at 1,000 tok/s, peak RSS MB: nc 30.0 against fx 7.75 (3.87x worse)
- Fleet: 8 concurrent headless runs, 1,000 tokens each at 1,000 tok/s, peak RSS per run MB: nc 28.9 against fx 7.59 (3.81x worse)
- Listing 1,000 sessions, CPU ms: nc 1.83 against fx 0.487 (3.77x worse)
- Listing 1,000 sessions, peak RSS MB: nc 12.4 against fx 3.38 (3.67x worse)
- One-shot headless prompt, peak RSS MB: nc 27.3 against fx 7.49 (3.64x worse)
- Cold start: --version after dropping the page cache, CPU ms: nc 1.96 against fx 0.559 (3.50x worse)
- Time to first frame, peak RSS MB: nc 30.1 against fx 8.83 (3.41x worse)
- Continuing the latest of 1,000 sessions, peak RSS MB: nc 28.8 against fx 8.76 (3.28x worse)
- Install size, install MB: nc 37.5 against fx 12.2 (3.07x worse)
- Idle terminal interface, peak RSS MB: nc 30.3 against fx 9.91 (3.05x worse)
- Streaming headless at 10,000 tok/s, peak RSS MB: nc 33.5 against fx 11.1 (3.02x worse)
- Long tool turn: 10 shell steps, headless, peak RSS MB: nc 42.2 against fx 15.7 (2.68x worse)
- Time to first frame, first frame ms: nc 5.41 against fx 2.02 (2.68x worse)
- Streaming headless at 1,000 tok/s, CPU, whole run ms: nc 53.0 against fx 21.9 (2.43x worse)
- Streaming headless at 1,000 tok/s, CPU per token µs: nc 10.6 against fx 4.37 (2.43x worse)
- Streaming TUI 120x40 at 1,000 tok/s, peak RSS MB: nc 36.3 against fx 16.1 (2.26x worse)
- Long tool turn: 10 shell steps, headless, CPU per step ms: nc 5.55 against fx 2.70 (2.06x worse)
- Time to first frame, first byte ms: nc 1.99 against fx 1.01 (1.96x worse)
- Streaming TUI 120x40 at 10,000 tok/s, peak RSS MB: nc 41.6 against fx 22.3 (1.86x worse)
- Streaming TUI 120x40 at 1,000 tok/s, on-screen lag p50 ms: nc 8.06 against codex 5.53 (1.46x worse)
- Streaming headless at 10,000 tok/s, CPU, whole run ms: nc 259 against fx 183 (1.42x worse)
- Streaming headless at 10,000 tok/s, CPU per token µs: nc 5.18 against fx 3.65 (1.42x worse)
- Cold start: --version after dropping the page cache, wall ms: nc 9.20 against fx 7.08 (1.30x worse)
- Streaming TUI 120x40 at 1,000 tok/s, on-screen lag p99 ms: nc 17.1 against pi 15.1 (1.13x worse)
- Startup: --version, wall ms: nc 2.98 against fx 2.75 (1.08x worse)
- Listing 1,000 sessions, wall ms: nc 2.94 against fx 2.75 (1.07x worse)
- Idle terminal interface, bytes written while idle: nc 0.000 against claude -96.0 (-Infinityx better)
- Idle terminal interface, idle CPU ms/s: nc 0.081 against pi -0.199 (-2.45x better)

## Ties (within 5%)

- One-shot headless prompt, spawn to request ms: nc 12.4 against fx 12.5
- One-shot headless prompt, side requests: nc 0.000 against fx 0.000
- One-shot headless prompt, processes left behind: nc 0.000 against fx 0.000
- Fleet: 8 concurrent headless runs, 1,000 tokens each at 1,000 tok/s, wall ms: nc 1,030 against fx 1,039
- Continuing the latest of 1,000 sessions, spawn to request ms: nc 12.4 against fx 12.1
- Streaming headless at 1,000 tok/s, tokens never on stdout: nc 0.000 against fx 0.000
- Streaming headless at 10,000 tok/s, tokens never on stdout: nc 0.000 against fx 0.000
- Streaming TUI 120x40 at 10,000 tok/s, Enter to request ms: nc 3.24 against fx 3.33
- Streaming TUI 120x40 at 1,000 tok/s, tokens never on screen: nc 0.000 against fx 0.000
- Streaming TUI 120x40 at 10,000 tok/s, tokens never on screen: nc 0.000 against fx 0.000
