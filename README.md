<h1 align="center">nanocode</h1>

<p align="center">
  <a href="https://nanocode.xyz">Site</a> · <a href="https://nanocode.xyz/docs/introduction/getting-started">Docs</a> · <a href="https://nanocode.xyz/experiments">Experiments</a> · <a href="https://nanocode.xyz/cloudflare">Cloudflare Demo</a> · <a href="https://nanocode.xyz/vercel">Vercel Demo</a>
</p>

SDK for frontier embedded agents, in Rust. One runtime for the Codex and Claude harnesses, with their own prompts and tools, that runs in a terminal, a server, a Worker, a browser tab, or an app.

<p align="center">
  <img alt="nc in a terminal: a question about this workspace, one Code Mode cell that reads it, and the answer after 10.5 seconds" src="assets/nc.gif" width="100%" />
</p>

- **Durable.** Every model call and tool call is journaled, so a turn finishes after its process dies.
- **Code Mode.** The model writes one program that calls every tool, and only its result enters the context.
- **Hands.** No sandbox per session. A turn reaches a laptop, a server, or a browser tab only for the calls that need one.
- **Subagents.** A cell spawns children, each a durable session of its own.
- **Any payer.** A ChatGPT or Claude subscription, an API key, or Machine Payments from a Tempo Wallet.

## The CLI

```sh
curl -fsSL https://nanocode.xyz/install | sh
nc
```

## The SDK

```toml
[dependencies]
nanocode = { git = "https://github.com/nanocodexyz/nanocode" }
```

The same code runs in other languages:

| Language | Repository |
| --- | --- |
| TypeScript, as one WebAssembly module | [nanocode-ts](https://github.com/nanocodexyz/nanocode-ts) |
| Swift, for macOS and iOS | [nanocode-swift](https://github.com/nanocodexyz/nanocode-swift) |

[The crate layout](crates/README.md) describes the workspace, and `DECISIONS.md` records what each change established.

## License

[MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE).
