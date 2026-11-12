# CLI benchmark: 2026-11-12-epyc4245p-lite3

Machine: AMD EPYC 4245P 6-Core Processor, 12 threads, 46 GB, Ubuntu 24.04.5 LTS, 6.8.0-147-generic. 

# Versions (staged 2026-11-12 on the Hand)
- fx: 0.0.16 (releases.fx.sh)
- nc: nanocode at 7837aae16, LLVM_BOLT=llvm-bolt-23 scripts/build-cli.sh nanocode-cli nanocode (dist, non-PIE, BOLT), 82145536 bytes
- nclite: nc-lite at 7837aae16, PROFILE=lite scripts/build-cli.sh nc-lite nc-lite (non-PIE, BOLT), 19063696 bytes

Medians with the sample count in parentheses and [min–max] where the spread matters. CPU and memory are the whole process tree (cgroup or /proc sampling; rusage when neither is available). Lower is better everywhere.

## Startup: --version

| lane | samples | errors | wall ms | CPU ms | peak RSS MB | instructions |
|---|---:|---:|---:|---:|---:|---:|
| true | 100 | 0 | 2.65 [2.60–3.83] (100) | 0.149 (100) | 1.25 (100) | n/a |
| nc | 100 | 0 | 2.63 [2.59–2.86] (100) | 0.444 (100) | 5.88 (100) | n/a |
| fx | 100 | 0 | 2.62 [2.58–2.75] (100) | 0.094 (100) | 1.38 (100) | n/a |
| nclite | 100 | 0 | 2.62 [2.56–2.78] (100) | 0.252 (100) | 3.25 (100) | n/a |

## Cold start: --version after dropping the page cache

| lane | samples | errors | wall ms | CPU ms | peak RSS MB |
|---|---:|---:|---:|---:|---:|
| nc | 0 | 10 | failed (10) | failed (10) | failed (10) |
| fx | 0 | 10 | failed (10) | failed (10) | failed (10) |
| nclite | 0 | 10 | failed (10) | failed (10) | failed (10) |

- nc error: cannot drop the page cache (needs Linux and passwordless sudo)
- fx error: cannot drop the page cache (needs Linux and passwordless sudo)
- nclite error: cannot drop the page cache (needs Linux and passwordless sudo)

## Time to first frame

| lane | samples | errors | first byte ms | first frame ms | CPU ms | peak RSS MB | processes |
|---|---:|---:|---:|---:|---:|---:|---:|
| nc | 30 | 0 | 1.52 (30) | 2.02 [1.86–2.37] (30) | 6.84 (30) | 19.0 (30) | 0.000 (30) |
| fx | 30 | 0 | 0.939 (30) | 1.86 [1.66–2.20] (30) | 3.81 (30) | 8.93 (30) | 0.000 (30) |
| nclite | 30 | 0 | 0.864 (30) | 0.959 [0.836–1.35] (30) | 0.917 (30) | 5.19 (30) | 0.000 (30) |

## Memory one second after the first frame

Summed over the CLI's process tree from /proc/<pid>/smaps_rollup. RSS counts the executable's file pages, which every instance shares; PSS splits shared pages between the processes that map them; USS is memory only this tree holds.

| lane | samples | errors | RSS MB | PSS MB | private (USS) MB | anonymous MB |
|---|---:|---:|---:|---:|---:|---:|
| nc | 30 | 0 | 19.0 (30) | 16.7 (30) | 16.4 (30) | 4.36 (30) |
| fx | 30 | 0 | 8.91 (30) | 8.90 (30) | 8.89 (30) | 1.25 (30) |
| nclite | 30 | 0 | 5.19 (30) | 3.07 (30) | 2.84 (30) | 0.352 (30) |

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
| nc | 10 | 0 | -0.023 [-0.041–0.020] (10) | 0.000 (10) | 19.0 (10) | 0.000 (10) |
| fx | 10 | 0 | 0.619 [0.553–0.673] (10) | 0.000 (10) | 9.46 (10) | 0.000 (10) |
| nclite | 10 | 0 | -0.003 [-0.016–0.018] (10) | 0.000 (10) | 5.22 (10) | 0.000 (10) |

## Keystroke echo

