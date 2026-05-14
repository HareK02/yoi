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

## 結論: Session / Segment / Entry の 3 階層

```
Session  ← ユーザー視点の会話の家系（fork tree 全体。Segment 群の grouping）
  └── Segment  ← Compaction / Fork で切れる物理 append-only 単位（現在の SessionId 相当）
        └── Entry  ← 1 永続化イベント
```

セマンティクスの対応:

- **resume** = 同 Session 内の指定 leaf Segment に append
- **compaction** = 同 Session, new Segment（lineage: `compacted_from`）
- **fork** (live / 過去いずれも) = 同 Session, new Segment（lineage: `forked_from`）
- **branching は Session 内で完結**する。Segment 間が DAG、Segment 内は完全 linear。
- **Session は分岐ツリーを持つ静的な構造**。「今どの Segment に書いているか」(current active Segment) は Pod state 側が動的に保持する。

文脈合成 (merge / cherry-pick) は今後の要件として想定しないため、entry レベルの DAG（任意の親 pointer）は採用しない。Segment 内 linear + Segment 間 DAG の 2 階層に閉じることで、同 Segment 内は `(segment_id, seq)` の連番 PK で直線探索でき、Segment 間の lineage は粗粒度な DAG として軽量に管理できる。

fork で新しい Session を切らないのは、compaction で Segment を切るのと fork で Segment を切るのが対称な操作であり、ユーザー視点でも「同じ起源から派生した枝」として 1 つの単位で扱う方が自然なため。これにより Session 数が分岐で爆発せず、`WHERE session_id = ?` だけで fork tree 全体が取れる。

### Restore / Resume の API

Session が分岐を含むため、resume の指定は **(SessionId, leaf SegmentId) の組** で行う:

- TUI 経路: ユーザーは **Session を選択 → その Session 内の leaf を選択**。
- 内部 API: restore は `(SessionId, SegmentId)` を取り、指定 Segment から replay する。leaf 以外を指定すれば read-only な過去状態の参照も可能。
- 「最後に active だった leaf」は Pod state が保持するので、TUI が初期選択候補として使える。

### 既存コードとの対応

| 既存 | 新名称 |
|---|---|
| `SessionId` | `SegmentId` にリネーム |
| (なし) | `SessionId` 新設（Segment 群をまとめる、ユーザー視点 ID） |

`session-store` crate の型・関数は Segment 中心の命名に揃える。crate 名自体を変えるかは別論点（中身は引き続き Segment 単位の append-only log を扱う）。

### llm-worker への影響

llm-worker は session 概念を持たない（`Worker` は `history` / `turn_count` / `RequestConfig` を持つだけで永続化への hook も無い）。Session / Segment 階層の導入は llm-worker 層に染み出さず、影響範囲は `session-store` / `pod` / `pod-registry` / `pod-cli` に閉じる。

## Entry hash の廃止

現状、各 entry は SHA-256 hash chain (`prev_hash` → `hash`) を持つが、実際に効いている用途は 2 つだけ:

1. `ensure_head_or_fork` (pod.rs:1348) — store の末尾と Pod の保持する `head_hash` を比較し、不一致なら auto-fork。**末尾識別子があれば良い**（hash chain そのものは要らない）。
2. `fork_at(source_id, at_hash)` (session.rs:425) — 過去 entry pointer から fork。`pod-session-fork.md` の入口仕様は turn number で、entry hash は内部 pointer に過ぎない。turn boundary (TurnEnd entry の index) で代替可能。

改竄検知 (tamper-evident chain) は宣伝されているがコード上は walk して verify するルートが無いため、削除しても regression にならない。

廃止に伴う対応:

- `HashedEntry` 廃止、JSONL は 1 行 1 `LogEntry`。
- `SessionOrigin.at_hash` → `at_turn_index` (TurnEnd 由来) に置換。
- `ensure_head_or_fork` の検知ロジックは、Segment 末尾の terminal marker entry または末尾 seq 比較に置換（形式は実装時に決める）。

### 廃止前の足場 (前提)

本セクションを実装に移すタイミングでは、log writer が既に sync 化されていることを前提にする (`tickets/log-entry-singular-and-direct-commit.md`)。具体的には:

- `Store::append` / `read_all` 等が `std::fs` ベースの sync API
- `SessionLogWriter::append_entry()` が sync 関数
- `session_head` mutex は `parking_lot::Mutex` / `std::sync::Mutex`
- `LogCommand` / drain task / Flush バリアは既に撤廃済み

この状態で hash chain を廃止すると追加で取れる単純化:

