# CLI benchmark: 2026-11-11-epyc4245p

Machine: AMD EPYC 4245P 6-Core Processor, 12 threads, 46 GB, Ubuntu 24.04.5 LTS, 6.8.0-147-generic. 

# Installed CLIs

Linux x86_64. nc and nclite are release-profile builds (lto off, as cargo build --release) of nanocodexyz/nanocode master 9f373a4b9 on Ubuntu 24.04; the published nanocode-cli release uses the dist profile (fat LTO). Harness: cli-bench ab0f0fc73.

| Lane | Version | Install | Main binary | Runtime not counted |
| --- | --- | --- | --- | --- |
| nc | nanocode 0.7.0 (9f373a4b9) | cargo build --locked --release --manifest-path bin/nanocode/Cargo.toml --bin nanocode | tools/install/ncm/nanocode | none |
| nclite | nc-lite 0.7.0 (9f373a4b9) | cargo build --locked --release -p nanocode --example nc_lite | tools/install/nclite/nc-lite | none |
| fx | 0.0.16 | curl https://releases.fx.sh/0.0.16/fx-linux-x86_64.tar.gz | tools/install/fx/fx | none |

Medians with the sample count in parentheses and [min–max] where the spread matters. CPU and memory are the whole process tree (cgroup or /proc sampling; rusage when neither is available). Lower is better everywhere.

## Startup: --version

| lane | samples | errors | wall ms | CPU ms | peak RSS MB | instructions |
|---|---:|---:|---:|---:|---:|---:|
| true | 100 | 0 | 2.66 [2.61–5.90] (100) | 0.152 (100) | 1.25 (100) | n/a |
| nc | 100 | 0 | 2.64 [2.58–2.97] (100) | 0.747 (100) | 9.00 (100) | n/a |
| fx | 100 | 0 | 2.63 [2.57–2.81] (100) | 0.099 (100) | 1.38 (100) | n/a |
| nclite | 100 | 0 | 2.64 [2.59–2.81] (100) | 0.467 (100) | 5.25 (100) | n/a |

## Cold start: --version after dropping the page cache

| lane | samples | errors | wall ms | CPU ms | peak RSS MB |
|---|---:|---:|---:|---:|---:|
| nc | 10 | 10 | 10.9 [10.9–11.0] (10) | 2.67 (10) | 8.88 (10) |
| fx | 10 | 10 | 4.87 [4.76–5.96] (10) | 0.599 (10) | 1.38 (10) |
| nclite | 10 | 10 | 6.92 [6.86–7.03] (10) | 1.36 (10) | 5.25 (10) |

- nc error: cannot drop the page cache (needs Linux and passwordless sudo)
- fx error: cannot drop the page cache (needs Linux and passwordless sudo)
- nclite error: cannot drop the page cache (needs Linux and passwordless sudo)

## Time to first frame

| lane | samples | errors | first byte ms | first frame ms | CPU ms | peak RSS MB | processes |
|---|---:|---:|---:|---:|---:|---:|---:|
| nc | 30 | 0 | 2.06 (30) | 4.56 [4.27–4.92] (30) | 11.1 (30) | 37.2 (30) | 0.000 (30) |
| fx | 30 | 0 | 0.963 (30) | 1.89 [1.70–2.23] (30) | 3.79 (30) | 8.92 (30) | 0.000 (30) |

## Memory one second after the first frame

Summed over the CLI's process tree from /proc/<pid>/smaps_rollup. RSS counts the executable's file pages, which every instance shares; PSS splits shared pages between the processes that map them; USS is memory only this tree holds.

| lane | samples | errors | RSS MB | PSS MB | private (USS) MB | anonymous MB |
|---|---:|---:|---:|---:|---:|---:|
| nc | 30 | 0 | 37.2 (30) | 34.9 (30) | 34.6 (30) | 5.28 (30) |
| fx | 30 | 0 | 8.91 (30) | 8.90 (30) | 8.89 (30) | 1.24 (30) |

## Time to first frame, cold page cache

| lane | samples | errors | first frame ms | CPU ms |
|---|---:|---:|---:|---:|
| nc | 0 | 10 | failed (10) | failed (10) |
| fx | 0 | 10 | failed (10) | failed (10) |

- nc error: cannot drop the page cache
- fx error: cannot drop the page cache