| lane | samples | errors | echo p50 ms | echo max ms | keys echoed of 26 |
|---|---:|---:|---:|---:|---:|
| nc | 10 | 0 | 0.076 [0.048–0.082] (10) | 0.101 (10) | 26.0 (10) |
| fx | 10 | 0 | 0.319 [0.168–0.327] (10) | 113 (10) | 26.0 (10) |
| nclite | 10 | 0 | 0.014 [0.012–0.014] (10) | 0.028 (10) | 26.0 (10) |

## One-shot headless prompt

| lane | samples | errors | wall ms | spawn to request ms | reply end to exit ms | CPU ms | peak RSS MB | first request bytes | side requests | processes left behind |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| nc | 30 | 0 | 4.83 [4.75–5.00] (30) | 2.62 (30) | 2.01 (30) | 4.93 (30) | 19.0 (30) | 62,482 (30) | 0.000 (30) | 0.000 (30) |
| fx | 30 | 0 | 22.9 [22.8–33.0] (30) | 12.1 (30) | 10.6 (30) | 1.62 (30) | 7.63 (30) | 26,274 (30) | 0.000 (30) | 0.000 (30) |
| nclite | 30 | 0 | 2.81 [2.77–4.97] (30) | 1.79 (30) | 0.902 (30) | 1.77 (30) | 7.75 (30) | 23,941 (30) | 0.000 (30) | 0.000 (30) |

## Long tool turn: 10 shell steps, headless

| lane | samples | errors | wall ms | tool result to next request p50 ms | CPU per step ms | peak RSS MB | last request bytes |
|---|---:|---:|---:|---:|---:|---:|---:|
| nc | 10 | 0 | 21.0 [19.0–23.1] (10) | 0.925 (10) | 3.73 (10) | 35.0 (10) | 66,786 (10) |
| fx | 10 | 0 | 229 [229–239] (10) | 20.4 (10) | 2.49 (10) | 15.7 (10) | 33,000 (10) |
| nclite | 10 | 0 | 17.0 [14.9–17.1] (10) | 0.839 (10) | 3.19 (10) | 22.0 (10) | 27,531 (10) |

## Long tool turn: 50 shell steps, headless

| lane | samples | errors | wall ms | tool result to next request p50 ms | CPU per step ms | peak RSS MB | last request bytes |
|---|---:|---:|---:|---:|---:|---:|---:|
| nc | 10 | 0 | 63.1 [61.1–65.1] (10) | 0.830 (10) | 2.88 (10) | 36.1 (10) | 83,906 (10) |
| fx | 10 | 0 | 1,057 [1,055–1,069] (10) | 20.5 (10) | 2.22 (10) | 18.5 (10) | 59,760 (10) |
| nclite | 10 | 0 | 53.1 [53.0–58.3] (10) | 0.782 (10) | 2.72 (10) | 23.9 (10) | 41,771 (10) |

## Fleet: 1 concurrent headless runs, 1,000 tokens each at 1,000 tok/s

| lane | samples | errors | wall ms | CPU per run ms | peak RSS per run MB | stream end to exit p50 ms |
|---|---:|---:|---:|---:|---:|---:|
| nc | 10 | 0 | 1,006 [1,006–1,006] (10) | 10.4 (10) | 19.1 (10) | 2.57 (10) |
| fx | 10 | 0 | 1,020 [1,020–1,038] (10) | 5.11 (10) | 7.85 (10) | 7.43 (10) |
| nclite | 10 | 0 | 1,004 [1,004–1,005] (10) | 9.45 (10) | 7.63 (10) | 1.66 (10) |

## Fleet: 8 concurrent headless runs, 1,000 tokens each at 1,000 tok/s

| lane | samples | errors | wall ms | CPU per run ms | peak RSS per run MB | stream end to exit p50 ms |
|---|---:|---:|---:|---:|---:|---:|
| nc | 10 | 0 | 1,012 [1,011–1,012] (10) | 12.3 (10) | 19.1 (10) | 2.33 (10) |
| fx | 10 | 0 | 1,024 [1,024–1,025] (10) | 5.46 (10) | 7.73 (10) | 7.84 (10) |
| nclite | 10 | 0 | 1,008 [1,008–1,008] (10) | 10.8 (10) | 7.68 (10) | 1.86 (10) |

## Fleet: 32 concurrent headless runs, 1,000 tokens each at 1,000 tok/s

