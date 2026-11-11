# CLI benchmark: 2026-11-11-epyc4245p-lite2

Machine: AMD EPYC 4245P 6-Core Processor, 12 threads, 46 GB, Ubuntu 24.04.5 LTS, 6.8.0-147-generic. 

# Installed CLIs

Linux x86_64. nc and nclite are release-profile builds (cargo build --release, lto off) of nanocodexyz/nanocode master 1da9dd20c on Ubuntu 24.04; the published nanocode-cli release uses the dist profile (fat LTO), and nc-lite's dist build is 26.9 MB.

| Lane | Version | Install | Main binary | Runtime not counted |
| --- | --- | --- | --- | --- |
| nc | nanocode 0.7.0 (1da9dd20c) | cargo build --locked --release --manifest-path bin/nanocode/Cargo.toml --bin nanocode | tools/install/ncm/nanocode | none |
| nclite | nc-lite 0.7.0 (1da9dd20c) | cargo build --locked --release -p nc-lite | tools/install/nclite/nc-lite | none |
| fx | 0.0.16 | curl https://releases.fx.sh/0.0.16/fx-linux-x86_64.tar.gz | tools/install/fx/fx | none |

Medians with the sample count in parentheses and [min–max] where the spread matters. CPU and memory are the whole process tree (cgroup or /proc sampling; rusage when neither is available). Lower is better everywhere.

## Startup: --version

| lane | samples | errors | wall ms | CPU ms | peak RSS MB | instructions |
|---|---:|---:|---:|---:|---:|---:|
| true | 100 | 0 | 2.67 [2.62–3.88] (100) | 0.152 (100) | 1.25 (100) | n/a |
| nc | 100 | 0 | 2.64 [2.58–2.96] (100) | 1.25 (100) | 15.2 (100) | n/a |
| fx | 100 | 0 | 2.64 [1.12–2.85] (100) | 0.109 (100) | 1.38 (100) | n/a |
| nclite | 100 | 0 | 2.64 [2.59–2.93] (100) | 0.530 (100) | 5.63 (100) | n/a |

## Cold start: --version after dropping the page cache

| lane | samples | errors | wall ms | CPU ms | peak RSS MB |
|---|---:|---:|---:|---:|---:|
| nc | 10 | 10 | 10.8 [10.8–11.0] (10) | 3.15 (10) | 15.0 (10) |
| fx | 10 | 10 | 4.83 [4.78–8.77] (10) | 0.538 (10) | 1.38 (10) |
| nclite | 10 | 10 | 6.81 [6.78–6.87] (10) | 1.45 (10) | 5.63 (10) |

- nc error: cannot drop the page cache (needs Linux and passwordless sudo)
- fx error: cannot drop the page cache (needs Linux and passwordless sudo)
- nclite error: cannot drop the page cache (needs Linux and passwordless sudo)

## Time to first frame

| lane | samples | errors | first byte ms | first frame ms | CPU ms | peak RSS MB | processes |
|---|---:|---:|---:|---:|---:|---:|---:|
| nc | 30 | 0 | 3.28 (30) | 6.14 [5.35–7.00] (30) | 25.3 (30) | 59.3 (30) | 0.000 (30) |
| fx | 30 | 0 | 1.01 (30) | 1.99 [1.74–2.41] (30) | 3.90 (30) | 8.97 (30) | 0.000 (30) |
| nclite | 30 | 0 | 1.21 (30) | 1.33 [1.14–1.61] (30) | 3.85 (30) | 16.7 (30) | 0.000 (30) |

## Memory one second after the first frame

Summed over the CLI's process tree from /proc/<pid>/smaps_rollup. RSS counts the executable's file pages, which every instance shares; PSS splits shared pages between the processes that map them; USS is memory only this tree holds.

| lane | samples | errors | RSS MB | PSS MB | private (USS) MB | anonymous MB |
|---|---:|---:|---:|---:|---:|---:|
| nc | 30 | 0 | 58.8 (30) | 56.5 (30) | 56.3 (30) | 26.5 (30) |
| fx | 30 | 0 | 8.91 (30) | 8.90 (30) | 8.89 (30) | 1.25 (30) |
| nclite | 30 | 0 | 16.7 (30) | 14.6 (30) | 14.4 (30) | 1.90 (30) |

## Time to first frame, cold page cache

| lane | samples | errors | first frame ms | CPU ms |
|---|---:|---:|---:|---:|
| nc | 0 | 10 | failed (10) | failed (10) |
| fx | 0 | 10 | failed (10) | failed (10) |
| nclite | 0 | 10 | failed (10) | failed (10) |

