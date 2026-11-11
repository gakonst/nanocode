import fs from "node:fs";
import { NODE, baseEnv, bins } from "./common.mjs";

// One entry per CLI. `verified` says what ran end to end against the fixture and when, `flags` every non-default setting.
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
const ncliteArgs = (home, base) => ["--home", home + "/.nc-lite", "--base-url", base + "/v1", "-m", "luna"];
const ncArgs = (home, base) => ["--home", home + "/.nanocode", "--gateway", "vercel", "--gateway-base-url", base + "/v1", "-m", "luna"];

function claudeSetup(home, cwd) {
  fs.mkdirSync(home + "/.claude", { recursive: true });
  fs.writeFileSync(home + "/.claude/.claude.json", JSON.stringify({ hasCompletedOnboarding: true, bypassPermissionsModeAccepted: true, customApiKeyResponses: { approved: ["fixture-key".slice(-20)], rejected: [] }, projects: { [fs.realpathSync(cwd)]: { hasTrustDialogAccepted: true, hasCompletedProjectOnboarding: true } } }));
  fs.writeFileSync(home + "/.claude/settings.json", JSON.stringify({ theme: "dark" }));
}
const claudeEnv = (home, base) => ({ ...baseEnv(home), ANTHROPIC_BASE_URL: base + "/sa", ANTHROPIC_API_KEY: "fixture-key", CLAUDE_CONFIG_DIR: home + "/.claude", DISABLE_TELEMETRY: "1", DISABLE_AUTOUPDATER: "1", DISABLE_ERROR_REPORTING: "1", CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC: "1", DISABLE_UPDATES: "1" });

function codexSetup(home) {
  fs.mkdirSync(home + "/.codex", { recursive: true });
  fs.writeFileSync(home + "/.codex/config.toml", "check_for_update_on_startup = false\n\n[analytics]\nenabled = false\n\n[feedback]\nenabled = false\n");
}
const codexEnv = (home) => ({ ...baseEnv(home), CODEX_HOME: home + "/.codex", BENCH_KEY: "fixture-key" });
const codexArgs = (base) => ["-c", 'model_provider="bench"', "-c", 'model_providers.bench={name="bench",base_url="' + base + '/sa/v1",wire_api="responses",env_key="BENCH_KEY"}', "-m", "bench/model"];

function opencodeSetup(home, base) {
  fs.mkdirSync(home + "/.config/opencode", { recursive: true });
  fs.writeFileSync(home + "/.config/opencode/opencode.json", JSON.stringify({ $schema: "https://opencode.ai/config.json", autoupdate: false, share: "disabled", model: "bench/model", small_model: "bench/model", provider: { bench: { npm: "@ai-sdk/openai-compatible", name: "bench", options: { baseURL: base + "/sa/v1", apiKey: "fixture-key" }, models: { model: { name: "Bench" } } } } }));
}

const opencodeEnv = (home) => ({ ...baseEnv(home), OPENCODE_DISABLE_AUTOUPDATE: "1", OPENCODE_DISABLE_MODELS_FETCH: "1", OPENCODE_DISABLE_LSP_DOWNLOAD: "1" });

