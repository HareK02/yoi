---
title: 'Workspace panel: show Ticket-associated Intake Pods adjacent to Ticket rows'
state: 'ready'
created_at: '2026-06-13T10:54:31Z'
updated_at: '2026-06-14T14:12:11Z'
assignee: null
readiness: 'implementation_ready'
risk_flags: ['panel-ux', 'local-role-session-registry', 'pod-session-state']
---

## Background

Panel からの操作・表示で、チケットに紐づく Intake Pod をチケットと並べて見たい。

`00001KTFTQDBR` で workspace-scoped local role session registry と Ticket claim index は実装済み。既存実装により Ticket と local Intake Pod/session の関連付け自体は表現されているが、Panel 表示上は subtitle や独立した Pod list に埋もれやすい。Ticket row を見たときに、その Ticket に対応する Intake Pod が隣接・関連表示され、open/attach する対象として認識しやすい状態にする。

既存の設計上の前提:

- local Pod assignment は git-tracked Ticket metadata/thread に書かない。
- Ticket は高々 1 active local Pod claim を持つ。
- Intake Pod/session は Ticket と 1:1 とは限らない。
- pre-Ticket Intake session は Ticket 未紐づけのまま存在できる。
- Panel は Ticket state / local overlay / Pod metadata を join して表示してよい。
- 自動 polling や自動 Intake spawn はしない。

今回の作業は registry/schema の再設計ではなく、Panel 上での関連 Pod 表示・操作導線の改善に限定する。

## Requirements

- Workspace Panel の Ticket 表示で、Ticket に紐づく Intake Pod がチケットと隣接して認識できるようにする。
  - 例: Ticket row の直下/隣接 row、子 row、明示的な inline/column 表示など。
  - 具体的なレイアウトは既存 Panel UI と整合する範囲で実装側に裁量を残す。
- 関連判定は既存の local role/session registry、Ticket claim、Pod metadata を使う。
- Intake role の Pod/session を明確に区別して表示する。
- live / restorable / stale の claim status が分かるようにする。
- Ticket に紐づく Intake Pod を選択・Enter/Open した場合は、可能ならその Pod を open/attach する導線にする。
- 既存の「Ticket を選んで Intake を開始する」挙動では、既存 claim がある場合に二重起動せず、関連 Intake Pod を開く/案内する既存方針を維持する。
- Ticket と無関係な pre-Ticket Intake Pod は、無理に Ticket row に紐づけて表示しない。
- 表示は bounded にし、Panel が大量 Pod/Ticket で読みにくくならないようにする。

## Acceptance criteria

- Ticket に local Intake claim / related Intake session がある場合、Panel 上でその Intake Pod が Ticket と隣接・関連表示される。
- 表示は単なる subtitle だけに埋もれず、ユーザーが「この Ticket の Intake Pod」と認識できる。
- live / restorable / stale の状態が確認できる。
- 関連 Intake Pod の open/attach 導線が Panel 操作から使える、または既存の open/attach 操作へ明確に誘導される。
- Ticket claim の one-active-claim invariant は維持される。
- local Pod assignment は `.yoi/tickets` の git-tracked records に書かれない。
- pre-Ticket Intake Pod は Ticket 未紐づけのまま表示され、特定 Ticket に誤って関連付けられない。
- focused test で、Ticket row と関連 Intake Pod 表示の ViewModel/row ordering または rendering contract が確認される。

## Binding decisions / invariants

- Ticket metadata/frontmatter/thread に local Pod name、socket、claim state、runtime status を保存しない。
- Panel 表示改善のために automatic polling / automatic Intake spawn を追加しない。
- selected arbitrary Pod direct-send UX を復活させない。
- Intake Pod と Ticket の関係を 1:1 と仮定しない。
- 既存 local role/session registry を基本入力とし、新しい durable Ticket schema は導入しない。

## Implementation latitude

- UI 表現は既存 Panel の行構造に合わせて選んでよい。
  - Ticket row の下に associated Pod row を挿入する。
  - Ticket row に role/status chips を追加する。
  - Ticket-focused row group として表示する。
- 既存の `related_pods` / `local_claim` ViewModel を拡張するか、新しい typed row kind を追加するかは実装側判断でよい。
- 既存 subtitle 表示を残すか整理するかは、重複して読みにくくならない範囲で判断してよい。

## Readiness

- readiness: implementation_ready
- risk_flags: [panel-ux, local-role-session-registry, pod-session-state]

## Escalation conditions

- Panel の selection model / keyboard semantics を大きく変える必要が出た場合。
- one-active-claim-per-Ticket を崩さないと目的を満たせないように見える場合。
- pre-Ticket Intake と existing-Ticket Intake の表示分類が曖昧になり、誤関連付けのリスクがある場合。
- Registry schema migration が必要になりそうな場合。

## Validation

- `cargo test -p tui workspace_panel --lib`
- 関連箇所を触る場合:
  - `cargo test -p tui multi_pod --lib`
  - `cargo test -p tui role_session_registry --lib`
- `cargo fmt --check`
- `git diff --check`

## Related work

- `00001KTFTQDBR` — Workspace panel local role session registry
- `00001KTFQ109S` — Workspace panel Companion interface
- Code areas:
  - `crates/tui/src/workspace_panel.rs`
  - `crates/tui/src/multi_pod.rs`
  - `crates/tui/src/role_session_registry.rs`