- nc error: cannot drop the page cache
- fx error: cannot drop the page cache
- nclite error: cannot drop the page cache

## Idle terminal interface

| lane | samples | errors | idle CPU ms/s | bytes written while idle | peak RSS MB | processes |
|---|---:|---:|---:|---:|---:|---:|
| nc | 10 | 0 | 0.001 [-0.252–0.301] (10) | 0.000 (10) | 59.0 (10) | 0.000 (10) |
| fx | 10 | 0 | 0.617 [0.442–0.643] (10) | 0.000 (10) | 9.46 (10) | 0.000 (10) |
| nclite | 10 | 0 | -0.005 [-0.017–0.005] (10) | 0.000 (10) | 16.8 (10) | 0.000 (10) |

## Keystroke echo

| lane | samples | errors | echo p50 ms | echo max ms | keys echoed of 26 |
|---|---:|---:|---:|---:|---:|
| nc | 10 | 0 | 0.098 [0.092–0.099] (10) | 0.125 (10) | 26.0 (10) |
| fx | 10 | 0 | 0.314 [0.271–0.318] (10) | 113 (10) | 26.0 (10) |
| nclite | 10 | 0 | 0.013 [0.010–0.013] (10) | 0.031 (10) | 26.0 (10) |

## One-shot headless prompt

| lane | samples | errors | wall ms | spawn to request ms | reply end to exit ms | CPU ms | peak RSS MB | first request bytes | side requests | processes left behind |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| nc | 30 | 0 | 8.95 [8.89–11.2] (30) | 6.92 (30) | 1.73 (30) | 11.0 (30) | 56.6 (30) | 62,441 (30) | 0.000 (30) | 0.000 (30) |
| fx | 30 | 0 | 23.1 [22.8–33.1] (30) | 12.5 (30) | 10.6 (30) | 1.77 (30) | 7.61 (30) | 26,274 (30) | 0.000 (30) | 0.000 (30) |
| nclite | 30 | 0 | 6.90 [6.83–7.14] (30) | 4.95 (30) | 1.84 (30) | 5.35 (30) | 27.5 (30) | 23,941 (30) | 0.000 (30) | 0.000 (30) |

## Long tool turn: 10 shell steps, headless

| lane | samples | errors | wall ms | tool result to next request p50 ms | CPU per step ms | peak RSS MB | last request bytes |
|---|---:|---:|---:|---:|---:|---:|---:|
| nc | 10 | 0 | 31.0 [29.0–35.0] (10) | 1.33 (10) | 6.08 (10) | 77.2 (10) | 66,745 (10) |
| fx | 10 | 0 | 231 [229–241] (10) | 20.4 (10) | 2.59 (10) | 16.2 (10) | 33,000 (10) |
| nclite | 10 | 0 | 21.0 [19.0–21.1] (10) | 0.915 (10) | 3.66 (10) | 40.5 (10) | 27,531 (10) |

## Long tool turn: 50 shell steps, headless

| lane | samples | errors | wall ms | tool result to next request p50 ms | CPU per step ms | peak RSS MB | last request bytes |
|---|---:|---:|---:|---:|---:|---:|---:|
| nc | 10 | 0 | 87.1 [85.1–89.1] (10) | 1.16 (10) | 3.84 (10) | 80.3 (10) | 83,865 (10) |
| fx | 10 | 0 | 1,060 [1,057–1,065] (10) | 20.5 (10) | 2.25 (10) | 18.4 (10) | 59,760 (10) |
| nclite | 10 | 0 | 59.1 [57.1–61.1] (10) | 0.826 (10) | 2.84 (10) | 42.3 (10) | 41,771 (10) |

## Fleet: 1 concurrent headless runs, 1,000 tokens each at 1,000 tok/s

| lane | samples | errors | wall ms | CPU per run ms | peak RSS per run MB | stream end to exit p50 ms |
|---|---:|---:|---:|---:|---:|---:|
| nc | 10 | 0 | 1,010 [1,010–1,012] (10) | 17.0 (10) | 56.4 (10) | 2.53 (10) |
| fx | 10 | 0 | 1,020 [1,020–1,038] (10) | 5.31 (10) | 7.75 (10) | 7.25 (10) |
| nclite | 10 | 0 | 1,008 [1,008–1,009] (10) | 12.8 (10) | 27.7 (10) | 2.67 (10) |

## Fleet: 8 concurrent headless runs, 1,000 tokens each at 1,000 tok/s