| lane | samples | errors | wall ms | CPU per run ms | peak RSS per run MB | stream end to exit p50 ms |
|---|---:|---:|---:|---:|---:|---:|
| nc | 10 | 0 | 1,042 [1,039–1,045] (10) | 13.3 (10) | 19.2 (10) | 2.17 (10) |
| fx | 10 | 0 | 1,047 [1,040–1,052] (10) | 5.86 (10) | 7.69 (10) | 7.48 (10) |
| nclite | 10 | 0 | 1,029 [1,024–1,032] (10) | 12.2 (10) | 7.71 (10) | 1.83 (10) |

## Session store: 1,000 one-turn sessions

| lane | samples | errors | store KB | KB per session | files | CPU per saved session ms |
|---|---:|---:|---:|---:|---:|---:|
| nc | 1 | 0 | 4,252 (1) | 4.25 (1) | 1,006 (1) | 10.3 (1) |
| fx | 1 | 0 | 12,780 (1) | 12.8 (1) | 2,005 (1) | 1.59 (1) |
| nclite | 1 | 0 | 4,208 (1) | 4.21 (1) | 1,003 (1) | 10.5 (1) |

## Listing 1,000 sessions

| lane | samples | errors | wall ms | CPU ms | peak RSS MB |
|---|---:|---:|---:|---:|---:|
| nc | 20 | 0 | 7.25 [7.01–9.65] (20) | 5.99 (20) | 13.5 (20) |
| fx | 20 | 0 | 6.94 [6.87–7.12] (20) | 4.87 (20) | 6.31 (20) |
| nclite | 20 | 0 | 9.01 [8.95–9.36] (20) | 6.86 (20) | 7.87 (20) |

## Continuing the latest of 1,000 sessions

| lane | samples | errors | wall ms | spawn to request ms | CPU ms | peak RSS MB |
|---|---:|---:|---:|---:|---:|---:|
| nc | 20 | 0 | 17.1 [17.0–19.3] (20) | 8.09 (20) | 18.0 (20) | 29.3 (20) |
| fx | 20 | 0 | 28.9 [26.9–38.9] (20) | 16.4 (20) | 6.22 (20) | 9.01 (20) |
| nclite | 20 | 0 | 25.0 [24.9–27.0] (20) | 8.88 (20) | 23.1 (20) | 13.2 (20) |

## Install size

Runtimes a lane needs but does not install (Node for npm packages) are not counted; VERSIONS.md lists them.

| lane | samples | errors | install MB | main binary MB |
|---|---:|---:|---:|---:|
| nc | 0 | 1 | failed (1) | failed (1) |
| fx | 0 | 1 | failed (1) | failed (1) |
| nclite | 0 | 1 | failed (1) | failed (1) |

- nc error: set BENCH_INSTALL_NC to the paths the install added, separated by colons
- fx error: set BENCH_INSTALL_FX to the paths the install added, separated by colons
- nclite error: set BENCH_INSTALL_NCLITE to the paths the install added, separated by colons

## Streaming, headless

**per-token lag p50 ms**

| lane | 100 tok/s | 1,000 tok/s | 10,000 tok/s |
|---|---:|---:|---:|
| nc | 0.015 (6) | 0.008 (6) | 0.008 (6) |
| fx | 0.012 (6) | 0.007 (6) | 0.007 (6) |
| nclite | 0.019 (6) | 0.011 (6) | 0.010 (6) |

**per-token lag p99 ms**

| lane | 100 tok/s | 1,000 tok/s | 10,000 tok/s |
|---|---:|---:|---:|
| nc | 0.026 (6) | 0.010 (6) | 0.009 (6) |
| fx | 0.020 (6) | 0.009 (6) | 0.008 (6) |
| nclite | 0.025 (6) | 0.013 (6) | 0.012 (6) |

**stream end to exit ms**

| lane | 100 tok/s | 1,000 tok/s | 10,000 tok/s |
|---|---:|---:|---:|
| nc | 1.52 (6) | 2.09 (6) | 6.23 (6) |
| fx | 6.44 (6) | 7.12 (6) | 7.71 (6) |
| nclite | 2.67 (6) | 1.50 (6) | 5.56 (6) |

**CPU, whole run ms**

| lane | 100 tok/s | 1,000 tok/s | 10,000 tok/s |
|---|---:|---:|---:|
| nc | 9.82 (6) | 28.5 (6) | 222 (6) |
| fx | 5.22 (6) | 18.2 (6) | 152 (6) |
| nclite | 8.29 (6) | 38.3 (6) | 366 (6) |

**CPU per token µs**