export const lanes = {
  fx: {
    verified: "2026-11-10 (0.0.16, Linux x86_64): headless and terminal smoke pass, one stream request, no side requests; netaudit sees no outbound traffic",
    flags: "ask --yolo --no-save (headless); settings.json: provider gateway, max_agent_steps 0, session_titles false; env AI_GATEWAY_API_KEY, FX_GATEWAY_BASE_URL and FX_E2E_* point the gateway at the fixture, FX_SOUND=0",
    version: () => [bins.fx(), ["--version"]],
    setup: (home) => fxSetup(home),
    env: (home, base) => fxEnv(home, base),
    headless: (home, base, text, { save = false } = {}) => [bins.fx(), ["ask", "--yolo", ...(save ? [] : ["--no-save"]), text]],
    tui: () => ({ cmd: [bins.fx()], marker: "Run /help", exit: ["\x04"] }),
    store: (home) => home + "/.fx",
  },
  pi: {
    verified: "2026-11-10 (1.5.2, Linux x86_64): headless (-p --mode json, since plain -p prints only at the end) and terminal smoke pass, one stream request; netaudit sees no outbound traffic",
    flags: "--provider bench --model bench/model, --no-session; -p --mode json (headless); models.json adds the fixture provider; settings.json quietStartup; PI_OFFLINE=1",
    version: () => [NODE, [bins.pi(), "--version"]],
    setup: (home, base) => piSetup(home, base),
    env: (home) => piEnv(home),
    headless: (home, base, text, { save = false } = {}) => [NODE, [bins.pi(), "-p", "--mode", "json", ...(save ? [] : ["--no-session"]), ...piArgs, text]],
    tui: () => ({ cmd: [NODE, bins.pi(), ...piArgs, "--no-session"], marker: "bench/model", exit: ["\x03", "\x03"] }),
    store: (home) => home + "/.pi",
  },
  nc: {
    verified: "2026-11-10 (0.7.0 release, Linux x86_64): headless and terminal smoke pass, one stream request, no side requests. By default it connects to its Tempo tools (mercator.sh, gateway.tempo-dev.workers.dev MCP); that stays on (a default feature) and tools/netguard.sh blocks it",
    flags: "--home, --gateway vercel --gateway-base-url <fixture>/v1, -m luna, --ephemeral; exec (headless); NANOCODE_TELEMETRY=off",
    version: () => [bins.nc(), ["--version"]],
    setup: () => undefined,
    env: (home) => ncEnv(home),
    headless: (home, base, text, { save = false } = {}) => [bins.nc(), ["exec", ...ncArgs(home, base), ...(save ? [] : ["--ephemeral"]), text]],
    tui: (home, base) => ({ cmd: [bins.nc(), ...ncArgs(home, base), "--ephemeral"], marker: "Ask anything", exit: ["\x03", "\x03"] }),
    store: (home) => home + "/.nanocode",
  },
  nclite: {
    verified: "2026-11-11 (nc_lite example on the SDK, Linux x86_64): exec, saved sessions, --continue, sessions and a 50-step shell turn pass against the fixture, one stream request, no side requests. nanocode cut down to what fx ships: one harness (Codex), plain function tools (ToolExposure::DirectOnly, no Code Mode), any model through the AI Gateway, a JSONL store, no TUI, no MCP, no computer use, no runners, web search and image generation off",
    flags: "exec --home, --base-url <fixture>/v1, -m luna, --ephemeral; AI_GATEWAY_API_KEY",
    version: () => [bins.nclite(), ["--version"]],
    setup: () => undefined,
    env: (home) => ({ ...baseEnv(home), AI_GATEWAY_API_KEY: "fixture-key" }),
    headless: (home, base, text, { save = false } = {}) => [bins.nclite(), ["exec", ...ncliteArgs(home, base), ...(save ? [] : ["--ephemeral"]), text]],
    tui: null,
    store: (home) => home + "/.nc-lite",
  },
  claude: {
    verified: "2026-11-10 (2.1.322, Linux x86_64): headless and terminal smoke pass, one stream request, no side requests. The config is read from CLAUDE_CONFIG_DIR/.claude.json (onboarding and trust are pre-accepted there); the Auto mode notice is informational and does not block input. The marker is the prompt glyph, since the header is drawn with cursor moves. With the env below netaudit sees no outbound traffic except the official plugin marketplace clone (github.com), a default feature left on",
    flags: "-p --output-format stream-json --include-partial-messages --verbose (headless); env ANTHROPIC_BASE_URL, ANTHROPIC_API_KEY, CLAUDE_CONFIG_DIR, DISABLE_TELEMETRY, DISABLE_AUTOUPDATER, DISABLE_UPDATES, DISABLE_ERROR_REPORTING, CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC; .claude.json onboarding, trust and API key approval; settings.json theme dark",
    version: () => [bins.claude(), ["--version"]],
    setup: (home, base, cwd) => claudeSetup(home, cwd),
    env: (home, base) => claudeEnv(home, base),
    headless: (home, base, text) => [bins.claude(), ["-p", "--output-format", "stream-json", "--include-partial-messages", "--verbose", text]],
    tui: () => ({ cmd: [bins.claude()], marker: "❯", exit: ["\x03", "\x03"] }),
    store: (home) => home + "/.claude",
  },
  codex: {
    verified: "2026-11-10 (0.167.0, Linux x86_64): headless and terminal smoke pass, one stream request plus one side request (a thread title). No headless mode streams: exec prints the reply at the end both as text and with --json (item.completed only, no deltas); only app-server (JSON-RPC) emits deltas. The TUI needs about 150 ms between typing the prompt and Enter, or its paste-burst detection swallows the Enter (tui.py enter_gap_ms). config.toml turns off the update check, analytics and feedback; it still fetches announcement_tip.toml from raw.githubusercontent.com at TUI start (unconditional) and, headless too, syncs curated plugins from github.com, api.github.com and chatgpt.com (a default feature left on); tools/netguard.sh blocks both",
    flags: "-c model_provider=bench, -c model_providers.bench={base_url <fixture>/sa/v1, wire_api responses, env_key BENCH_KEY}, -m bench/model; exec --skip-git-repo-check --json (headless); config.toml check_for_update_on_startup=false, analytics.enabled=false, feedback.enabled=false; CODEX_HOME",
    version: () => [bins.codex(), ["--version"]],
    setup: (home) => codexSetup(home),
    env: (home) => codexEnv(home),
    headless: (home, base, text) => [bins.codex(), ["exec", "--skip-git-repo-check", "--json", ...codexArgs(base), text]],
    tui: (home, base) => ({ cmd: [bins.codex(), ...codexArgs(base)], marker: "bench/model", exit: ["\x03", "\x03"] }),
    store: (home) => home + "/.codex",
  },
  opencode: {
    verified: "2026-11-10 (1.19.5, Linux x86_64): headless and terminal smoke pass, one stream request plus one side request (title, via small_model). The env below stops the models.dev fetch and update check; it still resolves registry.npmjs.org at start, which tools/netguard.sh blocks. Its terminal interface takes about 3.4 s to first frame, also under netguard",
    flags: "run -m bench/model (headless), -m bench/model (TUI); opencode.json: autoupdate false, share disabled, model and small_model bench/model, an openai-compatible provider at <fixture>/sa/v1; env OPENCODE_DISABLE_AUTOUPDATE, OPENCODE_DISABLE_MODELS_FETCH, OPENCODE_DISABLE_LSP_DOWNLOAD",
    version: () => [bins.opencode(), ["--version"]],
    setup: (home, base) => opencodeSetup(home, base),
    env: (home) => opencodeEnv(home),
    headless: (home, base, text) => [bins.opencode(), ["run", "-m", "bench/model", text]],
    tui: () => ({ cmd: [bins.opencode(), "-m", "bench/model"], marker: "bench", exit: ["\x03", "\x03"] }),
    store: (home) => home + "/.local/share/opencode",
  },
};

export const selected = (only) => (only ? only.split(",") : Object.keys(lanes)).filter((name) => {
  if (!lanes[name]) throw new Error("unknown lane " + name);
  return true;
});
