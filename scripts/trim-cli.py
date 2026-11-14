import struct
import sys

PT_LOAD = 1
PT_GNU_EH_FRAME = 0x6474E550
SHT_NOBITS = 8
DW_EH_PE_udata4 = 0x03
DW_EH_PE_datarel_sdata4 = 0x3B
DEAD = (b".bolt.org.eh_frame", b".bolt.org.eh_frame_hdr")
PHDR = "<IIQQQQQQ"
SHDR = "<IIQQQQIIQQ"

path = sys.argv[1]
data = bytearray(open(path, "rb").read())
if data[:4] != b"\x7fELF" or data[4] != 2 or data[5] != 1:
    sys.exit(f"trim-cli.py: {path} is not a little-endian ELF64 file")
phoff, shoff = struct.unpack_from("<QQ", data, 0x20)
phentsize, phnum, shentsize, shnum, shstrndx = struct.unpack_from("<HHHHH", data, 0x36)
phdrs = [list(struct.unpack_from(PHDR, data, phoff + i * phentsize)) for i in range(phnum)]
shdrs = [list(struct.unpack_from(SHDR, data, shoff + i * shentsize)) for i in range(shnum)]
strtab = shdrs[shstrndx][4]


def name(s):
    start = strtab + s[0]
    return bytes(data[start:data.index(0, start)])


dead = sorted((s for s in shdrs if name(s) in DEAD), key=lambda s: s[4])
if sorted(name(s) for s in dead) != sorted(DEAD):
    sys.exit(f"trim-cli.py: {path} lacks {DEAD}")
lo = dead[0][4]
hi = dead[-1][4] + dead[-1][5]
if any(lo < s[4] + s[5] and s[4] < hi and s not in dead and s[1] != SHT_NOBITS for s in shdrs):
    sys.exit("trim-cli.py: a live section lies between the original unwind tables")
eh = [p for p in phdrs if p[0] == PT_GNU_EH_FRAME]
if len(eh) != 1 or lo <= eh[0][2] < hi:
    sys.exit("trim-cli.py: PT_GNU_EH_FRAME does not name the rewritten .eh_frame_hdr")
hdr = eh[0][2]
if data[hdr + 2:hdr + 4] != bytes((DW_EH_PE_udata4, DW_EH_PE_datarel_sdata4)):
    sys.exit("trim-cli.py: .eh_frame_hdr has no binary search table")
(count,) = struct.unpack_from("<I", data, hdr + 8)
fdes = struct.unpack_from(f"<{2 * count}i", data, hdr + 12)[1::2]
if any(dead[0][3] <= eh[0][3] + f < dead[-1][3] + dead[-1][5] for f in fdes):
    sys.exit("trim-cli.py: .eh_frame_hdr points into the original .eh_frame")
seg = [p for p in phdrs if p[0] == PT_LOAD and p[2] <= lo and hi == p[2] + p[5]]
if len(seg) != 1 or seg[0][5] != seg[0][6]:
    sys.exit("trim-cli.py: the original unwind tables do not end their segment")
seg = seg[0]
moved = [p for p in phdrs if p[0] == PT_LOAD and p[2] >= hi]
align = max(p[7] for p in moved)
cut = (min(p[2] for p in moved) - lo) // align * align
seg[5] = seg[6] = lo - seg[2]
for p in phdrs:
    if p[2] >= lo + cut:
        p[2] -= cut
    elif p is not seg and p[2] + p[5] > lo and p[5]:
        sys.exit("trim-cli.py: a segment overlaps the original unwind tables")
for s in shdrs:
    if s in dead:
        s[1], s[4] = SHT_NOBITS, lo
    elif s[4] >= lo + cut:
        s[4] -= cut
if shoff >= lo + cut:
    shoff -= cut
if phoff >= lo + cut:
    phoff -= cut
del data[lo:lo + cut]
struct.pack_into("<QQ", data, 0x20, phoff, shoff)
for i, p in enumerate(phdrs):
    struct.pack_into(PHDR, data, phoff + i * phentsize, *p)
for i, s in enumerate(shdrs):
    struct.pack_into(SHDR, data, shoff + i * shentsize, *s)
open(path, "wb").write(data)
