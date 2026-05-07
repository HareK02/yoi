# ファイルベース Ticket と Scope 委譲の相性

日付: 2026-05-05

## 要旨

insomnia を使って insomnia を開発する運用では、ファイルベースの ticket / review lifecycle と、Pod の write scope 委譲が衝突しやすい。特に「実装 Pod が worktree を書く」直後に「親 Pod または reviewer が同じ worktree の ticket review artifact を書く」流れでは、同じファイルツリーに複数主体が順番に書きたい一方、Scope は write 権限を排他的に委譲するため、自然なレビュー操作が詰まる。

## 今回起きたこと

`tickets/permission-extension-point.md` の実装で、worktree workflow に従って `.worktree/permission-extension-point` を作り、実装用 Pod を Spawn した。

実装 Pod には worktree への write scope を渡したため、親 Pod から見るとその worktree は委譲中の領域になった。その後、同じ worktree に対して ticket review workflow を行い、`tickets/permission-extension-point.review.md` の作成と `tickets/permission-extension-point.md` の Review section 追記をしようとしたところ、親 Pod の `Write` tool は次のように拒否された。

```text
path is read-only in this scope: <repo>/.worktree/permission-extension-point/tickets/permission-extension-point.review.md
```

実装 Pod を `StopPod` して scope を回収した後に review artifact を書く必要があった。

また、最初に SpawnPod した際、write scope を worktree のみに絞ると spawned Pod の cwd である `<repo>` が readable scope 外になり、起動に失敗した。実装用 Pod には worktree write に加えて親 project root の read scope も渡す必要があった。

## 障壁

### 1. Ticket lifecycle は同じファイルツリーへの複数主体の逐次書き込みを要求する

このプロジェクトの ticket lifecycle は、実装後に以下のようなファイル操作を行う。

- `tickets/<ticket>.md` を読む
- `tickets/<ticket>.review.md` を作る
- `tickets/<ticket>.md` に Review section を追記する
- 完了時には ticket / review file を削除する

つまり ticket は単なる入力仕様ではなく、実装・レビュー・完了状態を表す共有ファイルでもある。実装者 Pod、親 Pod、reviewer Pod のいずれも、同じ worktree 内の `tickets/` を書きたくなる。

### 2. Scope は write ownership を安全側に排他的にする

SpawnPod の scope 委譲は、子 Pod に渡した write 領域を親から切り離す安全モデルになっている。このモデル自体は正しい。子が作業中の worktree を親が同時に書けないため、意図しない競合や横取りを防げる。

ただし file-based ticket workflow では、レビューを書く段階でも同じ worktree を書く必要がある。実装 Pod がまだ alive で write scope を保持していると、親が review artifact を書けない。reviewer Pod を別に Spawn する場合も、同じ write scope を同時に渡せない。

### 3. Review のために実装 Pod を止めると対話継続性が落ちる

今回の回避策は実装 Pod を停止して scope を回収することだった。これは review artifact を書くには有効だが、実装者 Pod に追加質問したり、レビュー指摘をそのまま反映させたりする流れとは相性が悪い。

本来欲しい流れは以下に近い。

1. 実装 Pod が worktree に実装する
2. reviewer が同じ diff を見て review artifact を書く
3. 実装 Pod に修正を依頼する
4. reviewer が再レビューする

現在の排他的 write scope では、2 と 3 の間で scope owner を何度も移す必要がある。

## 影響

- Ticket review が「実装 Pod を停止してからでないと書けない」運用になりやすい
- 実装者 Pod と reviewer Pod の並行・往復がしにくい
- file-based ticket が作業対象 worktree 内にあるため、コード変更と管理メタデータ変更が同じ scope boundary に巻き込まれる
- 親 Pod が orchestration をしているのに、review artifact のような管理操作まで child scope に阻まれる

## 暫定運用

当面は次の運用が現実的。

- 実装 Pod に worktree write scope を渡したら、レビュー artifact を親が書く前に実装 Pod を停止して scope を回収する
- 実装 Pod に追加修正を依頼したい場合は、レビュー作成後に再 Spawn する
- Spawn 時は worktree write scope だけでなく、Pod 起動 cwd や参照元 ticket を読める read scope も明示する

この運用は安全だが、レビュー往復の体験は重い。

## 改善案

### A. Review artifact の書き込み場所を親側に逃がす

review を worktree 内の `tickets/*.review.md` ではなく、親 session 側の report / review storage に書く案。Scope 衝突は避けられるが、現行の ticket lifecycle と git 上の履歴管理から外れるため、採用するなら ticket lifecycle 自体の再設計が必要。

### B. Scope に「管理ファイルの親書き込み例外」を作る

親 Pod が delegated worktree のうち `tickets/*.review.md` や `tickets/*.md` の Review section だけを書ける例外を作る案。実装はできるが、Scope の安全モデルに穴を開けるため慎重に扱う必要がある。パターンベース permission と似た問題になり、どのファイルを管理操作とみなすかの境界も曖昧になる。

### C. Scope owner の handoff を明示操作にする

`StopPod` より軽い「write scope を一時返却する / 再取得する」操作を protocol に作る案。実装 Pod の会話状態を保ったまま、reviewer / parent が短時間だけ同じ worktree に書ける。ただし停止していない Pod が後で再開した時の整合性、同時実行防止、失効した tool call の扱いを設計する必要がある。

### D. Reviewer を同じ Pod に依頼する

実装 Pod 自身に review artifact まで書かせる案。Scope 衝突は起きないが、実装者と reviewer の分離が弱くなる。ticket-reviewer の目的である独立レビューには向かない。

## 現時点の判断

短期的には「実装完了 → 実装 Pod 停止 → 親または reviewer が review artifact 作成」を標準手順として明記するのが良い。中期的には、Scope owner の handoff か、ticket/review artifact の置き場所を再検討する必要がある。

今回の障壁はバグというより、insomnia の安全な scope 委譲モデルと、ファイルを状態機械として使う ticket lifecycle の設計上の摩擦である。
