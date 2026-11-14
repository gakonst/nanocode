# CLI benchmark: 2026-11-13-epyc4245p-c

Machine: AMD EPYC 4244P 6-Core Processor, 12 threads, 62 GB, Ubuntu 24.04.5 LTS, 6.8.0-149-generic. 

# Installed CLIs

Written by tools/install.sh on Linux x86_64. The size is the main binary's; BENCH_INSTALL_<LANE> in env.sh is the whole install directory.

| Lane | Version | Install | Date (UTC) | Main binary | Runtime not counted |
| --- | --- | --- | --- | --- | --- |
| nclite | nc-lite 0.7.0 | PROFILE=lite scripts/build-cli.sh nc-lite nc-lite at https://github.com/nanocodexyz/nanocode 7837aae16afe1e46fbf72471c902f5cf2e683270 | 2026-11-13T23:59Z | `tools/install/nclite/nc-lite` (18.2 MiB) | none |
| fx | 0.0.16 | curl https://releases.fx.sh/$(latest.txt)/fx-linux-x86_64.tar.gz (the archive https://fx.sh/setup.sh fetches; binary only) | 2026-11-13T23:59Z | `tools/install/fx/fx` (12.2 MiB) | none |
| pi | 1.5.2 | npm install --prefix tools/install/pi @earendil-works/pi-coding-agent@latest | 2026-11-13T23:59Z | `tools/install/pi/node_modules/@earendil-works/pi-coding-agent/dist/bundle/cli.js` (0.0 MiB) | Node v22.23.5 |
| opencode | 1.19.5 | npm install --prefix tools/install/opencode opencode-ai@latest (the package's bin is the native binary from its platform package) | 2026-11-13T23:59Z | `tools/install/opencode/node_modules/opencode-ai/bin/opencode.exe` (204.5 MiB) | none |
| claude | 2.1.325 (Claude Code) | npm install --prefix tools/install/claude @anthropic-ai/claude-code@latest (the package's bin is the native binary from its platform package) | 2026-11-13T23:59Z | `tools/install/claude/node_modules/@anthropic-ai/claude-code/bin/claude.exe` (348.9 MiB) | none |
| codex | codex-cli 0.169.0 | npm install --prefix tools/install/codex @openai/codex@latest (CODEX_BIN is the platform binary: the npm codex command is a Node launcher that execs it) | 2026-11-13T23:59Z | `tools/install/codex/node_modules/@openai/codex-linux-x64/vendor/x86_64-unknown-linux-musl/bin/codex` (280.6 MiB) | none (the npm launcher needs Node) |

Medians with the sample count in parentheses and [min–max] where the spread matters. CPU and memory are the whole process tree (cgroup or /proc sampling; rusage when neither is available). Lower is better everywhere.

## Streaming, terminal interface 120x40

**on-screen lag p50 ms**

| lane | 50,000 tok/s |
|---|---:|
| nc | 3.25 (10) |
| fx | 72.7 (5) |
| pi | 2,141 (5) |
| opencode | 1,864 (5) |
| claude | 107 (5) |
| codex | 10,798 (5) |
| nclite | 0.023 (5) |

**on-screen lag p99 ms**

| lane | 50,000 tok/s |
|---|---:|
| nc | 8.48 (10) |
| fx | 349 (5) |
| pi | 7,932 (5) |
| opencode | 3,697 (5) |
| claude | 2,432 (5) |
| codex | 21,517 (5) |
| nclite | 0.044 (5) |

**stream end to last token on screen ms**

| lane | 50,000 tok/s |
|---|---:|
| nc | 2.48 (10) |
| fx | 241 (5) |
| pi | n/a |
| opencode | 1,945 (5) |
| claude | 245 (5) |
| codex | n/a |
| nclite | -0.550 (5) |

**Enter to request ms**

| lane | 50,000 tok/s |
|---|---:|
| nc | 1.04 (10) |
| fx | 4.59 (5) |
| pi | 26.4 (5) |
| opencode | 694 (5) |
| claude | 40.0 (5) |
| codex | 28.0 (5) |
| nclite | 2.05 (5) |

**CPU, whole session ms**

| lane | 50,000 tok/s |
|---|---:|
| nc | 2,983 (10) |
| fx | 6,364 (5) |
| pi | 10,163 (5) |
| opencode | 25,251 (5) |
| claude | 8,051 (5) |
| codex | 42,117 (5) |
| nclite | 3,682 (5) |

**peak RSS MB**

| lane | 50,000 tok/s |
|---|---:|
| nc | 48.6 (10) |
| fx | 110 (5) |
| pi | 350 (5) |
| opencode | 1,692 (5) |
| claude | 526 (5) |
| codex | 469 (5) |
| nclite | 24.9 (5) |

**terminal bytes per token**

| lane | 50,000 tok/s |
|---|---:|
| nc | 15.5 (10) |
| fx | 9.62 (5) |
| pi | 4.64 (5) |
| opencode | 15.0 (5) |
| claude | 0.483 (5) |
| codex | 31.9 (5) |
| nclite | 7.63 (5) |

**tokens never on screen**

| lane | 50,000 tok/s |
|---|---:|
| nc | 0.000 (10) |
| fx | 0.000 (5) |
| pi | 171,899 (5) |
| opencode | 0.000 (5) |
| claude | 0.000 (5) |
| codex | 78,558 (5) |
| nclite | 0.000 (5) |

- fx: 5 failed samples, e.g. Error: FX_BIN is not set; run tools/install.sh or see README.md for the binary each lane needs at required (file:///home/ubuntu/bench/bench/cli/harness/common.mjs:20:21) at Object.fx (file:///home/ubu
- pi: 5 failed samples, e.g. Error: PI_CLI is not set; run tools/install.sh or see README.md for the binary each lane needs at required (file:///home/ubuntu/bench/bench/cli/harness/common.mjs:20:21) at Object.pi (file:///home/ubu
- opencode: 5 failed samples, e.g. Error: OPENCODE_BIN is not set; run tools/install.sh or see README.md for the binary each lane needs at required (file:///home/ubuntu/bench/bench/cli/harness/common.mjs:20:21) at Object.opencode (file
- claude: 5 failed samples, e.g. Error: CLAUDE_BIN is not set; run tools/install.sh or see README.md for the binary each lane needs at required (file:///home/ubuntu/bench/bench/cli/harness/common.mjs:20:21) at Object.claude (file:///
- codex: 5 failed samples, e.g. Error: CODEX_BIN is not set; run tools/install.sh or see README.md for the binary each lane needs at required (file:///home/ubuntu/bench/bench/cli/harness/common.mjs:20:21) at Object.codex (file:///ho
- nclite: 5 failed samples, e.g. tui parse: ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^ File "/usr/lib/python3.12/subprocess.py", line 1026, in __init__ self._exec

## Where nc leads

- Streaming TUI 120x40 at 50,000 tok/s, stream end to last token on screen ms: nc 2.48 against fx 241 (97.06x better)
- Streaming TUI 120x40 at 50,000 tok/s, on-screen lag p99 ms: nc 8.48 against fx 349 (41.19x better)
- Streaming TUI 120x40 at 50,000 tok/s, on-screen lag p50 ms: nc 3.25 against fx 72.7 (22.38x better)
- Streaming TUI 120x40 at 50,000 tok/s, Enter to request ms: nc 1.04 against fx 4.59 (4.41x better)
- Streaming TUI 120x40 at 50,000 tok/s, peak RSS MB: nc 48.6 against fx 110 (2.26x better)
- Streaming TUI 120x40 at 50,000 tok/s, CPU, whole session ms: nc 2,983 against fx 6,364 (2.13x better)

## Where nc trails

- Streaming TUI 120x40 at 50,000 tok/s, terminal bytes per token: nc 15.5 against claude 0.483 (32.08x worse)

## Ties (within 5%)

- Streaming TUI 120x40 at 50,000 tok/s, tokens never on screen: nc 0.000 against fx 0.000
