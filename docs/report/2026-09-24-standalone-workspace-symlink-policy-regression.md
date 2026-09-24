# Standalone workspace scope defaults reject logical symlinks

## Symptom

A standalone-style Worker rooted at the repository cannot read the repository-local alias:

```text
yoi.local -> /home/hare/.local/share/yoi/
```

`Read`, `Glob`, and `Grep` reject paths through that alias because the resolved target is outside the repository scope, even though the alias itself is below the Worker root.

## Cause

The Workdir provider supports both symlink identities:

- `Resolved` matches the provider-resolved target and is the least-authority default.
- `Logical` matches the path presented through the Workdir and intentionally permits an in-scope alias to reach its target.

Focused provider tests confirm both behaviors:

- an explicit logical policy can read, list, glob, and grep through an external directory alias;
- the default resolved policy rejects a symlink escape.

Before correction, the standalone launch path did not select the implemented logical behavior. `StandaloneLaunchConfig::resolve` canonicalized the cwd and resolved `builtin:default`. That profile declared `scope = "workspace_write"` and `delegation_scope = "workspace_write"`. `profile_scope_intent_to_config` converted each workspace intent into a recursive root rule with:

```rust
symlink_policy: Default::default()
```

`SymlinkPolicy::default()` is `Resolved`. Consequently, a request for `yoi.local/...` is checked using `/home/hare/.local/share/yoi/...`, which is not below the repository root and is denied.

This behavior was introduced when selective Workdir symlink policies were added in commit `8a3e06bc`. Provider support for explicit logical rules was added, but reusable profile workspace intents and standalone launch wiring retained the new least-authority default. Reusable profiles also expose only `workspace_read`/`workspace_write` intents, so `builtin:default` cannot opt into logical identity through its profile declaration.

## Scope of the finding

This explains the `yoi.local` rejection. It does not explain a rejection of an ordinary path such as `crates` or `Cargo.toml`: those contain no symlink. An error such as:

```text
Workdir path `crates` exceeds the provider attachment scope
```

means the restored parent attachment scope does not cover the Workdir's own logical root. That is a separate restore/attachment authority defect and cannot be fixed by requesting `symlink_policy: logical` on a Reviewer child.

## Implemented correction

A dedicated `builtin:standalone` profile now imports `builtin:default` and overrides only the standalone-specific contract:

- model: `codex-oauth/gpt-5.6-sol`;
- direct workspace scope: `workspace_write` with `Logical` symlink identity;
- delegation scope: `workspace_write` with `Logical` symlink identity.

Profile scope tables now accept an explicit `symlink_policy`, while the string shorthand and all existing profiles retain the least-authority `Resolved` default. Standalone launch and its TUI picker select `builtin:standalone` only when the registry still has the built-in `builtin:default` fallback; an operator-configured user default retains precedence. The global registry default remains `builtin:default`, so omitted SubWorker profiles do not acquire standalone-specific identity or policy.

Regression coverage verifies the built-in resource inheritance, model selection, default registry selection, and the concrete direct/delegation scope policies.

Existing persisted Worker manifest snapshots require explicit consideration: restore reuses the saved resolved scope, so the new profile will not repair already-restored Workers until they are replaced or their persisted authority is migrated deliberately.
