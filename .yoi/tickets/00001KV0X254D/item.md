---
title: 'Panel Orchestrator の orchestration branch 名を ticket.config.toml で設定可能にする'
state: 'ready'
created_at: '2026-06-13T16:29:25Z'
updated_at: '2026-06-13T16:29:41Z'
assignee: null
readiness: 'implementation_ready'
risk_flags: ['config-schema', 'git-worktree', 'panel-orchestration']
---

## Background

`00001KTTHP8HE` で、Panel Orchestrator 起動時に dedicated orchestration worktree を作成・再利用する仕組みが導入された。

現在の layout は概ね次の default 導出になっている。

- path: `<workspace>/.worktree/orchestration/<workspace-orchestrator-pod-name>`
- branch: `orchestration/<workspace-orchestrator-pod-name>`

`yoi` workspace では `workspace_orchestrator_pod` が `yoi-orchestrator` のため、branch は `orchestration/yoi-orchestrator` になる。

現状の Yoi では Ticket orchestration 設定の中心が `.yoi/ticket.config.toml` であり、ticket がほぼ orchestration の意味を持っているため、Panel Orchestrator 用 orchestration branch 名もこの設定ファイルで扱う。

## Requirements

- `.yoi/ticket.config.toml` で Panel Orchestrator の orchestration branch 名を指定できるようにする。
- 設定が存在しない場合は既存挙動を維持する。
  - default: `orchestration/<workspace-orchestrator-pod-name>`
  - `yoi` では引き続き `orchestration/yoi-orchestrator`
- 設定は typed Ticket config として扱い、Panel 専用の ad-hoc 読み取りや string literal の散在にしない。
- Schema は次のような形を基本案とする。

```toml
[orchestration]
branch = "orchestration/yoi-orchestrator"
```

- exact key name は既存 config model との整合性を見て実装時に調整してよいが、documented / tested な workspace config として扱う。
- worktree 作成・復元・reuse validation・diagnostics が、すべて同じ resolved branch 名を使う。
- hard-coded `orchestration/<stem>` は default 導出としてのみ残し、実際の lifecycle は resolved config value 経由にする。
- invalid branch name は Git 操作前に拒否し、破壊的操作をしない。
- configured branch が既存 orchestration worktree の branch と一致しない場合は、安全に診断する。
  - 既存 worktree を勝手に checkout / reset / delete しない。
  - 必要なら migration / remediation guidance を diagnostic に出す。
- Panel diagnostics で、resolved orchestration branch が分かるようにする。
- Queue / Orchestrator restore / launch context など、expected orchestration branch を前提にする経路がある場合は同じ設定を使う。

## Acceptance criteria

- `.yoi/ticket.config.toml` で orchestration branch 名を指定できる。
- 設定なしの場合、既存 default branch 名が維持される。
- custom branch 設定時、Panel Orchestrator の worktree create / reuse / restore validation が custom branch を使う。
- invalid branch 設定では Git worktree 作成前に分かる error / diagnostic で止まる。
- 既存 worktree が configured branch と異なる場合、破壊的 cleanup や silent checkout をせず、分かる diagnostic を返す。
- Panel 表示または launch diagnostic から resolved branch 名を確認できる。
- tests が追加・更新されている。
  - default branch resolution
  - configured branch resolution
  - invalid branch rejection
  - existing worktree branch mismatch diagnostic
  - Panel orchestration worktree lifecycle が resolved branch を使うこと

## Binding decisions / invariants

- 設定ファイルは `.yoi/ticket.config.toml` とする。
- 設定なしの既存挙動は維持する。
- Git worktree / branch の安全境界は緩めない。
- dirty / unknown / mismatched worktree を自動で修復・削除しない。
- branch 名設定は Orchestrator の runtime workspace / Ticket backend root の安全性を変えない。
- Ticket backend / role Profile / prompt context への hidden injection ではなく、明示的な workspace config として扱う。

## Implementation latitude

- exact config key name は、既存 config 構造との整合性を見て実装時に決めてよい。
- branch validation は Git の refname validation または同等の安全な内部 validation を使ってよい。
- worktree path も configured branch から derive すべきか、既存 path policy を維持するかは実装で判断してよい。ただし、branch config 変更時に既存 path と衝突する場合は安全な diagnostic を出すこと。
- docs / sample config の更新範囲は、現在の config documentation 体系に合わせて最小限でよい。

## Readiness

- readiness: implementation_ready
- risk_flags: [config-schema, git-worktree, panel-orchestration]
- blocking open questions: なし

## Escalation conditions

- config schema の置き場所が `.yoi/ticket.config.toml` では不自然で、Profile / manifest / panel-local config との境界判断が必要になった場合。
- branch 設定と worktree path 設定を分離しないと安全な migration path が作れない場合。
- Queue handoff / Orchestrator restore が main workspace と orchestration worktree のどちらの config を読むべきか曖昧になった場合。
- backward compatibility のために旧 branch/worktree を自動移行したくなる場合。自動移行は別判断にする。

## Validation

- `cargo test -p ticket config`
- `cargo test -p tui orchestration --lib` または該当 targeted tests
- `cargo fmt --check`
- `git diff --check`
- `target/debug/yoi ticket doctor`

## Related work

- `00001KTTHP8HE`: Panel Orchestrator 起動時に専用 orchestration worktree を自動作成・再利用する
- `00001KTCDHFPG`: Ticket config role profile mapping
- `00001KTWPE3KQ`: Panel Queue時にdevとOrchestrator worktreeを同期する
