---
title: 'Panel 表示を現在 workspace の Pod に限定する'
state: 'inprogress'
created_at: '2026-06-18T14:09:59Z'
updated_at: '2026-06-18T15:29:08Z'
assignee: null
readiness: 'ready'
risk_flags: ['panel', 'pod-metadata', 'workspace-boundary', 'runtime-observation']
queued_by: 'workspace-panel'
queued_at: '2026-06-18T14:47:10Z'
---

## Background

`yoi panel` は workspace の状況確認・Ticket queue・Orchestrator 操作の入口になっているため、別 workspace の Pod が一覧に混ざると、現在 workspace の状態として誤認しやすい。

Yoi では runtime workspace root、Pod identity、Profile、cwd、Ticket backend checkout が別概念として整理されている。したがって Panel の表示対象も、Pod 名や cwd の heuristic ではなく、Pod metadata に記録された runtime workspace identity を基準に workspace-scoped にする必要がある。

Request snapshot:

- `yoi panel` の Pod 表示で、現在の workspace に属さない Pod が表示されないようにしたい。
- Panel handoff context:
  - workspace: `yoi`
  - workspace_orchestrator_pod: `yoi-orchestrator`

## Requirements

- `yoi panel` の通常 Pod 表示は、現在の runtime workspace root に属する Pod のみに限定する。
- 別 workspace の Pod は Panel の通常一覧・通常 action target に表示しない。
- workspace Orchestrator Pod、Companion Pod、Ticket role Pod など、現在 workspace に属する role Pod は表示対象に残す。
- dedicated orchestration worktree や implementation worktree を cwd にしている Pod でも、runtime workspace が現在 workspace なら表示対象に残す。
- Pod 名 prefix や cwd だけで workspace 所属を推測しない。可能な限り persisted Pod metadata / resolved runtime workspace root を authority とする。
- metadata が壊れている、または workspace 判定不能な Pod は、通常一覧に混ぜず、必要なら bounded diagnostic として扱う。
- no-Ticket workspace / Pod-centric fallback でも、現在 workspace の Pod discovery と attach/open は維持する。
- Ticket rows / queue actions / Panel Orchestrator lifecycle は維持する。

## Acceptance criteria

- `yoi panel` を workspace `A` で開いたとき、workspace `B` の Pod が通常 Pod list に表示されない。
- 現在 workspace の `workspace_orchestrator_pod`、Companion、Ticket role Pod は引き続き表示・操作できる。
- cwd が `.worktree/...` 配下でも、runtime workspace が現在 workspace なら隠されない。
- workspace 判定不能な legacy/corrupt Pod metadata が、現在 workspace の通常 row として誤表示されない。
- Panel の attach/open は、表示されている現在 workspace Pod に対して従来通り機能する。
- Focused unit tests または E2E/fixture tests で、複数 workspace の Pod metadata が存在する場合に Panel が現在 workspace の Pod だけを表示することを確認する。

## Binding decisions / invariants

- Panel 表示スコープの authority は、runtime workspace root / Pod metadata に基づく。Pod name prefix や process cwd のみでは判定しない。
- `role_workspace_root` / `original_workspace_root` / `implementation_worktree_root` / `merge_target_workspace_root` は混同しない。
- dedicated orchestration worktree を使う Orchestrator は、cwd が workspace 外に見えても、runtime workspace が元 workspace なら表示対象に残す。
- ワークスペース外 Pod を通常一覧に出してから UI 上で注意表示するのではなく、通常一覧から除外する。
- 別 workspace の Pod に対する attach/open/action path を Panel から提供しない。
- 既存 Pod metadata の破壊的 migration はこの Ticket の範囲外。必要なら escalation する。
- ユーザー承認により、metadata が古く workspace 判定不能な Pod は通常 Panel 表示から隠し、必要なら diagnostic にだけ出す。

## Implementation latitude

- workspace root の canonicalization / path comparison の具体実装は Coder が調査して選んでよい。
- 判定不能 Pod の diagnostic 表示方法は、既存 Panel diagnostic pattern に合わせてよい。
- 既存 ViewModel / Pod listing abstraction のどこで filter するかは実装調査に任せてよい。
- E2E が重すぎる場合、まず unit/fixture test で複数 workspace Pod metadata を作る focused coverage でもよい。ただし user-visible Panel 挙動の確認手段は残す。

## Readiness

- readiness: implementation_ready
- risk_flags: [panel, pod-metadata, workspace-boundary, runtime-observation]
- open_questions: none

## Escalation conditions

- 既存 metadata に runtime workspace root が十分に保存されておらず、正しい workspace 判定に schema/storage 変更が必要な場合。
- legacy Pod を隠すことで既存の復元/attach 導線が実用上失われる場合。
- workspace root canonicalization で symlink / moved checkout / worktree の扱いに人間判断が必要な場合。
- Panel の no-Ticket fallback が「全 Pod dashboard」であるべきか「current workspace Pod dashboard」であるべきか、既存設計と衝突する場合。
- 別 workspace Pod を診断用に見せる必要が出た場合。その場合も通常 action row とは分離する。

## Validation

- Focused tests for Panel ViewModel / Pod list filtering:
  - current workspace Pod is visible;
  - other workspace Pod is hidden;
  - workspace Orchestrator / role Pod for current workspace is visible;
  - cwd/worktree difference alone does not hide current workspace Pod;
  - unknown/corrupt workspace metadata is not treated as current workspace.
- Existing Panel/TUI tests continue to pass.
- If practical: Panel E2E fixture with multiple workspace Pod metadata records.
- `cargo fmt --check`
- `git diff --check`
- relevant `cargo test` / `cargo check`

## Related work

- `00001KTFQ109S`: Workspace panel Companion interface。
- `00001KTFQ109V`: Remove workspace panel direct Pod send。
- `00001KV0YK5S0`: E2E harness を完全な tmp runtime/data/workspace 隔離と cleanup に対応させる。
- Runtime workspace / Pod identity decisions in existing memory and related closed runtime-workspace Tickets。
