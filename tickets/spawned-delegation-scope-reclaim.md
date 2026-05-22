# Spawned child 終了時に delegated scope を parent へ reclaim する

## 背景

`SpawnPod` は child Pod に write scope を委譲する際、parent Pod 側に動的な Write-deny を追加して、同じ path を parent と child が同時に write できないようにしている。この narrowed scope は session log の `pod.scope` snapshot として永続化され、parent Pod の resume 後にも復元される。

一方、`StopPod` / child `ShutDown` callback / 起動時の unreachable child prune では、child allocation と `SpawnedPodRegistry` record は削除されるが、parent 側の復元済み `SharedScope` と pod-registry の parent allocation `scope_deny` が更新されない。結果として、child が存在しなくなった後も parent が委譲 path に対して Write-deny されたまま残る可能性がある。

これは `StopPod` の「delegated scope を reclaim する」契約と矛盾し、再起動後に parent Pod が不要に Read-only 化される blocking bug である。

## 要件

- child removal を単なる `SpawnedPodRegistry` からの削除ではなく、Pod-owned operation として扱う。
- 以下の経路で、child に委譲した scope を parent 側へ reclaim する。
  - `StopPod`
  - child からの `ShutDown` callback
  - parent Pod 起動時 / registry restore 時の unreachable child prune
- reclaim 時に以下をすべて整合させる。
  - parent の runtime `SharedScope`
  - pod-registry に登録されている parent allocation の `scope_deny`
  - session log に永続化される parent `pod.scope` snapshot
  - Pod state に永続化される spawned children registry
  - runtime dir の `spawned_pods.json` mirror
- delegation によって追加された dynamic deny と、manifest / user policy 由来の base deny を区別する。
  - child を止めても、明示的な user / manifest deny を広げてはいけない。
  - 現状の `Scope` が deny rule を安全に remove できない場合は、可逆な delegation layer を追加して再計算する。
- reclaim 操作は冪等にする。
  - child が既に消えている場合でも parent scope を過剰に広げない。
  - 到達不能 child prune と後続 `StopPod` が重なっても壊れない。
- live writer 二重起動防止の責務は引き続き pod-registry / scope allocation に置き、Pod state に lock 責務を持たせない。

## 完了条件

- parent Pod が child に write scope を委譲した後、`StopPod` により parent の write permission が復元される。
- parent Pod を再起動して narrowed scope が復元された状態から、到達不能 child を prune した場合も parent の write permission が復元される。
- child `ShutDown` callback を受けた場合も parent の write permission が復元される。
- manifest / user policy 由来の explicit deny は reclaim 後も維持される。
- parent allocation の `scope_deny` が reclaim 後の `SharedScope` と一致する。
- reclaim 後の `pod.scope` snapshot が session log に記録され、再 resume 後にも reclaimed 状態が維持される。
- tests を追加/更新する。
  - `StopPod` reclaim
  - unreachable child prune reclaim
  - child `ShutDown` callback reclaim
  - explicit base deny を広げないこと
  - runtime registry mirror / Pod state children list との整合
- `cargo fmt --check`
- `cargo check --workspace`
- `cargo test -p pod -p pod-registry`

## 範囲外

- spawned child の自動再起動
- Pod state による lock 実装
- 過去 Pod 探索 / restore tool
- scope policy 全体の再設計
- UI 上の scope 表示

## レビュー指摘元

Pod 単位永続化全体レビューでの blocking finding:

- `StopPod` / child pruning does not actually restore the parent Pod’s effective scope after delegated children disappear.
- 関連箇所:
  - `crates/pod/src/pod.rs` の resume 時 scope restore
  - `crates/pod/src/spawn/registry.rs` の unreachable child prune
  - `crates/pod/src/spawn/comm_tools.rs` の `StopPod`
  - `crates/pod/src/ipc/event.rs` の child `ShutDown` callback
  - `crates/manifest/src/scope.rs` の deny rule handling
