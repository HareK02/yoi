# prune-projection レビュー

## 要件の充足

チケットが定義した3つの設計方針は全て達成されている:

1. **判定結果を index 集合として保持** — `prunable_indices` がそのまま該当
2. **PreLlmRequest hook でコンテキスト構築時に content を省く** —
   `PruneHook::call` で `context` に対してのみ書き換え
3. **Worker の history を一切変更しない** — `worker.rs:701` の
   `let mut request_context = self.history.clone()` により、hook に渡るのは
   clone。`build_request` で使われた後に破棄され、`self.history` には戻らない

## アーキテクチャ

責務分離が綺麗に実装されている:

- `llm-worker::prune` には pure な `prunable_indices` と `PruneConfig` のみ。
  mutation 系 API (`apply_prune`, `PruneResult`) は完全に削除
- `PruneHook` 内に content ドロップの inline ループ。意図的にインライン化されており、
  「射影は呼び出し側の責任」という設計が形になっている
- `PruneHook` の doc comment で `worker.rs:701` への参照により
  「なぜ mutate しても安全か」を明示

## 指摘と対処

### 1. clone 依存の脆さ（非ブロッカー、未対処）

現在の安全性は `worker.rs:701` の clone に依存している。worker の実装変更で
clone が消えると設計が即座に壊れる。対策案:

- worker.rs の clone 箇所に「この clone は射影 hook に依存されている」旨の
  コメントを足す（軽い対処）
- `PreLlmRequest` hook の型を `RequestContext` のような専用型にして
  型レベルで分離（本格対処）

将来的な事故防止の観点で気になるが、チケット範疇の外。

### 2. PruneHook の射影ロジックに単体テストが無い（非ブロッカー、未対処）

`llm-worker::prune` の `apply_prune` テストを削除したのは正しいが、
`PruneHook::call` の射影ロジックには単体テストがない。
`PruneHook::call` を呼んで、context が変更され、かつ元 history が
変わらないことを検証する統合テストがあると安心。

## 判定（初回）

承認。ただし初回レビューで「pure ロジックが `llm-worker::prune` にあるのに
適用は pod 側 Hook で行う」という責務の不整合が見つかり、追加作業
「Worker への統合」をチケットに追記して再実装。

---

# 追加作業レビュー: Worker への統合

## 要件の充足

追加作業で定義した変更は全て実装されている:

| 項目 | 実装 |
|---|---|
| `Worker::set_prune_config` | `worker.rs:317` |
| `Worker::set_savings_estimator` | `worker.rs:329` |
| `build_request` 直前での射影 | `worker.rs:733-758` |
| `SavingsEstimator` 型定義 | `llm-worker::prune::SavingsEstimator` |
| `pod::prune_hook` モジュール削除 | 削除済み |
| Pod 側は config と estimator を渡すだけ | `pod::prune::attach_prune` |

Worker が prune の責任を持ち、pod は usage 履歴に依存する estimator
コールバックを注入するだけ、という責務分離はチケット通り。Locked 状態への
lock 時にも `prune_config` / `savings_estimator` が保持される点も対処済み
(`worker.rs:1214-1217, 1286-1289`)。

## 指摘と対処

### A. `attach_prune` がどこからも呼ばれていない（未対処、要判断）

`Pod::attach_prune` は実装されているが、コードベース内で呼び出し箇所が無い。
履歴を見ると、リファクタ前の `PruneHook` も一度も registration されていない
デッドコードだった (`be1119d` 以降 `PruneHook::new` を呼んだ箇所無し)。
つまりこのリファクタは「デッドコードを別の形のデッドコードに置き換えた」状態。

チケットの「追加作業」範疇としては設計通り完結しているが、prune 機能が実際に
有効化されていないのは事実。Manifest や `Controller::spawn` のどこで
`attach_prune` を呼ぶかは別チケット扱いにするか、このチケット内で一緒に
対処するかを要判断。

### B. 閉包内でのロック取得（非ブロッカー、未対処）

`attach_prune` の estimator は毎回 `usage.lock().expect(...).clone()` する。
現在 `usage_history` を触る箇所は:

- `Pod::persist_turn`（run 終了後、短時間）
- `Pod::compact`（同上）
- `Pod::usage_history()`（snapshot 取得、短い）
- estimator（request ごと、clone のみ）

estimator 発火は worker.rs の `build_request` 直前、つまり Pod の `run()` が
待機中なので他のロック取得と並走しない。現時点では安全。将来 `usage_history`
を別スレッドから触るコードが増えた時の事故防止として、pod.rs の
`usage_history_handle` doc コメントに「Mutex は短時間 clone 専用」という
前提を明示すべき。

### C. Worker 内部のインラインループ（非ブロッカー、未対処）

`worker.rs:733-758` に content 射影のインラインループが直書き。`prune`
モジュール側に `apply_projection(&mut Vec<Item>, &[usize])` のような
pure 関数を用意して Worker はそれを呼ぶだけにすれば、Worker のメインループが
短くなり、`prune` モジュール内でのテストも書きやすくなる。現状 Worker の
`run` ループが肥大化する方向に寄っている。

### D. 射影ロジックに単体テストが無い（初回指摘2から継続、未対処）

Worker に統合したことで Worker のテストとして書きやすくなったはずだが、
テストは追加されていない。指摘 C で `apply_projection` に切り出せば、
その pure 関数単位でテストが書ける。

## 判定（追加作業）

条件付き承認。指摘 A が機能有効化の観点で重要。意図通り（別チケットで
manifest 配線）なら承認、このチケット内で対処するなら未完了扱い。
指摘 B/C/D はコード品質の改善で非ブロッカー。
