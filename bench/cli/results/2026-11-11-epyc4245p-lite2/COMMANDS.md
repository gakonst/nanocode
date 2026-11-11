# Commands (2026-11-11T05:39:41Z, cwd bench/cli, harness commit )
+ tools/netguard.sh node harness/smoke.mjs --only=fx,nclite,nc
+ node harness/netaudit.mjs --only=fx,nclite,nc --dir=/home/ubuntu/bench/cli/results/2026-11-11-epyc4245p-lite2
+ tools/netguard.sh node harness/suite.mjs --parts=startup --rounds=100 --only=fx,nclite,nc
+ tools/netguard.sh node harness/suite.mjs --parts=coldstart --coldrounds=10 --only=fx,nclite,nc
+ tools/netguard.sh node harness/suite.mjs --parts=firstframe --rounds=30 --only=fx,nclite,nc
+ tools/netguard.sh node harness/suite.mjs --parts=coldframe --coldrounds=10 --only=fx,nclite,nc
+ tools/netguard.sh node harness/suite.mjs --parts=idle --idle=10 --only=fx,nclite,nc
+ tools/netguard.sh node harness/suite.mjs --parts=keys --keyrounds=10 --only=fx,nclite,nc
+ tools/netguard.sh node harness/suite.mjs --parts=oneshot --rounds=30 --only=fx,nclite,nc
+ tools/netguard.sh node harness/suite.mjs --parts=tools --steps=10,50 --toolrounds=10 --only=fx,nclite,nc
+ tools/netguard.sh node harness/suite.mjs --parts=fleet --fleet=1,8,32 --fleetrounds=10 --only=fx,nclite,nc
+ tools/netguard.sh node harness/suite.mjs --parts=store --sessions=1000 --storerounds=20 --only=fx,nclite,nc
+ node harness/suite.mjs --parts=disk --only=fx,nclite,nc
+ tools/netguard.sh node harness/sweep.mjs --mode=cli --rates=100,1000,10000 --samples=10 --only=fx,nclite,nc
+ tools/netguard.sh node harness/sweep.mjs --mode=tui --rates=100,1000,10000 --samples=10 --only=fx,nclite,nc
+ tools/netguard.sh node harness/sweep.mjs --mode=tui --rates=50000 --samples=10 --tag=50k --only=fx,nclite,nc
+ tools/netguard.sh node harness/sweep.mjs --mode=tui --rates=10000 --samples=10 --cols=240 --rows=70 --tag=240x70 --only=fx,nclite,nc
# Commands (2026-11-11T07:31:58Z, cwd bench/cli, harness commit )
+ node harness/suite.mjs --parts=coldstart --coldrounds=10 --only=fx,nclite,nc
- run.log: the 240x70 sweep stopped at the box lifetime with 8 of 10 samples per lane; coldstart was rerun without netguard after the main run, while that sweep was still going.
