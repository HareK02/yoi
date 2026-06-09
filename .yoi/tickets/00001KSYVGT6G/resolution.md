Removed runtime filesystem resource discovery for bundled assets and removed `INSOMNIA_RESOURCE_DIR` as an active configuration surface.

Implementation:
- Removed `RESOURCE_DIR_ENV`, `resource_dir()`, and `builtin_profiles_dir()` from `manifest::paths`.
- Embedded builtin Lua profile source (`default.lua`) and exposed builtin profile metadata without fake filesystem paths.
- Preserved user/project registry profile and explicit path profile filesystem semantics.
- Replaced manifest-side builtin model context lookup with an embedded `resources/models/builtin.toml` source, avoiding a `manifest -> provider` dependency.
- Removed installed runtime `share/insomnia/resources` packaging and checks from Nix package output.
- Updated environment/Nix/pod-factory docs so runtime resource directory and `INSOMNIA_RESOURCE_DIR` are no longer supported/documented surfaces.

Review:
- External reviewer `remove-resource-dir-reviewer-20260531` approved implementation commit `365ec8b7fad008ab36bdc4de3adadb3696739a07`.
- Reviewer found no blockers. A suggested non-blocking future regression test for embedded profile local `require` diagnostics was recorded in the review thread.

Validation after merge:
- `cargo fmt --check`
- `cargo test -p manifest profile`
- `cargo test -p provider catalog`
- `cargo test -p pod prompt`
- `cargo check -p manifest -p provider -p pod -p tui`
- `./tickets.sh doctor`
- `git diff --check`
- `rg -n 'INSOMNIA_RESOURCE_DIR|RESOURCE_DIR_ENV|resource_dir\(|builtin_profiles_dir\(' crates docs package.nix || true` produced no active hits.
- `nix build .#insomnia`
- `test -x result/bin/insomnia`
- `test ! -e result/bin/insomnia-pod`
- `test ! -e result/share/insomnia/resources`
- `result/bin/insomnia pod --help`

Note:
- Initial post-merge validation hit `No space left on device` while linking. `cargo clean` freed build artifacts, after which Rust and Nix validation passed.