| lane | 100 tok/s | 1,000 tok/s | 10,000 tok/s |
|---|---:|---:|---:|
| nc | 19.6 (6) | 5.69 (6) | 4.45 (6) |
| fx | 10.4 (6) | 3.64 (6) | 3.04 (6) |
| nclite | 16.6 (6) | 7.67 (6) | 7.31 (6) |

**peak RSS MB**

| lane | 100 tok/s | 1,000 tok/s | 10,000 tok/s |
|---|---:|---:|---:|
| nc | 19.0 (6) | 19.0 (6) | 26.0 (6) |
| fx | 7.81 (6) | 8.00 (6) | 11.2 (6) |
| nclite | 7.90 (6) | 8.25 (6) | 11.2 (6) |

**tokens never on stdout**

| lane | 100 tok/s | 1,000 tok/s | 10,000 tok/s |
|---|---:|---:|---:|
| nc | 0.000 (6) | 0.000 (6) | 0.000 (6) |
| fx | 0.000 (6) | 0.000 (6) | 0.000 (6) |
| nclite | 0.000 (6) | 0.000 (6) | 0.000 (6) |

## Streaming, terminal interface 240x70

**on-screen lag p50 ms**

| lane | 10,000 tok/s |
|---|---:|
| nc | 3.60 (6) |
| fx | 22.9 (6) |
| nclite | 0.012 (6) |

**on-screen lag p99 ms**

| lane | 10,000 tok/s |
|---|---:|
| nc | 8.87 (6) |
| fx | 39.4 (6) |
| nclite | 0.112 (6) |

**stream end to last token on screen ms**

| lane | 10,000 tok/s |
|---|---:|
| nc | 2.42 (6) |
| fx | 33.4 (6) |
| nclite | -0.144 (6) |

**Enter to request ms**

| lane | 10,000 tok/s |
|---|---:|
| nc | 1.57 (6) |
| fx | 3.81 (6) |
| nclite | 1.49 (6) |

**CPU, whole session ms**

| lane | 10,000 tok/s |
|---|---:|
| nc | 582 (6) |
| fx | 2,640 (6) |
| nclite | 376 (6) |

**peak RSS MB**

| lane | 10,000 tok/s |
|---|---:|
| nc | 33.2 (6) |
| fx | 27.5 (6) |
| nclite | 11.6 (6) |

**terminal bytes per token**

| lane | 10,000 tok/s |
|---|---:|
| nc | 142 (6) |
| fx | 55.6 (6) |
| nclite | 6.86 (6) |

**tokens never on screen**

| lane | 10,000 tok/s |
|---|---:|
| nc | 0.000 (6) |
| fx | 0.000 (6) |
| nclite | 0.000 (6) |

## Streaming, terminal interface 120x40

**on-screen lag p50 ms**

| lane | 100 tok/s | 1,000 tok/s | 10,000 tok/s | 50,000 tok/s |
|---|---:|---:|---:|---:|
| nc | 0.067 (6) | 5.05 (6) | 4.08 (6) | 4.06 (6) |
| fx | 200 (6) | 32.5 (6) | 21.9 (6) | 59.0 (6) |
| nclite | 0.020 (6) | 0.013 (6) | 0.011 (6) | 0.011 (6) |

**on-screen lag p99 ms**

| lane | 100 tok/s | 1,000 tok/s | 10,000 tok/s | 50,000 tok/s |
|---|---:|---:|---:|---:|
| nc | 0.204 (6) | 9.18 (6) | 8.43 (6) | 8.45 (6) |
| fx | 403 (6) | 54.1 (6) | 38.1 (6) | 256 (6) |
| nclite | 0.032 (6) | 0.016 (6) | 0.112 (6) | 0.032 (6) |

**stream end to last token on screen ms**

| lane | 100 tok/s | 1,000 tok/s | 10,000 tok/s | 50,000 tok/s |
|---|---:|---:|---:|---:|
| nc | 0.102 (6) | 0.658 (6) | 2.96 (6) | 3.57 (6) |
| fx | 15.1 (6) | 13.3 (6) | 34.2 (6) | 191 (6) |
| nclite | 0.039 (6) | 0.005 (6) | -0.155 (6) | -0.872 (6) |

**Enter to request ms**

| lane | 100 tok/s | 1,000 tok/s | 10,000 tok/s | 50,000 tok/s |
|---|---:|---:|---:|---:|
| nc | 1.41 (6) | 1.56 (6) | 1.46 (6) | 1.56 (6) |
| fx | 3.20 (6) | 3.09 (6) | 3.13 (6) | 3.17 (6) |
| nclite | 1.42 (6) | 1.36 (6) | 1.45 (6) | 1.54 (6) |

