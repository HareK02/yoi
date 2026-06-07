---
id: 20260529-145355-manifest-profile-encrypted-secrets
slug: manifest-profile-encrypted-secrets
title: 'Manifest/Profile: local key-value secret store'
status: closed
kind: feature
priority: P2
labels: [manifest, profiles, secrets, security, cli, tui]
created_at: 2026-05-29T14:53:55Z
updated_at: 2026-05-31T22:23:34Z
assignee: null
legacy_ticket: null
---

## Background

Credential configuration still relies on process environment variables in important paths:

- provider API keys use `AuthRef::ApiKey { env, file }` and provider-default `INSOMNIA_API_KEY_*` names;
- WebSearch currently uses `web.search.api_key_env`;
- normal runtime intentionally does not load `.env` files.

This should not be solved by implicit `.env` loading. `.env` files are easy to leak into projects, do not solve profile-specific credential selection cleanly, and still expose secrets through process environments.

The desired replacement is a local key-value secret store plus explicit references from manifest/profile/tool configuration.

The security target is intentionally modest. This is not a high-assurance password manager. The goal is to avoid casual plaintext exposure and generic environment-variable scraping, not to defend against a local attacker with the user's account or process memory access.

## Intent

Implement a provider-independent local key-value secret store and use it as the normal credential path for provider and WebSearch credentials.

The logical store model is just:

```text
{
  "anthropic/default" = "sk-..."
  "web/brave/default" = "..."
}
```

The store must not know that a key is Anthropic, Brave, OpenAI, or any other provider-specific kind. Provider/model/tool configuration chooses which key to reference.

## Requirements

### Store model

- Add a local secret/key store that maps a validated string id to a secret string value.
- Keep the user-visible logical schema provider-independent: `id -> value`.
- Do not add provider-specific slots, credential kinds, or required metadata to the store schema.
- Technical envelope fields needed for versioning/nonce/ciphertext/checksum are allowed, but they must not become user-facing semantic metadata.
- Store data outside the repository, under the user data directory, e.g. `<data_dir>/secrets/store.json` or equivalent.
- Use atomic writes.
- Validate ids:
  - reject empty ids;
  - reject path traversal / absolute-path-like ids;
  - reject control characters;
  - bound length;
  - allow a conservative useful set such as ASCII alnum plus `._/-`.

### Obfuscation / encryption stance

- Apply lightweight encryption or obfuscation at rest so the file is not a casual plaintext key dump.
- Do not claim strong local security guarantees.
- Do not introduce OS keychain dependency or interactive passphrase UX in this ticket.
- Do not store plaintext values in logs, work items, session history, diagnostics, or normal command output.
- Decryption/decoding failures must fail closed and name only the key id, not the value.

### `insomnia keys` TUI management

- Add `insomnia keys` as an interactive TUI key manager.
- The product CLI owner (`insomnia` crate) routes the subcommand.
- Use the TUI implementation crate for the terminal screen if practical.
- Minimum UI features:
  - list key ids;
  - add/set a key;
  - delete a key with confirmation;
  - quit.
- Do not display plaintext values in the list or normal screen output.
- During add/set, mask the value input or otherwise avoid echoing plaintext.
- Scriptable commands such as `insomnia keys set <id> --stdin` may be added if convenient, but the required user surface is the TUI manager.

### Config references / consumers

- Use explicit secret references from configuration.
- Existing `AuthRef::SecretRef { ref_ }` in manifest model should resolve through the new store for provider API keys.
- WebSearch must gain an explicit secret reference path so Brave search can be configured without `BRAVE_SEARCH_API_KEY` / `api_key_env`.
  - Prefer a generic auth/secret-ref shape if it stays small.
  - A focused `api_key_secret = "web/brave/default"` field is acceptable if it avoids a broad schema redesign.
- Secret refs are resolved at the consumer/runtime boundary only; resolved config/debug output must not contain plaintext.
- The store must not implicitly choose default keys based on provider name. No ambient lookup like "anthropic automatically reads anthropic/default" unless the profile/config explicitly references it.

### Env credential removal

- Do not load `.env` files.
- Do not add new credential environment variables.
- Do not keep migration/backward-compatibility behavior for credential env config in the normal profile path.
- Remove credential env configuration from normal provider/WebSearch use as part of this ticket.
- Docs and diagnostics should point users to `insomnia keys` + secret refs as the credential path.

### Codex OAuth relationship

- Codex OAuth is not part of this key-value secret store in this ticket.
- Current Codex OAuth intentionally interoperates with Codex CLI's `auth.json` file and refresh behavior; that file contains a structured token bundle, not a single provider API key string.
- Do not store or refresh Codex OAuth token bundles through the key-value store as part of this ticket.
- Do not change `CODEX_HOME` / `$HOME/.codex` lookup behavior in this ticket.
- A future Insomnia-owned Codex login/token store could be designed separately if needed, but it should be a dedicated OAuth token-store design, not an implicit use of the simple key-value API-key store.

## Phases within this ticket

1. Core store
   - key-value store API;
   - id validation;
   - lightweight encrypted/obfuscated file format;
   - atomic load/save;
   - focused tests.
2. `insomnia keys` TUI manager
   - list/add/delete;
   - masked input;
   - no plaintext display.
3. Provider integration
   - implement provider `AuthRef::SecretRef` resolution through the store;
   - keep plaintext in memory only;
   - fail closed on missing/invalid/decode failures.
4. WebSearch integration
   - add a secret-ref credential path;
   - make Brave search usable without env credentials.
5. Docs and env removal
   - update `docs/environment.md` and manifest/profile docs;
   - document the modest security target honestly;
   - point users to `insomnia keys` and secret refs as the credential path;
   - remove credential env configuration from normal provider/WebSearch docs and code paths.

## Non-goals

- A high-assurance password manager.
- OS keychain integration.
- Passphrase prompt UX.
- Provider-specific secret-store schema.
- Automatic provider-name-to-secret-id lookup.
- Loading `.env` files.
- Changing Codex OAuth behavior. Codex OAuth remains an external structured token-source integration in this ticket.
- Reworking model/provider catalog ownership.

## Acceptance criteria

- A user can run `insomnia keys` and manage key ids interactively.
- The store persists key-value entries under the user data directory without plaintext values in the on-disk file.
- Store id validation rejects unsafe ids.
- Provider `AuthRef::SecretRef` resolves through the store and does not print/serialize plaintext.
- WebSearch can use a configured secret ref without exporting an environment variable.
- Missing key, invalid id, unreadable store, and decode/decrypt failure produce clear fail-closed errors naming only the key id.
- `docs/environment.md` no longer presents credential env vars as the normal path, removes normal provider/WebSearch credential env configuration, and documents the limited protection goal.
- Focused tests cover store round-trip, id validation, decode failure, provider secret-ref resolution, WebSearch secret-ref resolution, and no-plaintext debug/serialization paths where applicable.
- `cargo fmt --check`, relevant crate tests/checks, `./tickets.sh doctor`, and `git diff --check` pass.
