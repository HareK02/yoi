---
title: 'Orchestratorに独立queued workの並列開始を促す'
state: 'planning'
created_at: '2026-06-09T10:17:32Z'
updated_at: '2026-06-09T10:17:32Z'
assignee: null
---

## 背景

現行の Orchestrator workflow は安全側の制約が強く、並列稼働は許可されているが積極的に促されていない。

現状の強い指示:

- Queue notification だけで blind spawn しない。
- `queued -> inprogress` acceptance 前に worktree / coder Pod / reviewer Pod を起動しない。
- dependency / conflict / capacity / risk / current workspace state を確認する。
- OrchestrationPlan は automatic scheduler ではなく、Orchestrator が読んで明示判断する。

一方で、以下の指示が弱い:

- independent queued Tickets が複数あり capacity が空いているなら、1件ずつ完了待ちするのではなく並列に受理・起動する。
- idle capacity を残さないよう active work set を更新する。
- Orchestrator は safety checks を満たした範囲で parallel work を優先する。

結果として、Orchestrator が保守的に 1 Ticket ずつ処理し、coder/reviewer capacity を使い切らない傾向がある。

## ゴール

Orchestrator / multi-agent workflow guidance を更新し、独立した queued work があり capacity が空いている場合は、明示的な safety checks を満たした上で並列稼働を優先するようにする。

## 要件

- `ticket-orchestrator-routing` に parallel capacity policy を追加する。
- `multi-agent-workflow` の並列運用 guidance を、注意事項だけでなく積極的な scheduling/acceptance stance として補強する。
- Orchestrator は automatic scheduler ではないが、明示的に起動された routing / queue review / panel kick の中では idle capacity を残さないように判断する。
- 複数 queued Tickets がある場合、Orchestrator は以下を確認した上で並列開始を優先する:
  - each Ticket is queued and human-authorized for routing;
  - each Ticket has no unresolved `depends_on` / incoming blocking relation;
  - no `do_not_parallelize` / conflict / shared write-scope constraint blocks concurrent work;
  - separate worktree / branch / write scope can be created;
  - coder/reviewer capacity is available and trackable;
  - each Ticket can be accepted with `queued -> inprogress` before side effects;
  - existing active inprogress work is not waiting for a bottleneck that makes new work unsafe.
- If parallel start is not chosen despite available queued work, Orchestrator should record a bounded reason:
  - dependency;
  - conflict;
  - capacity;
  - missing planning decision;
  - workspace dirty state;
  - reviewer/coder bottleneck;
  - user/human gate.
- OrchestrationPlan records may help express waiting/capacity/conflict decisions, but they are not automatic scheduler authority.
- The guidance should distinguish:
  - active work that is waiting on coder/reviewer completion, where re-kick should not thrash;
  - idle Orchestrator with queued/planned work, where starting another independent Ticket is preferred.
- Keep all existing safety invariants:
  - no implementation side effects before `queued -> inprogress`;
  - no blind spawn from notification alone;
  - no shared write scope between coder Pods;
  - reviewer remains read-only unless explicitly scoped;
  - dependency/conflict relations are respected.

## 非目標

- Implementing an automatic background scheduler.
- Starting unqueued Tickets.
- Ignoring relation blockers or workspace conflicts.
- Implementing OrchestrationPlan store changes.
- Changing Pod runtime scheduling.

## 受け入れ条件

- Orchestrator workflow says independent queued work with available capacity should be started in parallel after explicit checks, rather than waiting one Ticket at a time by default.
- Multi-agent workflow includes active parallel work-set management guidance, not only safety notes.
- If Orchestrator leaves queued work idle while capacity appears available, it records a reason.
- Safety invariants around `queued -> inprogress`, worktree isolation, scope separation, relation blockers, and review loop remain explicit.
- Focused prompt/workflow tests or snapshot tests are updated if present.
- `target/debug/yoi ticket doctor` and `git diff --check` pass.