**CPU, whole session ms**

| lane | 100 tok/s | 1,000 tok/s | 10,000 tok/s | 50,000 tok/s |
|---|---:|---:|---:|---:|
| nc | 46.3 (6) | 89.0 (6) | 515 (6) | 2,331 (6) |
| fx | 49.0 (6) | 225 (6) | 2,493 (6) | 5,043 (6) |
| nclite | 8.65 (6) | 41.2 (6) | 374 (6) | 1,849 (6) |

**peak RSS MB**

| lane | 100 tok/s | 1,000 tok/s | 10,000 tok/s | 50,000 tok/s |
|---|---:|---:|---:|---:|
| nc | 22.3 (6) | 23.0 (6) | 32.1 (6) | 64.0 (6) |
| fx | 12.7 (6) | 15.6 (6) | 23.0 (6) | 109 (6) |
| nclite | 7.92 (6) | 8.35 (6) | 11.6 (6) | 25.2 (6) |

**terminal bytes per token**

| lane | 100 tok/s | 1,000 tok/s | 10,000 tok/s | 50,000 tok/s |
|---|---:|---:|---:|---:|
| nc | 190 (6) | 315 (6) | 56.3 (6) | 15.3 (6) |
| fx | 39.3 (6) | 73.8 (6) | 30.7 (6) | 9.98 (6) |
| nclite | 5.50 (6) | 5.89 (6) | 6.86 (6) | 7.63 (6) |

**tokens never on screen**

| lane | 100 tok/s | 1,000 tok/s | 10,000 tok/s | 50,000 tok/s |
|---|---:|---:|---:|---:|
| nc | 0.000 (6) | 0.000 (6) | 0.000 (6) | 0.000 (6) |
| fx | 0.000 (6) | 0.000 (6) | 0.000 (6) | 0.000 (6) |
| nclite | 0.000 (6) | 0.000 (6) | 0.000 (6) | 0.000 (6) |

## Outbound connection attempts (network blocked)

| lane | attempts | destinations | names |
|---|---:|---:|---:|
|  | 8 | 6 | api.inference.boundless.network, gateway.tempo-dev.workers.dev, mercator.sh |

## Where nc leads

