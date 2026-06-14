---
title: 'Profile extend API を廃止して import + Lua 代入に寄せる'
state: 'inprogress'
created_at: '2026-06-13T07:31:09Z'
updated_at: '2026-06-14T06:29:17Z'
assignee: null
readiness: 'implementation_ready'
risk_flags: ['profiles', 'lua-api', 'builtin-resources', 'migration']
queued_by: 'workspace-panel'
queued_at: '2026-06-14T06:08:28Z'
---

## Background

`yoi.profile.extend(base, overrides)` は base Profile と override object を Rust 側で deep merge する convenience API だった。しかし deep merge semantics が隠れるため、object field を「置換したい」のか「部分 merge したい」のかが Profile authoring 上で読み取りづらい。

今回の問題では、以下のような Profile が scope を置換するつもりで object を渡すと、`builtin:default` 側の inherited value と deep merge され、workspace write が残り得た。

```lua
return yoi.profile.extend("builtin:default", {
  scope = yoi.scope.workspace_read(),
})
```

一方で Lua には通常の table 操作があるため、Profile 継承と field replacement は次の形で明示できる。

```lua
local p = yoi.profile.import("builtin:default")
p.worker.instruction = "$yoi/role/orchestrator"
p.feature.task.enabled = false
p.feature.ticket = { enabled = true, access = "lifecycle" }
return p
```

この形なら、top-level/nested field の代入は ordinary Lua assignment であり、merge/replace の意図が読みやすい。Profile scope については別 Ticket `00001KV11DHGZ` で concrete runtime authority を launch policy に移すため、`extend()` に replacement API を足す必要性はさらに下がった。

## Requirements

- `yoi.profile.extend` を廃止する。
  - builtin Profile resources は `extend()` を使わず、`yoi.profile.import(...)` + explicit Lua assignment に移行する。
  - Profile authoring convention は ordinary Lua table mutation / assignment を基本とする。
- `yoi.profile.import` は残す。
  - base Profile を取得して手元で変更し、`return p` する構造を公式パターンにする。
- `extend()` の deep merge semantics に依存する builtin tests/resources を更新する。
- `extend()` を public API として残す場合は deprecated diagnostic を出すか、削除して明確に fail させる。
  - どちらにするかは実装時に決めてよいが、builtin resources は使わない。
  - 不必要な compatibility alias は作らない。
- Docs/tests/comments から「scope replacement API を足す」方向の記述を削除する。
  - scope/delegation_scope の concrete authority は `00001KV11DHGZ` の launch policy 側で扱う。
  - scope 以外でも object replacement が必要な場合は ordinary Lua assignment を使う。
- Profile merge helper がどうしても必要になった場合は、`extend()` のような曖昧な名前ではなく、`deep_merge` / `shallow_merge` など semantics が明示された別 API として後続 Ticket で検討する。

## Acceptance criteria

- Repository builtin Profile resources no longer call `yoi.profile.extend`.
- The Profile Lua API supports the official inheritance pattern:

```lua
local p = yoi.profile.import("builtin:default")
-- mutate p explicitly
return p
```

- Tests cover that `import()` + assignment replaces object fields without deep merge surprise.
- Existing role Profiles still resolve to the same intended non-scope behavior: worker instructions, feature enablement, model/defaults, compaction, etc.
- If `extend()` is removed, calling it fails with a clear Lua/profile error. If deprecated instead, it emits/records a clear deprecation diagnostic and builtin resources do not use it.
- The old requirement to add `yoi.scope.replace(...)` / `replace = true` for scope replacement is removed or explicitly superseded.
- Validation: focused profile resolution tests and `cargo build -p yoi`. Run `nix build .#yoi` only if resource packaging or lockfile/package changes require it.

## Non-goals

- Moving concrete scope/delegation_scope out of Profiles; tracked by `00001KV11DHGZ`.
- Designing a general-purpose Lua table utility library.
- Preserving `extend()` compatibility indefinitely.
- Plugin/MCP permission design.

## Related work

- Move concrete scope to launch policy: `00001KV11DHGZ`
- Original scope replacement concern is superseded by this Ticket plus `00001KV11DHGZ`.
