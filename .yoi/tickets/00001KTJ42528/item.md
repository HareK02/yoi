---
title: "Ticket lifecycle commit noise を減らし commit / publication policy を一本化する"
state: 'closed'
created_at: "2026-06-07T22:43:09Z"
updated_at: '2026-06-18T12:22:10Z'
---

## 背景

Ticket-driven / multi-agent 運用では、Ticket record の lifecycle event ごとに小さな commit が増えやすい。典型的には、1つの実装 Ticket だけでも以下のような commit 列になる。

```text
ticket: planning ...
ticket: delegate ...
feat/fix: ...
ticket: report implementation ...
ticket: approve ...
merge: ...
ticket: close ...
```

この運用は監査 trail を細かく残せる一方で、project の Git history が Ticket lifecycle event で過度に細分化される。さらに Panel / Intake / Orchestrator が Ticket record を更新するたびに `.yoi/tickets` が dirty になり、main workspace での人間の draft、Orchestrator の queue / progress、project record として残すべき履歴が混ざりやすい。

もともと `00001KTJ1YA5G` は Panel / Intake の Ticket state change を auto-commit するか dirty として見せるかを扱う narrow ticket だった。しかし Orchestrator 専用 worktree / filesystem backend 分離の方針により、Panel / Intake の auto-commit 可否だけを単独で決める前提は古くなった。

この Ticket は `00001KTJ1YA5G` を吸収し、Ticket lifecycle 全体の低ノイズ commit policy と、orchestration worktree から main project history へ何を publish するかの policy を一本化して定義する。

## ゴール

Ticket audit trail を失わずに、Git commit 数と main workspace の dirty noise を減らす。Panel / Intake / Orchestrator / manual orchestration が生成する Ticket record について、どの状態変更を即時 commit するか、どの状態変更を batch するか、どの記録を main workspace に publish するかを明確にする。

## 方針

- Ticket thread / item / resolution に残る event や evidence は削らない。
- Git commit grouping を粗くし、1 event = 1 commit の機械的運用を避ける。
- Panel / Intake の state transition policy はこの Ticket に統合する。
- Orchestrator 専用 worktree 方針を前提に、active queue / in-progress coordination と main project history を分けて考える。
- main workspace に publish するのは、project record として価値がある節目に寄せる。
- 実装 code change と Ticket policy / lifecycle record の commit は、混ぜる必要がある場合を除き分ける。

## 要件

- Git history を Ticket record の重要な audit / publication layer として維持する。
- ただし、active orchestration coordination のすべてを main workspace の Git history に即時反映する前提にはしない。
- Ticket lifecycle event の batch 方針を決める。
  - planning + delegation は、同じ orchestration step なら `ticket: delegate ...` などの1 commit にまとめてよい。
  - implementation report + reviewer approval は、commit 前に approval まで揃っているなら `ticket: approve ...` などにまとめてよい。
  - follow-up Ticket 作成は、同じ会話・判断 burst で複数作成された場合、1つの `ticket: add ... followups` 系 commit にまとめてよい。
  - close / resolution は原則として節目 commit とするが、publication policy と矛盾しない範囲で明示的に batch 可否を決める。
- Panel / Intake / Orchestrator による Ticket-only update の扱いを決める。
  - `planning -> ready`
  - `ready -> queued`
  - `queued -> inprogress`
  - comments / intake summaries / handoff reports
  - review / approval / implementation report
  - close / done transition / resolution
- Panel / Intake が main workspace で Ticket draft や state change を作った場合、それを即時 auto-commit するのか、dirty state として明示するのか、orchestration worktree へ promote するのかを定義する。
- Orchestrator 専用 worktree 上の Ticket record を、main workspace にいつ・どの粒度で publish するかを定義する。
  - active queue / transient progress は main に即時 publish しない選択肢を検討する。
  - accepted requirements、重要 decision、implementation report、review approval、resolution など、project record として残すべき節目を分類する。
- workflow / prompt / docs を更新し、agent が Ticket event ごとに機械的に commit しないようにする。
- auto-commit を導入・維持する場合は、対象 Ticket path だけを stage し、 unrelated user changes を含めない。
- auto-commit しない場合は、Panel / actionbar / diagnostics などで dirty Ticket state と必要な次 action を明確に示す。
- review / approval / merge / close の evidence は files 上に残し、commit grouping の変更で監査性を失わない。
- 過去 Git history の rewrite は行わない。

## 受け入れ条件

- `00001KTJ1YA5G` の scope がこの Ticket に吸収されていることが記録されている。
- Panel / Intake の Ticket state change について、auto-commit / dirty-state / promote-to-orchestration-worktree の policy が明文化されている。
- Orchestrator / multi-agent workflow で生成される Ticket lifecycle record の batch commit 方針が文書化されている。
- Orchestrator 専用 worktree から main workspace へ publish する record の種類と粒度が定義されている。
- `/multi-agent-workflow`、`/worktree-workflow`、Ticket role prompt / workflow guidance が、低ノイズ commit / publication policy と矛盾しない。
- Ticket close / merge / approval evidence が引き続き auditable な file record として残る。
- unrelated user changes を巻き込む broad auto-commit を避ける方針が明記されている。
- 必要な実装変更がある場合、targeted tests が追加・更新されている。
- `target/debug/yoi ticket doctor`、`cargo fmt --check`、`git diff --check` が通る。

## 非目標

- Ticket backend を Git 外 DB に移行すること。
- 過去の Git history を rewrite して既存 commit noise を整理すること。
- implementation code changes を Ticket lifecycle policy update に混ぜること。
- Orchestrator が push すること。
- project record として残すべき evidence を削ること。
