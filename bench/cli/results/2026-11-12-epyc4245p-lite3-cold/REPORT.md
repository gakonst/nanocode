# CLI benchmark: 2026-11-12-epyc4245p-lite3-cold

Machine: AMD EPYC 4245P 6-Core Processor, 12 threads, 46 GB, Ubuntu 24.04.5 LTS, 6.8.0-147-generic. 

# Versions (staged 2026-11-12 on the Hand)
- fx: 0.0.16 (releases.fx.sh)
- nc: nanocode at 7837aae16, LLVM_BOLT=llvm-bolt-23 scripts/build-cli.sh nanocode-cli nanocode (dist, non-PIE, BOLT), 82145536 bytes
- nclite: nc-lite at 7837aae16, PROFILE=lite scripts/build-cli.sh nc-lite nc-lite (non-PIE, BOLT), 19063696 bytes

Medians with the sample count in parentheses and [min–max] where the spread matters. CPU and memory are the whole process tree (cgroup or /proc sampling; rusage when neither is available). Lower is better everywhere.

## Cold start: --version after dropping the page cache

| lane | samples | errors | wall ms | CPU ms | peak RSS MB |
|---|---:|---:|---:|---:|---:|
| nc | 10 | 0 | 8.84 [8.70–8.90] (10) | 1.56 (10) | 5.75 (10) |
| fx | 10 | 0 | 4.74 [4.70–6.02] (10) | 0.565 (10) | 1.38 (10) |
| nclite | 10 | 0 | 6.81 [6.76–7.08] (10) | 0.731 (10) | 3.13 (10) |

## Where nc leads


## Where nc trails

- Cold start: --version after dropping the page cache, peak RSS MB: nc 5.75 against fx 1.38 (4.18x worse)
- Cold start: --version after dropping the page cache, CPU ms: nc 1.56 against fx 0.565 (2.76x worse)
- Cold start: --version after dropping the page cache, wall ms: nc 8.84 against fx 4.74 (1.86x worse)

## Ties (within 5%)