- **`session_head` mutex そのものを撤去できる**。 hash chain が無いので「`head_hash` を直前 entry から取得して次に渡す」 という serialize 必須の依存が消える。 1 行 < `PIPE_BUF` (Linux 4KB) の `O_APPEND` write は kernel 側で atomic に直列化されるので、 user space で mutex を持つ必要が無い
- `session_head` が消えると Pod / interceptor / worker callback が writer ハンドルだけ持てば良くなる。 `Arc<SessionLogWriter>` は単に `Arc<Store> + sink` を抱えるだけの値で、 hot-path の競合がない
- `compute_hash` 呼び出しが消える分、 append が serialize + open + write + close の 3 syscall まで詰まる

つまり「sync 化」 が先に来て、 「hash 廃止」 で mutex まで消える、 という 2 段階の単純化になる。

## Fork: 2 種類の書き込み方

Session 境界の話ではなく **元 Segment への marker 書き込みの有無**で 2 種類を分ける。Session はどちらの場合も同じで、新 Segment が同 Session 内に生える。

- **live auto-fork**（concurrent writer 検知）
  - 元 Segment の末尾に terminal marker (`Forked { to: SegmentId }` 等) を append → 以降の writer は marker を見て新 Segment へ自動移動。
  - CoW semantics: 元 Segment は immutable、生まれた Segment 同士は対等な兄弟。
- **過去 fork**（UI で turn 選択）
  - 元 Segment は **無変更**、replay して新 Segment を生やすだけ。
  - 起点は `(source_segment_id, at_turn_index)`。
  - 元 Segment に書き込まないため、過去 fork を nested に重ねても解釈が単純。

両者を同じ marker で扱おうとすると、過去 fork から更に過去で fork した場合に元 Segment への marker 位置解釈が複雑化して破綻するため、過去 fork 側は元 Segment に触れない方針で固定する。

### fork の物理操作（物理コピーは不要）

「fork = 同 Session 内に Segment を増やす」と言っても、過去 Segment を丸ごとコピーする必要は無い:

- source segment の seq=N までを replay して得られた `Vec<Item>` を、新 Segment の seed entry (`SessionStart` 相当) に 1 回書き込む。
- compaction の transitive な圧縮効果がここに乗る（source segment の `SessionStart.history` に過去 Segment の compaction 結果が既に埋まっている）ので、**source segment 1 本だけ replay すれば fork seed が作れる**。さらに過去の Segment は touch しない。
- 新 Segment が持つ lineage 情報は `(parent_segment_id, fork_at_turn)` のメタデータのみ。

これは現状の `fork_at` (`session.rs:430-456`) の挙動と同じで、Session 階層が乗っても操作は変わらない。

## RDB backend を想定した概念モデル

将来 DB backend を追加しても歪みにくい形として、以下の関係を仮定する:

```
sessions (
  id PK,
  ...                                  -- ユーザー視点 metadata
)

segments (
  id PK,
  session_id FK,
  parent_segment_id FK NULL,           -- compaction / fork の元
  fork_at_turn INT NULL,
  origin_kind ENUM (new | compact | fork),
  lineage_path ltree,                  -- 祖先・子孫の逆引き用 (materialized path)
  ...
)

entries (
  segment_id FK,
  seq INT,
  kind, payload, ts,
  PRIMARY KEY (segment_id, seq)        -- 同 Segment 内 linear scan
)
```

- 同 Segment 内 entry は `(segment_id, seq)` PK で linear scan、surrogate identity なので hash 不要。
- Segment 間 lineage は `parent_segment_id` chain。深さは compaction / fork 回数のみ（Session あたり数〜数十）。
- Segment lineage の祖先・子孫逆引きは `lineage_path` の `<@` / `@>` で 1 index 引き。entry 単位の DAG ではないため materialized path のメンテコストも軽い。
- 通常 append は `lineage_path` 更新を伴わない（Segment 生成時に確定）。
- 「Session の fork tree 全体」は `WHERE session_id = ?` で取れる。Session 単位の listing / GC が自然。

FsStore 実装はこの構造のサブセット相当として位置付ける（1 Segment = 1 jsonl、`session_id` は Segment の metadata に持たせるか別ファイルに index する）。

## 残る検討事項

- pod.scope extension entry を Pod state 側に寄せるか、Segment log に残すか（`tickets/pod-persistent-state.md` 側と合わせて決定）。
  - 撤廃の選択肢: (a) Segment log から削除し Pod state を唯一の正本にする / (b) snapshot 保持責務だけ Pod state に寄せ、scope 変更 event は Segment log に残す / (c) 現状維持で Pod state は Segment への参照のみ。
- live auto-fork の marker 形式 (terminal entry vs Segment 末尾 seq 比較)。
- pod-cli / TUI の `--session` 引数を Session 単位にするか Segment 単位にするか。debug 用 ID とユーザー向け ID の分離。
- `session-store` crate 名のリネーム要否。

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
- Session を跨ぐ merge / cherry-pick。

## 関連

- `tickets/pod-persistent-state.md`
- `tickets/pod-session-fork.md`
- `crates/session-store/`
- `crates/pod/src/pod.rs`
