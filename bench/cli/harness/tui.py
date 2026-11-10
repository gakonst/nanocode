#!/usr/bin/env python3
import fcntl, json, os, re, select, signal, struct, subprocess, sys, termios, time

QUERY_REPLIES = [
    (re.compile(rb"\x1b\]11;\?(?:\x1b\\|\x07)"), b"\x1b]11;rgb:1d1d/1f1f/2121\x1b\\"),
    (re.compile(rb"\x1b\]10;\?(?:\x1b\\|\x07)"), b"\x1b]10;rgb:e5e5/e5e5/e5e5\x1b\\"),
    (re.compile(rb"\x1b\[6n"), b"\x1b[1;1R"),
    (re.compile(rb"\x1b\[\?996n"), b"\x1b[?997;1n"),
    (re.compile(rb"\x1b\[\?u"), b"\x1b[?0u"),
    (re.compile(rb"\x1b\[>q"), b"\x1bP>|bench-term\x1b\\"),
    (re.compile(rb"\x1b\[\?(\d+)\$p"), None),
    (re.compile(rb"\x1b\[0?c"), b"\x1b[?62;22c"),
    (re.compile(rb"\x1b\[18t"), b"\x1b[8;40;120t"),
    (re.compile(rb"\x1b\[14t"), b"\x1b[4;800;1200t"),
    (re.compile(rb"\x1b\[16t"), b"\x1b[6;20;10t"),
]
ANSI = re.compile(rb"\x1b\[[0-9;?<>=!]*[ -/]*[@-~]|\x1b\][^\x07\x1b]*(?:\x07|\x1b\\)|\x1bP[^\x1b]*\x1b\\|\x1b[()][0-9A-Za-z]|\x1b[=>78DEHMNOZc]")
TOKEN = re.compile(rb"w(\d+)(?=[ \r\n\x1b\\])")
SYNC = b"\x1b[?2026h"

CLOCK = time.CLOCK_MONOTONIC_RAW if sys.platform == "darwin" else time.CLOCK_MONOTONIC

def ns():
    return time.clock_gettime_ns(CLOCK)

def replies(chunk):
    out = b""
    for pattern, reply in QUERY_REPLIES:
        for m in pattern.finditer(chunk):
            out += (b"\x1b[?" + m.group(1) + b";2$y") if reply is None else reply
    return out

