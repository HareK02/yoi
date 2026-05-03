# Resume 時の Scope Claim の改善

## 背景

`tickets/dynamic-scope.md` で in-process Scope の縮小（SpawnPod による委譲時の Write revoke）と pod-registry 上の delegation 記録が揃った。これにより「セッション中に scope が縮む」状態を Pod / registry の双方が一貫して表現できる。

一方で `tui -r` 経由の resume は、`crates/tui/src/spawn.rs` の `build_overlay_toml` を通じて fresh spawn と同じロジックで overlay を合成する。manifest cascade に scope 宣言が無い場合、cwd 直下に `write` 再帰の rule を毎回付ける挙動。

このため次のような衝突が起きる:

- セッション S が稼働中に SpawnPod で子 C を作り、cwd 配下のサブパスを委譲した
- 親が exit、子 C は registry 上にエントリが残存（あるいはまだ稼働中）
- ユーザーが S を resume しようとすると、新しい Pod が cwd 全体に `write` を claim → 委譲された部分と overlap して registry が拒否

resume の意図は「過去のセッションの続きを取る」であって「過去の effective scope より広い範囲を新たに掴み直す」ではない。現状は後者になっており、過去に手放した scope を resume が勝手に取り戻そうとする形になっている。

## ゴール

セッション resume 時に claim する scope が、当該セッションが最後に持っていた effective scope に揃う。委譲済み・他 Pod が保持中の部分は claim 対象から外れ、resume された Pod は当時と同じ範囲だけで動作する。

## 要件

- resume 時の overlay 合成は cwd 盲信ではなく、当該セッションが過去に持っていた scope を反映する。情報源は session log / registry / その他のいずれでも良いが、何らかの永続情報から復元できること
- 過去の scope 情報が取得できないセッション（旧形式 / 破損）は、明示的なエラーで止めるか、ユーザーに確認させてから fresh claim にフォールバックする（黙って広げない）
- claim 試行が registry の既存 allocation と衝突した場合、エラーメッセージで衝突相手の Pod 名 と target rule の双方が伝わる（現状は Pod 名のみ）
- 委譲済みエントリ（`delegated_from` を持つ allocation）が同じセッションの委譲チェーンに属する場合、resume はその範囲を claim せずに進行する

## 完了条件

- 「親 Pod がセッション中に SpawnPod を実行 → 子に委譲 → 親 exit → 親セッションを resume」のフローが、既存子 allocation を残したまま衝突なしで成功する
- 既存の無関係な Pod と衝突するケースは、衝突 rule と相手 Pod 名を含む明確なエラーで失敗する
- 単体テスト or 統合テストで上記 2 ケースが検証される
- 既存の fresh spawn (resume なし) の挙動には変化なし

## 範囲外

- 過去スコープの永続化スキーマを新規導入するかの判断は実装時に決める（session log の既存フィールドで足りるなら追加しない）
- 自動的に既存 Pod を kill / reclaim して claim を通す挙動
- protocol 経由の外部からの GrantScope / RevokeScope（`tickets/dynamic-scope.md` の範囲外宣言を継承）
- registry 側のエラー型の全面再設計（rule 情報を含めるための最小限の拡張のみで足りる想定）

## Review

- 状態: Approve
- レビュー詳細: [./resume-scope-claim.review.md](./resume-scope-claim.review.md)
- 日付: 2026-05-03