## Idle terminal interface

| lane | samples | errors | idle CPU ms/s | bytes written while idle | peak RSS MB | processes |
|---|---:|---:|---:|---:|---:|---:|
| nc | 10 | 0 | -0.005 [-0.033–0.056] (10) | 0.000 (10) | 37.3 (10) | 0.000 (10) |
| fx | 10 | 0 | 0.624 [0.382–0.663] (10) | 0.000 (10) | 9.46 (10) | 0.000 (10) |

## Keystroke echo

| lane | samples | errors | echo p50 ms | echo max ms | keys echoed of 26 |
|---|---:|---:|---:|---:|---:|
| nc | 10 | 0 | 0.096 [0.075–0.098] (10) | 0.129 (10) | 26.0 (10) |
| fx | 10 | 0 | 0.320 [0.168–0.329] (10) | 113 (10) | 26.0 (10) |

## One-shot headless prompt

| lane | samples | errors | wall ms | spawn to request ms | reply end to exit ms | CPU ms | peak RSS MB | first request bytes | side requests | processes left behind |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| nc | 30 | 0 | 8.86 [6.86–9.09] (30) | 5.96 (30) | 2.65 (30) | 9.05 (30) | 39.1 (30) | 62,441 (30) | 0.000 (30) | 0.000 (30) |
| fx | 30 | 0 | 22.9 [22.8–23.6] (30) | 12.2 (30) | 10.6 (30) | 1.67 (30) | 7.63 (30) | 26,274 (30) | 0.000 (30) | 0.000 (30) |
| nclite | 30 | 0 | 6.85 [6.80–7.11] (30) | 4.63 (30) | 2.06 (30) | 5.10 (30) | 27.0 (30) | 36,887 (30) | 0.000 (30) | 0.000 (30) |

## Long tool turn: 10 shell steps, headless

| lane | samples | errors | wall ms | tool result to next request p50 ms | CPU per step ms | peak RSS MB | last request bytes |
|---|---:|---:|---:|---:|---:|---:|---:|
| nc | 10 | 0 | 29.0 [29.0–31.0] (10) | 1.34 (10) | 4.92 (10) | 54.8 (10) | 66,745 (10) |
| fx | 10 | 0 | 229 [229–239] (10) | 20.4 (10) | 2.54 (10) | 16.3 (10) | 33,000 (10) |
| nclite | 10 | 0 | 21.0 [20.9–23.1] (10) | 0.904 (10) | 3.68 (10) | 40.8 (10) | 40,477 (10) |

## Long tool turn: 50 shell steps, headless

| lane | samples | errors | wall ms | tool result to next request p50 ms | CPU per step ms | peak RSS MB | last request bytes |
|---|---:|---:|---:|---:|---:|---:|---:|
| nc | 10 | 0 | 87.1 [85.1–89.1] (10) | 1.22 (10) | 3.67 (10) | 58.9 (10) | 83,865 (10) |
| fx | 10 | 0 | 1,058 [1,057–1,059] (10) | 20.5 (10) | 2.24 (10) | 18.5 (10) | 59,760 (10) |
| nclite | 10 | 0 | 61.1 [59.1–63.1] (10) | 0.837 (10) | 2.87 (10) | 43.0 (10) | 54,717 (10) |

## Fleet: 1 concurrent headless runs, 1,000 tokens each at 1,000 tok/s

| lane | samples | errors | wall ms | CPU per run ms | peak RSS per run MB | stream end to exit p50 ms |
|---|---:|---:|---:|---:|---:|---:|
| nc | 10 | 0 | 1,010 [1,010–1,011] (10) | 15.5 (10) | 39.2 (10) | 3.15 (10) |
| fx | 10 | 0 | 1,020 [1,020–1,038] (10) | 5.10 (10) | 7.75 (10) | 7.39 (10) |
| nclite | 10 | 0 | 1,008 [1,008–1,008] (10) | 12.8 (10) | 27.3 (10) | 2.60 (10) |

## Fleet: 8 concurrent headless runs, 1,000 tokens each at 1,000 tok/s

