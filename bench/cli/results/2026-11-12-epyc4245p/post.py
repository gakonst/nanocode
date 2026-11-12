#!/usr/bin/env python3
# post.py <results dir>: renders POST.md from POST.template.md, with nc and nc-lite release (BOLT) rows from the -lite3 runs beside this run's.
import json, os, re, subprocess, sys, tempfile
D = os.path.abspath(sys.argv[1])
R = os.path.dirname(D)
BOLT = {"nc": "nc-bolt", "nclite": "nclite-bolt"}
SUITE = {"startup", "firstframe", "idle", "keys", "oneshot", "tools", "fleet", "store", "store-list", "store-resume"}
tmp = tempfile.mkdtemp()
for name in os.listdir(D):
    if name.endswith(".jsonl"):
        with open(os.path.join(D, name)) as src, open(os.path.join(tmp, name), "w") as dst:
            dst.write(src.read())
with open(os.path.join(tmp, "suite-bolt.jsonl"), "w") as out:
    for run, parts in (("-lite3", SUITE), ("-lite3-cold", {"coldstart"})):
        for line in open(os.path.join(R, "2026-11-12-epyc4245p" + run, "suite.jsonl")):
            row = json.loads(line)
            if row.get("part") in parts and row.get("harness") in BOLT:
                out.write(json.dumps({**row, "harness": BOLT[row["harness"]]}) + "\n")
here = os.path.dirname(os.path.abspath(__file__))
report = subprocess.run(["node", os.path.join(here, "../../harness/report.mjs"), "--dir=" + tmp, "--lanes=true,nc,nc-bolt,fx,pi,opencode,claude,codex,nclite,nclite-bolt"], capture_output=True, text=True, check=True).stdout
secs = {m.group(1).strip(): m.group(2).strip() for m in re.finditer(r"^## (.+?)\n(.*?)(?=^## |\Z)", report, re.S | re.M)}
def sub(m):
    key = m.group(1)
    if key == "STORE":
        keys = [k for k in secs if k.startswith(("Session store", "Listing", "Continuing"))]
        return "## Session store\n\n" + "\n\n".join("### " + k + "\n\n" + secs[k] for k in keys)
    return "### " + key + "\n\n" + secs[key]
template = open(os.path.join(here, "POST.template.md")).read()
open(os.path.join(D, "POST.md"), "w").write(re.sub(r"\{\{(.+?)\}\}", sub, template).rstrip() + "\n")
