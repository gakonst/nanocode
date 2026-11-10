# CLI benchmark: 2026-11-10-ab1

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
nc variants: ncbase=/home/ubuntu/ncbase,ncnew=/home/ubuntu/ncnew,nc120=/home/ubuntu/nc120

Medians with the sample count in parentheses and [min–max] where the spread matters. CPU and memory are the whole process tree (cgroup or /proc sampling; rusage when neither is available). Lower is better everywhere.

## Startup: --version

| lane | samples | errors | wall ms | CPU ms | peak RSS MB | instructions |
|---|---:|---:|---:|---:|---:|---:|
| true | 100 | 0 | 2.70 [2.64–3.73] (100) | 0.158 (100) | 1.25 (100) | n/a |
| fx | 100 | 0 | 2.64 [1.25–2.82] (100) | 0.104 (100) | 1.38 (100) | n/a |
| nc120 | 100 | 0 | 2.65 [2.58–2.90] (100) | 0.865 (100) | 9.13 (100) | n/a |
| ncbase | 100 | 0 | 2.64 [2.60–2.92] (100) | 0.851 (100) | 9.13 (100) | n/a |
| ncnew | 100 | 0 | 2.65 [2.58–6.02] (100) | 0.859 (100) | 9.19 (100) | n/a |

## Time to first frame

| lane | samples | errors | first byte ms | first frame ms | CPU ms | peak RSS MB | processes |
|---|---:|---:|---:|---:|---:|---:|---:|
| fx | 30 | 0 | 0.979 (30) | 1.94 [1.88–2.35] (30) | 4.13 (30) | 8.84 (30) | 0.000 (30) |
| nc120 | 30 | 0 | 2.30 (30) | 4.84 [4.63–5.36] (30) | 11.8 (30) | 37.0 (30) | 0.000 (30) |
| ncbase | 30 | 0 | 2.27 (30) | 7.97 [7.69–8.86] (30) | 23.7 (30) | 39.1 (30) | 0.000 (30) |
| ncnew | 30 | 0 | 2.30 (30) | 5.00 [4.71–5.27] (30) | 12.2 (30) | 37.0 (30) | 0.000 (30) |

## One-shot headless prompt

| lane | samples | errors | wall ms | spawn to request ms | reply end to exit ms | CPU ms | peak RSS MB | first request bytes | side requests | processes left behind |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| fx | 30 | 0 | 23.0 [22.8–32.9] (30) | 12.3 (30) | 10.6 (30) | 1.75 (30) | 7.59 (30) | 26,276 (30) | 0.000 (30) | 0.000 (30) |
| nc120 | 30 | 0 | 8.92 [8.84–11.0] (30) | 6.61 (30) | 2.13 (30) | 9.87 (30) | 38.9 (30) | 62,184 (30) | 0.000 (30) | 0.000 (30) |
| ncbase | 30 | 0 | 18.9 [18.8–23.6] (30) | 16.9 (30) | 1.79 (30) | 22.0 (30) | 38.9 (30) | 62,185 (30) | 0.000 (30) | 0.000 (30) |
| ncnew | 30 | 0 | 8.96 [8.83–9.04] (30) | 6.67 (30) | 2.11 (30) | 9.94 (30) | 38.7 (30) | 62,184 (30) | 0.000 (30) | 0.000 (30) |

## Long tool turn: 10 shell steps, headless

| lane | samples | errors | wall ms | tool result to next request p50 ms | CPU per step ms | peak RSS MB | last request bytes |
|---|---:|---:|---:|---:|---:|---:|---:|
| fx | 10 | 0 | 230 [229–241] (10) | 20.4 (10) | 2.62 (10) | 14.9 (10) | 33,001 (10) |
| nc120 | 0 | 10 | failed (10) | failed (10) | failed (10) | failed (10) | failed (10) |
| ncbase | 0 | 10 | failed (10) | failed (10) | failed (10) | failed (10) | failed (10) |
| ncnew | 0 | 10 | failed (10) | failed (10) | failed (10) | failed (10) | failed (10) |