| lane | samples | errors | wall ms | CPU per run ms | peak RSS per run MB | stream end to exit p50 ms |
|---|---:|---:|---:|---:|---:|---:|
| nc | 10 | 0 | 1,017 [1,016–1,020] (10) | 19.4 (10) | 39.2 (10) | 2.72 (10) |
| fx | 10 | 0 | 1,024 [1,024–1,025] (10) | 5.42 (10) | 7.74 (10) | 7.85 (10) |
| nclite | 10 | 0 | 1,013 [1,012–1,014] (10) | 15.0 (10) | 27.0 (10) | 2.59 (10) |

## Fleet: 32 concurrent headless runs, 1,000 tokens each at 1,000 tok/s

| lane | samples | errors | wall ms | CPU per run ms | peak RSS per run MB | stream end to exit p50 ms |
|---|---:|---:|---:|---:|---:|---:|
| nc | 10 | 0 | 1,066 [1,063–1,070] (10) | 27.6 (10) | 39.3 (10) | 2.52 (10) |
| fx | 10 | 0 | 1,047 [1,043–1,051] (10) | 5.87 (10) | 7.68 (10) | 7.51 (10) |
| nclite | 9 | 1 | 1,043 [1,040–1,045] (9) | 23.6 (9) | 27.0 (9) | 2.14 (9) |

- nclite error: missing tokens

## Session store: 1,000 one-turn sessions

| lane | samples | errors | store KB | KB per session | files | CPU per saved session ms |
|---|---:|---:|---:|---:|---:|---:|
| nc | 1 | 0 | 68,072 (1) | 68.1 (1) | 1,000 (1) | 9.03 (1) |
| fx | 1 | 0 | 12,780 (1) | 12.8 (1) | 2,005 (1) | 1.60 (1) |
| nclite | 1 | 0 | 44,076 (1) | 44.1 (1) | 1,000 (1) | 4.94 (1) |

## Listing 1,000 sessions

| lane | samples | errors | wall ms | CPU ms | peak RSS MB |
|---|---:|---:|---:|---:|---:|
| nc | 20 | 0 | 11.2 [9.10–337] (20) | 8.67 (20) | 18.9 (20) |
| fx | 20 | 0 | 6.89 [6.84–6.97] (20) | 4.85 (20) | 6.27 (20) |
| nclite | 20 | 0 | 9.02 [8.95–257] (20) | 7.71 (20) | 11.9 (20) |

## Continuing the latest of 1,000 sessions

| lane | samples | errors | wall ms | spawn to request ms | CPU ms | peak RSS MB |
|---|---:|---:|---:|---:|---:|---:|
| nc | 20 | 0 | 15.1 [15.0–15.5] (20) | 12.4 (20) | 16.0 (20) | 42.2 (20) |
| fx | 20 | 0 | 28.9 [26.9–29.0] (20) | 16.3 (20) | 6.09 (20) | 8.97 (20) |
| nclite | 20 | 0 | 13.0 [12.9–15.6] (20) | 11.4 (20) | 11.8 (20) | 29.9 (20) |

## Install size

Runtimes a lane needs but does not install (Node for npm packages) are not counted; VERSIONS.md lists them.

| lane | samples | errors | install MB | main binary MB |
|---|---:|---:|---:|---:|
| nc | 1 | 0 | 65.4 (1) | 65.4 (1) |
| fx | 1 | 0 | 12.2 (1) | 12.2 (1) |
| nclite | 1 | 0 | 43.1 (1) | 43.0 (1) |

## Streaming, headless

**per-token lag p50 ms**

| lane | 100 tok/s | 1,000 tok/s | 10,000 tok/s |
|---|---:|---:|---:|
| nc | 0.016 (10) | 0.009 (10) | 0.009 (10) |
| fx | 0.012 (10) | 0.007 (10) | 0.007 (10) |
| nclite | 0.018 (10) | 0.011 (10) | 0.010 (10) |

**per-token lag p99 ms**

| lane | 100 tok/s | 1,000 tok/s | 10,000 tok/s |
|---|---:|---:|---:|
| nc | 0.029 (10) | 0.012 (10) | 0.010 (10) |
| fx | 0.020 (10) | 0.009 (10) | 0.008 (10) |
| nclite | 0.029 (10) | 0.013 (10) | 0.012 (10) |

**stream end to exit ms**

| lane | 100 tok/s | 1,000 tok/s | 10,000 tok/s |
|---|---:|---:|---:|
| nc | 2.11 (10) | 2.66 (10) | 5.24 (10) |
| fx | 6.40 (10) | 7.32 (10) | 7.70 (10) |
| nclite | 1.53 (10) | 2.31 (10) | 4.74 (10) |

