import { execFileSync } from "node:child_process";
import { relative } from "node:path";
import { fileURLToPath } from "node:url";

const root = fileURLToPath(new URL("../", import.meta.url));
const metadata = JSON.parse(execFileSync("cargo", [
  "metadata", "--offline", "--locked", "--format-version", "1", "--no-deps",
], { cwd: root, encoding: "utf8" }));
const packages = new Map(metadata.packages.map(pkg => [pkg.name, pkg]));
const layer = name => relative(root, packages.get(name).manifest_path).split("/").slice(0, 2).join("/");
const provider = name => relative(root, packages.get(name).manifest_path).split("/").slice(0, 3).join("/");
const forbidden = {
  "crates/core": ["crates/runtime", "crates/backends", "crates/integrations", "crates/nanocode"],
  "crates/runtime": ["crates/backends", "crates/integrations", "crates/nanocode"],
  "crates/integrations": ["crates/backends", "crates/nanocode"],
  "crates/backends": ["crates/nanocode"],
  "crates/tui": ["crates/core", "crates/runtime", "crates/backends", "crates/integrations", "crates/nanocode"],
};
const forbiddenDependencies = {
  "nanocode-image": new Set(["nanocode-types"]),
  "nanocode-code-mode": new Set(["nanocode-types"]),
  "nanocode-backend-claude-tools": new Set([
    "nanocode-backend-claude-client", "nanocode-agent-core", "nanocode-agent-types",
    "nanocode-agent-config", "nanocode-tool-host", "nanocode-code-mode", "nanocode-durability",
  ]),
  "nanocode-agent-types": new Set(["nanocode-agent-core", "nanocode-types", "nanocode-platform", "tokio", "futures-util"]),
  "nanocode-types-core": new Set(["tokio", "nanocode-platform", "futures-util"]),
  "nanocode-durability-core": new Set(["nanocode-agent-core", "nanocode-types", "nanocode-types-core"]),
  "nanocode-tool-contracts": new Set([
    "nanocode-tools", "nanocode-tools-macros", "schemars", "proc-macro-crate", "async-trait",
  ]),
  "nanocode-store-jsonl": new Set(["rusqlite", "libsqlite3-sys", "nanocode-store-sql", "nanocode-durability"]),
  "nanocode-store-sql": new Set(["nanocode-durability"]),
  "nanocode-agent-config": new Set(["nanocode-agent", "nanocode-subagents"]),
  "nanocode-backend-codex-client": new Set([
    "nanocode-agent-core",
    "nanocode-agent-config", "nanocode-tool-host", "nanocode-code-mode", "nanocode-durability",
  ]),
  "nanocode-backend-claude-client": new Set([
    "nanocode-agent-core", "nanocode-backend-claude-tools",
    "nanocode-agent-config", "nanocode-tool-host", "nanocode-code-mode", "nanocode-durability",
  ]),
};
const allowedDependencies = {
  "nanocode-tui": new Set(["nanocode-sync", "nanocode-sys", "nanocode-platform", "nanocode-digest"]),
};
const backendImplementations = new Set([
  "nanocode-tool-runtime", "nanocode-mcp-tools", "nanocode-mcp-http",
  "nanocode-agent", "nanocode-subagents",
]);

let checked = 0;
for (const name of packages.keys()) {
  for (const dep of packages.get(name).dependencies) {
    if (dep.kind !== "dev" && (dep.name === "symphonia" || dep.name.startsWith("symphonia-"))) {
      throw new Error(`Removed audio dependency: ${name} → ${dep.name}`);
    }
  }
  const banned = forbidden[layer(name)];
  if (!banned) continue;
  checked += 1;
  const visited = new Set();
  const pending = [[name]];
  while (pending.length) {
    const path = pending.pop();
    const current = path.at(-1);
    if (banned.includes(layer(current)) && !allowedDependencies[name]?.has(current)) {
      throw new Error(`${layer(name)} crate depends on ${layer(current)}: ${path.join(" → ")}`);
    }
    if (layer(name) === "crates/backends" && backendImplementations.has(current)) {
      throw new Error(`Backend depends on host implementation: ${path.join(" → ")}`);
    }
    if (layer(name) === "crates/backends" && layer(current) === "crates/backends" && provider(current) !== provider(name)) {
      throw new Error(`Backend depends on another backend: ${path.join(" → ")}`);
    }
    if (visited.has(current)) continue;
    visited.add(current);
    for (const dep of packages.get(current).dependencies) {
      if (dep.kind === "dev") continue;
      if (forbiddenDependencies[name]?.has(dep.name)) {
        throw new Error(`Forbidden dependency: ${[...path, dep.name].join(" → ")}`);
      }
      if (packages.has(dep.name)) pending.push([...path, dep.name]);
    }
  }
}
console.log(`Layering verified for ${checked} core, runtime, integration and tui crates.`);
