# Commands (2026-11-14, cwd bench/cli, on dev-georgios)
+ LANES=fx sh tools/install.sh
+ NC_BIN=<nanocode 62a0936e4> tools/netguard.sh node harness/suite.mjs --parts=store --only=nc --sessions=14600 --storerounds=0 --storehome=$HOME/store14600 --tag=post-gen   (resumed after interruptions; .progress keeps the count)
+ tools/netguard.sh node harness/suite.mjs --parts=store --only=fx --sessions=14600 --storerounds=0 --storehome=$HOME/store14600 --tag=post-gen
+ sudo sysctl -q kernel.perf_event_paranoid=-1
+ BENCH_PERF=1 NC_BIN=<nanocode 62a0936e4> tools/netguard.sh node harness/suite.mjs --parts=store --only=nc,fx --sessions=14600 --storerounds=10 --storehome=$HOME/store14600 --tag=time
+ BENCH_INSTALL_NC=<nanocode 62a0936e4> node harness/suite.mjs --parts=disk --only=nc,fx --tag=disk
