# Prints the final screen of a raw terminal capture written by tui.py's "dump" option: python harness/screen.py FILE [rows] [cols]
import sys
import pyte

rows = int(sys.argv[2]) if len(sys.argv) > 2 else 40
cols = int(sys.argv[3]) if len(sys.argv) > 3 else 120
screen = pyte.Screen(cols, rows)
stream = pyte.ByteStream(screen)
stream.feed(open(sys.argv[1], "rb").read())
for line in screen.display:
    if line.strip():
        print(line.rstrip())
