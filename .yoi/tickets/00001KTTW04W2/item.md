---
title: 'Orchestrator進捗をAutoKickなしでCompanionへ通知する'
state: 'planning'
created_at: '2026-06-11T08:15:24Z'
updated_at: '2026-06-11T08:15:24Z'
assignee: null
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
  - Companion が stopped/missing の場合、通知で spawn/restore しない。次回 Companion 起動時に読める snapshot として扱う案を検討する。
- UI 上の通知と LLM context injection の境界を守る。
  - Companion へ model context として渡すなら、必ず history に残る形で渡す。
  - history に残らない transient context injection はしない。
  - 単なる Panel 表示 notice と Companion への context message を分ける。
- Prompt / workflow 文言を Rust code に直書きしない。
  - LLM-facing summary framing が必要な場合は `resources/prompts` 側に置く。

## 受け入れ条件

- Orchestrator の Ticket 消化 progress を Companion が説明できる read-only context として受け取れる、または起動時に参照できる。
- progress notification は Orchestrator AutoKick / re-kick を発生させない。
- Companion が missing/stopped の場合に通知だけで自動 spawn/restore しない。
- Companion default profile の tool/feature policy は強化されず、実装・Ticket mutation・Pod spawn・merge authority を持たない。
- 通知内容は bounded で、Ticket id/title/state、role pod status、short reason、artifact/path references を中心にする。
- 通知は durable/queryable state から再構成できる情報を source とし、Pod completion notification だけを authority としない。
- Companion model context に渡す情報は history に残る形で扱われ、context-only injection にならない。
- Panel から、Companion が持つ Orchestrator progress context の鮮度または last updated が分かる。
- targeted tests が追加・更新されている。
  - AutoKick が発生しないこと。
  - live Companion への notice delivery または snapshot update。
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
