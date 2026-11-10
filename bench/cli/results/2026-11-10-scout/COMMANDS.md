# Commands (2026-11-10T19:49:30Z, cwd bench/cli, harness commit )
+ node harness/smoke.mjs
+ node harness/suite.mjs --parts=startup --rounds=100
+ node harness/suite.mjs --parts=coldstart --coldrounds=3
+ node harness/suite.mjs --parts=firstframe --rounds=10
+ node harness/suite.mjs --parts=idle --idle=3
+ node harness/suite.mjs --parts=keys --keyrounds=3
+ node harness/suite.mjs --parts=oneshot --rounds=10
+ node harness/suite.mjs --parts=tools --steps=10 --toolrounds=2
+ node harness/suite.mjs --parts=fleet --fleet=8 --fleetrounds=2
+ node harness/suite.mjs --parts=store --sessions=50 --storerounds=3
+ node harness/suite.mjs --parts=disk
+ node harness/sweep.mjs --mode=cli --rates=1000,10000 --samples=3
+ node harness/sweep.mjs --mode=tui --rates=1000,10000 --samples=3