- Idle terminal interface, idle CPU ms/s: nc -0.023 against fx 0.619 (-27.15x better)
- Streaming TUI 120x40 at 100 tok/s, on-screen lag p50 ms: nc 0.067 against fx 200 (2989.15x better)
- Streaming TUI 120x40 at 100 tok/s, on-screen lag p99 ms: nc 0.204 against fx 403 (1975.39x better)
- Keystroke echo, echo max ms: nc 0.101 against fx 113 (1123.65x better)
- Streaming TUI 120x40 at 100 tok/s, stream end to last token on screen ms: nc 0.102 against fx 15.1 (148.52x better)
- Streaming TUI 120x40 at 50,000 tok/s, stream end to last token on screen ms: nc 3.57 against fx 191 (53.35x better)
- Streaming TUI 120x40 at 50,000 tok/s, on-screen lag p99 ms: nc 8.45 against fx 256 (30.35x better)
- Long tool turn: 50 shell steps, headless, tool result to next request p50 ms: nc 0.830 against fx 20.5 (24.71x better)
- Long tool turn: 10 shell steps, headless, tool result to next request p50 ms: nc 0.925 against fx 20.4 (22.08x better)
- Streaming TUI 120x40 at 1,000 tok/s, stream end to last token on screen ms: nc 0.658 against fx 13.3 (20.26x better)
- Long tool turn: 50 shell steps, headless, wall ms: nc 63.1 against fx 1,057 (16.76x better)
- Streaming TUI 120x40 at 50,000 tok/s, on-screen lag p50 ms: nc 4.06 against fx 59.0 (14.53x better)
- Streaming TUI 240x70 at 10,000 tok/s, stream end to last token on screen ms: nc 2.42 against fx 33.4 (13.79x better)
- Streaming TUI 120x40 at 10,000 tok/s, stream end to last token on screen ms: nc 2.96 against fx 34.2 (11.57x better)
- Long tool turn: 10 shell steps, headless, wall ms: nc 21.0 against fx 229 (10.91x better)
- Streaming TUI 120x40 at 1,000 tok/s, on-screen lag p50 ms: nc 5.05 against fx 32.5 (6.44x better)
- Streaming TUI 240x70 at 10,000 tok/s, on-screen lag p50 ms: nc 3.60 against fx 22.9 (6.36x better)
- Streaming TUI 120x40 at 1,000 tok/s, on-screen lag p99 ms: nc 9.18 against fx 54.1 (5.89x better)
- Streaming TUI 120x40 at 10,000 tok/s, on-screen lag p50 ms: nc 4.08 against fx 21.9 (5.37x better)
- One-shot headless prompt, reply end to exit ms: nc 2.01 against fx 10.6 (5.29x better)
- Streaming TUI 120x40 at 10,000 tok/s, CPU, whole session ms: nc 515 against fx 2,493 (4.84x better)
- One-shot headless prompt, wall ms: nc 4.83 against fx 22.9 (4.74x better)
- One-shot headless prompt, spawn to request ms: nc 2.62 against fx 12.1 (4.61x better)
- Streaming TUI 240x70 at 10,000 tok/s, CPU, whole session ms: nc 582 against fx 2,640 (4.53x better)
- Streaming TUI 120x40 at 10,000 tok/s, on-screen lag p99 ms: nc 8.43 against fx 38.1 (4.52x better)
- Streaming TUI 240x70 at 10,000 tok/s, on-screen lag p99 ms: nc 8.87 against fx 39.4 (4.45x better)
- Streaming headless at 100 tok/s, stream end to exit ms: nc 1.52 against fx 6.44 (4.24x better)
- Keystroke echo, echo p50 ms: nc 0.076 against fx 0.319 (4.18x better)
- Fleet: 32 concurrent headless runs, 1,000 tokens each at 1,000 tok/s, stream end to exit p50 ms: nc 2.17 against fx 7.48 (3.45x better)
- Streaming headless at 1,000 tok/s, stream end to exit ms: nc 2.09 against fx 7.12 (3.40x better)
- Fleet: 8 concurrent headless runs, 1,000 tokens each at 1,000 tok/s, stream end to exit p50 ms: nc 2.33 against fx 7.84 (3.36x better)
- Session store: 1,000 one-turn sessions, store KB: nc 4,252 against fx 12,780 (3.01x better)
- Session store: 1,000 one-turn sessions, KB per session: nc 4.25 against fx 12.8 (3.01x better)
- Fleet: 1 concurrent headless runs, 1,000 tokens each at 1,000 tok/s, stream end to exit p50 ms: nc 2.57 against fx 7.43 (2.89x better)
- Streaming TUI 120x40 at 1,000 tok/s, CPU, whole session ms: nc 89.0 against fx 225 (2.53x better)
- Streaming TUI 240x70 at 10,000 tok/s, Enter to request ms: nc 1.57 against fx 3.81 (2.43x better)
- Streaming TUI 120x40 at 100 tok/s, Enter to request ms: nc 1.41 against fx 3.20 (2.28x better)
- Streaming TUI 120x40 at 50,000 tok/s, CPU, whole session ms: nc 2,331 against fx 5,043 (2.16x better)
- Streaming TUI 120x40 at 10,000 tok/s, Enter to request ms: nc 1.46 against fx 3.13 (2.14x better)
- Continuing the latest of 1,000 sessions, spawn to request ms: nc 8.09 against fx 16.4 (2.03x better)
- Streaming TUI 120x40 at 50,000 tok/s, Enter to request ms: nc 1.56 against fx 3.17 (2.03x better)
- Session store: 1,000 one-turn sessions, files: nc 1,006 against fx 2,005 (1.99x better)
- Streaming TUI 120x40 at 1,000 tok/s, Enter to request ms: nc 1.56 against fx 3.09 (1.98x better)
- Streaming TUI 120x40 at 50,000 tok/s, peak RSS MB: nc 64.0 against fx 109 (1.71x better)
- Continuing the latest of 1,000 sessions, wall ms: nc 17.1 against fx 28.9 (1.69x better)
- Streaming headless at 10,000 tok/s, stream end to exit ms: nc 6.23 against fx 7.71 (1.24x better)
- Streaming TUI 120x40 at 100 tok/s, CPU, whole session ms: nc 46.3 against fx 49.0 (1.06x better)

## Where nc trails

