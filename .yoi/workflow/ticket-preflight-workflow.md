---
description: 互換 slug を残した Ticket planning / requirements sync workflow。preflight を独立 lane や workflow_state として扱わない。
model_invokation: true
user_invocable: true
requires: []
---

# Ticket Planning / Requirements Sync Workflow

このファイル名は既存の workflow discovery / durable references 互換のために残す。新しい概念としての `preflight` state / lane / long-lived operation は作らない。ここで扱う作業は、Ticket を `planning` に戻して不足している決定・情報・受け入れ条件を同期するための planning activity である。

## 目的

実装に入る前または Orchestrator routing 中に、具体的な未決定事項が見つかった Ticket を planning に戻し、Ticket thread に監査可能な形で同期内容を残す。

この workflow は次をしてはいけない。

- `preflight` を workflow_state として扱う。
- `preflight` vocabulary を current Ticket metadata として新規に書く。
- 「リスクがある」だけで Ticket を戻す。
- Coder / Reviewer / worktree mechanics を再設計する。

## 適用条件

次のいずれかを満たす場合に使う。

- `planning` Ticket の要件・受け入れ条件・制約を明確化する。
- `ready` または `queued` Ticket について、Orchestrator が実装開始前に具体的な不足情報・未決定事項を特定した。
- 既存 Ticket に obsolete state vocabulary が残っており、planning terminology へ整理する必要がある。

適用しない条件:

- Orchestrator が具体的な不足項目を言語化できない。
- 単に変更範囲が広い、リスクが高い、またはレビュー観点が多いだけである。
- すでに `queued -> inprogress` が記録され、実装 side effect が始まっている。

この場合は Ticket を戻さず、IntentPacket に escalation / reviewer focus を明記して進める。

## 手順

1. Ticket の current frontmatter と recent thread を読む。
2. 不足している decision / information / acceptance condition を箇条書きで特定する。
3. `ready` または `queued` から戻す場合は、typed state change で `to = planning` を記録する。reason/body には具体的な不足項目を含める。
4. 既存の claimed live/restorable Intake/Planning Pod があり、利用可能な通知経路がある場合は、その Pod に同じ不足理由を通知する。実用的な経路が無い場合は follow-up として report する。
5. Ticket body または thread に requirements sync 結果を残す。
6. Ticket が queue 可能になったら `planning -> ready` を typed state change / `TicketIntakeReady` で記録する。

## 記録テンプレート

```markdown
## Planning sync

Missing decisions / information:
- ...

Decisions made:
- ...

Acceptance criteria changes:
- ...

Risk / reviewer focus:
- ...

Readiness:
- Keep in planning because ...
- or mark ready because ...
```

## 完了条件

- Ticket に具体的な不足項目または解決済み decision が記録されている。
- `planning` に戻した場合、state_changed event に from/to/reason/body が残っている。
- `ready` に進める場合、未解決の blocking attention/action が残っていない。
