---
title: '実装ブランチをOrchestrator branch HEADから切る'
state: 'planning'
created_at: '2026-06-12T04:34:05Z'
updated_at: '2026-06-12T04:34:15Z'
assignee: null
---

## 背景

Orchestrator 用 dedicated worktree を導入したことで、Panel / human が操作する root workspace と、Orchestrator が Ticket backend として見る orchestration worktree は別 checkout になった。

この設計では、Queue handoff 後の Orchestrator は orchestration branch 上で routing、`queued -> inprogress` acceptance、waiting reason、implementation intent などの durable Ticket 記録を作る。そのため、Orchestrator が作成するチケット実装用 branch は root workspace の `develop` から直接切るのではなく、Orchestrator workspace の現在 HEAD、つまり orchestration branch HEAD から切る必要がある。

実装 worktree を配置する場所と、実装 branch の base は別の概念である。

```text
実装 worktree の配置場所:
  original workspace 配下の .worktree/<task>

実装 branch の base:
  Orchestrator workspace の current HEAD / orchestration branch HEAD
```

現状の prompt / routing guidance では「implementation worktree は original workspace 配下に作る」ことは書かれているが、「implementation branch は Orchestrator branch HEAD から切る」ことが明示されていない。そのため Orchestrator が original workspace / merge target workspace の `develop` を base と誤解し、Orchestrator branch 上の Ticket 記録を含まない implementation branch を作る余地がある。

## ゴール

Orchestrator が child implementation worktree / branch を作るとき、worktree の配置場所は original workspace 配下に保ちつつ、branch base は Orchestrator workspace の current HEAD にすることを prompt / launch context / helper 実装で明確にする。

## 要件

- Orchestrator routing guidance は、次を明示する。
  - implementation worktree は `original_workspace_root/.worktree/<task>` 配下に作る。
  - implementation branch は Orchestrator workspace の current HEAD / orchestration branch HEAD から切る。
  - merge target workspace / `develop` から implementation branch を直接切らない。
- `worktree-workflow` または該当する dogfood workflow / prompt が、同じ base branch rule を明示する。
- Ticket role launch context の `implementation_worktree_root` は配置場所の authority であり、branch base の authority ではないことを LLM-facing text で区別する。
- merge completion guidance は、implementation branch を orchestration branch に戻し、その後 orchestration branch を merge target workspace / `develop` に戻す順序を明示する。
- helper code が implementation worktree 作成コマンドを組み立てている場合、その base が root/develop に固定されていないことを確認し、必要なら Orchestrator workspace HEAD を使うよう修正する。
- 既存の workspace/cwd 分離、Ticket backend が cwd 基準で動く設計を崩さない。

## 非目標

- implementation worktree の配置場所を Orchestrator worktree 配下に変えること。
- merge target workspace / `develop` を廃止すること。
- Queue handoff の dev -> orchestration sync policy をこの Ticket で実装すること。
- Orchestrator branch から develop への最終 merge policy 全体をこの Ticket で実装すること。

## 受け入れ条件

- Orchestrator 向け prompt / workflow guidance が、implementation branch base を Orchestrator workspace HEAD と明示している。
- `original_workspace_root` / `implementation_worktree_root` は worktree 配置先であり、branch base ではないことが分かる文面になっている。
- `develop` / merge target workspace から implementation branch を直接切るよう読める指示が残っていない。
- merge completion guidance が `implementation branch -> orchestration branch -> merge target develop` の順序を示している。
- 関連する prompt/resource 変更を含めて `nix build .#yoi` が通る。
