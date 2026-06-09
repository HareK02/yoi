---
title: 'action_requiredとattention_requiredをTicket schemaから削除する'
state: 'inprogress'
created_at: '2026-06-09T09:55:18Z'
updated_at: '2026-06-09T12:10:16Z'
assignee: null
queued_by: 'workspace-panel'
queued_at: '2026-06-09T10:11:38Z'
---

## 背景

`action_required` / `attention_required` は Ticket frontmatter overlay として曖昧で、state / relation / body sections / thread event と責務が重複している。

特に `attention_required` は Panel で強い意味を持っており、non-empty の場合は `state: ready` でも Queue できず、blocked/red + `Edit` action になる。実際には「実装時の注意事項」程度の内容が `attention_required` に入ったことで、ready Ticket が誤って Queue 不能になった。

今後は以下の分担に寄せる:

- Queue / routing を止める不足は `state: planning` と typed state/thread reason で表す。
- dependency / blocker は typed Ticket relation metadata で表す。
- 実装時の注意、不変条件、相談条件は Ticket body の `Binding decisions / invariants` / `Escalation conditions` に書く。
- 中長期の判断軸は Objective に置く。
- 一時 UI notice は Panel/TUI local state に置く。

## ゴール

`action_required` と `attention_required` を current Ticket schema / tool API / Panel action 判定から削除する。

## 要件

- Ticket frontmatter schema から `action_required` と `attention_required` を削除する。
  - New Ticket はこれらの fields を書かない。
  - `ticket doctor` はこれらを要求しない。
  - current records ではこれらの fields が無い状態を正とする。
- Ticket create / tool input / output から `action_required` / `attention_required` を削除する。
  - `TicketCreate` params から削除する。
  - `TicketList` / `TicketShow` の current metadata output から削除する。
  - 必要なら legacy parser compatibility は短期 migration 用に限定し、current output には出さない。
- Panel の action 判定から `attention_required` blocker を削除する。
  - `state: ready` は、unresolved relation blocker など明確な blocker が無ければ Queue 可能にする。
  - 人間判断が不足している場合は `state: planning` に戻す運用にする。
- Existing docs / workflows / tests / examples を更新する。
  - `attention_required` を human-attention overlay として説明している古い記述を削除または置換する。
  - `action_required` を current Ticket field として扱う記述を削除する。
- Existing Ticket records から field は削除済みだが、schema 削除後も doctor が通ることを確認する。
- Historical thread/body mentions は audit/history として残してよい。ただし current docs/examples として使う場合は更新する。

## 非目標

- Typed Ticket relation metadata の実装。
- Objective の実装。
- Panel の新しい確認 UX の設計。
- `state: planning` に戻す policy 全体の再設計。

## 受け入れ条件

- `action_required` / `attention_required` が current Ticket schema から消えている。
- `TicketCreate` / `TicketList` / `TicketShow` がこれらを current fields として露出しない。
- Panel は `attention_required` の有無で ready Ticket を blocked/Edit 扱いしない。
- Queue/routing を止める理由は state / relation / typed thread reason のいずれかで表現される。
- Tests cover:
  - ready Ticket without relation blockers derives Queue action;
  - historical/legacy `attention_required` があっても current behavior を壊さないか、または doctor で明確に診断される;
  - Ticket create/list/show output に fields が出ない。
- `target/debug/yoi ticket doctor`, focused tests, `cargo fmt --check`, and `git diff --check` pass.
