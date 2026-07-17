Implemented reusable Worker-internal runner and migrated memory extract to use it.

The new runner provides a Worker-owned path for isolated internal LLM jobs with caller-supplied prompt, input, limited tools, cache key, max turns, and usage capture. Memory extract now uses this runner while preserving the existing trigger and staging behavior.

Validation passed:
- `cargo fmt --check`
- `cargo test -p memory`
- `cargo test -p worker`
- `nix build .#yoi`
