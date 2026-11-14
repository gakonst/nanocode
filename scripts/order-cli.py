#!/usr/bin/env python3
import sys

order = [line.strip() for line in open(sys.argv[1]) if line.strip()]
startup = set()
for path in sys.argv[2:]:
    for line in open(path):
        fields = line.split()
        if len(fields) == 8 and int(fields[7]) > 0:
            startup.update((fields[1], fields[4]))
for name in [name for name in order if name in startup] + [name for name in order if name not in startup]:
    print(name)