- nc120 error: exit 1, 5 requests, done false: ode: computer use unavailable, continuing without it: cua-driver is not installed: run nanocode cua install, or set CUA_DRIVER_PATH session 01a1c71e-1b9a-7000-9001-a5defdef5c14 nanocode: durable operation rec
- ncbase error: exit 1, 5 requests, done false: ode: computer use unavailable, continuing without it: cua-driver is not installed: run nanocode cua install, or set CUA_DRIVER_PATH session 01a1c71d-a390-7000-822b-0e7ca01d71f0 nanocode: durable operation rec
- ncnew error: exit 1, 5 requests, done false: ode: computer use unavailable, continuing without it: cua-driver is not installed: run nanocode cua install, or set CUA_DRIVER_PATH session 01a1c71d-dd08-7000-8305-c893c9f5c89b nanocode: durable operation rec

## Streaming, headless

**per-token lag p50 ms**

| lane | 100 tok/s | 1,000 tok/s | 10,000 tok/s |
|---|---:|---:|---:|
| fx | 0.011 (10) | 0.008 (10) | 0.007 (10) |
| nc120 | 0.016 (10) | 0.010 (10) | 0.008 (10) |
| ncbase | 10.0 (10) | 8.01 (10) | 8.31 (10) |
| ncnew | 0.015 (10) | 0.010 (10) | 0.008 (10) |

**per-token lag p99 ms**

| lane | 100 tok/s | 1,000 tok/s | 10,000 tok/s |
|---|---:|---:|---:|
| fx | 0.020 (10) | 0.009 (10) | 0.008 (10) |
| nc120 | 0.034 (10) | 0.013 (10) | 0.010 (10) |
| ncbase | 18.1 (10) | 16.0 (10) | 16.5 (10) |
| ncnew | 0.040 (10) | 0.012 (10) | 0.010 (10) |

**stream end to exit ms**

| lane | 100 tok/s | 1,000 tok/s | 10,000 tok/s |
|---|---:|---:|---:|
| fx | 6.30 (10) | 7.13 (10) | 7.54 (10) |
| nc120 | 3.55 (10) | 4.31 (10) | 6.09 (10) |
| ncbase | 3.31 (10) | 4.10 (10) | 6.62 (10) |
| ncnew | 3.65 (10) | 4.41 (10) | 6.41 (10) |

**CPU, whole run ms**

| lane | 100 tok/s | 1,000 tok/s | 10,000 tok/s |
|---|---:|---:|---:|
| fx | 5.48 (10) | 18.2 (10) | 151 (10) |
| nc120 | 15.5 (10) | 37.8 (10) | 265 (10) |
| ncbase | 28.5 (10) | 47.9 (10) | 251 (10) |
| ncnew | 15.7 (10) | 37.9 (10) | 268 (10) |

**CPU per token µs**

| lane | 100 tok/s | 1,000 tok/s | 10,000 tok/s |
|---|---:|---:|---:|
| fx | 11.0 (10) | 3.64 (10) | 3.02 (10) |
| nc120 | 31.0 (10) | 7.57 (10) | 5.31 (10) |
| ncbase | 57.0 (10) | 9.57 (10) | 5.01 (10) |
| ncnew | 31.4 (10) | 7.59 (10) | 5.36 (10) |

**peak RSS MB**

| lane | 100 tok/s | 1,000 tok/s | 10,000 tok/s |
|---|---:|---:|---:|
| fx | 7.63 (10) | 7.81 (10) | 11.1 (10) |
| nc120 | 39.2 (10) | 39.3 (10) | 41.8 (10) |
| ncbase | 39.3 (10) | 39.6 (10) | 42.3 (10) |
| ncnew | 38.5 (10) | 39.3 (10) | 42.3 (10) |

**tokens never on stdout**

| lane | 100 tok/s | 1,000 tok/s | 10,000 tok/s |
|---|---:|---:|---:|
| fx | 0.000 (10) | 0.000 (10) | 0.000 (10) |
| nc120 | 0.000 (10) | 0.000 (10) | 0.000 (10) |
| ncbase | 0.000 (10) | 0.000 (10) | 0.000 (10) |
| ncnew | 0.000 (10) | 0.000 (10) | 0.000 (10) |

