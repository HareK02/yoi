<!-- event: create author: tickets.sh at: 2026-05-29T14:53:55Z -->

## Created

Created by tickets.sh create.

---

<!-- event: plan author: hare at: 2026-05-31T20:58:00Z -->

## Plan

Preflight/detailing status: requirements-sync-needed before implementation.

This ticket is the remaining credential/env cleanup boundary. It should not go directly to coding until the secret-store key-management decision is settled. The current code already has some typed-reference groundwork, but no runtime secret store exists.

Current code map:
- `crates/manifest/src/model.rs`
  - `AuthRef::SecretRef { ref_ }` already exists in the manifest model and serializes as `auth = { kind = "secret_ref", ref = "..." }`.
  - `AuthRef::ApiKey { env, file }` still documents and models env/file API-key sources.
  - `SchemeKind::default_env_var()` still provides `INSOMNIA_API_KEY_*` defaults.
- `crates/provider/src/lib.rs`
  - `AuthRef::ApiKey` resolves env first, then `auth.file`.
  - `AuthRef::SecretRef` currently errors with "secret store references are not implemented yet".
- `crates/tools/src/web.rs`
  - Brave WebSearch currently requires `web.search.api_key_env` and reads process env directly.
- `docs/environment.md`
  - credential env vars are documented as migration compatibility, not the target supported configuration path.

Proposed target architecture:
- Add a small secret-store boundary independent from `manifest`, `provider`, and `tools` cycles.
  - Candidate crate name: `secret-store` or `secrets`.
  - Responsibilities: secret id validation, encrypted store read/write, metadata listing, redacted diagnostics, and test-only in-memory/key fixtures.
  - Non-responsibilities: provider-specific auth semantics, profile selection, tool networking.
- Keep manifest/profile config as references only.
  - Model auth should use the existing `AuthRef::SecretRef { ref_ }` shape rather than adding another model-auth syntax.
  - WebSearch needs an explicit secret reference field, e.g. `web.search.api_key_secret = "web/brave/default"` or a typed `web.search.auth = { kind = "secret_ref", ref = "..." }`. Prefer the latter if it avoids another provider-specific one-off, but choose based on minimal config churn.
- Secret IDs should be logical names, not paths.
  - Allow a conservative character set such as `[A-Za-z0-9._/-]`.
  - Reject empty ids, absolute/path-traversal-like ids, control characters, and extremely long ids.
  - IDs are diagnostics-safe; values are not.
- Secret values are resolved only at consumer/runtime boundary.
  - Resolved manifests/profiles may contain secret refs, not plaintext.
  - Provider/WebSearch consumers receive plaintext only in memory and must not serialize/log it.

Key-management decision required before implementation:
- Do not store an encryption key next to the encrypted store; that provides obfuscation, not meaningful encryption.
- Preferred direction to evaluate: OS keychain/credential manager as the wrapping-key source where available.
  - Linux Secret Service / macOS Keychain / Windows Credential Manager through a maintained crate such as `keyring`, if acceptable for dependencies/packaging/headless use.
  - If no OS key provider is available, fail closed with clear diagnostics rather than silently writing plaintext or adjacent keys.
- Tests should use an explicit test-only key provider/in-memory store without process-env mutation.
- If a passphrase fallback is desired later, make it explicit interactive/CLI UX; do not read passphrases from ambient env.

Suggested store layout:
- Store encrypted blobs outside the repository, under the user data directory, e.g. `<data_dir>/secrets/store.json` or `<data_dir>/secrets/<id>.json`.
- Use atomic write + fsync where the project already has or can add an atomic-write helper.
- Store metadata needed for listing without plaintext values:
  - id
  - created_at / updated_at
  - optional description/provider/kind
  - encryption metadata: algorithm, nonce, key provider/version
- Do not put encrypted blobs under work-items, memory, session logs, project `.insomnia/`, or generated reports.

Encryption requirements:
- Authenticated encryption only (AEAD), e.g. XChaCha20-Poly1305 or AES-GCM depending on dependency choice.
- Unique nonce per encryption.
- Include associated data that binds ciphertext to secret id and store metadata version.
- Decryption/auth failure is a fail-closed error naming the secret id only.

CLI/TUI management scope:
- Initial scope can be CLI-first under `insomnia secrets ...`; TUI management can be follow-up unless UX is trivial.
- Minimum CLI:
  - `insomnia secrets set <id> --stdin [--description ...]`
  - `insomnia secrets list`
  - `insomnia secrets delete <id>`
  - optionally `insomnia secrets rename <old> <new>` later, not required for MVP.
- Do not print plaintext by default. Avoid adding `show` unless protected by an explicit `--reveal` decision in a later ticket.
- Non-interactive `set --stdin` is required so scripts can load keys without shell env exports.

Consumer migration plan:
1. Implement secret store + model `AuthRef::SecretRef` resolution in `provider`.
2. Add WebSearch secret reference support and tests.
3. Add CLI management commands.
4. Update docs and examples to use secret refs as the normal path.
5. Convert env-var credential paths into migration diagnostics or compatibility-only one-file/debug behavior, then remove from normal profile path.

Important migration constraints:
- Do not load `.env` files.
- Do not keep env as a normal fallback once secret refs are available.
- If env inputs remain temporarily, diagnostics should say they are migration compatibility and point to `insomnia secrets set ...` / profile secret refs.
- Avoid changing Codex OAuth in this ticket unless a clear secret-store integration is needed; `CODEX_HOME` remains external compatibility.

Acceptance criteria additions:
- `AuthRef::SecretRef` can resolve through the encrypted store for provider API keys without exposing plaintext in resolved config/debug output.
- WebSearch can use a secret ref without `BRAVE_SEARCH_API_KEY` or `web.search.api_key_env`.
- CLI can add/list/delete a secret without printing plaintext.
- Missing secret, invalid id, unavailable key provider, and decryption/auth failure produce clear fail-closed errors naming only the reference.
- Tests cover:
  - secret id validation;
  - encrypted round-trip with test key provider;
  - wrong-key/auth-failure diagnostics;
  - provider `AuthRef::SecretRef` resolution;
  - WebSearch secret ref resolution;
  - no plaintext in `Debug`/serialization paths checked by focused assertions.

Recommended next step:
- Create a short spike/preflight sub-ticket specifically for key provider/dependency choice (`keyring` vs explicit passphrase vs other OS-backed provider). Once that decision is recorded, the implementation ticket can be split into:
  1. secret-store crate + CLI management;
  2. provider `AuthRef::SecretRef` integration;
  3. WebSearch secret ref integration and env migration docs.


---
