# Seven agent CLIs on identical servers, 2026-11-12

*Draft for nanocode.xyz/experiments. Not published.*

We ran seven terminal coding agents through the same scripted work on AMD EPYC 4245P bare-metal servers (6 cores, 12 threads, 46 GB, Ubuntu 24.04, kernel 6.8) on 2026-11-12, between 03:11 and 05:22 UTC. The CLIs are nc (nanocode, built from master 3c16e6c8e with the dist profile, the one releases use), nc-lite (the same SDK trimmed to what fx ships, built from the same commit), Vercel fx 0.0.16, Pi 1.5.2, OpenCode 1.19.5, Claude Code 2.1.322 and Codex CLI 0.167.2. No lane talks to a real model: each one points at a local fixture that streams a fixed reply, runs with its own empty HOME, and runs inside a network namespace where only loopback works. Lanes take turns in every round. Every number is a median with the sample count in parentheses, and CPU and memory cover the CLI's whole process tree. The terminal runs use a 120x40 PTY. What reached the screen is replayed through a VT emulator, so lag means time until the token is visible. The raw JSONL, the commands and the generated [REPORT.md](REPORT.md) are next to this file.

Four servers of the same model ran the work in parallel: one ran the startup, first-frame, idle, keystroke, one-shot and tool-turn suites; one ran netaudit, the cold-cache rows, fleets and install size; two ran the streaming sweeps (headless and 50,000 tok/s; 120x40 and 240x70). The session store ran on a sweep server after its sweeps. [machine.json](machine.json) records each one.

## Startup and first frame

### Startup: --version

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

### Cold start: --version after dropping the page cache

| lane | samples | errors | wall ms | CPU ms | peak RSS MB |
|---|---:|---:|---:|---:|---:|
| nc | 10 | 0 | 11.1 [11.0–11.3] (10) | 2.64 (10) | 9.13 (10) |
| fx | 10 | 0 | 5.32 [5.27–5.58] (10) | 0.622 (10) | 1.38 (10) |
| pi | 10 | 0 | 119 [115–121] (10) | 130 (10) | 103 (10) |
| opencode | 10 | 0 | 236 [236–244] (10) | 281 (10) | 130 (10) |
| claude | 10 | 0 | 29.8 [29.7–31.9] (10) | 13.0 (10) | 21.6 (10) |
| codex | 10 | 0 | 27.6 [27.5–27.7] (10) | 9.62 (10) | 25.8 (10) |
| nclite | 10 | 0 | 7.07 [6.95–7.19] (10) | 1.21 (10) | 4.63 (10) |

### Time to first frame

| lane | samples | errors | first byte ms | first frame ms | CPU ms | peak RSS MB | processes |
|---|---:|---:|---:|---:|---:|---:|---:|
| nc | 30 | 0 | 2.26 (30) | 2.89 [2.67–3.31] (30) | 8.56 (30) | 31.0 (30) | 0.000 (30) |
| fx | 30 | 0 | 1.03 (30) | 2.01 [1.89–2.36] (30) | 3.90 (30) | 8.89 (30) | 0.000 (30) |
| pi | 30 | 0 | 126 (30) | 138 [127–145] (30) | 216 (30) | 121 (30) | 0.000 (30) |
| opencode | 30 | 0 | 285 (30) | 1,376 [1,357–1,399] (30) | 3,435 (30) | 923 (30) | 0.000 (30) |
| claude | 30 | 0 | 84.8 (30) | 151 [149–152] (30) | 404 (30) | 218 (30) | 4.00 (30) |
| codex | 30 | 0 | 5.76 (30) | 126 [118–129] (30) | 190 (30) | 177 (30) | 8.00 (30) |
| nclite | 30 | 0 | 1.14 (30) | 1.25 [1.11–1.77] (30) | 1.37 (30) | 10.0 (30) | 0.000 (30) |

### Time to first frame, cold page cache