## Streaming, terminal interface 120x40

**on-screen lag p50 ms**

| lane | 100 tok/s | 1,000 tok/s | 10,000 tok/s |
|---|---:|---:|---:|
| fx | 200 (10) | 32.6 (10) | 21.7 (10) |
| nc120 | 0.124 (10) | 4.08 (10) | 3.12 (10) |
| ncbase | 7.50 (10) | 7.71 (10) | 8.11 (10) |
| ncnew | 7.13 (10) | 7.35 (10) | 8.11 (10) |

**on-screen lag p99 ms**

| lane | 100 tok/s | 1,000 tok/s | 10,000 tok/s |
|---|---:|---:|---:|
| fx | 403 (10) | 54.1 (10) | 37.7 (10) |
| nc120 | 0.247 (10) | 8.26 (10) | 8.50 (10) |
| ncbase | 15.2 (10) | 16.8 (10) | 16.7 (10) |
| ncnew | 16.2 (10) | 16.3 (10) | 16.7 (10) |

**stream end to last token on screen ms**

| lane | 100 tok/s | 1,000 tok/s | 10,000 tok/s |
|---|---:|---:|---:|
| fx | 17.3 (10) | 12.2 (10) | 33.9 (10) |
| nc120 | 0.176 (10) | 0.890 (10) | 3.07 (10) |
| ncbase | 0.661 (10) | 0.881 (10) | 3.27 (10) |
| ncnew | 0.698 (10) | 0.894 (10) | 3.18 (10) |

**Enter to request ms**

| lane | 100 tok/s | 1,000 tok/s | 10,000 tok/s |
|---|---:|---:|---:|
| fx | 3.22 (10) | 3.25 (10) | 3.28 (10) |
| nc120 | 1.85 (10) | 1.83 (10) | 1.92 (10) |
| ncbase | 3.26 (10) | 3.28 (10) | 3.36 (10) |
| ncnew | 1.87 (10) | 1.75 (10) | 1.91 (10) |

**CPU, whole session ms**

| lane | 100 tok/s | 1,000 tok/s | 10,000 tok/s |
|---|---:|---:|---:|
| fx | 48.1 (10) | 217 (10) | 2,452 (10) |
| nc120 | 71.3 (10) | 119 (10) | 592 (10) |
| ncbase | 71.1 (10) | 113 (10) | 576 (10) |
| ncnew | 57.2 (10) | 103 (10) | 561 (10) |

**peak RSS MB**

| lane | 100 tok/s | 1,000 tok/s | 10,000 tok/s |
|---|---:|---:|---:|
| fx | 12.6 (10) | 15.5 (10) | 23.2 (10) |
| nc120 | 42.5 (10) | 43.0 (10) | 48.4 (10) |
| ncbase | 44.0 (10) | 44.7 (10) | 49.6 (10) |
| ncnew | 42.0 (10) | 43.0 (10) | 48.2 (10) |

**terminal bytes per token**

| lane | 100 tok/s | 1,000 tok/s | 10,000 tok/s |
|---|---:|---:|---:|
| fx | 39.4 (10) | 73.8 (10) | 30.9 (10) |
| nc120 | 190 (10) | 315 (10) | 58.6 (10) |
| ncbase | 163 (10) | 298 (10) | 32.9 (10) |
| ncnew | 163 (10) | 385 (10) | 32.8 (10) |

**tokens never on screen**

| lane | 100 tok/s | 1,000 tok/s | 10,000 tok/s |
|---|---:|---:|---:|
| fx | 0.000 (10) | 0.000 (10) | 0.000 (10) |
| nc120 | 0.000 (10) | 0.000 (10) | 0.000 (10) |
| ncbase | 0.000 (10) | 0.000 (10) | 0.000 (10) |
| ncnew | 0.000 (10) | 0.000 (10) | 0.000 (10) |

## Where nc leads


## Where nc trails

- nowhere in this data

## Ties (within 5%)