| lane | samples | errors | wall ms | CPU per run ms | peak RSS per run MB | stream end to exit p50 ms |
|---|---:|---:|---:|---:|---:|---:|
| nc | 10 | 0 | 1,027 [1,026–1,028] (10) | 28.6 (10) | 55.8 (10) | 3.15 (10) |
| fx | 10 | 0 | 1,024 [1,024–1,030] (10) | 5.29 (10) | 7.70 (10) | 7.80 (10) |
| nclite | 10 | 0 | 1,012 [1,012–1,013] (10) | 15.0 (10) | 27.4 (10) | 2.77 (10) |

## Fleet: 32 concurrent headless runs, 1,000 tokens each at 1,000 tok/s

| lane | samples | errors | wall ms | CPU per run ms | peak RSS per run MB | stream end to exit p50 ms |
|---|---:|---:|---:|---:|---:|---:|
| nc | 9 | 1 | 1,112 [1,107–1,113] (9) | 35.2 (9) | 55.7 (9) | 2.80 (9) |
| fx | 10 | 0 | 1,046 [1,041–1,055] (10) | 5.80 (10) | 7.67 (10) | 7.54 (10) |
| nclite | 10 | 0 | 1,041 [1,038–1,045] (10) | 23.7 (10) | 27.5 (10) | 2.17 (10) |

- nc error: nanocode: computer use unavailable, continuing without it: cua-driver is not installed: run nanocode cua install, or set CUA_DRIVER_PATH 

## Session store: 1,000 one-turn sessions

| lane | samples | errors | store KB | KB per session | files | CPU per saved session ms |
|---|---:|---:|---:|---:|---:|---:|
| nc | 1 | 0 | 16,152 (1) | 16.2 (1) | 1,005 (1) | 12.6 (1) |
| fx | 1 | 0 | 12,780 (1) | 12.8 (1) | 2,005 (1) | 1.59 (1) |
| nclite | 1 | 0 | 12,112 (1) | 12.1 (1) | 1,002 (1) | 4.83 (1) |

## Listing 1,000 sessions

| lane | samples | errors | wall ms | CPU ms | peak RSS MB |
|---|---:|---:|---:|---:|---:|
| nc | 20 | 0 | 11.6 [11.5–165] (20) | 10.4 (20) | 41.2 (20) |
| fx | 20 | 0 | 6.90 [6.84–7.01] (20) | 4.95 (20) | 6.34 (20) |
| nclite | 20 | 0 | 9.03 [8.99–137] (20) | 7.57 (20) | 12.4 (20) |

## Continuing the latest of 1,000 sessions

| lane | samples | errors | wall ms | spawn to request ms | CPU ms | peak RSS MB |
|---|---:|---:|---:|---:|---:|---:|
| nc | 20 | 0 | 17.5 [17.3–17.6] (20) | 14.4 (20) | 19.6 (20) | 70.2 (20) |
| fx | 20 | 0 | 28.9 [18.9–38.9] (20) | 16.5 (20) | 6.23 (20) | 9.02 (20) |
| nclite | 20 | 0 | 13.0 [13.0–13.3] (20) | 11.1 (20) | 11.5 (20) | 30.2 (20) |

## Install size

Runtimes a lane needs but does not install (Node for npm packages) are not counted; VERSIONS.md lists them.

| lane | samples | errors | install MB | main binary MB |
|---|---:|---:|---:|---:|
| nc | 1 | 0 | 65.5 (1) | 65.5 (1) |
| fx | 1 | 0 | 12.2 (1) | 12.2 (1) |
| nclite | 1 | 0 | 43.9 (1) | 43.8 (1) |

## Streaming, headless

**per-token lag p50 ms**

| lane | 100 tok/s | 1,000 tok/s | 10,000 tok/s |
|---|---:|---:|---:|
| nc | 0.014 (10) | 0.009 (10) | 0.008 (10) |
| fx | 0.011 (10) | 0.007 (10) | 0.007 (10) |
| nclite | 0.019 (10) | 0.011 (10) | 0.010 (10) |

**per-token lag p99 ms**

| lane | 100 tok/s | 1,000 tok/s | 10,000 tok/s |
|---|---:|---:|---:|
| nc | 0.035 (10) | 0.011 (10) | 0.010 (10) |
| fx | 0.019 (10) | 0.009 (10) | 0.008 (10) |
| nclite | 0.026 (10) | 0.013 (10) | 0.012 (10) |

**stream end to exit ms**

| lane | 100 tok/s | 1,000 tok/s | 10,000 tok/s |
|---|---:|---:|---:|
| nc | 3.48 (10) | 3.94 (10) | 6.74 (10) |
| fx | 6.46 (10) | 7.23 (10) | 7.92 (10) |
| nclite | 1.70 (10) | 2.38 (10) | 5.15 (10) |

