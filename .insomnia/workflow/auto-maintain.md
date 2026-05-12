---
description: TODO.md と tickets/ から半自動で実装候補を選び、worktree・実装 Pod・reviewer・人間確認を使い分けて完了候補まで進める
model_invokation: false
user_invocable: true
requires: []
---
# Auto Maintain Workflow

半自動 maintainer として、`TODO.md` と `tickets/` を俯瞰し、実装できる作業を選び、必要に応じて worktree / 実装 Pod / reviewer Pod を orchestration する。最終判断、git commit / merge / push、ticket 完了削除は人間に戻す。

この Workflow は常駐 scheduler ではない。ユーザーが `/auto-maintain` を明示的に呼んだ時だけ動く。

この Workflow は親 Pod / orchestrator 専用である。実装 Pod に `/auto-maintain` を渡してはならない。実装 Pod には、親 Pod が選んだ ticket、既に用意した child worktree、許可された write scope、禁止事項を具体的に渡す。

## 基本方針

- main workspace は制御面として扱う。
  - `.insomnia/`
  - maintainer inbox / trial log
- 実装差分は原則 child git worktree に隔離する。
- child worktree には `.insomnia` を置かない。必要なら `/worktree-workflow` の手順に従い sparse checkout で `.insomnia` を除外する。
- 実装 Pod と reviewer Pod は原則分ける。ただし scope 衝突や作業粒度により、親 Pod が review してよい。
- review artifact や完了候補の記録は main workspace 側に置き、実装 Pod には書かせない。
- git commit / merge / push / ticket 完了削除は行わず、人間へ完了候補として報告する。

## 事前調査

最初に以下を読む。

1. `TODO.md`
2. 対象候補の `tickets/*.md`
3. 関連 docs / report
4. 既存 review file があれば `tickets/*.review.md`

TODO と tickets の不整合を見つけた場合は、勝手に ticket を作成・削除せず、人間へ報告する。

## 着手候補の選び方

優先する作業:

- ticket の要件と完了条件が具体的
- 影響範囲が限定的
- build / test で確認しやすい
- scope / permission / history 永続化 / prompt context 加工原則に触れない
- narrow write scope を切りやすい

初回試走や小さな運用確認では、TUI 表示改善など局所的な ticket を優先する。

避ける、または人間確認してから進める作業:

- 複数の設計方針が自然に導け、将来構造に影響する
- permission / scope / Pod lifecycle / history persistence / prompt context 加工原則に触れる
- manifest cascade や protocol wire を広く変える
- test 不能または再現不能
- ticket 自体の大幅な要件変更が必要

## エスカレーション基準

以下では作業を止めて人間へ確認する。

- ticket の要件から複数の設計方針が自然に導ける
- scope / permission / history 永続化 / prompt context 加工原則など安全モデルに触れる
- 新 ticket 追加、既存 ticket の大幅変更、ticket 完了削除が必要
- git commit / merge / push / branch cleanup などの git 書き込み操作が必要
- `git worktree add` など、作業ツリー作成の許可が明示されていない
- テスト不能、再現不能、作業範囲外の不具合に遭遇した
- child worktree に `.insomnia` が出てしまった

## Worktree 作成

実装差分を隔離する必要がある場合、親 Pod が `/worktree-workflow` の手順を使う。実装 Pod に worktree 作成を任せてはならない。要点は以下。

```bash
git worktree add .worktree/<task-name> -b <task-name>

git -C .worktree/<task-name> sparse-checkout init --no-cone
git -C .worktree/<task-name> sparse-checkout set --no-cone \
  '/*' \
  '!/.insomnia/' \
  '!/.insomnia/**'
```

`git worktree add` は git 書き込み操作なので、ユーザーが明示的に許可していない場合は実行前に確認する。

## 実装 Pod の spawn

実装 Pod を使う場合は、対象 ticket、既に作成済みの child worktree path、write scope、禁止事項を明示する。実装 Pod は `/auto-maintain` や `/worktree-workflow` を実行せず、与えられた worktree 内で実装・確認・報告だけを行う。

推奨 scope:

- read: main workspace 全体
- write: child worktree 内の必要最小ディレクトリ

実装 Pod への依頼には以下を含める。

- 対象 ticket と完了条件
- 作業対象は child worktree であり、Bash は `cd <child-worktree> && ...` で実行すること
- `.insomnia` / `TODO.md` / `tickets/` / review artifact / inbox は書かないこと
- git commit / merge / push はしないこと
- build / test / format の結果を報告すること
- 未解決事項と人間確認が必要な点を報告すること

子 Pod の出力は `ReadPodOutput` で確認し、追加依頼が必要なら `SendToPod` を使う。不要になった Pod は `StopPod` する。

## Review

実装 Pod 完了後、原則として実装 Pod を停止して scope を回収してから review する。

reviewer Pod を使う場合は read-only を基本にする。reviewer には以下を依頼する。

- ticket の背景・要件・完了条件を読む
- diff が要件を満たすか確認する
- 既存設計を歪めていないか確認する
- 不必要な実装や過剰な抽象がないか確認する
- build / test 結果が妥当か確認する

review artifact を書く場合は、親 Pod が main workspace 側に書く。child worktree 内の `tickets/` に書く運用は scope 衝突を起こしやすいため避ける。

## 修正依頼

review 指摘があれば、指摘内容をまとめて実装 Pod に再依頼する。既存 Pod を止めている場合は、同じ child worktree と narrow write scope で再 spawn する。

修正後は build / test を再確認し、必要なら再 review する。

## 完了候補報告

最後は作業を完了削除せず、次の形式で人間へ報告する。

```text
完了候補:
- ticket: <path>
- worktree: <path>
- branch: <name>
- 実装概要: ...
- 変更ファイル: ...
- build/test: ...
- review: approve / changes requested / skipped
- 未解決事項: ...
- 人間に戻す判断:
  - commit するか
  - merge するか
  - ticket / review file を更新または削除するか
  - TODO.md から削除するか
```

## 試走記録

この Workflow 自体の試走では、対象 ticket、scope 方針、実装 Pod / reviewer の使い分け、停止した理由、不足点を記録する。不足が見つかっても、この Workflow 内で永続ジョブキュー、scope handoff、Pod sleep、ticket lifecycle 再設計を実装しない。必要なら別 ticket として人間に提案する。
