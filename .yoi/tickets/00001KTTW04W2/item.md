---
title: 'Orchestrator進捗をAutoKickなしでCompanionへ通知する'
state: 'inprogress'
created_at: '2026-06-11T08:15:24Z'
updated_at: '2026-06-12T15:40:14Z'
assignee: null
queued_by: 'workspace-panel'
queued_at: '2026-06-11T10:31:56Z'
---

## 背景

Workspace Panel / Orchestrator 運用では、Orchestrator が Ticket を queue から取り、coder/reviewer の spawn、review、merge-ready、merge/validation/close/cleanup へ進める。現在はユーザーが状況把握したい場合、Panel の rows、Ticket thread、Pod output、Orchestrator output を個別に見る必要があり、把握コストが高い。

一方で、queued work の starvation 防止として Orchestrator に AutoKick / re-kick を送る設計は別問題である。ユーザーが欲しいのは、Orchestrator を勝手に再起動・再促進することではなく、Companion が「今 Orchestrator がどこまで進めたか」「どこで止まっているか」「次に人間が見るべきものは何か」を把握できる通知である。

この Ticket は、Orchestrator の Ticket 消化進捗を Companion に通知・共有し、Companion が状況説明をしやすくするためのものとする。通知は AutoKick ではなく、Orchestrator に追加 action を促す side effect を持たない。

## ゴール

Companion が Workspace の Orchestrator 進捗を低コストで説明できるようにする。ユーザーが Companion に聞けば、Orchestrator の current progress、active Ticket、blocked/waiting reason、recent transitions、必要な人間確認を概観できる。

## 要件

- Orchestrator の進捗通知は AutoKick / re-kick ではない。
  - Orchestrator Pod に追加 user input / system input を送らない。
  - queued work を再提示して Orchestrator を動かす目的では使わない。
  - Orchestrator が idle かどうかの制御や scheduler 代替にしない。
- `Method::Notify` に `auto_run: bool` (default: true) を追加し、Companion 向けの弱い進捗通知では `auto_run: false` を使う。
  - `auto_run: true` は既存挙動を維持し、idle Pod なら `RunForNotification` を staged する。
  - `auto_run: false` は `NotifyBuffer` に積むだけで、idle Pod を起こさない。
  - running/paused Pod では既存 drain path に従い、次の turn/resume/run で history に入る。
- Companion に対して、Orchestrator progress を read-only context として共有する。
  - active Ticket / recently handled Ticket。
  - state transition: `queued -> inprogress`、review requested、review approved、merge-ready、merged、done/closed、blocked など。
  - running coder/reviewer Pod の概況。
  - waiting reason / blocker / human gate。
  - last validation / merge / cleanup result の summary。
- Companion の権限境界を維持する。
  - Companion は direct implementation worker ではない。
  - Ticket mutation、SpawnPod、merge、worktree cleanup などの authority を持たない default profile のままにする。
  - 通知を受けても、Companion が Orchestrator の代わりに作業を進めない。
- 通知の authority source を明確にする。
  - Pod completion notification だけを authority にしない。
  - Ticket state/thread、role session registry、Pod list / Pod output、Orchestrator lifecycle state など、durable/queryable state から再構成可能な情報を優先する。
  - 通知は UX hint であり、Companion が必要なら source を確認できるようにする。
- 通知内容は bounded にする。
  - 全 Ticket thread や長い Pod output を丸ごと Companion context に流さない。
  - Ticket id/title/state、role pod name/status、短い reason、次に確認すべき artifact/path 程度に制限する。
  - secret/private context、provider errors の sensitive detail、unbounded logs を含めない。
- Panel / Companion lifecycle と統合する。
  - Companion が live/reachable の場合に progress notice を届けるか、Companion が読むための local progress snapshot を更新する。
  - Companion が stopped/missing の場合、通知で spawn/restore しない。初期実装では persistent snapshot store は作らず、live/reachable Companion への `Notify { auto_run: false }` に限定する。
  - `auto_run: false` の通知は弱い通知であり、drain 前に Pod が停止すれば失われ得ることを許容する。
  - durable progress snapshot は必要性が出るまで導入しない。
- UI 上の通知と LLM context injection の境界を守る。
  - Companion へ model context として渡すなら、必ず history に残る形で渡す。
  - history に残らない transient context injection はしない。
  - 単なる Panel 表示 notice と Companion への context message を分ける。
- Prompt / workflow 文言を Rust code に直書きしない。
  - LLM-facing summary framing が必要な場合は `resources/prompts` 側に置く。

## 受け入れ条件

- Orchestrator の Ticket 消化 progress を、live/reachable Companion が `Notify { auto_run: false }` 経由の read-only weak notification として受け取れる。
- progress notification は Orchestrator AutoKick / re-kick を発生させない。
- Companion が missing/stopped の場合に通知だけで自動 spawn/restore せず、初期実装では persistent snapshot も作らない。
- Companion default profile の tool/feature policy は強化されず、実装・Ticket mutation・Pod spawn・merge authority を持たない。
- 通知内容は bounded で、Ticket id/title/state、role pod status、short reason、artifact/path references を中心にする。
- 通知内容は durable/queryable state から作るが、`auto_run: false` 通知自体は weak/best-effort として扱い、未drain状態の永続化は要求しない。
- Companion model context に渡す情報は history に残る形で扱われ、context-only injection にならない。
- Panel から、Companion が持つ Orchestrator progress context の鮮度または last updated が分かる。
- targeted tests が追加・更新されている。
  - `Method::Notify { auto_run: false }` が idle Pod に `RunForNotification` を staged しないこと。
  - live Companion への `Notify { auto_run: false }` delivery。
  - missing/stopped Companion で spawn/restore しないこと。
  - bounded summary generation。
  - sensitive/unbounded content を含めないこと。
- `cargo test -p tui` または該当 targeted tests、`cargo fmt --check`、`git diff --check`、`target/debug/yoi ticket doctor` が通る。

## 非目標

- Orchestrator starvation 防止 / AutoKick / re-kick の実装。
- Companion に Ticket mutation、Pod spawn、merge、worktree cleanup の authority を与えること。
- full project management dashboard を作ること。
- 全 Ticket thread / Pod output を Companion に常時流すこと。
- Orchestrator の scheduler を Companion に移すこと。