**CPU, whole run ms**

| lane | 100 tok/s | 1,000 tok/s | 10,000 tok/s |
|---|---:|---:|---:|
| nc | 15.2 (10) | 38.0 (10) | 270 (10) |
| fx | 5.36 (10) | 18.6 (10) | 151 (10) |
| nclite | 11.5 (10) | 42.0 (10) | 361 (10) |

**CPU per token µs**

| lane | 100 tok/s | 1,000 tok/s | 10,000 tok/s |
|---|---:|---:|---:|
| nc | 30.3 (10) | 7.59 (10) | 5.40 (10) |
| fx | 10.7 (10) | 3.72 (10) | 3.02 (10) |
| nclite | 23.0 (10) | 8.41 (10) | 7.21 (10) |

**peak RSS MB**

| lane | 100 tok/s | 1,000 tok/s | 10,000 tok/s |
|---|---:|---:|---:|
| nc | 39.0 (10) | 39.6 (10) | 42.2 (10) |
| fx | 7.75 (10) | 7.97 (10) | 11.2 (10) |
| nclite | 27.1 (10) | 27.4 (10) | 30.5 (10) |

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
| nc | 3.89 (10) |
| fx | 22.8 (10) |

**on-screen lag p99 ms**

| lane | 10,000 tok/s |
|---|---:|
| nc | 9.10 (10) |
| fx | 39.1 (10) |

**stream end to last token on screen ms**

| lane | 10,000 tok/s |
|---|---:|
| nc | 3.76 (10) |
| fx | 30.9 (10) |

**Enter to request ms**

| lane | 10,000 tok/s |
|---|---:|
| nc | 1.91 (10) |
| fx | 3.96 (10) |

**CPU, whole session ms**

| lane | 10,000 tok/s |
|---|---:|
| nc | 695 (10) |
| fx | 2,619 (10) |

**peak RSS MB**

| lane | 10,000 tok/s |
|---|---:|
| nc | 49.9 (10) |
| fx | 26.9 (10) |

**terminal bytes per token**

| lane | 10,000 tok/s |
|---|---:|
| nc | 163 (10) |
| fx | 55.8 (10) |

**tokens never on screen**

| lane | 10,000 tok/s |
|---|---:|
| nc | 0.000 (10) |
| fx | 0.000 (10) |

## Streaming, terminal interface 120x40

**on-screen lag p50 ms**

| lane | 100 tok/s | 1,000 tok/s | 10,000 tok/s | 50,000 tok/s |
|---|---:|---:|---:|---:|
| nc | 0.085 (10) | 4.08 (10) | 3.11 (10) | 4.07 (10) |
| fx | 200 (10) | 32.5 (10) | 21.9 (10) | 58.5 (10) |

**on-screen lag p99 ms**

| lane | 100 tok/s | 1,000 tok/s | 10,000 tok/s | 50,000 tok/s |
|---|---:|---:|---:|---:|
| nc | 0.245 (10) | 8.29 (10) | 8.50 (10) | 8.54 (10) |
| fx | 402 (10) | 54.0 (10) | 37.8 (10) | 256 (10) |

**stream end to last token on screen ms**

| lane | 100 tok/s | 1,000 tok/s | 10,000 tok/s | 50,000 tok/s |
|---|---:|---:|---:|---:|
| nc | 0.140 (10) | 0.952 (10) | 3.06 (10) | 3.48 (10) |
| fx | 15.1 (10) | 13.3 (10) | 30.8 (10) | 241 (10) |

**Enter to request ms**

| lane | 100 tok/s | 1,000 tok/s | 10,000 tok/s | 50,000 tok/s |
|---|---:|---:|---:|---:|
| nc | 1.89 (10) | 1.94 (10) | 1.97 (10) | 1.92 (10) |
| fx | 3.13 (10) | 3.11 (10) | 3.14 (10) | 3.16 (10) |

**CPU, whole session ms**

| lane | 100 tok/s | 1,000 tok/s | 10,000 tok/s | 50,000 tok/s |
|---|---:|---:|---:|---:|
| nc | 62.0 (10) | 117 (10) | 591 (10) | 2,654 (10) |
| fx | 49.6 (10) | 219 (10) | 2,469 (10) | 5,049 (10) |