**CPU, whole run ms**

| lane | 100 tok/s | 1,000 tok/s | 10,000 tok/s |
|---|---:|---:|---:|
| nc | 15.7 (10) | 38.8 (10) | 266 (10) |
| fx | 5.17 (10) | 18.6 (10) | 152 (10) |
| nclite | 11.8 (10) | 42.0 (10) | 354 (10) |

**CPU per token µs**

| lane | 100 tok/s | 1,000 tok/s | 10,000 tok/s |
|---|---:|---:|---:|
| nc | 31.4 (10) | 7.76 (10) | 5.32 (10) |
| fx | 10.3 (10) | 3.71 (10) | 3.04 (10) |
| nclite | 23.6 (10) | 8.40 (10) | 7.08 (10) |

**peak RSS MB**

| lane | 100 tok/s | 1,000 tok/s | 10,000 tok/s |
|---|---:|---:|---:|
| nc | 56.3 (10) | 57.0 (10) | 73.2 (10) |
| fx | 7.75 (10) | 8.00 (10) | 11.2 (10) |
| nclite | 27.5 (10) | 27.8 (10) | 30.8 (10) |

**tokens never on stdout**

| lane | 100 tok/s | 1,000 tok/s | 10,000 tok/s |
|---|---:|---:|---:|
| nc | 0.000 (10) | 0.000 (10) | 0.000 (10) |
| fx | 0.000 (10) | 0.000 (10) | 0.000 (10) |
| nclite | 0.000 (10) | 0.000 (10) | 0.000 (10) |

## Streaming, terminal interface 240x70

**on-screen lag p50 ms**

| lane | 10,000 tok/s |
|---|---:|
| nc | 3.68 (7) |
| fx | 22.6 (7) |
| nclite | 0.011 (7) |

**on-screen lag p99 ms**

| lane | 10,000 tok/s |
|---|---:|
| nc | 9.06 (7) |
| fx | 38.6 (7) |
| nclite | 0.111 (7) |

**stream end to last token on screen ms**

| lane | 10,000 tok/s |
|---|---:|
| nc | 3.01 (7) |
| fx | 37.0 (7) |
| nclite | -0.130 (7) |

**Enter to request ms**

| lane | 10,000 tok/s |
|---|---:|
| nc | 2.09 (7) |
| fx | 3.83 (7) |
| nclite | 1.93 (7) |

**CPU, whole session ms**

| lane | 10,000 tok/s |
|---|---:|
| nc | 696 (7) |
| fx | 2,607 (7) |
| nclite | 371 (7) |

**peak RSS MB**

| lane | 10,000 tok/s |
|---|---:|
| nc | 115 (7) |
| fx | 27.3 (7) |
| nclite | 30.8 (7) |

**terminal bytes per token**

| lane | 10,000 tok/s |
|---|---:|
| nc | 157 (7) |
| fx | 56.3 (7) |
| nclite | 6.86 (7) |

**tokens never on screen**

| lane | 10,000 tok/s |
|---|---:|
| nc | 0.000 (7) |
| fx | 0.000 (7) |
| nclite | 0.000 (7) |

## Streaming, terminal interface 120x40

**on-screen lag p50 ms**

| lane | 100 tok/s | 1,000 tok/s | 10,000 tok/s | 50,000 tok/s |
|---|---:|---:|---:|---:|
| nc | 0.103 (10) | 5.07 (10) | 3.19 (10) | 3.40 (10) |
| fx | 201 (10) | 32.6 (10) | 21.6 (10) | 57.9 (10) |
| nclite | 0.019 (10) | 0.012 (10) | 0.011 (10) | 0.011 (10) |

**on-screen lag p99 ms**

| lane | 100 tok/s | 1,000 tok/s | 10,000 tok/s | 50,000 tok/s |
|---|---:|---:|---:|---:|
| nc | 0.233 (10) | 9.25 (10) | 8.50 (10) | 8.51 (10) |
| fx | 402 (10) | 54.1 (10) | 37.6 (10) | 253 (10) |
| nclite | 0.044 (10) | 0.015 (10) | 0.111 (10) | 0.031 (10) |

**stream end to last token on screen ms**

| lane | 100 tok/s | 1,000 tok/s | 10,000 tok/s | 50,000 tok/s |
|---|---:|---:|---:|---:|
| nc | 0.124 (10) | 1.45 (10) | 2.13 (10) | 3.97 (10) |
| fx | 19.0 (10) | 12.7 (10) | 28.6 (10) | 223 (10) |
| nclite | 0.067 (10) | 0.009 (10) | -0.121 (10) | -0.942 (10) |

