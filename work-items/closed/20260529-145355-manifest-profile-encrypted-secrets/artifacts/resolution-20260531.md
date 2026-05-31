Implemented and merged local key-value secret store support.

Merged commits:

- `cc2c9a2 secrets: add local key store`
- `7ddf745 secrets: polish key manager and docs`
- `629159a merge: local secret store`

Review:

- Review approved in `c9e48b3 review: approve local secret store`.
- Focused follow-up review approved the docs example and key-manager terminal cleanup polish.

Summary:

- Added a provider-independent local `id -> value` secret store under the user data directory.
- Added id validation, atomic persistence, and lightweight at-rest obfuscation consistent with the ticket's modest security target.
- Added `insomnia keys` interactive TUI management for listing ids, setting values with masked display, deleting with confirmation, and quitting without displaying plaintext values.
- Wired provider `secret_ref` auth through the store.
- Added WebSearch `api_key_secret` and removed normal WebSearch/provider credential env configuration.
- Updated bundled resources and docs to point users to `insomnia keys` plus explicit secret refs.
- Left Codex OAuth behavior unchanged.

Validation after merge:

- `cargo fmt --check` — passed
- `cargo test -p secrets` — passed
- `cargo test -p manifest secret --lib` — passed
- `cargo test -p provider secret --lib` — passed
- `cargo test -p tools web::tests::search_requires_configuration --lib` — passed
- `cargo test -p tools web::tests::searches_brave_with_secret_ref --lib` — passed
- `cargo test -p tools web::tests::searches_brave_with_bounded_output --lib` — passed
- `cargo test -p tui keys::tests --lib` — passed
- `cargo test -p insomnia parse_keys_subcommand --bin insomnia` — passed
- `cargo check -p manifest -p provider -p tools -p tui -p insomnia` — passed
- `./tickets.sh doctor` — passed
- `git diff --check` — passed

Credential/env grep:

- `api_key_env`, `BRAVE_SEARCH_API_KEY`, `INSOMNIA_API_KEY`, and `default_env_var` are absent from `crates docs resources` after the merge.
- Remaining `sk-`/`secret-value`/`test-secret` hits are fake test values, docs/comments, or Codex OAuth test fixtures, not new persisted real credentials.

Caveat:

- The store should continue to be described as local obfuscation / limited at-rest protection, not a high-assurance password manager.
