---
title: 'E2E harness を完全な tmp runtime/data/workspace 隔離と cleanup に対応させる'
state: 'inprogress'
created_at: '2026-06-13T16:56:11Z'
updated_at: '2026-06-13T16:56:58Z'
assignee: null
readiness: 'ready'
queued_by: 'yoi ticket'
queued_at: '2026-06-13T16:56:31Z'
---

## 背景

E2E harness は `00001KSKBP9YG` / `00001KV0TJVN5` で Panel PTY E2E、最新 `yoi` binary build、tested subprocess の env isolation を導入した。しかし、ユーザーから `yoi-orchestrator-orchestrator` / `workspace-orchestrator` などの Pod/worktree artifact が出現したとの報告があり、host runtime / Pod registry / worktree artifact isolation と cleanup がまだ十分に証明されていない。

既知の問題:
- 初期 E2E は `env_clear()` 前に `XDG_RUNTIME_DIR` など host env を継承し得た。
- Fixture は `workspace` / `workspace-orchestrator` の Pod metadata を作るが、これは fixture-local でなければならない。
- 現在の env isolation は host env leak を防ぐが、E2E が完全に clean な tmp runtime/data/workspace で動き、実行後に cleanup することを明示的に保証・検証していない。

## 要件

- E2E は毎回完全に clean な temporary environment を作って実行する。
- Workspace / HOME / XDG_DATA_HOME / XDG_STATE_HOME / XDG_CONFIG_HOME / runtime dir / artifacts root を fixture ごとに分離する。
- Tested `yoi` subprocess は host runtime / Pod registry / session / worktree / data dir を見ない。
- Fixture で作る Pod metadata（例: `workspace`, `workspace-orchestrator`）は fixture-local であり、host/global registry に出ない。
- 実行後、fixture runtime/data/workspace/temp dirs は成功・失敗に関係なく cleanup される。失敗時に必要な artifact は `target/e2e-artifacts/...` にコピーしてから cleanup する。
- Cleanup policy / fixture root / runtime dir / data dir / removed paths を artifact に記録する。
- 既存の binary provider、env credential isolation、mouse capture tracking、quit pending barrier を壊さない。

## 受け入れ条件

- `cargo test -p yoi-e2e --features e2e --test panel -- --nocapture` が clean tmp env を使い、終了後に fixture temp root を残さない。
- E2E artifact から fixture workspace/data/runtime paths と cleanup result が確認できる。
- Test または assertion により、Panel が host live Pods / host runtime registry を見ていないことを確認する。
- Fixture-created `workspace-orchestrator` 等が fixture-local であり、cleanup 後に temp root ごと消えることを確認する。
- Host `XDG_RUNTIME_DIR` などを設定した状態でも tested `yoi` は fixture runtime だけを見る。
- `cargo fmt --check`、`git diff --check`、関連 `cargo check` / E2E tests が pass する。

## 関連

- `00001KSKBP9YG`: E2E harness first slice。
- `00001KV0TJVN5`: E2E binary provider / env isolation follow-up。