| lane | samples | errors | first frame ms | CPU ms |
|---|---:|---:|---:|---:|
| nc | 10 | 0 | 20.6 [20.3–21.0] (10) | 14.5 (10) |
| fx | 10 | 0 | 9.48 [9.00–9.84] (10) | 5.72 (10) |
| pi | 10 | 0 | 155 [146–157] (10) | 220 (10) |
| opencode | 10 | 0 | 1,409 [1,402–1,425] (10) | 3,427 (10) |
| claude | 10 | 0 | 213 [210–215] (10) | 430 (10) |
| codex | 10 | 0 | 223 [220–226] (10) | 222 (10) |
| nclite | 10 | 0 | 4.83 [4.42–5.14] (10) | 2.92 (10) |

### Memory one second after the first frame

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

## Headless runs

### One-shot headless prompt

| lane | samples | errors | wall ms | spawn to request ms | reply end to exit ms | CPU ms | peak RSS MB | first request bytes | side requests | processes left behind |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| nc | 30 | 0 | 6.87 [6.79–7.04] (30) | 4.27 (30) | 2.42 (30) | 7.36 (30) | 33.6 (30) | 62,483 (30) | 0.000 (30) | 0.000 (30) |
| fx | 30 | 0 | 23.2 [23.1–33.2] (30) | 12.5 (30) | 10.5 (30) | 1.87 (30) | 7.52 (30) | 26,281 (30) | 0.000 (30) | 0.000 (30) |
| pi | 30 | 0 | 151 [149–157] (30) | 139 (30) | 12.7 (30) | 203 (30) | 118 (30) | 5,444 (30) | 0.000 (30) | 0.000 (30) |
| opencode | 30 | 0 | 997 [987–1,019] (30) | 963 (30) | 33.4 (30) | 1,853 (30) | 676 (30) | 30,705 (30) | 1.00 (30) | 0.000 (30) |
| claude | 30 | 0 | 163 [159–167] (30) | 144 (30) | 18.9 (30) | 183 (30) | 186 (30) | 46,398 (30) | 0.000 (30) | 0.000 (30) |
| codex | 30 | 0 | 5,045 [5,043–5,047] (30) | 51.5 (30) | 4,993 (30) | 90.4 (30) | 153 (30) | 39,395 (30) | 0.000 (30) | 0.000 (30) |
| nclite | 30 | 0 | 4.89 [4.79–5.19] (30) | 2.52 (30) | 2.21 (30) | 2.70 (30) | 14.1 (30) | 23,942 (30) | 0.000 (30) | 0.000 (30) |

### Long tool turn: 50 shell steps, headless

| lane | samples | errors | wall ms | tool result to next request p50 ms | CPU per step ms | peak RSS MB | last request bytes |
|---|---:|---:|---:|---:|---:|---:|---:|
| nc | 10 | 0 | 69.1 [67.1–71.2] (10) | 0.903 (10) | 3.01 (10) | 51.7 (10) | 83,907 (10) |
| fx | 10 | 0 | 1,057 [1,049–1,061] (10) | 20.5 (10) | 2.12 (10) | 17.4 (10) | 59,767 (10) |
| pi | 10 | 0 | 299 [293–303] (10) | 2.56 (10) | 8.47 (10) | 163 (10) | 14,818 (10) |
| opencode | 10 | 0 | 1,593 [1,579–1,627] (10) | 10.3 (10) | 59.4 (10) | 770 (10) | 42,844 (10) |
| claude | 10 | 0 | 656 [653–665] (10) | 8.57 (10) | 18.4 (10) | 287 (10) | 60,617 (10) |
| codex | 10 | 0 | 5,045 [5,045–5,047] (10) | 41.0 (10) | 23.0 (10) | 319 (10) | 57,225 (10) |
| nclite | 10 | 0 | 55.1 [55.1–59.2] (10) | 0.813 (10) | 2.77 (10) | 29.8 (10) | 41,772 (10) |

### Fleet: 32 concurrent headless runs, 1,000 tokens each at 1,000 tok/s

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

## Streaming

### Streaming, headless

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

### Streaming, terminal interface 120x40

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

## Idle, keys, disk and network

### Idle terminal interface

