# Commands (2026-11-13T11:51:53Z, cwd bench/cli, harness commit 37b82b720)
+ tools/netguard.sh node harness/smoke.mjs --only=nc,nclite,fx,pi,opencode,claude,codex
+ tools/netguard.sh node harness/suite.mjs --parts=startup --rounds=100 --only=nc,nclite,fx,pi,opencode,claude,codex
+ tools/netguard.sh node harness/suite.mjs --parts=firstframe --rounds=20 --only=nc,nclite,fx,pi,opencode,claude,codex
+ tools/netguard.sh node harness/suite.mjs --parts=oneshot --rounds=20 --only=nc,nclite,fx,pi,opencode,claude,codex
+ tools/netguard.sh node harness/suite.mjs --parts=idle --idle=10 --only=nc,nclite,fx,pi,opencode,claude,codex
+ tools/netguard.sh node harness/suite.mjs --parts=tools --steps=10,50 --toolrounds=10 --only=nc,nclite,fx,pi,opencode,claude,codex
+ tools/netguard.sh node harness/suite.mjs --parts=fleet --fleet=1,8,32 --fleetrounds=10 --only=nc,nclite,fx,pi,opencode,claude,codex
+ node harness/suite.mjs --parts=disk --only=nc,nclite,fx,pi,opencode,claude,codex
+ tools/netguard.sh node harness/sweep.mjs --mode=tui --rates=100,1000 --samples=5 --only=nc,nclite,fx,pi,opencode,claude,codex
