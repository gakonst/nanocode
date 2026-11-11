# Crate layout

| Directory | Responsibility |
| --- | --- |
| `nanocode/` | The public facade: re-exports and builders |
| `core/` | Values, contracts and plumbing every layer shares |
| `runtime/` | Backend-generic orchestration: agents, subagents, Code Mode, durable sessions, tool selection |
| `backends/<provider>/` | One tree per provider: `client` (wire protocol, authentication, transport), Claude's `tools`, and the driver crate (agent, prompts, tool assembly, durability format) |
| `integrations/` | Native host adapters: shells and processes, computer use, browsers, sandboxing, MCP, remote hands and runners, stores, telemetry |
| `tui/` | Terminal UI components |
| `../bin/nanocode/` | The `nanocode` / `nc` executable |
| `../bin/cua-driver/` | The macOS computer-use helper |

`scripts/check-dependency-boundaries.mjs` enforces the layering: `core/` depends on nothing above it, `runtime/` only on `core/`, `integrations/` on neither `backends/` nor the facade, and a backend on no other backend and no host implementation. Provider `client` crates stay free of runtime, tool-host, Code Mode and durability dependencies, and `nanocode-types-core` and `nanocode-agent-types` stay free of Tokio.

## core

| Crate | Owns |
| --- | --- |
| `nanocode-types-core` | SDK values and event wire formats |
| `nanocode-types` | Event channels, over `nanocode-types-core` |
| `nanocode-agent-types` | Model, usage and error values |
| `nanocode-agent-core` | Agent lifecycle, handles and session contracts |
| `nanocode-durability-core` | `StateStore`, content-addressed payloads, the deterministic state machine, `SqlStore`, `RowStore`, `StagedStore`, `BlobStore` |
| `nanocode-tool-contracts`, `nanocode-tool-provider` | Tool invocation traits and dynamic tool providers |
| `nanocode-tools`, `nanocode-tools-macros` | The `#[tool]` macro and the crate its expansion names |
| `nanocode-process-contracts` | Portable shell and file host interfaces, `Redactor` |
| `nanocode-platform` | The host interface WASM embedders install |
| `nanocode-sync` | Lock-free primitives: `Atom`, `Mailbox`, `Actor`, `Sequencer`, `Slot`, `Signal`, ... |
| `nanocode-machine` | `machine!`: a lifecycle declared once as states, wire names and moves, with its stored-state check and fence text |
| `nanocode-http`, `nanocode-zstd`, `nanocode-tls` | HTTP layer contracts and retry rules, a streaming zstd decoder, cached TLS configuration |
| `nanocode-mcp`, `nanocode-mcp-http` | The MCP client: portable session engine; native Streamable HTTP and OAuth |
| `nanocode-schema` | JSON Schema validation for output contracts |
| `nanocode-home` | The Nanocode home, AGENTS.md and skill discovery, shell detection, user services |
| `nanocode-image`, `nanocode-audio` | Prompt image preparation; audio token accounting |

## runtime

| Crate | Owns |
| --- | --- |
| `nanocode-agent` | `AgentRuntime`, `TurnInbox`, `SessionLog`, session event batching |
| `nanocode-agent-config` | Assembly interfaces, so backends do not depend on `nanocode-agent` |
| `nanocode-subagents` | The task tree: spawning, waiting, messages, residency, completions, budgets, the journal |
| `nanocode-code-mode` | `exec` and `wait`, cell orchestration and replay, the remote hand protocol |
| `nanocode-code-mode-engine` | The QuickJS cell worker pool (native) and stepped execution (WASM) |
| `nanocode-durability` | Session drivers, the `SessionStore` catalog, `SharedStore`, `MemoryStore`, branches, the record cache |
| `nanocode-tool-host` | The `Tools` builder, tool selection, admission, `FunctionTool`, description pins |

## backends

