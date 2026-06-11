---
title: 'runtime workspace と process cwd を分離する'
state: 'done'
created_at: '2026-06-11T15:45:07Z'
updated_at: '2026-06-11T15:59:55Z'
assignee: null
queued_by: 'yoi ticket'
queued_at: '2026-06-11T15:45:53Z'
---

## 背景

Ticket role Pod、とくに workspace Orchestrator は、runtime workspace として original workspace を使い続けながら、Bash / file tool の default cwd だけ dedicated orchestration worktree にしたい。

直前の調査で `--workspace` を dedicated worktree に寄せたり、`--tool-cwd` のような追加 CLI surface を作る方向は不適切だと確認した。`--workspace` は runtime workspace / project context の基準であり、process cwd は通常の current directory として扱えばよい。

また、コード内で `pwd` と `cwd` に明確な意味差がない箇所は `cwd` に統一し、用語混乱を減らす。

## 要件

- `--workspace` は runtime workspace / project context の基準として維持する。
- process current directory は Pod の tool default cwd として起動時に snapshot する。
- Ticket role Orchestrator は `--workspace = original workspace` のまま、process cwd を dedicated orchestration worktree にして起動・restore される。
- `--tool-cwd` のような追加 CLI surface は作らない。
- `SpawnConfig` など host-side launch config で必要なら `cwd` を内部値として持ち、child process の `Command::current_dir` にだけ使う。
- `pwd` と `cwd` に意図的な意味差がない runtime field / helper / local 変数は `cwd` に統一する。
- cwd は authority ではないため、scope / delegation / workspace identity の基準として扱わない。

## 受け入れ条件

- Orchestrator role launch/restore が original workspace を runtime workspace として使い、dedicated orchestration worktree を process cwd として使う。
- child Pod へ `.worktree` 配下を delegate できる authority は original workspace profile/scope に基づいて評価される。
- `--tool-cwd` などの新しい CLI argument が残らない。
- `pwd` / `cwd` の命名が混在していた主要 runtime path は `cwd` に統一される。
- 既存の `SpawnPod.cwd` semantics は「child process cwd を指定する」で維持され、runtime workspace を変更しない。
- `nix build .#yoi` が通る。
