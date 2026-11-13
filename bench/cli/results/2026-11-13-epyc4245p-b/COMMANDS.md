# Commands (2026-11-13T16:14:03Z, cwd bench/cli, harness commit 5b313e9ec)
+ tools/netguard.sh node harness/smoke.mjs
+ node harness/netaudit.mjs --dir=/home/ubuntu/bench/bench/cli/results/2026-11-13-epyc4245p6coreproces
+ tools/netguard.sh node harness/suite.mjs --parts=startup --rounds=100
+ tools/netguard.sh node harness/suite.mjs --parts=coldstart --coldrounds=10
+ tools/netguard.sh node harness/suite.mjs --parts=firstframe --rounds=20
+ tools/netguard.sh node harness/suite.mjs --parts=coldframe --coldrounds=10
+ tools/netguard.sh node harness/suite.mjs --parts=idle --idle=10
+ tools/netguard.sh node harness/suite.mjs --parts=keys --keyrounds=10
+ tools/netguard.sh node harness/suite.mjs --parts=oneshot --rounds=20
+ tools/netguard.sh node harness/suite.mjs --parts=tools --steps=10,50 --toolrounds=10
+ tools/netguard.sh node harness/suite.mjs --parts=fleet --fleet=1,8,32 --fleetrounds=10
+ tools/netguard.sh node harness/suite.mjs --parts=store --sessions=1000 --storerounds=20
# Commands (2026-11-13T17:37:14Z, cwd bench/cli, harness commit 5b313e9ec)
+ node harness/suite.mjs --parts=disk
# Commands (2026-11-13T17:37:14Z, cwd bench/cli, harness commit 5b313e9ec)
+ tools/netguard.sh node harness/suite.mjs --parts=coldstart --coldrounds=5
# Commands (2026-11-13T17:37:17Z, cwd bench/cli, harness commit 5b313e9ec)
+ tools/netguard.sh node harness/suite.mjs --parts=coldframe --coldrounds=5
# Commands (2026-11-13T17:38:50Z, cwd bench/cli, harness commit 5b313e9ec)
+ tools/netguard.sh node harness/sweep.mjs --mode=tui --rates=10000 --samples=3 --cols=240 --rows=70 --tag=240x70