- Cold start: --version after dropping the page cache, wall ms: nc failed
- Cold start: --version after dropping the page cache, CPU ms: nc failed
- Cold start: --version after dropping the page cache, peak RSS MB: nc failed
- Time to first frame, cold page cache, first frame ms: nc failed
- Time to first frame, cold page cache, CPU ms: nc failed
- Install size, install MB: nc failed
- Install size, main binary MB: nc failed
- Session store: 1,000 one-turn sessions, CPU per saved session ms: nc 10.3 against fx 1.59 (6.49x worse)
- Streaming TUI 120x40 at 100 tok/s, terminal bytes per token: nc 190 against fx 39.3 (4.84x worse)
- Startup: --version, CPU ms: nc 0.444 against fx 0.094 (4.72x worse)
- Startup: --version, peak RSS MB: nc 5.88 against fx 1.38 (4.27x worse)
- Streaming TUI 120x40 at 1,000 tok/s, terminal bytes per token: nc 315 against fx 73.8 (4.27x worse)
- Memory one second after the first frame, anonymous MB: nc 4.36 against fx 1.25 (3.50x worse)
- Continuing the latest of 1,000 sessions, peak RSS MB: nc 29.3 against fx 9.01 (3.25x worse)
- One-shot headless prompt, CPU ms: nc 4.93 against fx 1.62 (3.05x worse)
- Continuing the latest of 1,000 sessions, CPU ms: nc 18.0 against fx 6.22 (2.90x worse)
- Streaming TUI 240x70 at 10,000 tok/s, terminal bytes per token: nc 142 against fx 55.6 (2.56x worse)
- Fleet: 32 concurrent headless runs, 1,000 tokens each at 1,000 tok/s, peak RSS per run MB: nc 19.2 against fx 7.69 (2.50x worse)
- One-shot headless prompt, peak RSS MB: nc 19.0 against fx 7.63 (2.49x worse)
- Fleet: 8 concurrent headless runs, 1,000 tokens each at 1,000 tok/s, peak RSS per run MB: nc 19.1 against fx 7.73 (2.47x worse)
- Streaming headless at 100 tok/s, peak RSS MB: nc 19.0 against fx 7.81 (2.43x worse)
- Fleet: 1 concurrent headless runs, 1,000 tokens each at 1,000 tok/s, peak RSS per run MB: nc 19.1 against fx 7.85 (2.43x worse)
- One-shot headless prompt, first request bytes: nc 62,482 against fx 26,274 (2.38x worse)
- Streaming headless at 1,000 tok/s, peak RSS MB: nc 19.0 against fx 8.00 (2.37x worse)
- Streaming headless at 10,000 tok/s, peak RSS MB: nc 26.0 against fx 11.2 (2.33x worse)
- Fleet: 32 concurrent headless runs, 1,000 tokens each at 1,000 tok/s, CPU per run ms: nc 13.3 against fx 5.86 (2.26x worse)
- Fleet: 8 concurrent headless runs, 1,000 tokens each at 1,000 tok/s, CPU per run ms: nc 12.3 against fx 5.46 (2.25x worse)
- Long tool turn: 10 shell steps, headless, peak RSS MB: nc 35.0 against fx 15.7 (2.23x worse)
- Listing 1,000 sessions, peak RSS MB: nc 13.5 against fx 6.31 (2.14x worse)
- Memory one second after the first frame, RSS MB: nc 19.0 against fx 8.91 (2.13x worse)
- Time to first frame, peak RSS MB: nc 19.0 against fx 8.93 (2.13x worse)
- Fleet: 1 concurrent headless runs, 1,000 tokens each at 1,000 tok/s, CPU per run ms: nc 10.4 against fx 5.11 (2.04x worse)
- Long tool turn: 10 shell steps, headless, last request bytes: nc 66,786 against fx 33,000 (2.02x worse)
- Idle terminal interface, peak RSS MB: nc 19.0 against fx 9.46 (2.01x worse)
- Long tool turn: 50 shell steps, headless, peak RSS MB: nc 36.1 against fx 18.5 (1.95x worse)
- Streaming headless at 100 tok/s, CPU, whole run ms: nc 9.82 against fx 5.22 (1.88x worse)
- Streaming headless at 100 tok/s, CPU per token µs: nc 19.6 against fx 10.4 (1.88x worse)
- Memory one second after the first frame, PSS MB: nc 16.7 against fx 8.90 (1.87x worse)
- Memory one second after the first frame, private (USS) MB: nc 16.4 against fx 8.89 (1.85x worse)
- Streaming TUI 120x40 at 10,000 tok/s, terminal bytes per token: nc 56.3 against fx 30.7 (1.83x worse)
- Time to first frame, CPU ms: nc 6.84 against fx 3.81 (1.80x worse)
- Streaming TUI 120x40 at 100 tok/s, peak RSS MB: nc 22.3 against fx 12.7 (1.76x worse)
- Time to first frame, first byte ms: nc 1.52 against fx 0.939 (1.62x worse)
- Streaming headless at 1,000 tok/s, CPU, whole run ms: nc 28.5 against fx 18.2 (1.56x worse)
- Streaming headless at 1,000 tok/s, CPU per token µs: nc 5.69 against fx 3.64 (1.56x worse)
- Streaming TUI 120x40 at 50,000 tok/s, terminal bytes per token: nc 15.3 against fx 9.98 (1.53x worse)
- Long tool turn: 10 shell steps, headless, CPU per step ms: nc 3.73 against fx 2.49 (1.50x worse)
- Streaming TUI 120x40 at 1,000 tok/s, peak RSS MB: nc 23.0 against fx 15.6 (1.48x worse)
- Streaming headless at 10,000 tok/s, CPU per token µs: nc 4.45 against fx 3.04 (1.46x worse)
- Streaming headless at 10,000 tok/s, CPU, whole run ms: nc 222 against fx 152 (1.46x worse)
- Long tool turn: 50 shell steps, headless, last request bytes: nc 83,906 against fx 59,760 (1.40x worse)
- Streaming TUI 120x40 at 10,000 tok/s, peak RSS MB: nc 32.1 against fx 23.0 (1.39x worse)
- Long tool turn: 50 shell steps, headless, CPU per step ms: nc 2.88 against fx 2.22 (1.29x worse)
- Listing 1,000 sessions, CPU ms: nc 5.99 against fx 4.87 (1.23x worse)
- Streaming TUI 240x70 at 10,000 tok/s, peak RSS MB: nc 33.2 against fx 27.5 (1.21x worse)
- Time to first frame, first frame ms: nc 2.02 against fx 1.86 (1.08x worse)