**Enter to request ms**

| lane | 100 tok/s | 1,000 tok/s | 10,000 tok/s | 50,000 tok/s |
|---|---:|---:|---:|---:|
| nc | 1.98 (10) | 2.02 (10) | 2.06 (10) | 1.98 (10) |
| fx | 3.14 (10) | 3.10 (10) | 3.15 (10) | 3.14 (10) |
| nclite | 1.73 (10) | 1.78 (10) | 1.85 (10) | 1.93 (10) |

**CPU, whole session ms**

| lane | 100 tok/s | 1,000 tok/s | 10,000 tok/s | 50,000 tok/s |
|---|---:|---:|---:|---:|
| nc | 79.5 (10) | 128 (10) | 603 (10) | 2,613 (10) |
| fx | 48.3 (10) | 219 (10) | 2,448 (10) | 5,044 (10) |
| nclite | 11.9 (10) | 43.8 (10) | 365 (10) | 1,784 (10) |

**peak RSS MB**

| lane | 100 tok/s | 1,000 tok/s | 10,000 tok/s | 50,000 tok/s |
|---|---:|---:|---:|---:|
| nc | 73.1 (10) | 79.9 (10) | 108 (10) | 136 (10) |
| fx | 12.7 (10) | 15.8 (10) | 23.2 (10) | 109 (10) |
| nclite | 27.4 (10) | 27.9 (10) | 30.8 (10) | 44.6 (10) |

**terminal bytes per token**

| lane | 100 tok/s | 1,000 tok/s | 10,000 tok/s | 50,000 tok/s |
|---|---:|---:|---:|---:|
| nc | 190 (10) | 388 (10) | 63.0 (10) | 15.7 (10) |
| fx | 39.4 (10) | 73.7 (10) | 31.1 (10) | 10.00 (10) |
| nclite | 5.43 (10) | 5.89 (10) | 6.86 (10) | 7.63 (10) |

**tokens never on screen**

| lane | 100 tok/s | 1,000 tok/s | 10,000 tok/s | 50,000 tok/s |
|---|---:|---:|---:|---:|
| nc | 0.000 (10) | 0.000 (10) | 0.000 (10) | 0.000 (10) |
| fx | 0.000 (10) | 0.000 (10) | 0.000 (10) | 0.000 (10) |
| nclite | 0.000 (10) | 0.000 (10) | 0.000 (10) | 0.000 (10) |

## Outbound connection attempts (network blocked)

| lane | attempts | destinations | names |
|---|---:|---:|---:|
|  | 8 | 6 | api.inference.boundless.network, gateway.tempo-dev.workers.dev, mercator.sh |

## Where nc leads

