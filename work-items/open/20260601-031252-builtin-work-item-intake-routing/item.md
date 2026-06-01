---
id: 20260601-031252-builtin-work-item-intake-routing
slug: builtin-work-item-intake-routing
title: Built-in work item management and intake routing
status: open
kind: task
priority: P1
labels: [work-item, intake, orchestration, tui]
created_at: 2026-06-01T03:12:52Z
updated_at: 2026-06-01T05:26:27Z
assignee: null
legacy_ticket: null
---

## Issue

現在の work item / ticket 運用は、Insomnia システムから切り離しても使えるように `tickets.sh` と markdown files で管理している。一方で、チケットは実質的に WorkItem と統合された構造化データとして運用され始めており、Intake Pod / Orchestrator / coder / reviewer の並行開発フローでは、チケット管理自体を Insomnia の feature として扱う必要が出てきている。

特に、ユーザーが単一 Orchestrator に依頼してから Orchestrator が調査・チケット化・実装委譲する現在の入口では、ユーザー待ち時間が残る。ユーザーの submit から Intake Pod が直接起動し、ユーザーと意図・要件をすり合わせたうえで正式な WorkItem / ticket を作成し、Orchestrator はそのチケット群の順序付け・割り込み・実装 orchestration に集中する構造へ移行したい。

## Direction

- `tickets.sh` 相当の work item 管理を、Insomnia の built-in feature / tool surface として設計・実装する。
- Intake Pod はユーザーと直接会話し、必要な調査・重複確認・要件同期を行い、合意済み WorkItem / ticket を作成できるようにする。
- Orchestrator はユーザー意図の一次解釈や ticket draft の再承認者ではなく、登録済み WorkItem の scheduling / prioritization / interruption / implementation delegation を担当する。
- ユーザー向け Inbox 専用 UI は初期スコープにしない。Intake は既存の `--multi` UI の延長として通常の Pod 会話で制御できるようにする。
- Orchestrator が WorkItem を見て不十分と判断し、Intake と Orchestrator 間だけで合意できない場合は、Intake Pod に action-required flag を持たせ、既存の TUI polling / multi-Pod 表示経路でユーザー対応要求として表示する。

## Scope / permission note

Scope は、エージェントが占有して作業する filesystem space を保証するための仕組みである。WorkItem / ticket 管理を built-in tool として提供する場合、その tool は単なる delegated workspace edit と同じ扱いにせず、既存の `tickets.sh` / Bash と同様に work item authority のための専用経路として設計してよい。

ただし、これは無制限のファイル編集許可を意味しない。WorkItem tool は対象を work item store に限定し、通常の code/worktree scope とは別の authority boundary と auditability を持つべきである。

## Requirements

- `tickets.sh` で行っている主要操作を、Insomnia の built-in work item management surface として扱えるようにすること。
  - create
  - show/list
  - comment / plan / decision / implementation report
  - review approve/request-changes
  - status transition
  - close / resolution
  - doctor / consistency check
- WorkItem / ticket の保存形式は、現在の markdown + frontmatter + thread / artifacts 方式との移行可能性を保つこと。
- 既存の `tickets.sh` 運用を即座に破壊しないこと。built-in 化の途中でも git history / work-items directory を authority として読めること。
- Intake Pod がユーザーと合意済み WorkItem を作成できる導線を設計すること。
- Intake Pod は正式作成前に、必要に応じて既存 ticket / duplicate / related work / relevant files を調査できること。
- Intake Pod が作った WorkItem には、ユーザー合意済みであること、背景、要件、acceptance criteria、non-goals、risk flags、needs-preflight などを構造化して残せること。
- Orchestrator は作成済み WorkItem queue を見て、実装順序・割り込み・preflight 要否・coder/reviewer 起動を判断できること。
- Orchestrator が WorkItem 不十分と判断した場合、必要な質問や修正要求を Intake Pod 経由でユーザーに戻せること。
- 専用 Inbox storage / UI を初期要件にしないこと。action-required state は Pod metadata / current state に近い形で持ち、既存 multi-Pod polling / display で見えるようにすること。
- action-required Pod は、ユーザーが対応するまで見落とされにくい表示・状態を持つこと。
- Intake 結果や WorkItem 操作は audit 可能で、git history / work item thread に経緯が残ること。
- secret / private context を WorkItem 本文・thread・artifact・diagnostics に不用意に書かないこと。
- work item store への並行書き込みが壊れないよう、atomicity / locking / conflict handling を設計すること。

## Acceptance criteria

- Insomnia 内部から、少なくとも `tickets.sh create/show/list/comment/review/close/doctor` に相当する操作を型付きに実行できる。
- Intake Pod がユーザーと合意した内容を正式 WorkItem として作成できる。
- Orchestrator は Intake が作成した WorkItem を、再解釈 gate なしに scheduling 対象として扱える。
- WorkItem が不十分な場合は、Orchestrator が action-required state を通じて Intake / user に差し戻せる。
- `--multi` 系 UI で、通常の Pod 状態に加えて user action required な Intake Pod を認識できる。
- 専用 Inbox storage を導入しなくても、未対応のユーザー確認要求が見落とされない。
- 既存 `work-items/` と git history に基づく運用・監査が継続できる。
