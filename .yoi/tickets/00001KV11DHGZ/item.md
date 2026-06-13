---
title: 'Orchestrator delegation scope を root read + worktree write に狭める'
state: 'planning'
created_at: '2026-06-13T17:45:32Z'
updated_at: '2026-06-13T17:46:37Z'
assignee: null
readiness: 'requirements_sync_needed'
risk_flags: ['scope', 'delegation-scope', 'orchestrator', 'profile-merge', 'spawnpod']
---

## Background

Orchestrator の direct scope は root workspace read-only であるべきだが、child Coder/Reviewer には implementation worktree への write と original workspace への read を委譲できる必要がある。

現在の暫定 Orchestrator profile は `resources/profiles/orchestrator.lua` で以下になっている。

```lua
scope = "workspace_read"
delegation_scope = "workspace_write"
```

これは fresh launch の delegation 不足を避けるが、delegation が広すぎる。概念的には child に original workspace root への write まで委譲可能になっている。

手動 restore 復旧時に delegation を `.worktree` write のみに狭めたところ、Reviewer 起動で root workspace read を再委譲できず失敗した。正しい effective shape は以下。

```text
direct scope:
  read  <original workspace root>

delegation_scope:
  read  <original workspace root>
  write <original workspace root>/.worktree
```

現状の scalar profile intent では、この mixed delegation scope を簡潔に表現できない。object/table form は `profile.extend()` の deep merge と scope merge semantics により inherited value の置換が難しいため、既存 Ticket `00001KTZY8HK2` の profile replace/clear semantics と関連する。

## Requirements

- Orchestrator fresh launch の effective scope を以下にする。
  - direct `scope`: original workspace root recursive read
  - `delegation_scope`: original workspace root recursive read + original workspace `.worktree` recursive write
- Reviewer/Coder child launch が必要とする root read と implementation worktree write を Orchestrator が再委譲できること。
- Orchestrator が child に original workspace root write を委譲できないこと。
- Profile 継承は維持する。
  - `builtin:orchestrator` は `builtin:default` から reusable defaults を継承してよい。
  - inherited authority-bearing scope fields を意図せず加算しないこと。
- 実装方法は `00001KTZY8HK2` の profile replacement/clear semantics と整合させる。
  - この Ticket で replacement API まで実装するか、Orchestrator launch context 側で role-specific delegation scope を構築するかは設計で決める。
  - いずれの場合も scalar string workaround への依存を増やさない。
- Restore path と fresh launch path の effective scope が意図せず乖離しないようにする。
  - metadata snapshot は restore 時に尊重する。
  - 新規生成される Orchestrator metadata snapshot には narrowed delegation scope が保存される。

## Acceptance criteria

- Fresh Orchestrator launch の resolved/effective manifest が以下を満たす test がある。
  - direct scope allows read on original workspace root
  - direct scope does not allow write on original workspace root
  - delegation scope allows read on original workspace root
  - delegation scope allows write under original workspace `.worktree`
  - delegation scope does not allow write on original workspace root outside `.worktree`
- Reviewer/Coder launch validation can be satisfied by the narrowed Orchestrator delegation scope.
- Scope allocator does not conflict with the Companion/top-level `yoi` Pod's workspace write allocation.
- `resources/profiles/orchestrator.lua` no longer needs to express delegation as broad `"workspace_write"` unless a separate explicit override intentionally narrows it later in launch resolution.
- Tests cover both profile resolution / launch context and child delegation validation where practical.
- Validation: focused scope/profile/client tests, `cargo build -p yoi`, and `nix build .#yoi`.

## Out of scope

- General Plugin/MCP permission design.
- Full replacement of profile scope semantics beyond what is needed here, unless this Ticket is intentionally merged with `00001KTZY8HK2`.
- OS-level sandboxing of child processes.

## Related work

- Profile replacement/clear semantics: `00001KTZY8HK2`
- Restore should preserve metadata manifest snapshot: current local fix in `crates/pod/src/pod.rs`
- Orchestrator role profile: `resources/profiles/orchestrator.lua`