**peak RSS MB**

| lane | 100 tok/s | 1,000 tok/s | 10,000 tok/s | 50,000 tok/s |
|---|---:|---:|---:|---:|
| nc | 42.6 (10) | 43.4 (10) | 48.8 (10) | 76.0 (10) |
| fx | 12.7 (10) | 16.4 (10) | 22.7 (10) | 109 (10) |

**terminal bytes per token**

| lane | 100 tok/s | 1,000 tok/s | 10,000 tok/s | 50,000 tok/s |
|---|---:|---:|---:|---:|
| nc | 190 (10) | 315 (10) | 58.0 (10) | 15.9 (10) |
| fx | 39.4 (10) | 73.7 (10) | 30.8 (10) | 9.98 (10) |

**tokens never on screen**

| lane | 100 tok/s | 1,000 tok/s | 10,000 tok/s | 50,000 tok/s |
|---|---:|---:|---:|---:|
| nc | 0.000 (10) | 0.000 (10) | 0.000 (10) | 0.000 (10) |
| fx | 0.000 (10) | 0.000 (10) | 0.000 (10) | 0.000 (10) |

## Outbound connection attempts (network blocked)

| lane | attempts | destinations | names |
|---|---:|---:|---:|
|  | 8 | 5 | api.inference.boundless.network, gateway.tempo-dev.workers.dev, mercator.sh |

## Where nc leads

