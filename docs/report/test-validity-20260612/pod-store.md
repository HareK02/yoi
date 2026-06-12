# テスト妥当性レビュー: pod-store

- 評価: 混在

## 確認範囲

- 対象 crate: `crates/pod-store`
- 主な読解対象:
  - `crates/pod-store/src/lib.rs`
  - `crates/pod/tests/restore_test.rs`
  - `crates/pod/src/discovery.rs`
  - `crates/pod/src/spawn/registry.rs`
- 実行した検証:
  - `cargo test -p pod-store -- --nocapture`

`pod-store` は、Pod 名をキーにした永続 metadata を `{pod_state_root}/{pod_name}/metadata.json` に保存する薄い永続化 crate。session log の replay は `session-store` に任せ、ここでは active session/segment pointer、spawned child state、reclaimed child history、peer visibility、resolved manifest snapshot を保持する責務を持つ。

## 現在のテストがよくカバーしていること

既存の 6 unit tests はすべて成功しており、crate の中心的な「metadata の各領域を更新しても他領域を壊さない」という invariant はある程度押さえられている。

特に良い点:

- `resolved_manifest_snapshot` の JSON roundtrip を確認している。
- `FsPodStore` が session root ではなく pod state root 配下に書くことを確認している。
- `set_active` が `spawned_children` を保持しつつ active pointer / manifest snapshot を更新することを確認している。
- `set_spawned_children` が active pointer と manifest snapshot を保持することを確認している。
- peer 追加が重複を避け、名前順に安定化され、削除できることを確認している。
- child reclaim が outstanding child から削除し、reclaimed history に記録することを確認している。

また、上位 crate 側には `FsPodStore` を使った restore/discovery/communication 系の integration tests があり、`pod-store` 単体ではなく利用経路側でも一部の実運用 invariant は間接的に検証されている。

## 不足 / 疑問のあるテスト

重要な filesystem / schema boundary のテストが不足しているため、durable persistence crate としてはまだ弱い部分がある。

- `validate_pod_name` の境界値が直接テストされていない。
  - empty / `.` / `..` / slash / NUL が拒否されること。
  - 通常名が通ること。
  - invalid name で `write`, `read_by_name`, `delete_by_name` が root 外へ出ないこと。
- `list_names` の仕様が未検証。
  - 名前順に sort されること。
  - `metadata.json` がない directory を無視すること。
  - file entry を無視すること。
  - invalid pod name directory を無視すること。
- `delete_by_name` が未検証。
  - missing は no-op。
  - metadata file を消す。
  - 空になった pod directory を削除する。
  - 余計なファイルがある場合の挙動を確認するか、仕様として割り切る必要がある。
- malformed JSON / unknown or partial old metadata の read 挙動が未検証。
  - `serde(default)` による後方互換読み込みはこの crate の重要な性質に見えるが、missing `spawned_children`, `reclaimed_children`, `peers`, `active`, `resolved_manifest_snapshot` の読み込みテストがない。
  - 壊れた JSON が `PodStoreError::Serde` になることも未確認。
- `CombinedStore` の delegation が未検証。
  - session-store trait methods を inner session store に委譲すること。
  - `PodMetadataStore` methods を inner pod store に委譲すること。
  - `root_dir` が pod store 側 root を返すこと。
- `update_by_name` の「missing metadata なら作成する」「closure が `pod_name` を変えても requested name に戻す」という contract が直接テストされていない。
- concurrency / atomicity は未検証。
  - 現実には read-modify-write の `update_by_name` が複数 writer で lost update し得る。crate が単一 writer 前提なら仕様化すべきで、複数 Pod/process から同じ metadata を触る前提ならテスト以前に設計上の保護が必要。
- `fs_store_writes_under_pod_state_root_only` は historical regression test としては妥当だが、実際の path escape 防止を保証するには invalid name tests が必要。現状では「正しい root に書く」ことは見ているが、「不正名で root 外へ書けない」ことは見ていない。

## 追加を提案するテスト

優先度順に追加するとよいテスト:

1. pod name validation / path traversal 防止
   - `validate_pod_name` が通常名を受け入れる。
   - `""`, `"."`, `".."`, `"a/b"`, `"a\0b"` を拒否する。
   - invalid name を渡した `FsPodStore::write/read_by_name/delete_by_name` が `InvalidPodName` を返す。
   - root 外にファイルが作られていないことも確認する。

2. list behavior
   - 複数 metadata を書いて `list_names()` が sorted names を返す。
   - metadata-less directory / normal file / invalid-name directory を無視する。

3. delete behavior
   - existing metadata を削除できる。
   - missing metadata delete が成功する。
   - delete 後 `read_by_name` が `None`、`list_names` から消える。

4. schema compatibility / error behavior
   - minimal JSON `{ "pod_name": "agent" }` を手で置いて default fields が空/None になること。
   - malformed JSON が serde error になること。
   - active pointer の `segment_id` omitted が pending segment として読めること。

5. update merge contract
   - `update_by_name` が missing metadata を作成する。
   - update closure が unrelated fields を壊さない。
   - closure が `metadata.pod_name` を変更しても requested name に正規化される。

6. CombinedStore thin-wrapper tests
   - pod-store crate 内で軽量に `CombinedStore<FsStore, FsPodStore>` を作り、pod metadata と session log operations がそれぞれ正しい backend に流れることを確認する。

## 実行したコマンド

```sh
cd /home/hare/Projects/yoi && cargo test -p pod-store -- --nocapture
```

結果:

```text
running 6 tests
test tests::pod_metadata_manifest_snapshot_roundtrips ... ok
test tests::fs_store_writes_under_pod_state_root_only ... ok
test tests::active_updates_preserve_children_and_manifest_snapshot ... ok
test tests::child_updates_preserve_active_and_manifest_snapshot ... ok
test tests::reclaim_children_removes_outstanding_and_records_history ... ok
test tests::peer_updates_preserve_active_children_and_manifest_snapshot ... ok

test result: ok. 6 passed; 0 failed; 0 ignored

Doc-tests pod_store
test result: ok. 0 passed; 0 failed

