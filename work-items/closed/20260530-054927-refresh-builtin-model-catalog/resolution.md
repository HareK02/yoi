Refreshed the builtin model catalog from recorded official/semiofficial sources. Anthropic, OpenAI/Codex OAuth, OpenRouter, and Ollama entries now point at current concrete model IDs; default profile remains `codex-oauth/gpt-5.5`; provider definitions were unchanged.

External review approved and validation passed:

- `cargo fmt --check`
- `cargo test -p provider`
- `cargo test -p manifest model`
- `cargo test -p manifest profile -- --nocapture`
- `cargo check -p provider -p manifest`
- `./tickets.sh doctor`
- `git diff --check`