- Idle terminal interface, idle CPU ms/s: nc -0.005 against fx 0.624 (-121.11x better)
- Streaming TUI 120x40 at 100 tok/s, on-screen lag p50 ms: nc 0.085 against fx 200 (2355.53x better)
- Streaming TUI 120x40 at 100 tok/s, on-screen lag p99 ms: nc 0.245 against fx 402 (1645.90x better)
- Keystroke echo, echo max ms: nc 0.129 against fx 113 (876.02x better)
- Streaming TUI 120x40 at 100 tok/s, stream end to last token on screen ms: nc 0.140 against fx 15.1 (107.53x better)
- Streaming TUI 120x40 at 50,000 tok/s, stream end to last token on screen ms: nc 3.48 against fx 241 (69.14x better)
- Streaming TUI 120x40 at 50,000 tok/s, on-screen lag p99 ms: nc 8.54 against fx 256 (30.04x better)
- Long tool turn: 50 shell steps, headless, tool result to next request p50 ms: nc 1.22 against fx 20.5 (16.78x better)
- Long tool turn: 10 shell steps, headless, tool result to next request p50 ms: nc 1.34 against fx 20.4 (15.30x better)
- Streaming TUI 120x40 at 50,000 tok/s, on-screen lag p50 ms: nc 4.07 against fx 58.5 (14.37x better)
- Streaming TUI 120x40 at 1,000 tok/s, stream end to last token on screen ms: nc 0.952 against fx 13.3 (13.94x better)
- Long tool turn: 50 shell steps, headless, wall ms: nc 87.1 against fx 1,058 (12.15x better)
- Streaming TUI 120x40 at 10,000 tok/s, stream end to last token on screen ms: nc 3.06 against fx 30.8 (10.07x better)
- Streaming TUI 240x70 at 10,000 tok/s, stream end to last token on screen ms: nc 3.76 against fx 30.9 (8.22x better)
- Streaming TUI 120x40 at 1,000 tok/s, on-screen lag p50 ms: nc 4.08 against fx 32.5 (7.97x better)
- Long tool turn: 10 shell steps, headless, wall ms: nc 29.0 against fx 229 (7.90x better)
- Streaming TUI 120x40 at 10,000 tok/s, on-screen lag p50 ms: nc 3.11 against fx 21.9 (7.03x better)
- Streaming TUI 120x40 at 1,000 tok/s, on-screen lag p99 ms: nc 8.29 against fx 54.0 (6.52x better)
- Streaming TUI 240x70 at 10,000 tok/s, on-screen lag p50 ms: nc 3.89 against fx 22.8 (5.85x better)
- Streaming TUI 120x40 at 10,000 tok/s, on-screen lag p99 ms: nc 8.50 against fx 37.8 (4.45x better)
- Streaming TUI 240x70 at 10,000 tok/s, on-screen lag p99 ms: nc 9.10 against fx 39.1 (4.30x better)
- Streaming TUI 120x40 at 10,000 tok/s, CPU, whole session ms: nc 591 against fx 2,469 (4.18x better)
- One-shot headless prompt, reply end to exit ms: nc 2.65 against fx 10.6 (4.00x better)
- Streaming TUI 240x70 at 10,000 tok/s, CPU, whole session ms: nc 695 against fx 2,619 (3.77x better)
- Keystroke echo, echo p50 ms: nc 0.096 against fx 0.320 (3.32x better)
- Streaming headless at 100 tok/s, stream end to exit ms: nc 2.11 against fx 6.40 (3.04x better)
- Fleet: 32 concurrent headless runs, 1,000 tokens each at 1,000 tok/s, stream end to exit p50 ms: nc 2.52 against fx 7.51 (2.97x better)
- Fleet: 8 concurrent headless runs, 1,000 tokens each at 1,000 tok/s, stream end to exit p50 ms: nc 2.72 against fx 7.85 (2.88x better)
- Streaming headless at 1,000 tok/s, stream end to exit ms: nc 2.66 against fx 7.32 (2.75x better)
- One-shot headless prompt, wall ms: nc 8.86 against fx 22.9 (2.58x better)
- Fleet: 1 concurrent headless runs, 1,000 tokens each at 1,000 tok/s, stream end to exit p50 ms: nc 3.15 against fx 7.39 (2.34x better)
- Streaming TUI 240x70 at 10,000 tok/s, Enter to request ms: nc 1.91 against fx 3.96 (2.07x better)
- One-shot headless prompt, spawn to request ms: nc 5.96 against fx 12.2 (2.04x better)
- Session store: 1,000 one-turn sessions, files: nc 1,000 against fx 2,005 (2.00x better)
- Continuing the latest of 1,000 sessions, wall ms: nc 15.1 against fx 28.9 (1.92x better)
- Streaming TUI 120x40 at 50,000 tok/s, CPU, whole session ms: nc 2,654 against fx 5,049 (1.90x better)
- Streaming TUI 120x40 at 1,000 tok/s, CPU, whole session ms: nc 117 against fx 219 (1.88x better)
- Streaming TUI 120x40 at 100 tok/s, Enter to request ms: nc 1.89 against fx 3.13 (1.66x better)
- Streaming TUI 120x40 at 50,000 tok/s, Enter to request ms: nc 1.92 against fx 3.16 (1.64x better)
- Streaming TUI 120x40 at 1,000 tok/s, Enter to request ms: nc 1.94 against fx 3.11 (1.60x better)
- Streaming TUI 120x40 at 10,000 tok/s, Enter to request ms: nc 1.97 against fx 3.14 (1.59x better)
- Streaming headless at 10,000 tok/s, stream end to exit ms: nc 5.24 against fx 7.70 (1.47x better)
- Streaming TUI 120x40 at 50,000 tok/s, peak RSS MB: nc 76.0 against fx 109 (1.44x better)
- Continuing the latest of 1,000 sessions, spawn to request ms: nc 12.4 against fx 16.3 (1.32x better)

## Where nc trails

