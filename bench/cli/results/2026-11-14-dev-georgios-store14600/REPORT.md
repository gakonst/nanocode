# CLI benchmark: 2026-11-14-dev-georgios-store14600

Machine: AMD EPYC 4585PX 16-Core Processor, 32 threads, 62 GB, Ubuntu 24.04.5 LTS, 6.8.0-149-generic. Shared and heavily loaded: only the relative nc/fx numbers and perf counters mean anything here.

# Installed CLIs

Written by tools/install.sh on Linux x86_64. The size is the main binary's; BENCH_INSTALL_<LANE> in env.sh is the whole install directory.

| Lane | Version | Install | Date (UTC) | Main binary | Runtime not counted |
| --- | --- | --- | --- | --- | --- |
| nc | nanocode 0.7.0 | scripts/build-cli.sh nanocode-cli nanocode at https://github.com/nanocodexyz/nanocode 62a0936e45933ebf2b0287d182650f378988fec3 | 2026-11-14T04:09Z | nanocode (44.1 MiB) | none |
| fx | 0.0.16 | curl https://releases.fx.sh/$(latest.txt)/fx-linux-x86_64.tar.gz (the archive https://fx.sh/setup.sh fetches; binary only) | 2026-11-14T03:47Z | `tools/install/fx/fx` (12.2 MiB) | none |

Medians with the sample count in parentheses and [min–max] where the spread matters. CPU and memory are the whole process tree (cgroup or /proc sampling; rusage when neither is available). Lower is better everywhere.

## Session store: 14,600 one-turn sessions

| lane | samples | errors | store KB | KB per session | files | CPU per saved session ms |
|---|---:|---:|---:|---:|---:|---:|
| nc | 1 | 0 | 61,996 (1) | 4.25 (1) | 14,597 (1) | 17.5 (1) |
| fx | 1 | 0 | 180,892 (1) | 12.4 (1) | 29,195 (1) | 31.5 (1) |

## Listing 14,600 sessions

| lane | samples | errors | wall ms | CPU ms | peak RSS MB | task-clock ms | user instructions | kernel instructions |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| nc | 10 | 0 | 2,673 [911–9,887] (10) | 949 (10) | 59.9 (10) | 932 (10) | 4,757,870,016 (10) | 446,614,404 (10) |
| fx | 10 | 0 | 191 [139–503] (10) | 87.8 (10) | 42.9 (10) | 75.1 (10) | 493,876,886 (10) | 114,337,173 (10) |

## Continuing the latest of 14,600 sessions

| lane | samples | errors | wall ms | spawn to request ms | CPU ms | peak RSS MB | task-clock ms | user instructions | kernel instructions |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| nc | 10 | 0 | 1,646 [842–8,101] (10) | 1,528 (10) | 791 (10) | 60.0 (10) | 772 (10) | 4,693,763,871 (10) | 293,897,032 (10) |
| fx | 10 | 0 | 373 [221–1,317] (10) | 331 (10) | 95.8 (10) | 36.5 (10) | 81.4 (10) | 497,231,275 (10) | 114,446,756 (10) |

## Install size

Runtimes a lane needs but does not install (Node for npm packages) are not counted; VERSIONS.md lists them.

| lane | samples | errors | install MB | main binary MB |
|---|---:|---:|---:|---:|
| nc | 1 | 0 | 44.1 (1) | 44.1 (1) |
| fx | 1 | 0 | 12.2 (1) | 12.2 (1) |

## Where nc leads

- Session store: 14,600 one-turn sessions, store KB: nc 61,996 against fx 180,892 (2.92x better)
- Session store: 14,600 one-turn sessions, KB per session: nc 4.25 against fx 12.4 (2.92x better)
- Session store: 14,600 one-turn sessions, files: nc 14,597 against fx 29,195 (2.00x better)
- Session store: 14,600 one-turn sessions, CPU per saved session ms: nc 17.5 against fx 31.5 (1.80x better)

## Where nc trails

- Listing 14,600 sessions, wall ms: nc 2,673 against fx 191 (13.96x worse)
- Listing 14,600 sessions, task-clock ms: nc 932 against fx 75.1 (12.41x worse)
- Listing 14,600 sessions, CPU ms: nc 949 against fx 87.8 (10.81x worse)
- Listing 14,600 sessions, user instructions: nc 4,757,870,016 against fx 493,876,886 (9.63x worse)
- Continuing the latest of 14,600 sessions, task-clock ms: nc 772 against fx 81.4 (9.49x worse)
- Continuing the latest of 14,600 sessions, user instructions: nc 4,693,763,871 against fx 497,231,275 (9.44x worse)
- Continuing the latest of 14,600 sessions, CPU ms: nc 791 against fx 95.8 (8.25x worse)
- Continuing the latest of 14,600 sessions, spawn to request ms: nc 1,528 against fx 331 (4.62x worse)
- Continuing the latest of 14,600 sessions, wall ms: nc 1,646 against fx 373 (4.42x worse)
- Listing 14,600 sessions, kernel instructions: nc 446,614,404 against fx 114,337,173 (3.91x worse)
- Install size, main binary MB: nc 44.1 against fx 12.2 (3.61x worse)
- Install size, install MB: nc 44.1 against fx 12.2 (3.61x worse)
- Continuing the latest of 14,600 sessions, kernel instructions: nc 293,897,032 against fx 114,446,756 (2.57x worse)
- Continuing the latest of 14,600 sessions, peak RSS MB: nc 60.0 against fx 36.5 (1.64x worse)
- Listing 14,600 sessions, peak RSS MB: nc 59.9 against fx 42.9 (1.40x worse)

## Ties (within 5%)

