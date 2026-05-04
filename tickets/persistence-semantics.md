# 永続化層のセマンティック整理

## 背景

現在の永続化は `SessionId` 単位の append-only JSONL log を中心に構成されている。これは実装上は扱いやすい一方で、今後 Pod 単位永続化、compaction、fork、DB backend 追加などを進めるにあたり、以下の概念が混ざり始めている。

- ユーザー視点の「同じ会話 / 作業の継続単位」
- Pod 視点の「現在 active な会話状態」
- append-only log の物理的 / 復元上の単位
- compaction によって生成される新しい履歴系列
- fork の起点となる履歴中の境界
- runtime dir に置かれる一時状態と、data dir / DB に置く永続正本

特に、現在は compaction によって新しい `SessionId` が発行される。これは append-only log の低レベル単位としては自然だが、ユーザー視点では「同じ会話が継続している」とも見えるため、`Session` という名称・粒度が今後の設計上あいまいになり得る。

このチケットでは、実装変更に入る前に、永続化層のドメイン概念・名称・責務境界を整理する。

## 目的

- 永続化層で扱う概念を、ユーザー視点 / Pod 視点 / storage 視点に分けて定義する。
- `SessionId` が今後も適切な中心概念か、あるいは別概念に分解すべきかを判断する。
- compaction / fork / resume / Pod state / spawned child registry が、どの粒度のデータに属するかを決める。
- 将来 DB backend を追加しても歪みにくいデータ構造を設計する。
- 既存の session-store JSONL 実装から段階的に移行できる命名・API 境界を決める。

## 検討事項

- 会話継続単位と append-only log 単位を分けるべきか。
  - 例: user-visible conversation/thread と、internal log segment の分離。
- compaction の扱い。
  - compaction 後の履歴を新しい低レベル log として扱うのは自然か。
  - その場合、ユーザー視点では同じ会話の継続としてどう表現するか。
- fork の扱い。
  - fork は新しい会話単位を作るのか、同一会話内の branch とするのか。
  - fork 起点を entry hash / turn boundary / checkpoint など、どの抽象度で表すか。
- Pod state の責務。
  - Pod 名から active な会話 / log を復元するために何を持つべきか。
  - Pod が過去に辿った session / log の順序付き履歴をどこに持つべきか。
- runtime state と persistent state の境界。
  - `history.json` / `status.json` / `spawned_pods.json` を永続正本として扱わない方針の確認。
- DB backend を想定した場合のテーブル / relation 相当の構造。
  - append-only entry log
  - lineage / origin
  - active pointer
  - Pod / child Pod registry
  - index / listing / GC の余地
- 既存 API / CLI 名称の移行方針。
  - `--session` の扱い
  - debug 用 ID とユーザー向け ID の分離

## 一案: Thread / Segment / Checkpoint に分ける案

これは現時点の決定ではなく、検討材料の一案として置く。

- `Pod`: agent 実行主体 / process identity。
- `Thread`: ユーザー視点の会話・作業継続単位。compaction しても同じ Thread と見なす。
- `Segment`: append-only log の物理的 / 復元上の単位。現在の `SessionId` に近い。compaction / fork で新しい Segment が生まれる。
- `Entry`: Segment 内の 1 永続化イベント。
- `Checkpoint`: fork / rollback / UI 選択などの起点を表す抽象境界。内部的には Segment + EntryHash を指してもよいが、表層 API では entry pointer を直接露出しすぎない。

この案では:

- compaction = same Thread, new Segment
- fork = new Thread または branch, new Segment
- resume = same Thread の active Segment に append
- Pod state = active Thread / spawned children / 必要な runtime 復元メタデータを保持
- lineage = Segment origin または Checkpoint reference として保持

この案を採用するかは本チケット内で改めて比較・判断する。

## 完了条件

- 永続化層の主要概念と名称が文書化されている。
- compaction / fork / resume / Pod state のデータ粒度が決まっている。
- 現在の `SessionId` / session-store API をどう扱うか、維持・alias・rename・段階移行の方針が決まっている。
- DB backend を追加する場合の概念モデルが、最低限テーブル / relation 相当で説明できる。
- `tickets/pod-persistent-state.md` や fork 関連チケットに反映すべき前提が整理されている。

## 範囲外

- このチケット単体での大規模 rename 実装。
- DB backend の実装。
- UI の履歴表示 / branch 表示の詳細 UX。
- GC / retention policy の実装。

## 関連

- `tickets/pod-persistent-state.md`
- `tickets/pod-session-fork.md`
- `crates/session-store/`
- `crates/pod/src/pod.rs`
