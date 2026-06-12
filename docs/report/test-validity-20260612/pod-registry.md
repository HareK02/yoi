# テスト妥当性レビュー: pod-registry
- 判定: 概ね良い

## 確認した範囲

- `crates/pod-registry/README.md`
- `crates/pod-registry/Cargo.toml`
- `crates/pod-registry/src/lib.rs`
- `crates/pod-registry/src/table.rs`
- `crates/pod-registry/src/conflict.rs`
- `crates/pod-registry/src/mutate.rs`
- `crates/pod-registry/src/lifecycle.rs`
- `crates/pod-registry/src/error.rs`
- `crates/pod-registry/src/test_util.rs`

`pod-registry` の責務は、machine-local な live Pod allocation、Pod 名衝突、write-scope / delegation-scope の衝突検出、stale allocation の回収、segment writer の重複防止を `pods.json` と file lock で調整すること。現状のテストは crate 内 unit test のみで、integration test / cross-process test はありません。

## 現在のテストがよくカバーしていること

- 中核的な mutation invariants はよくカバーされています:
  - top-level registration が write-scope conflicts を拒否すること
  - duplicate Pod name の拒否
  - read-only scope が write scope と衝突しないこと
  - release が allocation を削除し、scope を再度開放すること
  - child release / stale reclaim が descendants を reparent すること
  - delegated scope が release 後に parent へ戻ること
  - `reclaim_delegated_scope` が存在する/存在しない child allocation を扱い、一致する deny layer を 1 つだけ削除すること
- Delegation behavior は意味のある形でテストされています:
  - delegated scope が明示的な `DelegationScope` の subset でなければならないこと
  - delegation が parent の直接の current effective write set ではなく `DelegationScope` を使うこと
  - sibling overlap が拒否されること
  - effective parent write scope が delegated child regions を除外すること
- Conflict-owner behavior は有用な semantic boundaries でテストされています:
  - recursive prefix overlap
  - non-recursive direct-child behavior
  - 最も深い delegated owner が competitor として報告されること
  - restored-parent deny rules が fully denied regions は抑制できるが、partial-deny parent conflicts は抑制しないこと
- Segment-lock behavior は基本的にカバーされています:
  - `register_pod` が重複する `segment_id` を拒否すること
  - `update_segment` が segment ownership を書き換えること
  - `update_segment` が別の Pod にすでに保持されている target segment を拒否すること
  - `find_by_segment` が `segment_id = None` の pre-adoption placeholder allocations を無視すること
  - `lookup_segment` が live writer info を返し、guard release 後には `None` を返すこと
- File-backed table behavior には有用な smoke coverage があります:
  - lock file creation
  - `0600` file / `0700` directory permissions
  - save/reopen roundtrip
  - RAII `ScopeAllocationGuard` が drop 時に release すること
- Test helpers のスコープは妥当です。`reclaim_stale_with` は liveness の決定論的な seam を提供し、`RuntimeDirSandbox` はこの crate 内で env-var mutation を直列化しています。

## ギャップ / 疑問のあるテスト

- 最大のギャップは cross-process/file-lock tests がないことです。この crate の中心的な safety claim は、`flock` が無関係な Pod spawn sequences を直列化することですが、現状のテストは一度に 1 つの `LockFileGuard` を使う単一プロセスしか exercised していません。lock blocking、concurrent registration 下の atomicity、または 2 つのプロセスが同じ `pods.json` で race したときの behavior を検証していません。
- `adopt_allocation` には segment-conflict test がありません。`register_pod` と `update_segment` はどちらも別の Pod にすでに保持されている `segment_id` を拒否しますが、`adopt_allocation` は現状、そのようなテストなしに `segment_id` を書き換えます。“one live writer per segment” がすべての allocation entry points に対する invariant であるなら、これは意味のある coverage hole です。
- Error-path coverage は薄いです:
  - 壊れた `pods.json` の parse errors がテストされていない
  - unknown-pod errors は `adopt_allocation` ではカバーされているが、`release_pod` / `update_segment` ではカバーされていない
  - save failure / permission failure behavior がカバーされていない
  - `reclaim_stale` は意図的に save errors を握りつぶしますが、その best-effort behavior は直接テストされていない
- Path-overlap logic は少数の例でしかテストされていません。この behavior は重要かつ微妙ですが、すべての recursive/non-recursive combinations、equal paths、parent/child/grandchild、siblings、read-vs-write、deny coverage cases に対する table-driven matrix がありません。
- `register_pod_with_deny` には full deny と partial deny behavior のテストがありますが、複数の conflicting owners が同時に存在し、その一部だけが denied されている場合のテストはありません。これはまさに `find_conflict_owners` と `all_denied` semantics が重要になる branch です。
- `manifest::DelegationScope` および `ScopeRule` normalization との integration boundary は軽くしか exercised されていません。upstream code が canonical absolute paths を保証しているなら問題ありませんが、そうでない場合、この crate には relative paths、`..`、symlinks、trailing separators、または同等だが同一ではない paths に対するテストがありません。
- Stale reclaim は real `pid_alive` ではなく、注入された liveness predicate 経由でテストされています。これは決定論のためには良いことですが、Unix の `kill(pid, 0)` semantics は、他のテストで current process id を使う間接的な形を除いて未テストのままです。
- 実際の Pod spawn/restore flows 中にこの registry に依存する caller crates との integration tests がありません。小さな coordination crate としては理解できますが、これはテストが end-to-end runtime safety よりも local invariants を証明していることを意味します。

## 追加を推奨するテスト

- 2 つの child processes または helper binaries から同じ `pods.json` を開く小さな cross-process test を追加し、以下を検証する:
  - 2 つ目の writer が最初の lock が release されるまで block すること、または少なくとも stale unlocked state を observe/write できないこと
  - 同じ Pod name / scope / segment を claim する concurrent attempts の結果が成功 1 件だけになること
- `adopt_allocation_rejects_segment_id_collision` test を追加する、または adoption がすでに保持されている segment へ overwrite してよい理由を明示的に document し、test する。
- `rules_overlap` と `covers_fully` の table-driven tests を追加し、以下をカバーする:
  - recursive/recursive、recursive/non-recursive、non-recursive/recursive、non-recursive/non-recursive
  - equal path、direct child、grandchild、sibling、unrelated path
  - `covers_fully` における read/write permission ordering
- 複数 conflict に対する `register_pod_with_deny` tests を追加する:
  - all conflicts denied: allowed
  - 複数 conflicts のうち 1 つだけ denied: meaningful competitor とともに rejected
- persistence/error tests を追加する:
  - invalid JSON が `InvalidData` を返すこと
  - save されていない `data_mut` changes が永続化されないこと
  - unknown `release_pod` と unknown `update_segment` が `UnknownPod` を返すこと
- 可能であれば Pod spawn/restore caller path の近くに integration-level test を 1 つ追加し、“no two live Pods write the same session segment” という durable invariant に焦点を当てる。

## 実行したコマンド

- `cargo test -p pod-registry`
  - 結果: passed
  - 29 unit tests passed
  - 0 doc tests
- `cargo test -p pod-registry; echo EXIT:$?`
  - 結果: passed, `EXIT:0`
  - 29 unit tests passed
  - 0 doc tests

