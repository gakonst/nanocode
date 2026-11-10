import fs from "node:fs";
import { NODE, baseEnv, bins } from "./common.mjs";

// One entry per CLI. `verified` says what ran end to end against the fixture and when; README.md has the detail.
// Smoke every lane (harness/smoke.mjs) before measuring it.
// A lane whose requests include side requests (titles, token counts) uses the fixture's /sa prefix, which answers
// a request without tools with a short reply instead of the scripted stream.

function fxSetup(home) {
  fs.mkdirSync(home + "/.fx", { recursive: true, mode: 0o700 });
  fs.writeFileSync(home + "/.fx/settings.json", JSON.stringify({ provider: "gateway", model: "openai/gpt-5.5", max_agent_steps: 0, session_titles: false }));
}
const fxEnv = (home, base) => ({ ...baseEnv(home), AI_GATEWAY_API_KEY: "fixture-key", FX_GATEWAY_BASE_URL: base, FX_E2E_GATEWAY_CHAT_URL: base + "/chat", FX_E2E_GATEWAY_MODELS_URL: base + "/coding-agent/v1/models", FX_MODEL: "openai/gpt-5.5", FX_MAX_AGENT_STEPS: "0", FX_SOUND: "0" });

function piSetup(home, base) {
  const agent = home + "/.pi/agent";
  fs.mkdirSync(agent, { recursive: true });
  fs.writeFileSync(agent + "/models.json", JSON.stringify({ providers: { bench: { baseUrl: base + "/v1", api: "openai-responses", apiKey: "fixture-key", models: [{ id: "bench/model", name: "Bench", reasoning: false, input: ["text"], contextWindow: 1000000, maxTokens: 32000, cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 } }] } } }));
  fs.writeFileSync(agent + "/settings.json", JSON.stringify({ defaultProvider: "bench", defaultModel: "bench/model", quietStartup: true }));
}
const piEnv = (home) => ({ ...baseEnv(home), PI_CODING_AGENT_DIR: home + "/.pi/agent", PI_OFFLINE: "1" });
const piArgs = ["--provider", "bench", "--model", "bench/model"];

const ncEnv = (home) => ({ ...baseEnv(home), AI_GATEWAY_API_KEY: "fixture-key", OPENROUTER_API_KEY: "fixture-key", NANOCODE_TELEMETRY: "off" });
const ncArgs = (home, base) => ["--home", home + "/.nanocode", "--gateway", "vercel", "--gateway-base-url", base + "/v1", "-m", "luna"];

function claudeSetup(home, cwd) {
  fs.mkdirSync(home + "/.claude", { recursive: true });
  fs.writeFileSync(home + "/.claude/.claude.json", JSON.stringify({ hasCompletedOnboarding: true, bypassPermissionsModeAccepted: true, customApiKeyResponses: { approved: ["fixture-key".slice(-20)], rejected: [] }, projects: { [fs.realpathSync(cwd)]: { hasTrustDialogAccepted: true, hasCompletedProjectOnboarding: true } } }));
  fs.writeFileSync(home + "/.claude/settings.json", JSON.stringify({ theme: "dark" }));
}
const claudeEnv = (home, base) => ({ ...baseEnv(home), ANTHROPIC_BASE_URL: base + "/sa", ANTHROPIC_API_KEY: "fixture-key", CLAUDE_CONFIG_DIR: home + "/.claude", DISABLE_TELEMETRY: "1", DISABLE_AUTOUPDATER: "1", DISABLE_ERROR_REPORTING: "1", CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC: "1" });

const codexEnv = (home) => ({ ...baseEnv(home), CODEX_HOME: home + "/.codex", BENCH_KEY: "fixture-key" });
const codexArgs = (base) => ["-c", 'model_provider="bench"', "-c", 'model_providers.bench={name="bench",base_url="' + base + '/sa/v1",wire_api="responses",env_key="BENCH_KEY"}', "-m", "bench/model"];

