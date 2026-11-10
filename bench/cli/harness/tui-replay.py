import json, re, struct, sys
import pyte

TOKEN = re.compile(r"w(\d+)(?=[ ]|$)")

def main():
    path, rows, cols, target = sys.argv[1], int(sys.argv[2]), int(sys.argv[3]), int(sys.argv[4])
    screen = pyte.Screen(cols, rows)
    stream = pyte.ByteStream(screen)
    data = open(path, "rb").read()
    off = 0
    maxi = -1
    marks = []
    chunks = 0
    while off + 12 <= len(data):
        t, n = struct.unpack_from("<QI", data, off)
        off += 12
        chunk = data[off:off + n]
        off += n
        chunks += 1
        stream.feed(chunk)
        best = maxi
        for line in screen.display:
            for m in TOKEN.finditer(line):
                i = int(m.group(1))
                if best < i < target:
                    best = i
        if best > maxi:
            maxi = best
            marks.append([t, maxi])
    tail = [l.rstrip() for l in screen.display if l.strip()][-6:]
    print(json.dumps({"marks": marks, "max_token": maxi, "chunks": chunks, "screen_tail": tail}))

main()