- Streaming TUI 120x40 at 100 tok/s, on-screen lag p50 ms: nc 0.103 against fx 201 (1949.52x better)
- Streaming TUI 120x40 at 100 tok/s, on-screen lag p99 ms: nc 0.233 against fx 402 (1727.91x better)
- Keystroke echo, echo max ms: nc 0.125 against fx 113 (903.22x better)
- Idle terminal interface, idle CPU ms/s: nc 0.001 against fx 0.617 (483.96x better)
- Streaming TUI 120x40 at 100 tok/s, stream end to last token on screen ms: nc 0.124 against fx 19.0 (152.42x better)
- Streaming TUI 120x40 at 50,000 tok/s, stream end to last token on screen ms: nc 3.97 against fx 223 (56.24x better)
- Streaming TUI 120x40 at 50,000 tok/s, on-screen lag p99 ms: nc 8.51 against fx 253 (29.68x better)
- Long tool turn: 50 shell steps, headless, tool result to next request p50 ms: nc 1.16 against fx 20.5 (17.68x better)
- Streaming TUI 120x40 at 50,000 tok/s, on-screen lag p50 ms: nc 3.40 against fx 57.9 (17.06x better)
- Long tool turn: 10 shell steps, headless, tool result to next request p50 ms: nc 1.33 against fx 20.4 (15.32x better)
- Streaming TUI 120x40 at 10,000 tok/s, stream end to last token on screen ms: nc 2.13 against fx 28.6 (13.43x better)
- Streaming TUI 240x70 at 10,000 tok/s, stream end to last token on screen ms: nc 3.01 against fx 37.0 (12.32x better)
- Long tool turn: 50 shell steps, headless, wall ms: nc 87.1 against fx 1,060 (12.17x better)
- Streaming TUI 120x40 at 1,000 tok/s, stream end to last token on screen ms: nc 1.45 against fx 12.7 (8.78x better)
- Long tool turn: 10 shell steps, headless, wall ms: nc 31.0 against fx 231 (7.44x better)
- Streaming TUI 120x40 at 10,000 tok/s, on-screen lag p50 ms: nc 3.19 against fx 21.6 (6.79x better)
- Streaming TUI 120x40 at 1,000 tok/s, on-screen lag p50 ms: nc 5.07 against fx 32.6 (6.42x better)
- Streaming TUI 240x70 at 10,000 tok/s, on-screen lag p50 ms: nc 3.68 against fx 22.6 (6.16x better)
- One-shot headless prompt, reply end to exit ms: nc 1.73 against fx 10.6 (6.10x better)
- Streaming TUI 120x40 at 1,000 tok/s, on-screen lag p99 ms: nc 9.25 against fx 54.1 (5.84x better)
- Streaming TUI 120x40 at 10,000 tok/s, on-screen lag p99 ms: nc 8.50 against fx 37.6 (4.42x better)
- Streaming TUI 240x70 at 10,000 tok/s, on-screen lag p99 ms: nc 9.06 against fx 38.6 (4.26x better)
- Streaming TUI 120x40 at 10,000 tok/s, CPU, whole session ms: nc 603 against fx 2,448 (4.06x better)
- Streaming TUI 240x70 at 10,000 tok/s, CPU, whole session ms: nc 696 against fx 2,607 (3.74x better)
- Keystroke echo, echo p50 ms: nc 0.098 against fx 0.314 (3.21x better)
- Fleet: 1 concurrent headless runs, 1,000 tokens each at 1,000 tok/s, stream end to exit p50 ms: nc 2.53 against fx 7.25 (2.86x better)
- Fleet: 32 concurrent headless runs, 1,000 tokens each at 1,000 tok/s, stream end to exit p50 ms: nc 2.80 against fx 7.54 (2.69x better)
- One-shot headless prompt, wall ms: nc 8.95 against fx 23.1 (2.58x better)
- Fleet: 8 concurrent headless runs, 1,000 tokens each at 1,000 tok/s, stream end to exit p50 ms: nc 3.15 against fx 7.80 (2.48x better)
- Session store: 1,000 one-turn sessions, files: nc 1,005 against fx 2,005 (2.00x better)
- Streaming TUI 120x40 at 50,000 tok/s, CPU, whole session ms: nc 2,613 against fx 5,044 (1.93x better)
- Streaming headless at 100 tok/s, stream end to exit ms: nc 3.48 against fx 6.46 (1.86x better)
- Streaming headless at 1,000 tok/s, stream end to exit ms: nc 3.94 against fx 7.23 (1.83x better)
- Streaming TUI 240x70 at 10,000 tok/s, Enter to request ms: nc 2.09 against fx 3.83 (1.83x better)
- One-shot headless prompt, spawn to request ms: nc 6.92 against fx 12.5 (1.80x better)
- Streaming TUI 120x40 at 1,000 tok/s, CPU, whole session ms: nc 128 against fx 219 (1.71x better)
- Continuing the latest of 1,000 sessions, wall ms: nc 17.5 against fx 28.9 (1.65x better)
- Streaming TUI 120x40 at 50,000 tok/s, Enter to request ms: nc 1.98 against fx 3.14 (1.59x better)
- Streaming TUI 120x40 at 100 tok/s, Enter to request ms: nc 1.98 against fx 3.14 (1.58x better)
- Streaming TUI 120x40 at 1,000 tok/s, Enter to request ms: nc 2.02 against fx 3.10 (1.54x better)
- Streaming TUI 120x40 at 10,000 tok/s, Enter to request ms: nc 2.06 against fx 3.15 (1.53x better)
- Streaming headless at 10,000 tok/s, stream end to exit ms: nc 6.74 against fx 7.92 (1.17x better)
- Continuing the latest of 1,000 sessions, spawn to request ms: nc 14.4 against fx 16.5 (1.14x better)

## Where nc trails

