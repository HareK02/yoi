# agen-macros

Procedural macros used by [`agen`](https://crates.io/crates/agen) to declare LLM tools from Rust methods.

Applications should normally depend only on `agen` and import its re-exports:

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
```

`#[tool_registry]` generates the argument schema, a `Tool` implementation, and an `<method>_definition` constructor. It rejects arguments of its own, duplicate `#[tool]` markers, malformed or duplicate `#[description = "..."]` attributes, and non-identifier argument patterns.

Generated code targets the canonical `::agen` path and uses implementation dependencies re-exported by `agen`; consumers do not need direct `serde`, `schemars`, `serde_json`, or `async-trait` dependencies. Renaming the `agen` dependency in `Cargo.toml` is not currently supported.

This companion package is published before the matching `agen` release. Its public contract is the generated API consumed by `agen`, and its minor version compatibility follows the `agen` 0.2 series.

Licensed under the [MIT License](https://gitea.hareworks.net/Hare/yoi/src/branch/develop/LICENSE).
