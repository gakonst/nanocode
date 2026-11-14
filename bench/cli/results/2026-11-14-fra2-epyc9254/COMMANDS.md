# Commands (2026-11-14T06:48:19Z, cwd bench/cli, harness commit 437a58aee)
+ tools/netguard.sh node harness/smoke.mjs
+ node harness/netaudit.mjs --dir=/home/ubuntu/bench/bench/cli/results/2026-11-14-fra2-epyc9254
+ tools/netguard.sh node harness/suite.mjs --parts=startup --rounds=100
+ tools/netguard.sh node harness/suite.mjs --parts=coldstart --coldrounds=10
+ tools/netguard.sh node harness/suite.mjs --parts=firstframe --rounds=20
+ tools/netguard.sh node harness/suite.mjs --parts=coldframe --coldrounds=10
+ tools/netguard.sh node harness/suite.mjs --parts=keys --keyrounds=10
+ tools/netguard.sh node harness/suite.mjs --parts=oneshot --rounds=20
+ tools/netguard.sh node harness/suite.mjs --parts=tools --steps=10,50 --toolrounds=10
+ tools/netguard.sh node harness/suite.mjs --parts=fleet --fleet=1,8,32 --fleetrounds=10
# Commands (2026-11-14T07:19:07Z, cwd bench/cli, harness commit 437a58aee)
+ tools/netguard.sh node harness/suite.mjs --parts=store --sessions=1000 --storerounds=20
+ env BENCH_PERF=1 tools/netguard.sh node harness/suite.mjs --parts=store --only=nc,fx --sessions=14600 --storerounds=10 --storehome=/home/ubuntu/bench/bench/cli/results/2026-11-14-fra2-epyc9254/store14600
+ node harness/suite.mjs --parts=disk
+ tools/netguard.sh node harness/sweep.mjs --mode=tui --rates=100,1000,10000 --samples=10
+ python3 harness/post.py --out results/2026-11-14-fra2-epyc9254/POST.md results/2026-11-14-fra2-epyc9254 results/2026-11-13-epyc4245p results/2026-11-13-epyc4245p-b results/2026-11-13-epyc4245p-c results/2026-11-14-dev-georgios-store14600
