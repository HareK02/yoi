---
title: 'TicketListの出力を軽量化する'
state: 'queued'
created_at: '2026-06-09T08:52:12Z'
updated_at: '2026-06-09T10:03:41Z'
assignee: null
queued_by: 'workspace-panel'
queued_at: '2026-06-09T10:01:28Z'
---

## 背景

実セッションを `session analyze` で確認したところ、Orchestrator / Companion 系のセッションで `TicketList` の tool result が大きくなりやすいことが分かった。

観測例:

- `TicketList` result が 20KB〜65KB 程度になるケースがある。
- 一覧系 tool は頻繁に使われるため、1回の大きな summary が history/context を圧迫しやすい。
- Coder / Reviewer では `TicketList` は主因ではなかったが、Orchestrator / Companion では Ticket 一覧が routing / cleanup / backlog 判断の起点になりやすい。

TicketList は人間/AI の探索入口として必要だが、default output はもっと bounded にしてよい。

## ゴール

`TicketList` の default output を減量し、必要十分な bounded summary にする。詳細は `TicketShow` に委ねる。

## 要件

- `TicketList` default output の情報量を再設計する。
- default では item body / thread / artifacts の詳細を含めない。
- 1 Ticket あたりの summary を短くする。
  - canonical id
  - title
  - state
  - priority 相当が残る場合のみ
  - updated_at または queued_at など、必要な最小 timestamp
  - attention/blocking hint がある場合の短い indicator
- title や diagnostics が長い場合は bounded に切り詰める。
- `limit` の default / max を見直す。
  - default は current UI/LLM usage に適した小さい値にする。
  - max も context blow-up を起こしにくい上限にする。
- `state=all` / closed を含む一覧は特に慎重に制限する。
- 必要なら detail-heavy mode を明示 opt-in にする。
  - 例: `--verbose` / `--json-full` / separate command。
  - ただし default tool result は軽量に保つ。
- Tool output と CLI human output の両方を確認する。
  - LLM-facing TicketList tool は特に bounded であるべき。
  - CLI は人間可読性を保ちつつ、巨大 JSON/Markdown を default で出さない。
- Orchestrator が詳細判断する場合は `TicketShow <id>` を読む前提にする。
  - List は selection / triage / backlog overview 用。
  - routing / close / planning return / implementation acceptance の authority にはしない。

## 非目標

- TicketShow の詳細を削ること。
- Ticket backend schema を変更すること。
- Ticket relation / Objective / OrchestrationPlan の設計を変えること。
- List だけで Orchestrator が routing 判断できるようにすること。

## 受け入れ条件

- `TicketList` default result が現在より明確に小さい bounded summary になる。
- Large open/all Ticket set でも tool result が context を過度に圧迫しない。
- `TicketList` は detail authority ではなく selection/overview 用であることが docs/tool description/workflow guidance から分かる。
- `TicketShow` は詳細確認の authority として引き続き使える。
- Tests cover:
  - long title truncation;
  - large list limit behavior;
  - all/closed state listing cap;
  - JSON/tool output shape;
  - no body/thread leakage in list output.
- `target/debug/yoi ticket doctor`, focused tests, `cargo fmt --check`, and `git diff --check` pass.
