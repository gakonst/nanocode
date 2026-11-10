# Commands (2026-11-10T20:14:33Z, cwd bench/cli, harness commit )
+ tools/netguard.sh node harness/suite.mjs --parts=startup --rounds=100 --only=ncbase,ncnew,nc120,fx
+ tools/netguard.sh node harness/suite.mjs --parts=firstframe --rounds=30 --only=ncbase,ncnew,nc120,fx
+ tools/netguard.sh node harness/suite.mjs --parts=oneshot --rounds=30 --only=ncbase,ncnew,nc120,fx
+ tools/netguard.sh node harness/suite.mjs --parts=tools --steps=10 --toolrounds=10 --only=ncbase,ncnew,nc120,fx
+ tools/netguard.sh node harness/sweep.mjs --mode=cli --rates=100,1000,10000 --samples=10 --only=ncbase,ncnew,nc120,fx
+ tools/netguard.sh node harness/sweep.mjs --mode=tui --rates=100,1000,10000 --samples=10 --only=ncbase,ncnew,nc120,fx
