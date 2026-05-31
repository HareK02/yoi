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

<!-- event: decision author: hare at: 2026-05-31T21:04:45Z -->

## Decision

Decision: keep the secret store as a provider-independent key-value store.

User decision:
- The secret store path should be completely limited to a simple key-value store.
- Do not hard-code provider-specific slots or provider-specific semantics into the store.
- Do not require metadata in the store schema.
- Conceptual model is:

```text
{
  "anthropic/default" = "sk-..."
}
```

- Provider/model/tool configuration is responsible for choosing which secret key to reference.
- The store does not know whether a value is an Anthropic key, Brave key, OpenAI key, token, or anything else.
- Any provider-aware UX should be a higher-level helper that writes ordinary key-value entries and/or config references; it must not change the store schema.

Security stance:
- Use light encryption/obfuscation at rest if practical, but do not claim strong security guarantees.
- The goal is to avoid casual plaintext exposure in files, logs, work items, and accidental grep/cat output, not to defend against a local attacker with access to the user account.
- Avoid complicated key-management requirements such as OS keychain dependency as a prerequisite for this ticket unless a later explicit decision changes the security target.
- Documentation and diagnostics should be honest: this is an obfuscated/encrypted local key-value store with limited protection, not a high-assurance secret manager.

Implications for implementation planning:
- Remove the previous requirement for metadata such as provider/kind/description/created_at/updated_at unless the implementation needs internal versioning/encryption fields.
- Store format may still need technical envelope fields for version/nonce/ciphertext/checksum, but the user-visible logical schema is only `id -> value`.
- Secret id validation remains useful because ids are referenced from manifest/profile/tool config and diagnostics.
- Provider/WebSearch integration should resolve `secret_ref` by direct key lookup only.


---

<!-- event: plan author: hare at: 2026-05-31T21:19:29Z -->

## Plan

Preflight update: implementation-ready as a single phased ticket.

Finalized intent:
- Build a provider-independent local key-value secret store and wire it into provider/WebSearch credential resolution.
- Add `insomnia keys` as the required interactive TUI management surface.
- Keep the security claim modest: avoid casual plaintext exposure and generic env scraping; do not claim strong local security.

Settled decisions:
- Store model is user-visible `id -> value`; no provider-specific slots and no required metadata.
- Store may use technical envelope fields for version/nonce/ciphertext/checksum, but those are implementation details.
- No OS keychain, passphrase UX, or high-assurance key-management dependency in this ticket.
- No automatic provider-name-to-secret-id lookup. Config must explicitly reference a key id.
- This is one ticket with phases, not separate tickets.

Suggested implementation order:
1. Add a focused secret store crate/module with id validation, obfuscated/encrypted persistence, atomic writes, and tests.
2. Add `insomnia keys` TUI manager for list/add/delete with masked input.
3. Implement provider `AuthRef::SecretRef` resolution through the store.
4. Add WebSearch secret-ref configuration path.
5. Update docs and migration messaging.

Critical risks for coder/reviewer:
- Accidentally turning the store into provider-aware schema.
- Displaying plaintext in Debug/errors/TUI list output.
- Overstating security guarantees in docs.
- Adding ambient defaults that recreate the env-var problem in secret-store form.
- Expanding into OS keychain/passphrase design despite the settled modest threat model.

Validation should include focused tests for id validation, store round-trip, decode failure, provider secret-ref resolution, WebSearch secret-ref resolution, and no plaintext in display/debug paths where applicable.


---

<!-- event: decision author: hare at: 2026-05-31T21:23:46Z -->

## Decision

Decision update:
- Migration/backward compatibility for credential environment variables is not required.
- The implementation should remove credential env configuration from the normal provider/WebSearch path rather than keep env fallback as a compatibility layer.
- Docs should point to `insomnia keys` + explicit secret refs as the credential path.

Codex OAuth relationship:
- Codex OAuth should not be connected to this key-value API-key store in this ticket.
- The current Codex integration interoperates with Codex CLI's structured `auth.json` token bundle and refresh behavior.
- That is a different credential shape than `id -> secret string` API keys.
- Keep `CODEX_HOME` / `$HOME/.codex` behavior unchanged for this ticket.
- If Insomnia later owns Codex login/token storage, design it as a separate OAuth token-store feature, not as an implicit use of the simple key-value store.


---

<!-- event: review author: hare at: 2026-05-31T22:21:04Z status: approve -->

## Review: approve

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


---