| Crate | Owns |
| --- | --- |
| `nanocode-backend-claude-client` | The Messages wire format, authentication, the subscription wire, streaming, message threads |
| `nanocode-backend-claude-tools` | Claude's tool definitions and handlers |
| `nanocode-backend-claude` | The Claude driver, its pinned upstream prompts and tools, durability format |
| `nanocode-backend-codex-client` | The Responses protocol, ChatGPT and API authentication, WebSocket and HTTPS transports, gateways, pricing |
| `nanocode-backend-codex` | The Codex driver, built-in tools, compaction, durability format |

## integrations

| Crate | Owns |
| --- | --- |
| `nanocode-agent-host` | Native defaults: durable process routing, runner lifecycle, computer use |
| `nanocode-tool-runtime` | Host tool registration, workspace execution, MCP, hands and attachment runners |
| `nanocode-process` | Shell sessions, PTYs, the zsh shell server, warm bash processes |
| `nanocode-mcp-tools` | MCP discovery, catalogs, prepared tools, stdio processes |
| `nanocode-computer` | The `computer` tool over macOS accessibility, Linux AT-SPI, cua-driver and MCP providers; the `cua_repl` tools and browser extension host |
| `nanocode-chatgpt-extension` | Browser tabs through the official ChatGPT extension's native-messaging host |
| `nanocode-sandbox` | Commands confined to read-only paths, one writable directory and an allowlisting egress proxy |
| `nanocode-store-jsonl` | The append-only JSONL store, one file per session: the CLI's default |
| `nanocode-store-sql` | `Sql` hosts for SQLite (`SqliteSql`), Postgres (`PostgresSql`) and a host's own sqlx pool or transaction (`SqlxSql`, `SqlxTransaction`, `commit_staged`) |
| `nanocode-observability` | Tracing and OpenTelemetry setup |
| `nanocode-telemetry` | GenAI span export, held request traces, and events as log lines and counters, for a server host's own subscriber |

## Durability

Each backend implements `DurableAgentExt`, `DurableBackendRegistration` and a `CheckpointFormat` in its own `durability` module, and the composed runtime registers them on the store it is given. Native hosts default to `nanocode-store-jsonl` in the Nanocode home. `SqlStore`, `RowStore` and `SessionLog` run over any `Sql` host: `nanocode-store-sql` natively, a host's own SQL (Durable Objects, D1, libSQL, sqlite-wasm) on WASM. Memory is the WASM default and is not durable.

## Composition

| Host | Crates |
| --- | --- |
| Native TUI or CLI | `nanocode`, `nanocode-tui`, `nanocode-observability` |
| Native server | `nanocode`, `nanocode-store-sql`, `nanocode-observability` |
| WASM brain | `nanocode` with `default-features = false` (no QuickJS) and a host `Sql` or `StateStore` |
| WASM with local cells | `nanocode` with `local-cells`, built against wasi-sdk |

## WASM

Every reusable crate builds for native and `wasm32-unknown-unknown`. There is no packaged WASM SDK and no wasm-bindgen ABI; bindings live in other repositories. A WASM embedding installs one `Rc<dyn nanocode_platform::Host>` per execution thread, which schedules local futures, implements cancellable sleeps and supplies clocks and secure random bytes. Codex networking goes through `HostTransport` and Claude's through `HttpTransport`; native terminal, filesystem, database, process and telemetry adapters are gated out.

## Replacements and patches

- `nanocode-schema` is an adaptation of Boon (JSON Schema drafts 4 through 2020-12), in place of jsonschema; see [its README](core/schema/README.md).
- `nanocode-mcp` replaces rmcp and oauth2; see [its README](core/mcp/README.md). MCP tool search uses its own BM25 index; see [the scoring contract](integrations/mcp-tools/src/SEARCH.md).
- `nanocode-zstd` decodes zstd responses without a C library.
- `patches/` holds rustls-platform-verifier and iana-time-zone, patched so macOS builds link neither Security.framework nor CoreFoundation.
