---
title: 'Panel から ready Ticket を指示付きで planning に戻して Intake を再開できるようにする'
state: 'inprogress'
created_at: '2026-06-13T10:54:34Z'
updated_at: '2026-06-13T18:41:25Z'
assignee: null
readiness: 'implementation_ready'
risk_flags: ['panel-action', 'ticket-lifecycle', 'role-session', 'authority-boundary']
queued_by: 'workspace-panel'
queued_at: '2026-06-13T16:33:26Z'
---

## Background

`yoi panel` では `ready` Ticket に対して主に `Queue` 操作が提示される。

しかし、ユーザー確認なしに Ticket が `ready` になった場合や、`ready` 後に追加指示・要件修正・詳細化が必要になった場合、Panel から安全に `planning` に戻して Intake を再開する導線がないと困る。

`ready` は「人間が Queue できる状態」であり、「ユーザーが詳細化を諦めた状態」ではない。Panel は Queue gate だけでなく、ユーザーが明示的に planning / Intake に戻す操作も提供する必要がある。

## Requirements

- Panel の `ready` Ticket 行に、Queue とは別の「planning に戻して詳細化する」操作を追加する。
  - UI label / key は実装時に既存 action model に合わせて決めてよい。
  - 例: `Clarify`, `Revise`, `Back to planning`, `Intake` など。
- 操作はユーザー入力の追加指示 / 理由を受け取れること。
  - 指示は Ticket thread に durable に記録する。
  - 指示なしで実行する場合も、少なくとも bounded な理由または diagnostic を残す。
- 操作実行時は、現在の Ticket authority を再読込し、対象 Ticket がまだ `ready` であることを確認してから mutation する。
- `ready -> planning` の状態変更は typed Ticket backend / Rust Ticket API を使って記録する。
  - shell out や手書きファイル編集で代替しない。
  - reason は `user_requested_refinement` / `panel_return_to_planning` 相当の具体的な値にする。
- 状態変更後、同じ Ticket を対象に Intake role Pod を restore / launch して、追加指示を渡す。
  - 既存の live/restorable Intake claim がある場合はそれを優先して restore / notify する。
  - claim がない場合は既存の Intake launch path を使ってよい。
- Intake restore / launch に失敗した場合でも、Ticket が `planning` に戻ったことと、再開失敗の bounded diagnostic がユーザーに見えること。
  - silently `ready` に戻したり、Queue に進めたりしない。
- この操作は implementation side effect ではない。
  - Orchestrator queue / coder / reviewer spawn / worktree 作成を起こさない。
- Intake が再詳細化した後に `ready` へ戻すかどうかは、通常の Intake 合意 / readiness flow に従う。
  - 単に restore / launch しただけで自動的に `ready` にしない。

## Acceptance criteria

- Panel の `ready` Ticket 行から、Queue 以外に planning / Intake 再詳細化操作を選べる。
- 操作時にユーザー指示を保存でき、その指示が restored/launched Intake に渡る。
- 操作は stale view ではなく現在の Ticket state を再確認してから `ready -> planning` を記録する。
- `ready` でない Ticket に対しては安全に拒否し、理由を表示する。
- `ready -> planning` の typed `state_changed` event と、ユーザー指示 / refinement request が Ticket thread に残る。
- Intake Pod の restore / launch が試行され、成功 / 失敗が Panel 上で bounded に表示される。
- 操作によって Queue、`queued -> inprogress` acceptance、worktree 作成、coder/reviewer spawn は発生しない。
- Focused tests が少なくとも以下をカバーする。
  - ready Ticket の planning return 成功。
  - stale / non-ready Ticket の拒否。
  - ユーザー指示が Ticket thread と Intake launch context に入ること。
  - Intake restore / launch 失敗時の diagnostic。
  - Queue 操作の既存 semantics が壊れていないこと。

## Binding decisions / invariants

- Panel は scheduler ではない。
- この操作は implementation routing ではなく、ユーザー主導の requirements sync / refinement への戻しである。
- `ready` はユーザーが Queue できる状態であって、追加詳細化を禁止する状態ではない。
- `ready -> planning` はユーザー明示操作と指示 / 理由付きでのみ行う。
- `queued` / `inprogress` Ticket を戻す操作はこの Ticket の範囲外とする。
  - それらは Orchestrator ownership / acceptance semantics に関わるため、必要なら別 Ticket で扱う。
- Ticket mutation は typed backend / Rust API 経由に限定する。

## Implementation latitude

- Panel action label、key binding、composer / modal / prompt 形式は既存 Panel input model に合わせて選んでよい。
- Intake restore と launch のどちらを優先するかは、既存 role-session claim / Panel role registry の設計に合わせて実装してよい。
- 状態変更と Intake 起動を完全 atomic にできない場合は、Ticket state を durable に戻したうえで、起動失敗を明確に表示・記録する方針でよい。
- 既存の Ticket action dispatch / local role session registry / Intake launch helper を再利用する。

## Readiness

- readiness: implementation_ready
- risk_flags: [panel-action, ticket-lifecycle, role-session, authority-boundary]

## Escalation conditions

- `queued` / `inprogress` から planning へ戻す操作も同時に必要になった場合。
- Intake restore / launch に必要な role-session claim 情報が現在の Panel から安全に取得できない場合。
- 実装に Ticket lifecycle transition graph の変更が必要になった場合。
- Panel action model では指示入力 UI を安全に表現できず、composer model の設計変更が必要になった場合。

## Validation

- Focused Panel / workspace_panel tests。
- Ticket lifecycle transition tests where applicable。
- Intake launch / role-session claim tests where applicable。
- `cargo test -p tui workspace_panel`
- `cargo test -p ticket`
- `cargo run -q -p yoi -- ticket doctor`
- `cargo fmt --check`
- `git diff --check`

## Related work

- `00001KTDPFY8R` Workspace panel Ticket action dispatch
- `00001KTJ4QFP0` Replace Ticket intake state with planning state
- `00001KTK1FPYG` Require project context before Orchestrator returns Tickets to planning
- `00001KTFX202R` Workspace panel Orchestrator queue automation
