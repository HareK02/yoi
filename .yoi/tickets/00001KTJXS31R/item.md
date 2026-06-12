---
title: "Orchestrator Idle 時の queued Ticket 見落としを防ぐ"
state: 'closed'
created_at: "2026-06-08T06:12:35Z"
updated_at: '2026-06-12T16:12:42Z'
queued_by: 'workspace-panel'
queued_at: '2026-06-12T14:49:40Z'
---

## 背景

現在の Panel Queue automation は、主に次の遷移タイミングのイベントを扱っている。

```text
ready -> queued
  -> workspace Orchestrator に通知
```

これだけでは、安定した orchestration には足りない。通知漏れ、Orchestrator restore/spawn、planning への差し戻し、capacity 制限、複数 Ticket の調整などにより、`queued` Ticket が残り続けることがある。

ただし、この Ticket は常時 polling する scheduler を作るものではない。目的は、実行可能な queued work があり、Orchestrator Pod の state が `Idle` で、`active_inprogress` が導出されていないときにだけ、bounded な work set を渡して starvation を防ぐことである。

## ゴール

Orchestrator が `queued` work を見落とさず、かつ `active_inprogress` が導出されている間に無駄な re-kick を繰り返さないための **session-lifetime work set discovery / re-kick policy** を実装する。

Orchestrator Pod の state が `Idle` で、進められる work が存在する場合は、bounded attention により次の inspection または acceptance/routing に進める。一方で、implementation side effects は必ず `queued -> inprogress` acceptance の記録後に限定し、blind spawn や duplicate start を起こさない。

runtime 側で kick 可能かを見る判定は、Orchestrator Pod の state が `Idle` であることに限定する。re-kick を抑制するかどうかは、session-lifetime work set、role/session claims、visible Pod/worktree state から `active_inprogress` の有無として導出する。

## 現在の前提

- authoritative な Ticket lifecycle は frontmatter の `state` で表す。
- `new_queued` / `planned_queued` / `active_inprogress` は新しい core Ticket state ではなく、現在の Ticket `state`、session-lifetime work set、role/session claims、visible Pod/worktree state から導出する分類として扱う。
- work set は Task と同じく session-lifetime の runtime state として扱い、Ticket ごとの durable artifact log として積まない。
- work set が失われた場合でも、Ticket `state = queued` から `new_queued` として再検出できればよい。失われた session-level ordering / waiting reason は再 inspection で作り直す。
- Panel / lifecycle hook は Orchestrator に attention / kick を与えてよいが、unattended scheduler loop や常時 polling にはしない。
- `queued` は Orchestrator が routing / start-if-unblocked を検討できる状態であり、実装・Pod spawn・worktree 作成などの side effect は `queued -> inprogress` 記録後に限る。

## Work-set classification

実装上は少なくとも次の区別を導出できるようにする。

- `new_queued`: Ticket `state = queued` だが、現在の Orchestrator session work set にまだ取り込まれていない Ticket。
- `planned_queued`: Orchestrator が確認し、session work set の order / waiting set に置いたが、まだ `inprogress` として acceptance していない queued Ticket。
- `active_inprogress`: Orchestrator が acceptance 済みで、coder/reviewer/planning-sync/merge/cleanup などの delegated step の完了待ちとして記録・観測できる Ticket。
- `actionable_inprogress`: `inprogress` だが、次の action が delegated step の完了待ちではなく、Orchestrator の routing/判断/記録を必要とする Ticket。

この分類の名前は内部実装名として固定しなくてよいが、意味上の区別は必要である。

## 要件

### Work set discovery / re-kick

- Orchestrator attention が必要な Ticket を、少なくとも次の情報から導出する。
  - Ticket frontmatter の `state`。
  - Orchestrator session-lifetime work set。
  - role/session claims。
  - visible Pod/worktree state。
- Panel open、Orchestrator restore/spawn、明示的な user action などの境界で、actionable work がある場合に bounded work list を Orchestrator へ提示できるようにする。
- 無制限な background polling は避ける。明示イベント、Panel lifecycle kick、明示 user/Orchestrator action を優先する。
- duplicate start を防ぐ。re-kick は inspection または次の planned item の acceptance を促すものであり、coder Pod を blind spawn しない。

