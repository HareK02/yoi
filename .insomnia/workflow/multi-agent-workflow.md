---
description: worktree と子 Pod を使って複数 ticket の実装・レビュー・修正・完了処理を並列に進める orchestration フロー
model_invokation: true
user_invocable: true
requires: []
---
# Multi-agent Worktree Workflow

insomnia を insomnia で開発する際の、worktree + 実装 Pod + 親 Pod review の標準フロー。これは **実装を並列に進めるためのフロー** であり、worktree の機械的作成手順は `$user/worktree-workflow`、ticket 候補選定や方針探索の半自動 loop は `$user/auto-maintain` に分ける。

## 目的

- 実装差分を ticket ごとの child worktree に隔離する。
- 実装 Pod に narrow write scope を渡して並列実装させる。
- 親 Pod が diff / test / ticket 要件を review し、必要なら修正依頼する。
- approve 後に merge / ticket 完了処理 / main workspace での再検証を行う。

## 開始条件

以下が揃っている時に使う。

- 対象 ticket が決まっている。
- ticket の背景・要件・完了条件から実装方針が概ね導ける。
- worktree 作成と git 書き込み操作について、人間の許可がある。
- main workspace の unrelated dirty changes を把握している。

設計方針が複数自然に導ける場合、protocol / scope / permission / history persistence に触れる場合、ticket 自体の再定義が必要な場合は、実装委譲前に人間へ戻す。

## 親 Pod / orchestrator の責務

1. 状態確認
   - `git status --short --branch`
   - 対象 ticket
   - 関連 TODO / docs / 既存 worktree

2. worktree 作成
   - `$user/worktree-workflow` に従い `./.worktree/<task-name>` を作る。
   - `.insomnia` を sparse checkout で除外する。

3. 実装 Pod spawn
   - read scope: main workspace 全体。
   - write scope: child worktree、または必要最小 directory。
   - task には以下を明示する。
     - child worktree path / branch
     - 対象 ticket path
     - Bash は必ず child worktree に `cd` すること
     - main workspace の `TODO.md` / `tickets/` / `docs/report/` / `.insomnia` は編集しないこと
     - 範囲外事項
     - 実行すべき build / test / format
     - 完了報告項目

4. 監督
   - `ReadPodOutput` で報告を読む。
   - 通知が来ない場合でも、worktree の `git status` / `git diff` / test で完了状態を確認する。
   - 必要なら `SendToPod` で修正依頼する。

5. review
   - ticket の背景・要件・完了条件・範囲外に照らして diff を確認する。
   - build / test / `git diff --check` を確認する。
   - 必要なら reviewer Pod を read-only で立てる。

6. merge / lifecycle
   - approve 後に main workspace へ merge する。
   - `TODO.md` から該当行を削除し、`tickets/foo.md` を削除して完了 commit を作る。
   - main workspace で必要な test / `cargo check --workspace` / `cargo fmt --check` を再実行する。

## 実装 Pod の責務

- child worktree 内でのみ実装する。
- main workspace の管理ファイルを書かない。
- 指定された build / test / format を実行する。
- ticket 要件外の設計変更、依存関係追加、scope / permission / history persistence / prompt context 加工原則に触れる変更が必要なら止めて報告する。
- 完了時に以下を報告する。
  - worktree path / branch
  - commit hash（commit した場合）
  - 変更ファイル
  - 実装概要
  - 実行した build / test / format
  - 未解決事項
  - review に回せるか

## 実装 Pod の commit 方針

実装 Pod には child worktree 内での commit を許可してよい。

- commit は ticket 内で意味のある粒度にする。
  - 例: `feat: ...`、`fix: ...`、`test: ...`、`docs: ...`
- 実装 Pod は merge / push / branch deletion / worktree remove をしない。
- 実装 Pod は `TODO.md` / `tickets/` の完了処理 commit をしない。
- 親 Pod は review 時に commit 粒度も確認する。
- 必要な修正は、原則追加 commit として積む。履歴改変や squash は人間の明示指示がある時だけ行う。

## Review → 修正 → 完了の標準形

### Approve

1. 実装 Pod を停止し、scope を回収する。
2. 親 Pod が main workspace で `git merge --no-ff <branch>` する。
3. 親 Pod が `TODO.md` と `tickets/foo.md` を完了処理して commit する。
4. main workspace で検証コマンドを再実行する。
5. 変更内容・commit・検証結果・残 dirty changes を報告する。

### Request changes

1. blocking finding をファイル / 行 / 理由 / 修正方針つきで整理する。
2. 実装 Pod が生きていれば `SendToPod` で修正依頼する。
3. 停止済みなら、同じ worktree / branch / scope で再 spawn するか、親 Pod が最小修正する。
4. 修正後に focused test と必要な broader test を再実行する。
5. 再 review する。

### Non-blocking comments

- ticket 要件外の改善はその場で混ぜない。
- 必要なら後続 ticket / docs/report にする。
- non-blocking を理由に completion を遅らせない。

## 並列実装時の注意

- 1 ticket = 1 worktree = 1 branch を基本にする。
- 複数 Pod に同じ write scope を渡さない。
- parent は child の write scope 配下を直接編集しない。
- 依存関係がある ticket は、土台 branch を merge してから次 worktree を切る。
- parallel に走らせた Pod の完了通知は取りこぼしうるため、`ReadPodOutput` と worktree 状態で確認する。

## 完了報告の標準形

```text
完了:
- ticket: <path>
- branch: <name>
- commits:
  - <hash> <subject>
- 変更概要: ...
- 検証:
  - cargo fmt --check
  - cargo check --workspace
  - cargo test ...
- review: approve / approve with comments / request changes
- 未解決事項: ...
- 残 dirty changes: ...
```

## この Workflow で扱わないもの

以下は `$user/auto-maintain` または別の設計相談で扱う。

- ticket 候補を見繕うこと。
- 新規 ticket 作成判断。
- QA feedback / AI feedback を ticket / report / workflow に落とす判断。
- 長期 maintainer loop / WorkItemStore / LeaseStore の設計。
