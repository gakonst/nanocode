# 2026-11-10 A/B: nanocode master builds against fx

Machine in machine.json (boxctl bare metal, AMD EPYC 4245P, Ubuntu 24.04), every step under tools/netguard.sh, lanes interleaved.

- ncbase: release build of nanocode 5aa3971ee
- ncnew: 5aa3971ee plus the shared trust roots (09bb9a5d5) and the exec flush (0fbd0054a)
- nc120: ncnew with the terminal interface paced at 120 Hz instead of 60

The long-tool-turn rows for the nc builds failed because the fixture was told the lane name (ncbase) instead of nc and sent no tool it knows; suite.mjs now passes h=nc for nc variants. The scout run beside it (2026-11-10-scout) has all six lanes at fewer samples.
