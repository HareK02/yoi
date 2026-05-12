# Ticket lifecycle commit の配置ミス

日付: 2026-05-10

## 要旨

今回の `memory-usage-metrics` の review / merge workflow では、ticket lifecycle の状態遷移 commit を feature branch ではなく `develop` 側に作ってしまった。実装 branch を merge する前に review artifact 作成と ticket 完了削除を行う、という順序だけでなく、「その操作をどの branch / worktree で行うか」を workflow が強制・確認できていないことが問題だった。

## 今回起きたこと

`memory-usage-metrics` の実装 branch を review した後、親 worktree で以下を行った。

1. `tickets/memory-usage-metrics.review.md` を作成
2. `tickets/memory-usage-metrics.md` にレビュー状態を追記
3. `develop` に merge

その後、完了時には `.review.md` と ticket 本体を消す必要がある、という指摘を受けて、いったん merge を巻き戻して以下の順序に直した。

1. review commit
2. ticket / review / TODO 削除 commit
3. merge commit

しかし、この修正も `develop` 側で review commit と ticket 完了 commit を作っており、feature branch 側の lifecycle としては不正だった。最終的には、これらの commit を `memory-usage-metrics` branch に移し、`develop` は feature branch を merge するだけの形に直した。

最終形は次のようになった。

```text
*   merge: memory usage metrics              # develop
|\
| * docs(tickets): complete memory usage metrics
| * review: memory usage metrics
| * feat: add memory usage event metrics     # memory-usage-metrics
|/
* docs(tickets): complete memory phase naming cleanup
```

## 障壁

### 1. Ticket lifecycle の「順序」と「配置」が別々に失敗しうる

現行 lifecycle は、作成・レビュー・完了を commit の履歴で表現する。ただし、守るべき制約は少なくとも二つある。

- review commit の後に completion commit を置く
- それらを対象 ticket の作業 branch 側に置く

今回、最初の修正では前者だけを直し、後者を落とした。workflow の説明や自分のチェック観点が「merge 前に `.review.md` を消す」へ寄りすぎており、「それを向こうの branch でやる」という branch placement を明示的な検査項目にしていなかった。

### 2. 親 worktree で操作していると lifecycle commit を `develop` に作りやすい

reviewer / orchestrator は親 worktree にいることが多い。そこで ticket file を編集すると、自然に `develop` 上の変更になる。

一方で、このプロジェクトの運用では ticket / review artifact も feature branch の成果物として扱うべきで、`develop` の first-parent に review や ticket 完了 commit が直接並ぶのは望ましくない。親 worktree でレビュー文面を作ること自体は可能でも、commit 先は feature branch worktree でなければならない。

### 3. Git 履歴の見え方を確認するタイミングが遅かった

`git log --graph` は確認したが、最初は「merge commit ができたか」「ticket が消えているか」を見ており、first-parent 上に lifecycle commit が混ざっていないかを確認していなかった。

この問題は `git log --first-parent --oneline` を merge 前後に見るだけで早期に検出できる。feature branch workflow では、merge 前の `develop` first-parent に review / completion commit が増えていたら誤りである。

### 4. 無関係な `AGENTS.md` 変更を commit に巻き込みかけた

review commit 作成時に、既存の unrelated な `AGENTS.md` 変更を一度 commit に含めてしまい、soft reset で修正した。これは branch placement とは別問題だが、同じ workflow の中で起きた安全確認不足である。

`git add` の対象を明示しても、既に index に入っている unrelated change があると混入しうる。作業前に `git status --short` だけでなく `git diff --cached --stat` を確認する必要がある。

## 影響

- `develop` の first-parent に、feature branch 内で完結すべき review / ticket 完了 commit が直接混ざる
- ticket lifecycle の履歴を辿る時に、対象 branch の作業としてまとまって見えない
- merge 前後の状態遷移が曖昧になり、後から履歴を直すために reset / cherry-pick / re-merge が必要になる
- unrelated change の混入リスクが上がる

## 暫定運用

feature branch の ticket を review / complete するときは、次を標準手順にする。

1. 親 worktree ではなく対象 branch worktree に移動する
2. `git status --short --branch` で現在 branch を確認する
3. review artifact を作成し、ticket にレビュー状態を追記して commit する
4. completion として ticket / review / TODO を削除して commit する
5. 親 worktree の `develop` に戻り、対象 branch を merge する
6. `git log --first-parent --oneline -5` で `develop` first-parent に lifecycle commit が直接載っていないことを確認する
7. `git status --short` と `git diff --cached --stat` で unrelated change が混ざっていないことを確認する

## 改善案

### A. Workflow checklist に branch placement を明記する

「レビューを書いたか」「ticket を消したか」だけでは足りない。ticket lifecycle commit は対象 branch に置く、という項目を review / merge workflow の checklist に入れる。

### B. Merge 前検査をコマンド化する

merge 前に以下を確認する小さな doctor / script があるとよい。

- current branch が `develop` か
- feature branch 側に `.review.md` 作成 commit と ticket completion commit があるか
- `develop` first-parent が merge base から進んでいないか、または進んでいる場合は意図した mainline commit だけか
- index に unrelated staged changes がないか

### C. Ticket lifecycle 操作用の workflow を作る

review artifact 作成、ticket 状態追記、completion deletion、merge 前 first-parent 確認を一つの workflow としてまとめると、手順の抜けが減る。特に branch/worktree の切り替えを workflow 側が明示的に促すだけでも効果がある。

## 現時点の判断

今回の問題は Git 操作ミスというより、file-based ticket lifecycle を branch workflow に載せる時の検査不足である。今後は「どの順序で状態遷移したか」だけでなく、「どの branch に状態遷移 commit を置いたか」を first-class な確認項目にする必要がある。