- Time to first frame, cold page cache, first frame ms: nc failed
- Time to first frame, cold page cache, CPU ms: nc failed
- Memory one second after the first frame, anonymous MB: nc 26.5 against fx 1.25 (21.26x worse)
- Startup: --version, CPU ms: nc 1.25 against fx 0.109 (11.47x worse)
- Startup: --version, peak RSS MB: nc 15.2 against fx 1.38 (11.08x worse)
- Cold start: --version after dropping the page cache, peak RSS MB: nc 15.0 against fx 1.38 (10.93x worse)
- Session store: 1,000 one-turn sessions, CPU per saved session ms: nc 12.6 against fx 1.59 (7.93x worse)
- Continuing the latest of 1,000 sessions, peak RSS MB: nc 70.2 against fx 9.02 (7.78x worse)
- One-shot headless prompt, peak RSS MB: nc 56.6 against fx 7.61 (7.44x worse)
- Fleet: 1 concurrent headless runs, 1,000 tokens each at 1,000 tok/s, peak RSS per run MB: nc 56.4 against fx 7.75 (7.28x worse)
- Streaming headless at 100 tok/s, peak RSS MB: nc 56.3 against fx 7.75 (7.26x worse)
- Fleet: 8 concurrent headless runs, 1,000 tokens each at 1,000 tok/s, peak RSS per run MB: nc 55.8 against fx 7.70 (7.26x worse)
- Fleet: 32 concurrent headless runs, 1,000 tokens each at 1,000 tok/s, peak RSS per run MB: nc 55.7 against fx 7.67 (7.25x worse)
- Streaming headless at 1,000 tok/s, peak RSS MB: nc 57.0 against fx 8.00 (7.13x worse)
- Time to first frame, peak RSS MB: nc 59.3 against fx 8.97 (6.61x worse)
- Memory one second after the first frame, RSS MB: nc 58.8 against fx 8.91 (6.60x worse)
- Streaming headless at 10,000 tok/s, peak RSS MB: nc 73.2 against fx 11.2 (6.52x worse)
- Listing 1,000 sessions, peak RSS MB: nc 41.2 against fx 6.34 (6.49x worse)
- Time to first frame, CPU ms: nc 25.3 against fx 3.90 (6.48x worse)
- Memory one second after the first frame, PSS MB: nc 56.5 against fx 8.90 (6.35x worse)
- Memory one second after the first frame, private (USS) MB: nc 56.3 against fx 8.89 (6.33x worse)
- Idle terminal interface, peak RSS MB: nc 59.0 against fx 9.46 (6.23x worse)
- One-shot headless prompt, CPU ms: nc 11.0 against fx 1.77 (6.19x worse)
- Fleet: 32 concurrent headless runs, 1,000 tokens each at 1,000 tok/s, CPU per run ms: nc 35.2 against fx 5.80 (6.07x worse)
- Cold start: --version after dropping the page cache, CPU ms: nc 3.15 against fx 0.538 (5.86x worse)
- Streaming TUI 120x40 at 100 tok/s, peak RSS MB: nc 73.1 against fx 12.7 (5.74x worse)
- Fleet: 8 concurrent headless runs, 1,000 tokens each at 1,000 tok/s, CPU per run ms: nc 28.6 against fx 5.29 (5.41x worse)
- Install size, main binary MB: nc 65.5 against fx 12.2 (5.36x worse)
- Install size, install MB: nc 65.5 against fx 12.2 (5.36x worse)
- Streaming TUI 120x40 at 1,000 tok/s, terminal bytes per token: nc 388 against fx 73.7 (5.27x worse)
- Streaming TUI 120x40 at 1,000 tok/s, peak RSS MB: nc 79.9 against fx 15.8 (5.04x worse)
- Streaming TUI 120x40 at 100 tok/s, terminal bytes per token: nc 190 against fx 39.4 (4.84x worse)
- Long tool turn: 10 shell steps, headless, peak RSS MB: nc 77.2 against fx 16.2 (4.76x worse)
- Streaming TUI 120x40 at 10,000 tok/s, peak RSS MB: nc 108 against fx 23.2 (4.69x worse)
- Long tool turn: 50 shell steps, headless, peak RSS MB: nc 80.3 against fx 18.4 (4.38x worse)
- Streaming TUI 240x70 at 10,000 tok/s, peak RSS MB: nc 115 against fx 27.3 (4.23x worse)
- Time to first frame, first byte ms: nc 3.28 against fx 1.01 (3.24x worse)
- Fleet: 1 concurrent headless runs, 1,000 tokens each at 1,000 tok/s, CPU per run ms: nc 17.0 against fx 5.31 (3.20x worse)
- Continuing the latest of 1,000 sessions, CPU ms: nc 19.6 against fx 6.23 (3.15x worse)
- Time to first frame, first frame ms: nc 6.14 against fx 1.99 (3.09x worse)
- Streaming headless at 100 tok/s, CPU, whole run ms: nc 15.7 against fx 5.17 (3.03x worse)
- Streaming headless at 100 tok/s, CPU per token µs: nc 31.4 against fx 10.3 (3.03x worse)
- Streaming TUI 240x70 at 10,000 tok/s, terminal bytes per token: nc 157 against fx 56.3 (2.79x worse)
- One-shot headless prompt, first request bytes: nc 62,441 against fx 26,274 (2.38x worse)
- Long tool turn: 10 shell steps, headless, CPU per step ms: nc 6.08 against fx 2.59 (2.35x worse)
- Cold start: --version after dropping the page cache, wall ms: nc 10.8 against fx 4.83 (2.24x worse)
- Listing 1,000 sessions, CPU ms: nc 10.4 against fx 4.95 (2.09x worse)
- Streaming headless at 1,000 tok/s, CPU, whole run ms: nc 38.8 against fx 18.6 (2.09x worse)
- Streaming headless at 1,000 tok/s, CPU per token µs: nc 7.76 against fx 3.71 (2.09x worse)
- Streaming TUI 120x40 at 10,000 tok/s, terminal bytes per token: nc 63.0 against fx 31.1 (2.03x worse)
- Long tool turn: 10 shell steps, headless, last request bytes: nc 66,745 against fx 33,000 (2.02x worse)
- Streaming headless at 10,000 tok/s, CPU, whole run ms: nc 266 against fx 152 (1.75x worse)
- Streaming headless at 10,000 tok/s, CPU per token µs: nc 5.32 against fx 3.04 (1.75x worse)
- Long tool turn: 50 shell steps, headless, CPU per step ms: nc 3.84 against fx 2.25 (1.71x worse)
- Listing 1,000 sessions, wall ms: nc 11.6 against fx 6.90 (1.68x worse)
- Streaming TUI 120x40 at 100 tok/s, CPU, whole session ms: nc 79.5 against fx 48.3 (1.65x worse)
- Streaming TUI 120x40 at 50,000 tok/s, terminal bytes per token: nc 15.7 against fx 10.00 (1.58x worse)
- Long tool turn: 50 shell steps, headless, last request bytes: nc 83,865 against fx 59,760 (1.40x worse)
- Session store: 1,000 one-turn sessions, KB per session: nc 16.2 against fx 12.8 (1.26x worse)
- Session store: 1,000 one-turn sessions, store KB: nc 16,152 against fx 12,780 (1.26x worse)
- Streaming TUI 120x40 at 50,000 tok/s, peak RSS MB: nc 136 against fx 109 (1.24x worse)
- Fleet: 32 concurrent headless runs, 1,000 tokens each at 1,000 tok/s, wall ms: nc 1,112 against fx 1,046 (1.06x worse)

