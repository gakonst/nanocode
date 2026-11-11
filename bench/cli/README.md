# CLI benchmark

Compares agent CLIs on what the harness itself costs: startup, memory, CPU, install size, and how well each keeps up with a reply streamed at a fixed token rate. Nothing here calls a real model. Every CLI talks to `harness/fakellm.mjs`, a local server that speaks OpenAI Responses (SSE and WebSocket), Chat Completions, Anthropic Messages and fx's gateway stream, and writes `w<i> ` tokens on a timer at the requested rate.

This directory is a work in progress on the `cli-bench` branch. It came from a scratch harness that compared nc, fx and pi on 2026-10-28 (`prior/2026-10-28-nc-fx-pi.md`). On 2026-11-09 it was made portable and given lanes for Claude Code, Codex and OpenCode. No results from the six-way comparison exist yet. `PROMPT.md` is the task for the agent that runs it.

## Layout

| Path | Holds |
| --- | --- |
| `harness/fakellm.mjs` | The fixture. A prompt carries `BENCH[run=<id>,mode=stream,tokens=N,rate=R]`, and `GET /_log?run=<id>` returns what the server did, with timestamps. Under the `/sa` path prefix a request without tools gets a short reply, for CLIs that send title or summary requests. |
| `harness/lanes.mjs` | One entry per CLI: how to isolate it, run one headless prompt, and start its terminal interface. `verified` says what has run. |
| `harness/smoke.mjs` | Runs one short stream through a lane both ways and prints what happened. |
| `harness/suite.mjs` | Startup, one-shot, first frame, idle and install size. |
| `harness/sweep.mjs` | Streaming at fixed rates, headless (`--mode=cli`) and in the terminal interface (`--mode=tui`). |
| `harness/tui.py`, `tui-replay.py`, `screen.py` | A PTY driver that answers terminal queries and records output with timestamps, a replay through pyte that finds when each token reached the screen, and a helper that prints a capture's final screen. |
| `harness/analyze.mjs`, `report.mjs` | Lag statistics, and Markdown tables from `results/`. |
| `tools/runstat.c`, `runstat.py` | Run a command and write its CPU time and peak memory. The C one (macOS) adds peak footprint and instructions retired. |

## Run

```sh
cd bench/cli
npm install
python3 -m venv .venv && .venv/bin/pip install -r requirements.txt
cc -O2 -o tools/runstat tools/runstat.c            # macOS only; otherwise runstat.py is used

export PYTHON=$PWD/.venv/bin/python
export NC_BIN=... FX_BIN=... PI_CLI=.../pi-coding-agent/dist/cli.js CLAUDE_BIN=... CODEX_BIN=... OPENCODE_BIN=...

node harness/smoke.mjs --only=nc,fx                # make a lane work first
node harness/suite.mjs --parts=startup,oneshot,firstframe,idle --rounds=30
node harness/sweep.mjs --mode=cli --rates=1000,10000 --samples=10
node harness/sweep.mjs --mode=tui --rates=1000,10000 --samples=10
node harness/report.mjs
```

## nc-lite

The `nclite` lane is nanocode cut down to what fx ships, so the two compare like for like: `bin/nc-lite` in this repository, built on the SDK with one harness (Codex), fx's system prompt, any model through the AI Gateway, plain function tools instead of Code Mode, a JSONL home, `exec`, `--continue`, `sessions` and an inline terminal interface, and no MCP, computer use or runners. The full `nc` lane carries all of those. `LANES=nclite sh tools/install.sh` builds it with cargo (`--profile dist`) at `NCLITE_REV`.

Each run uses a fresh `HOME` under `work/` and a workspace in `/tmp/cli-bench-ws`, so no lane reads a real configuration or session.

## Measures

- **Lag:** the time a consumer (stdout, or the emulated screen) showed token `i`, minus the time the server wrote it. Both sides read the same monotonic clock.
- **Tail:** stream end to process exit (headless), or to the last token on screen (terminal interface).
- **CPU and memory:** the process's own `rusage`, so a CLI that starts helper processes needs its process group measured instead.

## State on 2026-11-09

Checked on one loaded laptop (macOS, load average about 38), which is why no numbers are recorded here.

| Lane | Headless | Terminal interface |
| --- | --- | --- |
| nc 0.7.0 | passes the smoke | passes |
| fx 0.0.15 | passes | passes |
| pi | ran on 2026-10-28 at 1.4.0, not rerun | same |
| claude 2.1.316 | passes, no side requests | reaches its prompt; a streamed turn has not run |
| codex 0.162.0-alpha | passes; `exec --json` prints the reply at the end | draws a frame, sends no request |
| opencode | not run | not run |
