---
title: 'Profile feature flagsでtool surfaceを制御する'
state: 'queued'
created_at: '2026-06-09T08:22:09Z'
updated_at: '2026-06-09T10:31:11Z'
assignee: null
queued_by: 'workspace-panel'
queued_at: '2026-06-09T10:31:11Z'
---

## 背景

最近追加した Ticket tools、Ticket orchestration tools、Pod orchestration tools などにより、Pod に渡される tool schema が増えている。すべての role にすべての tools を渡すと、コンテキストを圧迫し、tool choice の性能低下や role 外行動を誘発する。

現状は core tools、Task tools、Ticket tools、Pod orchestration tools などがかなり広く登録されており、role/profile から feature 単位で tool surface を選ぶ設定がない。

Coder は Ticket 管理や orchestration をしないなら、それらの tools を見るべきではない。SpawnPod などの Pod orchestration tools も、明示的に与えられた role/profile だけが使える optional capability にするべき。

## ゴール

Lua Profile / resolved Manifest に feature 単位の tool selection を導入し、role ごとに不要な high-level tools を登録しないようにする。

## 要件

### Profile syntax

- Lua Profile に `feature.<name>.enabled` を追加する。
- object の存在確認ではなく、明示 bool を見る。
  - `feature.ticket = {}` が true 扱いになるような挙動は避ける。
  - `enabled = true` / `enabled = false` を authority とする。
- まず対応する feature groups:
  - `task`
  - `ticket`
  - `ticket_orchestration`
  - `pods`
  - `memory`
  - `web`
- resolved Manifest では各 feature の enabled が concrete bool になるようにする。
- Profile source layer では optional でもよいが、project role profiles は明示的に設定する。

### Feature group semantics

- `feature.task`:
  - `TaskCreate`
  - `TaskUpdate`
  - `TaskGet`
  - `TaskList`
- `feature.ticket`:
  - Ticket の基本 lifecycle / review / state / close / doctor tools。
  - access level が必要なら `access` を追加する。
    - 候補: `read_only`, `intake`, `reviewer`, `lifecycle`。
- `feature.ticket_orchestration`:
  - Ticket relation / orchestration plan 系 tools。
  - 例: `TicketRelationRecord`, `TicketRelationQuery`, `TicketOrchestrationPlanRecord`, `TicketOrchestrationPlanQuery`。
  - Orchestrator 専用に近い。
- `feature.pods`:
  - Pod orchestration / communication tools。
  - 例: `SpawnPod`, `SendToPod`, `ReadPodOutput`, `StopPod`, `ListPods`, `RestorePod`, `SendToPeerPod`。
  - disabled の場合は tool schema に出さない。
- `feature.memory`:
  - Memory / Knowledge tools。
  - access level が必要なら後続または同 Ticket 内で `read_only` / `write` を検討する。
- `feature.web`:
  - `WebSearch`, `WebFetch`。
  - existing web provider config と連動する。
  - `feature.web.enabled=false` なら tools を登録しない。
  - provider config が不足している場合は、enabled でも existing fail-closed behavior を維持してよい。

### Core tools

- この feature grouping には core filesystem / shell tools を含めない。
  - `Read`
  - `Write`
  - `Edit`
  - `Glob`
  - `Grep`
  - `Bash`
- これらは workspace scope / tool permission policy の制御対象として扱う。
- `Bash` は将来的に別途 opt-in/tool permission refinement の対象になり得るが、この Ticket の first pass には含めない。

### Registration behavior

- disabled feature の tools は Worker に登録しない。
- permission deny で失敗させるのではなく、tool schema から消す。
- feature disabled によって normal prompt guidance も可能な範囲でその tool group を前提にしない。
- FeatureRegistry / controller の無条件 registration を profile/resolved manifest に基づく conditional registration に変更する。

### Role defaults

Project role profiles should set feature defaults explicitly.

- Orchestrator:
  - `ticket = enabled`, lifecycle access
  - `ticket_orchestration = enabled`
  - `pods = enabled`
  - `task = disabled` unless explicitly justified
  - `memory = enabled` preferably read/query oriented
  - `web = enabled` if configured
- Intake:
  - `ticket = enabled`, intake-oriented access
  - `ticket_orchestration = disabled`
  - `pods = disabled`
  - `task = disabled`
  - `memory = enabled` preferably read/query oriented
  - `web = enabled` if configured
- Coder:
  - `ticket = disabled` by default
  - `ticket_orchestration = disabled`
  - `pods = disabled`
  - `task = disabled`
  - `memory = enabled` only if desired for read/query context
  - `web = enabled` if configured
- Reviewer:
  - `ticket = disabled` or reviewer/read-only access depending on chosen review-report flow
  - `ticket_orchestration = disabled`
  - `pods = disabled`
  - `task = disabled`
- Companion:
  - `ticket = read_only` if useful
  - `ticket_orchestration = disabled`
  - `pods = disabled` unless a specific Companion Pod-management UI/tool role is designed
  - `task = disabled`

### Access granularity

- If `ticket.enabled=true` is too broad, implement or design `feature.ticket.access` in the same change.
- Existing `TicketFeatureAccess::ReadOnly` / `Lifecycle` should be reused or extended rather than ignored.
- `ticket_orchestration` should be separable from basic Ticket read/show/list tools.

## 非目標

- Redesigning core filesystem tools.
- Removing permission policy.
- Implementing user approval/ask flow.
- Making feature object existence imply enabled.
- Adding a plugin system beyond existing feature registry.
- Changing the semantics of individual Ticket/Pod tools beyond whether they are registered.

## 受け入れ条件

- Lua Profiles can explicitly set `feature.<name>.enabled` for the six initial feature groups.
- Resolved Manifest has concrete feature enablement values.
- Disabled features do not register their tools and therefore do not appear in tool schemas or greetings.
- Project role profiles set explicit feature defaults.
- Coder profile does not expose Ticket management, Ticket orchestration, Task, or Pod orchestration tools by default.
- Orchestrator profile exposes Ticket lifecycle, Ticket orchestration, and Pod orchestration tools as intended.
- Web tools are omitted when `feature.web.enabled=false` and still fail closed when enabled but provider config is insufficient.
- Tests cover tool definition lists for at least Orchestrator, Coder, Intake, Reviewer, and Companion profiles.
- Focused tests, `cargo fmt --check`, `git diff --check`, and `target/debug/yoi ticket doctor` pass.
