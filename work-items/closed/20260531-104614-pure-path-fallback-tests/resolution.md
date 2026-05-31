Made `manifest::paths` fallback tests independent from process-global environment mutation.

Implementation:
- Added narrow private per-key fallback helpers for config/data/runtime path resolution.
- Public path functions still read the same env vars and pass them in the same precedence order.
- Empty path env values still resolve as unset.
- Fallback-order tests now pass direct `Option<PathBuf>` inputs instead of using `std::env::set_var` / `remove_var`.
- No `PathEnv`, `EnvSnapshot`, env trait, test-support crate, or new env var surface was introduced.
- Credential env behavior and Pod runtime/process behavior were not changed.

Review:
- External reviewer `pure-path-fallback-reviewer-20260531` approved implementation commit `e232f54`.

Validation after merge:
- `cargo fmt --check`
- `cargo test -p manifest paths`
- `cargo test -p manifest`
- `cargo check -p manifest` (passed with unrelated existing `llm-worker` dead_code warning)
- `./tickets.sh doctor`
- `git diff --check`
- `git grep -n -E 'set_var\(|remove_var\(' -- crates/manifest/src/paths.rs || true` produced no matches.