def main():
    spec = json.loads(sys.argv[1])
    rows, cols = spec.get("rows", 40), spec.get("cols", 120)
    master, slave = os.openpty()
    fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", rows, cols, 0, 0))
    def leader():
        os.setsid()
        fcntl.ioctl(0, termios.TIOCSCTTY, 0)
    env = spec["env"]
    env.setdefault("TERM", "xterm-256color")
    env["COLUMNS"] = str(cols); env["LINES"] = str(rows)
    start = ns()
    proc = subprocess.Popen(spec["cmd"], cwd=spec["cwd"], env=env, stdin=slave, stdout=slave, stderr=slave, preexec_fn=leader, close_fds=True)
    os.close(slave)
    marker = re.compile(spec["marker"].encode()) if spec.get("marker") else None
    first_byte = first_frame = sent_at = exit_sent = None
    total = 0; after_send = 0; frames_after = 0; chunks_after = 0
    screen_tail = b""
    plain_tail = b""
    maxi = -1
    marks = []
    dump = bytearray() if spec.get("dump") else None
    target = spec.get("tokens")
    done_at = None
    last_data = start
    deadline = start + int(spec.get("timeout", 60) * 1e9)
    window = spec.get("window")
    send = spec.get("send")
    quiet_ns = int(spec.get("quiet_ms", 400) * 1e6)
    send_delay_ns = int(spec.get("send_delay_ms", 300) * 1e6)
    frame_at = None
    status = rusage = None
    min_ns = int(spec["min_ms"] * 1e6) if spec.get("min_ms") is not None else None
    max_ns = int(spec["max_ms"] * 1e6) if spec.get("max_ms") is not None else None
    record = open(spec["record"], "wb") if spec.get("record") else None
    keys = spec.get("keys")
    key_interval_ns = int(spec.get("key_interval_ms", 120) * 1e6)
    key_log = []
    key_pending = None
    key_next_at = None
    while True:
        t = ns()
        if send is not None and sent_at is None and frame_at is not None and t - frame_at >= send_delay_ns:
            for k, piece in enumerate(send):
                os.write(master, piece.encode())
                if k == len(send) - 1:
                    sent_at = ns()
                else:
                    time.sleep(0.05)
        if keys is not None and frame_at is not None:
            if key_next_at is None:
                key_next_at = frame_at + send_delay_ns
            if key_pending is None and len(key_log) < len(keys) and t >= key_next_at:
                os.write(master, keys[len(key_log)].encode())
                key_pending = ns()
            if key_pending is not None and t - key_pending > 2e9:
                key_log.append(None); key_pending = None; key_next_at = ns() + key_interval_ns
        finished = False
        if keys is not None and len(key_log) >= len(keys) and exit_sent is None:
            finished = True
        if window is not None and frame_at is not None and send is None and keys is None and t - start >= window * 1e9:
            finished = True
        if target is not None and done_at is not None and t - last_data >= quiet_ns:
            finished = True
        if min_ns is not None and sent_at is not None and t - sent_at >= min_ns and t - last_data >= quiet_ns:
            finished = True
        if max_ns is not None and sent_at is not None and t - sent_at >= max_ns:
            finished = True
        if t > deadline:
            finished = True
        if finished and exit_sent is None:
            for piece in spec.get("exit", ["\x04"]):
                try: os.write(master, piece.encode())
                except OSError: pass
                time.sleep(0.1)
            exit_sent = ns()
        r, _, _ = select.select([master], [], [], 0.005)
        if r:
            try:
                data = os.read(master, 1 << 16)
            except OSError:
                data = b""
            if data:
                stamp = ns()
                last_data = stamp
                total += len(data)
                if dump is not None: dump += data
                if record is not None and sent_at is not None:
                    record.write(struct.pack("<QI", stamp, len(data)) + data)
                if first_byte is None: first_byte = stamp
                if key_pending is not None and keys[len(key_log)].encode() in data:
                    key_log.append((stamp - key_pending) / 1e6); key_pending = None; key_next_at = stamp + key_interval_ns
                rep = replies(data)
                if rep: os.write(master, rep)
                if sent_at is not None:
                    after_send += len(data); chunks_after += 1
                    frames_after += data.count(SYNC)
                plain = ANSI.sub(b"", (screen_tail + data))
                screen_tail = data[-64:]
                if marker is not None and first_frame is None:
                    plain_tail = (plain_tail + plain)[-8192:]
                    if marker.search(plain_tail):
                        first_frame = stamp; frame_at = stamp
                if sent_at is not None and target is not None:
                    for m in TOKEN.finditer(plain):
                        i = int(m.group(1))
                        if i > maxi and i < target:
                            maxi = i
                    if not marks or marks[-1][1] != maxi:
                        marks.append([stamp, maxi])
                    if maxi >= target - 1 and done_at is None:
                        done_at = stamp
        pid, st, ru = os.wait4(proc.pid, os.WNOHANG)
        if pid:
            status, rusage = st, ru
            exited = ns()
            break
        if exit_sent is not None and ns() - exit_sent > 5e9:
            proc.kill()
            _, status, rusage = os.wait4(proc.pid, 0)
            exited = ns(); status = -9
            break
    os.close(master)
    if record is not None: record.close()
    ms = lambda x: None if x is None else (x - start) / 1e6
    out = {
        "start_ns": start, "first_byte_ms": ms(first_byte), "first_frame_ms": ms(first_frame), "sent_ns": sent_at, "done_ns": done_at,
        "exit_after_ms": None if exit_sent is None else (exited - exit_sent) / 1e6, "status": status,
        "cpu_ms": (rusage.ru_utime + rusage.ru_stime) * 1000, "user_ms": rusage.ru_utime * 1000, "sys_ms": rusage.ru_stime * 1000, "max_rss": rusage.ru_maxrss,
        "bytes_total": total, "bytes_after_send": after_send, "chunks_after_send": chunks_after, "sync_frames_after_send": frames_after, "max_token": maxi, "marks": marks, "key_latency_ms": key_log,
    }
    if dump is not None:
        with open(spec["dump"], "wb") as f: f.write(bytes(dump))
    print(json.dumps(out))

main()
