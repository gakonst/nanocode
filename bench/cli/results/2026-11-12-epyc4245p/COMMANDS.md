## r1/results
# Commands (2026-11-12T03:11:41Z, cwd bench/cli, harness commit 6751fbf48)
+ tools/netguard.sh node harness/smoke.mjs --only=fx,nclite,nc,pi,opencode,claude,codex
+ node harness/netaudit.mjs --only=fx,nclite,nc,pi,opencode,claude,codex --dir=/home/ubuntu/results
+ tools/netguard.sh node harness/suite.mjs --parts=startup --rounds=100 --only=fx,nclite,nc,pi,opencode,claude,codex
+ tools/netguard.sh node harness/suite.mjs --parts=coldstart --coldrounds=10 --only=fx,nclite,nc,pi,opencode,claude,codex
+ tools/netguard.sh node harness/suite.mjs --parts=firstframe --rounds=30 --only=fx,nclite,nc,pi,opencode,claude,codex
+ tools/netguard.sh node harness/suite.mjs --parts=coldframe --coldrounds=10 --only=fx,nclite,nc,pi,opencode,claude,codex
+ tools/netguard.sh node harness/suite.mjs --parts=idle --idle=10 --only=fx,nclite,nc,pi,opencode,claude,codex
+ tools/netguard.sh node harness/suite.mjs --parts=keys --keyrounds=10 --only=fx,nclite,nc,pi,opencode,claude,codex
+ tools/netguard.sh node harness/suite.mjs --parts=oneshot --rounds=30 --only=fx,nclite,nc,pi,opencode,claude,codex
+ tools/netguard.sh node harness/suite.mjs --parts=tools --steps=10,50 --toolrounds=10 --only=fx,nclite,nc,pi,opencode,claude,codex
+ tools/netguard.sh node harness/suite.mjs --parts=fleet --fleet=1,8,32 --fleetrounds=10 --only=fx,nclite,nc,pi,opencode,claude,codex

## r2/results2
# Commands (2026-11-12T04:13:49Z, cwd bench/cli, harness commit 6751fbf48)
+ node harness/netaudit.mjs --only=fx,nclite,nc,pi,opencode,claude,codex --dir=/home/ubuntu/results2
+ tools/netguard.sh node harness/suite.mjs --parts=coldstart --coldrounds=10 --only=fx,nclite,nc,pi,opencode,claude,codex
+ tools/netguard.sh node harness/suite.mjs --parts=coldframe --coldrounds=10 --only=fx,nclite,nc,pi,opencode,claude,codex
+ tools/netguard.sh node harness/suite.mjs --parts=fleet --fleet=1,8,32 --fleetrounds=10 --only=fx,nclite,nc,pi,opencode,claude,codex
+ node harness/suite.mjs --parts=disk --only=fx,nclite,nc,pi,opencode,claude,codex

## ra/results
# Commands (2026-11-12T03:58:02Z, cwd bench/cli, harness commit 6751fbf48)
+ tools/netguard.sh node harness/smoke.mjs --only=fx,nclite,nc,pi,opencode,claude,codex
+ tools/netguard.sh node harness/sweep.mjs --mode=cli --rates=100,1000,10000 --samples=10 --only=fx,nclite,nc,pi,opencode,claude,codex
+ tools/netguard.sh node harness/sweep.mjs --mode=tui --rates=50000 --samples=10 --tag=50k --only=fx,nclite,nc,pi,opencode,claude,codex

## rb/results
# Commands (2026-11-12T03:57:34Z, cwd bench/cli, harness commit 6751fbf48)
+ tools/netguard.sh node harness/smoke.mjs --only=fx,nclite,nc,pi,opencode,claude,codex
+ tools/netguard.sh node harness/sweep.mjs --mode=tui --rates=100,1000,10000 --samples=10 --only=fx,nclite,nc,pi,opencode,claude,codex
+ tools/netguard.sh node harness/sweep.mjs --mode=tui --rates=10000 --samples=10 --cols=240 --rows=70 --tag=240x70 --only=fx,nclite,nc,pi,opencode,claude,codex

## r3/results3
# Commands (2026-11-12T05:16:09Z, cwd bench/cli, harness commit 6751fbf48)
+ tools/netguard.sh node harness/suite.mjs --parts=store --sessions=1000 --storerounds=20 --only=fx,nclite,nc,pi,claude

