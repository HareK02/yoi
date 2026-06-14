---
title: 'Profile から concrete scope を外して launch policy で付与する'
state: 'ready'
created_at: '2026-06-13T17:45:32Z'
updated_at: '2026-06-13T19:02:42Z'
assignee: null
readiness: 'implementation_ready'
risk_flags: ['scope', 'delegation-scope', 'profiles', 'launch-policy', 'orchestrator', 'spawnpod', 'restore']
---

## Background

Profile は reusable behavior / model / prompt / feature policy の単位であるべきだが、現在は concrete filesystem authority である `scope` / `delegation_scope` も持っている。これにより、runtime launch policy と Profile authority が衝突している。

実際に起きた問題:

- `builtin:default` の workspace write scope が role profile 継承に混ざり、Orchestrator direct scope が workspace write を要求した。
- 暫定対応として `resources/profiles/orchestrator.lua` に `scope = "workspace_read"` / `delegation_scope = "workspace_write"` を置いたが、これは scalar replacement に依存した非自明な workaround である。
- Orchestrator の本来の authority は mixed shape である。
  - direct scope: original workspace root read
  - delegation scope: original workspace root read + `<workspace>/.worktree` write
- Profile の Lua API / merge semantics でこの mixed authority を表現しようとすると、`yoi.profile.extend()` の deep merge や authority-bearing field replacement の問題に引きずられる。
- Restore では metadata snapshot を正本として尊重すべきであり、current Profile/default manifest 由来の scope で上書きしてはいけない。

根本方針:

```text
Profile = reusable behavior / prompt / model / feature policy
Launch policy = concrete runtime authority / workspace root / cwd
metadata snapshot = restore 時の effective authority 正本
```

この Ticket は、Orchestrator delegation を狭めるだけでなく、built-in/Profile から concrete scope を外し、起動経路ごとの launch policy が effective `scope` / `delegation_scope` を確定する形へ整理する。

## Requirements

- Reusable Profiles から concrete filesystem authority を外す。
  - Builtin role Profiles (`builtin:companion`, `builtin:orchestrator`, `builtin:coder`, `builtin:reviewer` など) は `scope` / `delegation_scope` を role behavior の正本として持たない。
  - `resources/profiles/orchestrator.lua` の `scope = "workspace_read"` / `delegation_scope = "workspace_write"` workaround を解消する。
  - Profile は model / worker instruction / feature policy / compaction 等の reusable behavior を担う。
- Launch surface が concrete authority を構築する。
  - normal TUI / Companion launch は workspace write など、その起動経路の policy に基づいて direct scope を付与する。
  - Ticket Orchestrator launch は Orchestrator role policy に基づいて scope/delegation を付与する。
  - SpawnPod / role child launch は explicit delegated child scope を child direct scope として渡す。
  - Child delegation scope は明示的に必要な場合のみ付与し、profile inheritance から暗黙に継承しない。
- Orchestrator launch policy を以下にする。

```text
direct scope:
  read  <original workspace root>

delegation_scope:
  read  <original workspace root>
  write <original workspace root>/.worktree
```

- Reviewer/Coder child launch が必要とする root read と implementation worktree write を Orchestrator が再委譲できること。
- Orchestrator が child に original workspace root write を委譲できないこと。
- Restore path は metadata snapshot を尊重する。
  - metadata snapshot がある場合、current profile/default/launch policy で scope/delegation_scope を上書きしない。
  - 新規 launch で保存される metadata snapshot には launch policy 適用後の effective scope/delegation_scope が入る。
- Existing Profile scope support の扱いを明確にする。
  - この Ticket で Profile schema から完全削除するか、built-in role Profiles では禁止/無視しつつ user Profile support を deprecated として残すかは実装時に決めてよい。
  - ただし built-in role launch の concrete authority は Profile scope に依存しないこと。
- `00001KTZY8HK2` の Lua/profile replacement API は、この Ticket の前提にしない。
  - scope 問題は Profile replacement API で解くのではなく、concrete authority を launch policy へ移すことで解く。
  - scope 以外の field replacement が必要なら `00001KTZY8HK2` を後続の別問題として扱う。

## Acceptance criteria

- Builtin Orchestrator Profile の resolved reusable behavior から broad `delegation_scope = "workspace_write"` 依存がなくなる。
- Fresh Orchestrator launch の effective manifest が以下を満たす test がある。
  - direct scope allows read on original workspace root
  - direct scope does not allow write on original workspace root
  - delegation scope allows read on original workspace root
  - delegation scope allows write under original workspace `.worktree`
  - delegation scope does not allow write on original workspace root outside `.worktree`
- Reviewer/Coder launch validation can be satisfied by the narrowed Orchestrator delegation scope.
- Scope allocator does not conflict with the Companion/top-level `yoi` Pod's workspace write allocation.
- Normal Companion/TUI launch still receives the expected workspace write direct scope from launch policy, not from reusable Profile authority.
- SpawnPod child config still replaces inherited/profile scope with the explicitly delegated child scope.
- Restore from metadata snapshot preserves saved scope/delegation_scope and does not reapply current Profile/launch default authority over the snapshot.
- Tests cover at least:
  - Profile resolution no longer leaking default workspace write into Orchestrator authority
  - launch policy authority for Orchestrator
  - launch policy authority for normal top-level/Companion launch where practical
  - restore snapshot preservation
  - child delegation validation for root read + worktree write
- Validation: focused scope/profile/client/pod tests and `cargo build -p yoi`. Run `nix build .#yoi` only if Cargo.lock, packaging, or resource inclusion changes require it.

## Out of scope

- General Plugin/MCP permission design.
- OS-level sandboxing of child processes.
- Designing a full replacement/clear Lua API for every Profile field.
- Full removal of user Profile `scope` compatibility unless the implementation chooses that as the cleanest route and updates tests/docs accordingly.

## Related work

- Profile replacement/clear semantics follow-up: `00001KTZY8HK2`
- Restore should preserve metadata manifest snapshot: `9be3f132`
- Orchestrator role profile: `resources/profiles/orchestrator.lua`
- SpawnPod child scope/delegation path: `crates/pod/src/spawn/tool.rs`
