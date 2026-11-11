import { NODE, bins } from "./common.mjs";
import { lanes } from "./lanes.mjs";

// Session-store and tool-turn commands per lane, built on each lane's headless command (whose last argument is the prompt).
// saveHeadless saves the session; list prints the saved sessions; resume continues the latest one; toolHeadless runs a turn that calls the shell.
// Each added flag is the CLI's documented one; tool lanes add only what lets the shell run without a prompt, as fx's --yolo already does.
const withFlags = (spec, flags, { drop = [] } = {}) => { const [cmd, argv] = spec; const rest = argv.slice(0, -1).filter((a) => !drop.includes(a)); return [cmd, [...rest, ...flags, argv.at(-1)]]; };
const save = (name) => (home, base, text) => lanes[name].headless(home, base, text, { save: true });

export const extra = {
  nc: {
    saveHeadless: save("nc"),
    list: (home) => [bins.nc(), ["sessions", "--home", home + "/.nanocode"]],
    resume: (home, base, text) => withFlags(save("nc")(home, base, text), ["--continue"]),
    toolHeadless: (home, base, text) => lanes.nc.headless(home, base, text),
    store: (home) => home + "/.nanocode",
  },
  nclite: {
    saveHeadless: save("nclite"),
    list: (home) => [bins.nclite(), ["sessions", "--home", home + "/.nc-lite"]],
    resume: (home, base, text) => withFlags(save("nclite")(home, base, text), ["--continue"]),
    toolHeadless: (home, base, text) => lanes.nclite.headless(home, base, text),
    store: (home) => home + "/.nc-lite",
  },
  fx: {
    saveHeadless: save("fx"),
    list: () => [bins.fx(), ["sessions", "--json"]],
    resume: (home, base, text) => withFlags(save("fx")(home, base, text), ["--resume", "last"]),
    toolHeadless: (home, base, text) => lanes.fx.headless(home, base, text),
    store: (home) => home + "/.fx",
  },
  pi: {
    saveHeadless: save("pi"),
    resume: (home, base, text) => withFlags(save("pi")(home, base, text), ["--continue"]),
    toolHeadless: (home, base, text) => lanes.pi.headless(home, base, text),
    store: (home) => home + "/.pi",
  },
  claude: {
    saveHeadless: save("claude"),
    resume: (home, base, text) => withFlags(save("claude")(home, base, text), ["--continue"]),
    toolHeadless: (home, base, text) => withFlags(lanes.claude.headless(home, base, text), ["--dangerously-skip-permissions"]),
    store: (home) => home + "/.claude",
  },
  codex: {
    saveHeadless: save("codex"),
    resume: (home, base, text) => { const [cmd, argv] = save("codex")(home, base, text); const i = argv.indexOf("exec"); return [cmd, [...argv.slice(0, i + 1), ...argv.slice(i + 1, -1), "resume", "--last", argv.at(-1)]]; },
    toolHeadless: (home, base, text) => lanes.codex.headless(home, base, text),
    store: (home) => home + "/.codex",
  },
  opencode: {
    saveHeadless: save("opencode"),
    list: () => [bins.opencode(), ["session", "list"]],
    resume: (home, base, text) => withFlags(save("opencode")(home, base, text), ["--continue"]),
    toolHeadless: (home, base, text) => lanes.opencode.headless(home, base, text),
    store: (home) => home + "/.local/share/opencode",
  },
};
// BENCH_NC_VARIANTS=ncdev=/path/to/nanocode,ncnew=/other/nanocode adds lanes that run the nc lane with another binary.
for (const pair of (process.env.BENCH_NC_VARIANTS ?? "").split(",").filter(Boolean)) {
  const [name, bin] = pair.split("=");
  const swap = (spec) => (spec[0] === bins.nc() ? [bin, spec[1]] : spec);
  lanes[name] = { ...lanes.nc, verified: "variant of nc: " + bin, version: () => swap(lanes.nc.version()), headless: (...a) => swap(lanes.nc.headless(...a)), tui: (...a) => { const t = lanes.nc.tui(...a); return { ...t, cmd: t.cmd[0] === bins.nc() ? [bin, ...t.cmd.slice(1)] : t.cmd }; } };
  process.env["BENCH_INSTALL_" + name.toUpperCase()] ??= bin;
}
for (const name of Object.keys(lanes)) if (!extra[name] && name.startsWith("nc")) extra[name] = { ...extra.nc, saveHeadless: save(name), toolHeadless: (home, base, text) => lanes[name].headless(home, base, text), resume: (home, base, text) => withFlags(save(name)(home, base, text), ["--continue"]), list: (home) => [lanes[name].version()[0], ["sessions", "--home", home + "/.nanocode"]] };