## Ties (within 5%)

- Startup: --version, wall ms: nc 2.63 against fx 2.62
- Idle terminal interface, bytes written while idle: nc 0.000 against fx 0.000
- One-shot headless prompt, side requests: nc 0.000 against fx 0.000
- One-shot headless prompt, processes left behind: nc 0.000 against fx 0.000
- Fleet: 1 concurrent headless runs, 1,000 tokens each at 1,000 tok/s, wall ms: nc 1,006 against fx 1,020
- Fleet: 8 concurrent headless runs, 1,000 tokens each at 1,000 tok/s, wall ms: nc 1,012 against fx 1,024
- Fleet: 32 concurrent headless runs, 1,000 tokens each at 1,000 tok/s, wall ms: nc 1,042 against fx 1,047
- Listing 1,000 sessions, wall ms: nc 7.25 against fx 6.94
- Streaming headless at 100 tok/s, per-token lag p50 ms: nc 0.015 against fx 0.012
- Streaming headless at 1,000 tok/s, per-token lag p50 ms: nc 0.008 against fx 0.007
- Streaming headless at 10,000 tok/s, per-token lag p50 ms: nc 0.008 against fx 0.007
- Streaming headless at 100 tok/s, per-token lag p99 ms: nc 0.026 against fx 0.020
- Streaming headless at 1,000 tok/s, per-token lag p99 ms: nc 0.010 against fx 0.009
- Streaming headless at 10,000 tok/s, per-token lag p99 ms: nc 0.009 against fx 0.008
- Streaming headless at 100 tok/s, tokens never on stdout: nc 0.000 against fx 0.000
- Streaming headless at 1,000 tok/s, tokens never on stdout: nc 0.000 against fx 0.000
- Streaming headless at 10,000 tok/s, tokens never on stdout: nc 0.000 against fx 0.000
- Streaming TUI 240x70 at 10,000 tok/s, tokens never on screen: nc 0.000 against fx 0.000
- Streaming TUI 120x40 at 100 tok/s, tokens never on screen: nc 0.000 against fx 0.000
- Streaming TUI 120x40 at 1,000 tok/s, tokens never on screen: nc 0.000 against fx 0.000
- Streaming TUI 120x40 at 10,000 tok/s, tokens never on screen: nc 0.000 against fx 0.000
- Streaming TUI 120x40 at 50,000 tok/s, tokens never on screen: nc 0.000 against fx 0.000