| lane | samples | errors | idle CPU ms/s | bytes written while idle | peak RSS MB | processes |
|---|---:|---:|---:|---:|---:|---:|
| nc | 10 | 0 | -0.009 [-0.073–0.036] (10) | 0.000 (10) | 30.9 (10) | 0.000 (10) |
| fx | 10 | 0 | 0.550 [0.372–0.646] (10) | 0.000 (10) | 9.54 (10) | 0.000 (10) |
| pi | 10 | 0 | -0.031 [-0.402–0.638] (10) | 0.000 (10) | 121 (10) | 0.000 (10) |
| opencode | 10 | 0 | 24.2 [20.1–27.0] (10) | 0.000 (10) | 926 (10) | 0.000 (10) |
| claude | 10 | 0 | 5.46 [3.29–6.42] (10) | 241 (10) | 219 (10) | 4.00 (10) |
| codex | 10 | 0 | 0.413 [0.249–0.705] (10) | 0.000 (10) | 176 (10) | 7.00 (10) |
| nclite | 10 | 0 | -0.002 [-0.011–0.010] (10) | 0.000 (10) | 10.1 (10) | 0.000 (10) |

### Keystroke echo

| lane | samples | errors | echo p50 ms | echo max ms | keys echoed of 26 |
|---|---:|---:|---:|---:|---:|
| nc | 10 | 0 | 0.078 [0.076–0.081] (10) | 0.100 (10) | 26.0 (10) |
| fx | 10 | 0 | 0.315 [0.308–0.319] (10) | 113 (10) | 26.0 (10) |
| pi | 10 | 0 | 0.317 [0.198–0.406] (10) | 2.18 (10) | 26.0 (10) |
| opencode | 10 | 0 | 1.50 [1.43–1.56] (10) | 21.4 (10) | 26.0 (10) |
| claude | 10 | 0 | 1.30 [0.992–1.36] (10) | 9.83 (10) | 26.0 (10) |
| codex | 10 | 0 | 20.9 [20.8–21.0] (10) | 22.0 (10) | 26.0 (10) |
| nclite | 10 | 0 | 0.013 [0.011–0.014] (10) | 0.031 (10) | 26.0 (10) |

### Install size

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

### Outbound connection attempts (network blocked)

| lane | attempts | destinations | names |
|---|---:|---:|---:|
| nc | 8 | 3 | api.inference.boundless.network, gateway.tempo-dev.workers.dev, mercator.sh |
| fx | 0 | 0 | none |
| pi | 0 | 0 | none |
| opencode | 0 | 0 | registry.npmjs.org |
| claude | 3 | 2 | downloads.claude.ai, github.com |
| codex | 9 | 4 | api.github.com, chatgpt.com, github.com, raw.githubusercontent.com |
| nclite | 0 | 0 | none |

## Session store

### Session store: 1,000 one-turn sessions

| lane | samples | errors | store KB | KB per session | files | CPU per saved session ms |
|---|---:|---:|---:|---:|---:|---:|
| nc | 1 | 0 | 4,264 (1) | 4.26 (1) | 1,006 (1) | 11.4 (1) |
| fx | 1 | 0 | 12,780 (1) | 12.8 (1) | 2,005 (1) | 1.57 (1) |
| pi | 1 | 0 | 8,144 (1) | 8.14 (1) | 1,004 (1) | 203 (1) |
| claude | 1 | 0 | 132,132 (1) | 132 (1) | 1,004 (1) | 176 (1) |
| nclite | 1 | 0 | 4,216 (1) | 4.22 (1) | 1,003 (1) | 9.34 (1) |

### Listing 1,000 sessions

| lane | samples | errors | wall ms | CPU ms | peak RSS MB |
|---|---:|---:|---:|---:|---:|
| nc | 20 | 0 | 9.23 [7.11–9.31] (20) | 6.50 (20) | 20.0 (20) |
| fx | 20 | 0 | 6.88 [6.83–7.00] (20) | 4.94 (20) | 6.28 (20) |
| nclite | 20 | 0 | 8.98 [8.90–9.10] (20) | 6.72 (20) | 11.1 (20) |

### Continuing the latest of 1,000 sessions