- Time to first frame, cold page cache, first frame ms: nc failed
- Time to first frame, cold page cache, CPU ms: nc failed
- Startup: --version, CPU ms: nc 0.747 against fx 0.099 (7.58x worse)
- Startup: --version, peak RSS MB: nc 9.00 against fx 1.38 (6.55x worse)
- Cold start: --version after dropping the page cache, peak RSS MB: nc 8.88 against fx 1.38 (6.45x worse)
- Session store: 1,000 one-turn sessions, CPU per saved session ms: nc 9.03 against fx 1.60 (5.66x worse)
- One-shot headless prompt, CPU ms: nc 9.05 against fx 1.67 (5.43x worse)
- Install size, main binary MB: nc 65.4 against fx 12.2 (5.35x worse)
- Install size, install MB: nc 65.4 against fx 12.2 (5.35x worse)
- Session store: 1,000 one-turn sessions, store KB: nc 68,072 against fx 12,780 (5.33x worse)
- Session store: 1,000 one-turn sessions, KB per session: nc 68.1 against fx 12.8 (5.33x worse)
- One-shot headless prompt, peak RSS MB: nc 39.1 against fx 7.63 (5.13x worse)
- Fleet: 32 concurrent headless runs, 1,000 tokens each at 1,000 tok/s, peak RSS per run MB: nc 39.3 against fx 7.68 (5.11x worse)
- Fleet: 8 concurrent headless runs, 1,000 tokens each at 1,000 tok/s, peak RSS per run MB: nc 39.2 against fx 7.74 (5.06x worse)
- Fleet: 1 concurrent headless runs, 1,000 tokens each at 1,000 tok/s, peak RSS per run MB: nc 39.2 against fx 7.75 (5.06x worse)
- Streaming headless at 100 tok/s, peak RSS MB: nc 39.0 against fx 7.75 (5.04x worse)
- Streaming headless at 1,000 tok/s, peak RSS MB: nc 39.6 against fx 7.97 (4.96x worse)
- Streaming TUI 120x40 at 100 tok/s, terminal bytes per token: nc 190 against fx 39.4 (4.84x worse)
- Fleet: 32 concurrent headless runs, 1,000 tokens each at 1,000 tok/s, CPU per run ms: nc 27.6 against fx 5.87 (4.70x worse)
- Continuing the latest of 1,000 sessions, peak RSS MB: nc 42.2 against fx 8.97 (4.70x worse)
- Cold start: --version after dropping the page cache, CPU ms: nc 2.67 against fx 0.599 (4.45x worse)
- Streaming TUI 120x40 at 1,000 tok/s, terminal bytes per token: nc 315 against fx 73.7 (4.27x worse)
- Memory one second after the first frame, anonymous MB: nc 5.28 against fx 1.24 (4.25x worse)
- Memory one second after the first frame, RSS MB: nc 37.2 against fx 8.91 (4.18x worse)
- Time to first frame, peak RSS MB: nc 37.2 against fx 8.92 (4.17x worse)
- Idle terminal interface, peak RSS MB: nc 37.3 against fx 9.46 (3.94x worse)
- Memory one second after the first frame, PSS MB: nc 34.9 against fx 8.90 (3.92x worse)
- Memory one second after the first frame, private (USS) MB: nc 34.6 against fx 8.89 (3.89x worse)
- Streaming headless at 10,000 tok/s, peak RSS MB: nc 42.2 against fx 11.2 (3.76x worse)
- Fleet: 8 concurrent headless runs, 1,000 tokens each at 1,000 tok/s, CPU per run ms: nc 19.4 against fx 5.42 (3.58x worse)
- Long tool turn: 10 shell steps, headless, peak RSS MB: nc 54.8 against fx 16.3 (3.37x worse)
- Streaming TUI 120x40 at 100 tok/s, peak RSS MB: nc 42.6 against fx 12.7 (3.34x worse)
- Long tool turn: 50 shell steps, headless, peak RSS MB: nc 58.9 against fx 18.5 (3.18x worse)
- Fleet: 1 concurrent headless runs, 1,000 tokens each at 1,000 tok/s, CPU per run ms: nc 15.5 against fx 5.10 (3.04x worse)
- Listing 1,000 sessions, peak RSS MB: nc 18.9 against fx 6.27 (3.02x worse)
- Time to first frame, CPU ms: nc 11.1 against fx 3.79 (2.94x worse)
- Streaming TUI 240x70 at 10,000 tok/s, terminal bytes per token: nc 163 against fx 55.8 (2.92x worse)
- Streaming headless at 100 tok/s, CPU, whole run ms: nc 15.2 against fx 5.36 (2.83x worse)
- Streaming headless at 100 tok/s, CPU per token µs: nc 30.3 against fx 10.7 (2.83x worse)
- Streaming TUI 120x40 at 1,000 tok/s, peak RSS MB: nc 43.4 against fx 16.4 (2.65x worse)
- Continuing the latest of 1,000 sessions, CPU ms: nc 16.0 against fx 6.09 (2.62x worse)
- Time to first frame, first frame ms: nc 4.56 against fx 1.89 (2.40x worse)
- One-shot headless prompt, first request bytes: nc 62,441 against fx 26,274 (2.38x worse)
- Cold start: --version after dropping the page cache, wall ms: nc 10.9 against fx 4.87 (2.24x worse)
- Streaming TUI 120x40 at 10,000 tok/s, peak RSS MB: nc 48.8 against fx 22.7 (2.15x worse)
- Time to first frame, first byte ms: nc 2.06 against fx 0.963 (2.14x worse)
- Streaming headless at 1,000 tok/s, CPU per token µs: nc 7.59 against fx 3.72 (2.04x worse)
- Streaming headless at 1,000 tok/s, CPU, whole run ms: nc 38.0 against fx 18.6 (2.04x worse)
- Long tool turn: 10 shell steps, headless, last request bytes: nc 66,745 against fx 33,000 (2.02x worse)
- Long tool turn: 10 shell steps, headless, CPU per step ms: nc 4.92 against fx 2.54 (1.94x worse)
- Streaming TUI 120x40 at 10,000 tok/s, terminal bytes per token: nc 58.0 against fx 30.8 (1.88x worse)
- Streaming TUI 240x70 at 10,000 tok/s, peak RSS MB: nc 49.9 against fx 26.9 (1.86x worse)
- Listing 1,000 sessions, CPU ms: nc 8.67 against fx 4.85 (1.79x worse)
- Streaming headless at 10,000 tok/s, CPU, whole run ms: nc 270 against fx 151 (1.79x worse)
- Streaming headless at 10,000 tok/s, CPU per token µs: nc 5.40 against fx 3.02 (1.79x worse)
- Long tool turn: 50 shell steps, headless, CPU per step ms: nc 3.67 against fx 2.24 (1.64x worse)
- Listing 1,000 sessions, wall ms: nc 11.2 against fx 6.89 (1.62x worse)
- Streaming TUI 120x40 at 50,000 tok/s, terminal bytes per token: nc 15.9 against fx 9.98 (1.59x worse)
- Long tool turn: 50 shell steps, headless, last request bytes: nc 83,865 against fx 59,760 (1.40x worse)
- Streaming TUI 120x40 at 100 tok/s, CPU, whole session ms: nc 62.0 against fx 49.6 (1.25x worse)

