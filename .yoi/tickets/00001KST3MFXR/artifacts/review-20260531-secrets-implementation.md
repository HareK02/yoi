# Review: local key-value secret store implementation

Implementation reviewed on branch `manifest-profile-encrypted-secrets`.

Reviewed commits:

- `cc2c9a2 secrets: add local key store`
- `7ddf745 secrets: polish key manager and docs`

Verdict: approve.

Summary:

- Core provider-independent `id -> value` local secret store satisfies the ticket model.
- Store values are not persisted as casual plaintext and error/debug surfaces avoid secret values within the stated modest protection boundary.
- Provider auth now resolves explicit `secret_ref` values through the local store without env credential fallback.
- WebSearch uses explicit `api_key_secret` and no longer depends on `BRAVE_SEARCH_API_KEY` / `api_key_env` in the normal path.
- `insomnia keys` provides interactive list/add-set/delete management without displaying plaintext values.
- Codex OAuth behavior remains separate and unchanged.
- Follow-up review confirmed the docs credential example is schema-valid and key-manager terminal setup cleanup was added.

Validation reported by coder/reviewer:

- `cargo fmt --check`
- `cargo test -p secrets`
- focused manifest/provider/tools/tui/insomnia tests
- `cargo check -p manifest -p provider -p tools -p tui -p insomnia`
- `./tickets.sh doctor`
- `git diff --check`
- credential/env greps confirming `api_key_env`, `BRAVE_SEARCH_API_KEY`, `INSOMNIA_API_KEY`, and `default_env_var` are absent from crates/docs/resources

Remaining caveat:

- Continue to describe this as local obfuscation / limited at-rest protection, not a high-assurance password manager or OS-keychain-backed vault.
