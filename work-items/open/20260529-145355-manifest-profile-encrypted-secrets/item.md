---
id: 20260529-145355-manifest-profile-encrypted-secrets
slug: manifest-profile-encrypted-secrets
title: Encrypted secret store for manifest profiles
status: open
kind: feature
priority: P2
labels: [manifest, profiles, secrets, security]
created_at: 2026-05-29T14:53:55Z
updated_at: 2026-05-29T14:53:55Z
assignee: null
legacy_ticket: null
---

## Background

WebSearch/WebFetch made API keys more visible as a UX problem: `WebSearch` currently expects `web.search.api_key_env`, so users must export `BRAVE_SEARCH_API_KEY` before starting the Pod/TUI process. That is inconvenient for long-lived Pods, profile switching, and per-project/provider configuration.

This should not be solved by adding `.env` loading as an implicit side effect. `.env` files are easy to leak into projects, do not solve profile-specific credential selection cleanly, and still expose secrets through process environments. Instead, when manifest profiles are designed/implemented, add a first-class encrypted secret store that manifests/profiles can reference.

Related work item: `work-items/open/20260527-000022-manifest-profiles/item.md`.

## Requirements

- Design a typed secret reference format for manifest/profile fields that need credentials.
  - Existing env references such as `api_key_env = "BRAVE_SEARCH_API_KEY"` should keep working.
  - Add a new encrypted-store reference form, e.g. `api_key_secret = "brave.search.default"` or a more general `SecretRef` enum.
  - Secret references must be explicit in resolved config; do not silently read arbitrary `.env` files.
- Add an encrypted local secret store suitable for API keys/tokens.
  - Store secrets outside tracked project files by default, under the user data/config directory.
  - Use authenticated encryption and atomic writes.
  - Do not log plaintext secrets, include them in session logs, expose them to model context, or return them through normal tool output.
  - Keep encrypted blobs out of git-managed work-items/memory/session records.
- Integrate with manifest profiles.
  - Profiles should be able to select different secret names for different roles/providers, e.g. Orchestrator/Coder/Researcher or web search provider variants.
  - Profile resolution should validate that referenced secrets exist or produce a clear startup/tool diagnostic.
  - A profile switch must not require restarting the shell just to change API keys.
- Provide a small CLI/TUI management surface.
  - Add/update/list/delete secrets without printing plaintext by default.
  - Support non-interactive set from stdin for scripts.
  - Show references and metadata, not secret values.
  - Consider migration helpers from existing env-var based configuration, but keep migration optional.
- Update credential consumers.
  - WebSearch should support encrypted secret refs in addition to env vars.
  - Provider API keys/tokens and future hosted/search credentials should be able to use the same mechanism.
  - Existing env-var behavior remains as a fallback/compatibility path.
- Security and UX constraints.
  - Fail closed when a referenced secret is missing or cannot be decrypted.
  - Diagnostics should name the missing reference, not the secret value.
  - Do not add hidden context injection or history mutation for secret resolution.
  - Document the threat model and limitations, including OS account access and backup implications.

## Acceptance criteria

- Manifest/profile schema has a typed credential reference that can point either to an env var or encrypted secret-store entry.
- Encrypted secret-store files are created outside the repository by default and use authenticated encryption with atomic update behavior.
- A user can add/list/delete a Brave Search API key in the secret store and configure `WebSearch` to use it without exporting an environment variable.
- Resolved configuration and diagnostics never display plaintext secrets.
- Missing/decryption-failed secrets produce clear fail-closed errors.
- Existing env-var based configuration continues to work.
- Documentation explains how profiles reference secrets and how to manage them.
- Focused tests cover config parsing/resolution, missing secret diagnostics, no-plaintext serialization/logging paths, and WebSearch secret resolution.
- `cargo fmt --check`
- Relevant manifest/provider/tools/pod tests pass.
