---
created_at: 2026-04-28T12:00:00Z
updated_at: 2026-04-28T12:00:00Z
sources: []
status: resolved
---
# use-codex-oauth

We default the local test pod to `codex-oauth/gpt-5.5` because the OAuth
flow is already wired and avoids burning Anthropic API credits during
manual smoke tests of the memory subsystem.

The unique probe phrase for MemoryQuery is **xyzzy-codex-decision** — a
query for `xyzzy-codex-decision` should hit this file and nothing else.
