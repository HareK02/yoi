---
title: 'Orchestratorを専用worktreeで実行し実装worktreeをworkspace root配下に作る'
state: 'planning'
created_at: '2026-06-11T03:20:32Z'
updated_at: '2026-06-11T03:20:32Z'
assignee: null
---

## 背景

現在の filesystem Ticket backend では、`.yoi/tickets` が Git 管理ファイルとして workspace に存在する。このため、ユーザーが main workspace で未投入の Ticket draft や整理中の Ticket を作るだけで、Orchestrator からも見え、Git dirty state としても扱われる。Orchestrator とユーザーが同じ worktree / branch で同時に Ticket を触る運用は、Ticket queue、draft、project record、Git commit 対象が混ざりやすい。

Ticket backend をすぐに Git 外 store へ移行するのではなく、まず filesystem backend のまま運用分離する。Orchestrator は専用の orchestration worktree を `pwd` として実行し、active Ticket queue / orchestration state をその worktree 側で扱う。実装用 coder worktree は Orchestrator の `pwd` 相対ではなく、元 workspace root から `.worktree/<ticket>` に sibling として作る。

これにより、main workspace の未投入 draft が Orchestrator に見える問題と、Orchestrator の Ticket churn が main workspace を汚す問題を減らす。

## 方針

想定する構成:

```text
<workspace-root>/
  main workspace / human workspace
  .worktree/
    orchestration/<session-or-branch>/   # Orchestrator pwd
    <ticket-or-branch>/                  # implementation coder worktrees
```

- Orchestrator は dedicated orchestration worktree で起動する。
- Orchestrator が見る `.yoi/tickets` は orchestration branch/worktree 側のものを正とする。
- ユーザーが main workspace で作る未投入 draft / local planning は、明示的に orchestration worktree へ渡すまで Orchestrator の queue ではない。
- Coder / Reviewer 用 implementation worktree は orchestration worktree から子として作らず、original workspace root の `.worktree/<ticket>` に sibling として作る。
- Implementation branch の base は原則 main/target code branch とし、orchestration branch の Ticket churn を implementation branch に混ぜない。
- Orchestrator prompt / workflow guidance では、`pwd` が orchestration worktree であることと、implementation worktree 作成時の root path を明示する。
- merge-ready dossier は原則として停止点ではなく checkpoint とする。reviewer approval・安全な target workspace・未解決 human gate なし・standing merge authority が揃う場合、Orchestrator は merge / validation / close / cleanup まで進む。

## 要件

- Workspace Panel / role launcher / Orchestrator 起動経路で、Orchestrator の working directory を専用 orchestration worktree にできる設計を行う。
- orchestration worktree の作成・再利用・命名・branch policy を定義する。
  - 例: `.worktree/orchestration/<slug>` または `.worktree/orchestrator-<date>`。
  - branch: `orchestration/<slug>` など。
  - 既存 worktree がある場合の attach/reuse/fail policy を決める。
- Orchestrator に渡す runtime context へ original workspace root を含める。
  - Orchestrator `pwd` は orchestration worktree。
  - implementation worktree root は original workspace root の `.worktree`。
- `worktree-workflow` / `multi-agent-workflow` / Orchestrator routing guidance を更新し、implementation worktree を Orchestrator `pwd` 相対に作らないようにする。
- Orchestrator merge guidance を更新し、reviewer approve 後に必要条件が揃っている場合は merge-ready dossier で止まらず merge / post-merge validation / Ticket close or done transition / worktree cleanup まで進めるようにする。
  - `resources/prompts/ticket_role/orchestrator_worktree_routing.md` の `Stop at a merge-ready dossier` 系の抑制を checkpoint 表現に変更する。
  - `resources/prompts/ticket_role/orchestrator_merge_completion.md` に、dogfooding/workspace standing merge authority がある場合の自動継続条件を明記する。
  - `.yoi/workflow/multi-agent-workflow.md` の approve 後 merge/validate/close/cleanup 方針と role prompt を矛盾させない。
  - `resources/prompts/common/pod-orchestration.md` の一般安全則は維持し、Pod tool があるだけでは authority にならないことと、authorized Orchestrator workflow による merge-completion を区別する。
- Implementation worktree creation は sibling layout を使う。
  - `<original-workspace-root>/.worktree/<ticket-or-task>`。
  - orchestration branch から nested/descendant branch を切らない。
- merge target workspace / target branch を Orchestrator `pwd` と分離して明示する。
  - Orchestrator `pwd` は orchestration worktree。
  - implementation branch/worktree は original workspace root 配下の sibling worktree。
  - merge / post-merge validation は configured target workspace / target branch で行う。
- Orchestrator 専用 worktree の Ticket record churn を main workspace にいつ・何を publish するかは、この Ticket では最小方針を定義する。
  - 少なくとも active queue / in-progress coordination は main workspace に即時反映しない。
  - resolution / important decision / final report を publish するかどうかは後続または明示操作にできる。
- UI / diagnostics で、現在 Panel/Orchestrator がどの workspace/worktree の Ticket backend を見ているか分かるようにする。
- cleanup 方針を定義する。
  - orchestration worktree をいつ消せるか。
  - implementation worktree と branch cleanup との関係。
- filesystem backend 前提の運用改善であり、Ticket store の Git 外 DB 化はこの Ticket では行わない。

## 受け入れ条件

- Orchestrator role を専用 orchestration worktree で起動する設計・実装方針が記録されている。
- Orchestrator `pwd` が orchestration worktree であっても、implementation worktree は original workspace root の `.worktree/<ticket>` に作る guidance になっている。
- Orchestrator workflow / multi-agent workflow / worktree workflow の Git/worktree 指示が sibling layout と矛盾しない。
- Orchestrator が main workspace の未投入 Ticket draft を暗黙に queue として扱わない運用境界が明文化されている。
- implementation branch に orchestration branch の Ticket churn が混ざらないことが設計上説明されている。
- Panel/launcher 側で Orchestrator の cwd / ticket backend root / original workspace root の扱いがテスト可能な形になっている。
- 既存の coder/reviewer delegation、worktree cleanup、merge-ready dossier 運用が新 layout と矛盾しない。
- reviewer approval 済みで blocker / human gate が残っておらず、standing merge authority と safe target workspace が確認できる場合、Orchestrator guidance は dossier stop ではなく merge-completion 継続を促す。
- merge-completion guidance は target workspace / target branch / post-merge validation / Ticket lifecycle transition / worktree and branch cleanup の実行場所を明確にしている。
- targeted tests または workflow-level validation が追加・更新されている。
- `target/debug/yoi ticket doctor` が通る。

## 非目標

- Ticket backend を SQLite/JSONL など Git 外 store に移行すること。
- Git を使わない orchestration storage を新規設計すること。
- Orchestrator が push すること。
- authority・reviewer approval・safe target workspace・未解決 human gate なしを確認せずに merge すること。
- main workspace の project record publication policy を完全に解くこと。
- 既存 `.yoi/tickets` の履歴を大規模に移行・rewrite すること。