## Ties (within 5%)

- Startup: --version, wall ms: nc 2.64 against fx 2.63
- Idle terminal interface, bytes written while idle: nc 0.000 against fx 0.000
- One-shot headless prompt, side requests: nc 0.000 against fx 0.000
- One-shot headless prompt, processes left behind: nc 0.000 against fx 0.000
- Fleet: 1 concurrent headless runs, 1,000 tokens each at 1,000 tok/s, wall ms: nc 1,010 against fx 1,020
- Fleet: 8 concurrent headless runs, 1,000 tokens each at 1,000 tok/s, wall ms: nc 1,017 against fx 1,024
- Fleet: 32 concurrent headless runs, 1,000 tokens each at 1,000 tok/s, wall ms: nc 1,066 against fx 1,047
- Streaming headless at 100 tok/s, per-token lag p50 ms: nc 0.016 against fx 0.012
- Streaming headless at 1,000 tok/s, per-token lag p50 ms: nc 0.009 against fx 0.007
- Streaming headless at 10,000 tok/s, per-token lag p50 ms: nc 0.009 against fx 0.007
- Streaming headless at 100 tok/s, per-token lag p99 ms: nc 0.029 against fx 0.020
- Streaming headless at 1,000 tok/s, per-token lag p99 ms: nc 0.012 against fx 0.009
- Streaming headless at 10,000 tok/s, per-token lag p99 ms: nc 0.010 against fx 0.008
- Streaming headless at 100 tok/s, tokens never on stdout: nc 0.000 against fx 0.000
- Streaming headless at 1,000 tok/s, tokens never on stdout: nc 0.000 against fx 0.000
- Streaming headless at 10,000 tok/s, tokens never on stdout: nc 0.000 against fx 0.000
- Streaming TUI 240x70 at 10,000 tok/s, tokens never on screen: nc 0.000 against fx 0.000
- Streaming TUI 120x40 at 100 tok/s, tokens never on screen: nc 0.000 against fx 0.000
- Streaming TUI 120x40 at 1,000 tok/s, tokens never on screen: nc 0.000 against fx 0.000
- Streaming TUI 120x40 at 10,000 tok/s, tokens never on screen: nc 0.000 against fx 0.000
- Streaming TUI 120x40 at 50,000 tok/s, tokens never on screen: nc 0.000 against fx 0.000