function opencodeSetup(home, base) {
  fs.mkdirSync(home + "/.config/opencode", { recursive: true });
  fs.writeFileSync(home + "/.config/opencode/opencode.json", JSON.stringify({ $schema: "https://opencode.ai/config.json", autoupdate: false, share: "disabled", model: "bench/model", small_model: "bench/model", provider: { bench: { npm: "@ai-sdk/openai-compatible", name: "bench", options: { baseURL: base + "/sa/v1", apiKey: "fixture-key" }, models: { model: { name: "Bench" } } } } }));
}

export const lanes = {
  fx: {
    verified: "2026-10-28 (0.0.15), smoked again 2026-11-09",
    version: () => [bins.fx(), ["--version"]],
    setup: (home) => fxSetup(home),
    env: (home, base) => fxEnv(home, base),
    headless: (home, base, text, { save = false } = {}) => [bins.fx(), ["ask", "--yolo", ...(save ? [] : ["--no-save"]), text]],
    tui: () => ({ cmd: [bins.fx()], marker: "Run /help", exit: ["\x04"] }),
    store: (home) => home + "/.fx",
  },
  pi: {
    verified: "2026-10-28 (1.4.0); not smoked on 2026-11-09",
    version: () => [NODE, [bins.pi(), "--version"]],
    setup: (home, base) => piSetup(home, base),
    env: (home) => piEnv(home),
    headless: (home, base, text, { save = false } = {}) => [NODE, [bins.pi(), "-p", "--mode", "json", ...(save ? [] : ["--no-session"]), ...piArgs, text]],
    tui: () => ({ cmd: [NODE, bins.pi(), ...piArgs, "--no-session"], marker: "bench/model", exit: ["\x03", "\x03"] }),
    store: (home) => home + "/.pi",
  },
  nc: {
    verified: "2026-10-28, smoked again 2026-11-09 (0.7.0)",
    version: () => [bins.nc(), ["--version"]],
    setup: () => undefined,
    env: (home) => ncEnv(home),
    headless: (home, base, text, { save = false } = {}) => [bins.nc(), ["exec", ...ncArgs(home, base), ...(save ? [] : ["--ephemeral"]), text]],
    tui: (home, base) => ({ cmd: [bins.nc(), ...ncArgs(home, base), "--ephemeral"], marker: "Ask anything", exit: ["\x03", "\x03"] }),
    store: (home) => home + "/.nanocode",
  },
  claude: {
    verified: "headless 2026-11-09 (2.1.316); the terminal interface reached its prompt, a streamed turn in it has not run",
    version: () => [bins.claude(), ["--version"]],
    setup: (home, base, cwd) => claudeSetup(home, cwd),
    env: (home, base) => claudeEnv(home, base),
    headless: (home, base, text) => [bins.claude(), ["-p", "--output-format", "stream-json", "--include-partial-messages", "--verbose", text]],
    tui: () => ({ cmd: [bins.claude()], marker: "Claude Code v", exit: ["\x03", "\x03"] }),
    store: (home) => home + "/.claude",
  },
  codex: {
    verified: "headless 2026-11-09 (0.162.0-alpha.17.2); the terminal interface drew a frame and sent no request",
    version: () => [bins.codex(), ["--version"]],
    setup: (home) => { fs.mkdirSync(home + "/.codex", { recursive: true }); },
    env: (home) => codexEnv(home),
    headless: (home, base, text) => [bins.codex(), ["exec", "--skip-git-repo-check", "--json", ...codexArgs(base), text]],
    tui: (home, base) => ({ cmd: [bins.codex(), ...codexArgs(base)], marker: "bench/model", exit: ["\x03", "\x03"] }),
    store: (home) => home + "/.codex",
  },
  opencode: {
    verified: false,
    version: () => [bins.opencode(), ["--version"]],
    setup: (home, base) => opencodeSetup(home, base),
    env: (home) => baseEnv(home),
    headless: (home, base, text) => [bins.opencode(), ["run", "-m", "bench/model", text]],
    tui: () => ({ cmd: [bins.opencode(), "-m", "bench/model"], marker: "bench", exit: ["\x03", "\x03"] }),
    store: (home) => home + "/.local/share/opencode",
  },
};

export const selected = (only) => (only ? only.split(",") : Object.keys(lanes)).filter((name) => {
  if (!lanes[name]) throw new Error("unknown lane " + name);
  return true;
});
