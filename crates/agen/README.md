# agen

`agen` is a provider-neutral Rust engine for streaming LLM applications that use tools. It owns the turn loop, typed conversation history, provider wire-format adapters, tool execution, interceptors, usage accounting, and cache-aware state transitions.

> `agen` is pre-1.0. Public APIs may change between minor releases.

## Installation

```toml
[dependencies]
agen = "0.2.1"
```

The default feature set is intentionally empty. Enable the experimental Codex/ChatGPT authentication adapter when needed:

```toml
agen = { version = "0.2.1", features = ["codex"] }
```

`agen` requires Rust 1.86 or newer. The companion `agen-macros` package requires Rust 1.85 or newer.

## Quick start

Supply an implementation of [`LlmClient`](https://docs.rs/agen/latest/agen/llm_client/trait.LlmClient.html), keep conversation history in your application, then run a turn. The first call consumes the mutable engine and returns a cache-locked engine for later turns.

```no_run
use agen::{Engine, EngineError, History};
use agen::llm_client::LlmClient;

async fn conversation<C: LlmClient>(client: C) -> Result<(), EngineError> {
    let mut history = History::new();
    let output = Engine::new(client)
        .system_prompt("You are a concise assistant.")
        .run(&mut history, "Explain typed state in one sentence.")
        .await;

    let mut engine = output.engine;
    let _result = engine.run(&mut history, "Give a Rust example.").await;
    Ok(())
}
```

## Declaring tools

The tool macros are re-exported by `agen`; applications do not need direct dependencies on `serde`, `schemars`, `serde_json`, or `async-trait` for generated code.

```rust
use agen::tool_registry;

#[derive(Clone)]
struct Tools;

#[tool_registry]
impl Tools {
    /// Returns the supplied text.
    #[tool]
    async fn echo(
        &self,
        #[description = "Text to return"] text: String,
    ) -> Result<String, std::io::Error> {
        Ok(text)
    }
}

let definition = Tools.echo_definition();
assert_eq!(definition().0.name, "echo");
```

The generated API uses the canonical crate name `agen`. Renaming the `agen` dependency in `Cargo.toml` is not currently supported by these macros.

## Features

| Feature | Default | Adds |
|---|---:|---|
| `codex` | No | Experimental Codex/ChatGPT auth-file loading and token refresh support |

The base crate includes provider-neutral transport and Anthropic, OpenAI-compatible, Gemini, and Ollama wire-format schemes. See [`llm_client`](https://docs.rs/agen/latest/agen/llm_client/) for the client boundary.

## Architecture and API scope

The current public modules cover the engine, typed history, client transport/schemes, timeline events, tools, interceptors, pruning, token estimation, and usage records. Their relationships are described in [Architecture](https://gitea.hareworks.net/Hare/yoi/src/branch/develop/crates/agen/docs/architecture.md); behavioral requirements are summarized in [Requirements](https://gitea.hareworks.net/Hare/yoi/src/branch/develop/crates/agen/docs/requirements.md).

Low-level modules remain public in the 0.2 series because downstream Yoi components implement custom clients, event handlers, pruning policies, and tool registries against them. This surface is versioned as pre-1.0 API rather than declared stable.

## Packaging and security

The published package contains source, public documentation, curated examples, and deterministic tests/fixtures. Credentialed fixture-recording utilities are intentionally excluded. Examples that contact a provider read credentials from environment variables and never embed production credentials.

## License

Licensed under the [MIT License](https://gitea.hareworks.net/Hare/yoi/src/branch/develop/LICENSE).