| lane | samples | errors | wall ms | spawn to request ms | CPU ms | peak RSS MB |
|---|---:|---:|---:|---:|---:|---:|
| nc | 20 | 0 | 19.1 [17.0–23.1] (20) | 8.92 (20) | 18.7 (20) | 41.9 (20) |
| fx | 20 | 0 | 28.9 [26.8–28.9] (20) | 16.3 (20) | 6.17 (20) | 8.88 (20) |
| pi | 20 | 0 | 155 [154–160] (20) | 143 (20) | 206 (20) | 114 (20) |
| claude | 20 | 0 | 166 [162–168] (20) | 148 (20) | 187 (20) | 195 (20) |
| nclite | 20 | 0 | 22.9 [20.9–23.0] (20) | 8.77 (20) | 20.2 (20) | 18.6 (20) |

## The three largest differences

**Runtime startup.** `pi --version` takes 101 ms and 126 ms of CPU, and `opencode --version` 201 ms and 267 ms. The native binaries (nc, nc-lite, fx) take 2.7 ms, the same as `/usr/bin/true` under this harness. Printing a version does no work of its own, so this gap is startup cost alone. Pi runs on Node (VERSIONS.md), and OpenCode ships a 204 MB executable. The same cost shows again in every run: OpenCode takes 1,376 ms and 3.4 s of CPU to its first frame, and 997 ms for a one-word headless reply. We did not profile inside either runtime.

**Codex waits five seconds after a headless reply.** Its one-shot run takes 5,045 ms, and 4,993 ms of that comes after the last byte of the reply. Its fleet and long-turn rows have the same 5 s floor. In the same runs, netaudit records four blocked connection attempts from headless Codex, to github.com, api.github.com and chatgpt.com. That matches the curated-plugin sync its lane note describes, which this setup cannot reach. The data shows the wait and the attempts. We did not trace whether Codex waits on those connections.

**When tokens reach the screen.** At 100 tok/s, fx shows a token 200 ms after the fixture sends it (p99 403 ms), and Claude Code after 151 ms (p99 331 ms). nc shows it after 0.081 ms and nc-lite after 0.018 ms. At 100 tok/s fx's lag is spread evenly from 0 to about 400 ms, which is what a redraw every 400 ms or so would give, and it falls as the rate rises (21.6 ms at 10,000 tok/s). We did not profile fx. nc pays for its low lag in bytes: it writes 190 terminal bytes per token at 100 tok/s against fx's 39.5 and nc-lite's 5.43, so the extra bytes come from how nc's interface repaints, not from the SDK under it. At 10,000 tok/s and above Claude Code writes the fewest bytes per token.

## Where nc is behind

nc is the heaviest of the native binaries. Its 71.4 MB executable holds 30.9 MB resident one second after the first frame (fx: 8.80 MB). It needs 7.36 ms of CPU for a one-word reply (fx: 1.87 ms) and 20.6 ms to its first frame from a cold page cache (fx: 9.48 ms). A profile of the nc TUI on the same machine (smaps, massif) attributes that memory as follows. About 25 MB is resident pages of the executable's code. About 9 MB is read-only data, of which about 6 MB is relocation tables and data the position-independent executable fixes up at load. The allocator's heap is about 7 MB for under 2 MB of live allocations. About 6 MB, and most of a one-shot run's remaining instructions, belong to the built-in Tempo tools: a payment proxy thread that loads and re-encodes the system's trusted roots at start, and their MCP clients. Those tools also make four outbound connection attempts at every start (netaudit), which fx and nc-lite do not make.

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

The tables above use the dist profile (fat LTO), so their absolute numbers are lower than these release-build numbers.

## What we could not measure

- Session store for OpenCode and Codex. Each headless OpenCode run takes about 1 s and each Codex run about 5 s, so writing 1,000 sessions would have taken 17 and 83 minutes, more than the servers had left.
- Listing sessions for Pi and Claude Code: harness/lanes.mjs defines a listing command only for nc, nc-lite and fx.
- Headless per-token lag for OpenCode and Codex. Neither prints the reply until it is complete (about 2.5 s median lag), so their per-token rows measure that wait, not streaming.
- Memory after the first frame for OpenCode: the smaps sample one second after its first frame came back empty in every round. Its peak RSS from rusage (about 960 MB) is in the first-frame table.
- Instructions retired: on Linux the harness falls back to tools/runstat.py, which reports no instruction counts, so that column says n/a.
- Pi's main binary size: its package entry point is a JavaScript bundle, and the Node runtime it needs is not counted.