### Re-kick / starvation-prevention semantics

- `new_queued` work が存在し、Orchestrator Pod の state が `Idle` の場合、Orchestrator に kick/notify して inspection と session work set への取り込みを促す。
- `active_inprogress` が導出されず、Orchestrator Pod の state が `Idle` で、unblocked かつ capacity/policy 上開始可能な `planned_queued` work がある場合、Orchestrator に kick/notify して次の Ticket の acceptance/routing に進める。
- `active_inprogress` が導出されている場合、queued/planned queued work が存在するだけでは re-kick しない。
- `planned_queued` work を開始しない理由が dependency / conflict / dirty workspace / capacity / human gate 等で説明できる場合は、session work set 上の bounded waiting/blocking reason として保持する。
- re-kick は attention signal と bounded context であり、`queued -> inprogress` acceptance や inspection を迂回する authority ではない。

### Session work set semantics

Orchestrator は、意味のある routing 境界で session work set を更新する。

- new queued work を確認し、session work set に取り込んだとき。
- `queued -> inprogress` acceptance を記録したとき。
- `inprogress` Ticket の次の action が delegated step 待ちか、Orchestrator action かを判断したとき。
- capacity stop により planned queued / waiting と reason を残すとき。
- merge-ready / done に到達し、`active_inprogress` が導出されなくなったため次の planned queued Ticket を検討するとき。

session work set は bounded で、Orchestrator の現在 session における判断補助として扱う。

- 何を取り込んだか。
- 次に acceptance / routing すべき候補は何か。
- 何を waiting としたか。
- waiting の理由は何か。
- `active_inprogress` として re-kick を抑制する対象は何か。

これらは project-level の永続ログではなく、Task と同様に session lifetime の状態でよい。ユーザー判断や Ticket lifecycle に残すべき内容が生じた場合だけ、Ticket comment / state transition / resolution など既存の durable surface に記録する。

## 非目標

- Panel 自体を scheduler にすること。
- `queued` になっていない Ticket を自動開始すること。
- `active_inprogress` が導出されている間に継続的な re-kick を行うこと。
- Orchestrator の inspection と `queued -> inprogress` acceptance なしに coder/reviewer Pod を spawn すること。
- full dependency graph solver を最初の実装で作ること。
- `new_queued` / `planned_queued` / `active_inprogress` を core Ticket state として追加すること。
- volatile な orchestration work set を Ticket ごとの durable artifact log として保存すること。

## 受け入れ条件

- Orchestrator Pod の state が `Idle` のとき、`new_queued` work を検出して bounded work-list attention または session work set への取り込みに進める。
- `active_inprogress` が導出されず、Orchestrator Pod の state が `Idle` で、`planned_queued` work が unblocked かつ capacity/policy 上開始可能なとき、Orchestrator が次の acceptance/routing を行える。
- `active_inprogress` が導出されている間は、queued/planned work の存在だけで re-kick しない。
- 開始しない `planned_queued` work には、session work set 上でユーザーに提示できる bounded waiting/blocking reason が残る。
- 既存の human gate、`queued -> inprogress` acceptance step、dirty-workspace/dependency/conflict/capacity checks を迂回しない。
- Ticket state、session work set、role/session claims、visible Pod/worktree state を確認し、duplicate Orchestrator/coder/reviewer/worktree start を起こさない。
- missed/stale queued Tickets を、ユーザーが手で再 queue しなくても Orchestrator に提示できる。
- Orchestrator session work set を失っても、`queued` Ticket を再検出して安全に再 inspection できる。

## 検証

- `nix build .#yoi` を通す。
- Ticket / panel / orchestrator routing 周辺の既存テストまたは追加テストで、少なくとも次の主要分岐を確認する。
  - Orchestrator Pod `Idle` state での queued detection。
  - `active_inprogress` 導出時の re-kick suppression。
  - session work set 上の waiting reason 保持。
  - duplicate-start prevention。
  - session work set が空でも `queued` Ticket から再検出できること。
- 実装報告では、work-set classification に使った情報と、implementation side effects が `queued -> inprogress` 後に限定されていることを明示する。