## Ties (within 5%)

- Startup: --version, wall ms: nc 2.64 against fx 2.64
- Idle terminal interface, bytes written while idle: nc 0.000 against fx 0.000
- One-shot headless prompt, side requests: nc 0.000 against fx 0.000
- One-shot headless prompt, processes left behind: nc 0.000 against fx 0.000
- Fleet: 1 concurrent headless runs, 1,000 tokens each at 1,000 tok/s, wall ms: nc 1,010 against fx 1,020
- Fleet: 8 concurrent headless runs, 1,000 tokens each at 1,000 tok/s, wall ms: nc 1,027 against fx 1,024
- Streaming headless at 100 tok/s, per-token lag p50 ms: nc 0.014 against fx 0.011
- Streaming headless at 1,000 tok/s, per-token lag p50 ms: nc 0.009 against fx 0.007
- Streaming headless at 10,000 tok/s, per-token lag p50 ms: nc 0.008 against fx 0.007
- Streaming headless at 100 tok/s, per-token lag p99 ms: nc 0.035 against fx 0.019
- Streaming headless at 1,000 tok/s, per-token lag p99 ms: nc 0.011 against fx 0.009
- Streaming headless at 10,000 tok/s, per-token lag p99 ms: nc 0.010 against fx 0.008
- Streaming headless at 100 tok/s, tokens never on stdout: nc 0.000 against fx 0.000
- Streaming headless at 1,000 tok/s, tokens never on stdout: nc 0.000 against fx 0.000
- Streaming headless at 10,000 tok/s, tokens never on stdout: nc 0.000 against fx 0.000
- Streaming TUI 240x70 at 10,000 tok/s, tokens never on screen: nc 0.000 against fx 0.000
- Streaming TUI 120x40 at 100 tok/s, tokens never on screen: nc 0.000 against fx 0.000
- Streaming TUI 120x40 at 1,000 tok/s, tokens never on screen: nc 0.000 against fx 0.000
- Streaming TUI 120x40 at 10,000 tok/s, tokens never on screen: nc 0.000 against fx 0.000
- Streaming TUI 120x40 at 50,000 tok/s, tokens never on screen: nc 0.000 against fx 0.000
