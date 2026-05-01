# 動的 Scope 変更

## 背景

現状の Pod の `Scope` は `Pod::from_manifest` 時に1回構築され、以後 immutable。pod-registry (`crates/pod-registry`) は登録・削除・衝突チェックを任意のタイミングで行えるためレジストリ側の制約はないが、Pod 内部の `Scope` と `ScopedFs` が起動時に固定されているため、実行中に scope を追加・縮小することができない。

オーケストレーションでは SpawnPod による scope 分譲で effective scope が縮小するが、これは pod-registry 上の記録にとどまり、Pod 側の `ScopedFs` は元の scope のまま動作している（ツール実行時に pod-registry と照合していない）。

また将来、外部から Pod に scope を動的に付与するケース（人間が「このディレクトリも触っていいよ」と追加する、別 Pod が scope を委譲してくる等）にも対応したい。

## ゴール

Pod の実行中に scope を追加・縮小でき、変更が即座にツール実行の permission チェックに反映される。

## 必要な変更

### Pod 側

- `Scope` を `Arc<RwLock<Scope>>`（または同等の共有可変参照）にする
- `ScopedFs` が `Scope` の共有参照を持ち、ツール実行時に最新の scope を参照する
- scope 変更メソッド: `pod.update_scope(new_scope_config)` → pod-registry 更新 + `Scope` 再構築 + `ScopedFs` に反映

### pod-registry との連携

- scope 追加時: `flock → 衝突チェック → 追加分を登録 → unlock → Pod 内 Scope 再構築`
- scope 縮小時（分譲）: `flock → 分譲を記録 → unlock → Pod 内 Scope 再構築`
- 現在の SpawnPod による分譲を、この汎用パスに統合する

### protocol 拡張（任意）

- `Method::GrantScope { scope }` — 外部から Pod に scope を動的付与
- `Method::RevokeScope { scope }` — 外部から Pod の scope を縮小
- 当面は Pod 内部（SpawnPod ツール）からの変更だけで十分なら不要

## 完了条件

- Pod の実行中に scope を追加でき、追加後のツール実行が新しい scope を反映する
- Pod の実行中に scope を縮小でき、縮小後のツール実行が制限を反映する
- scope 変更が pod-registry と Pod 内 Scope の両方に整合的に反映される
- 単体テストで動的追加・縮小後の permission チェックが検証される

## 範囲外

- protocol 経由の外部からの scope 付与 / 剥奪（必要になったら追加）
- scope 変更の履歴追跡・監査ログ

## Review

- 状態: Approve with follow-up
- レビュー詳細: [./dynamic-scope.review.md](./dynamic-scope.review.md)
- 日付: 2026-05-02
