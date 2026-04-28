# LLM 生成設定の manifest 露出整理

## 背景

`llm-worker::RequestConfig` には LLM 生成に関する共通設定として `max_tokens` / `temperature` / `top_p` / `top_k` / `stop_sequences` / `reasoning` がある。一方、Pod manifest の `[worker]` まで露出しているのは現状 `max_tokens` と `temperature` のみで、`top_p` / `top_k` / `stop_sequences` は manifest から指定できない。

`reasoning` は `tickets/model-reasoning-control.md` で内部抽象と manifest 経路を整理するため、本チケットではそれ以外の既存 `RequestConfig` 項目を manifest から指定できるようにする。

## 要件

### `[worker]` への生成設定追加

Pod manifest の `[worker]` セクションで、既存 `RequestConfig` に存在する以下の項目を指定できるようにする。

```toml
[worker]
top_p = 0.9
top_k = 40
stop_sequences = ["\n\n", "</stop>"]
```

- `top_p`: `Option<f32>` として扱い、指定時のみ request に渡す
- `top_k`: `Option<u32>` として扱い、指定時のみ request に渡す
- `stop_sequences`: `Vec<String>` として扱い、未指定時は空配列と同等にする

Provider / scheme によって効かない値がある場合でも、既存の scheme 投影に任せる。値の厳密な provider 別検証は本チケットでは行わない。

### manifest cascade / restore 経路

`WorkerManifestConfig` / `WorkerManifest` に上記フィールドを追加し、manifest cascade の merge 後に `pod::apply_worker_manifest()` から `RequestConfig` へ渡す。

`stop_sequences` は cascade 時に上位 manifest が指定した場合の意味を明確にする。初期方針は、`max_tokens` や `temperature` と同様に「上位指定があれば置き換え」とし、配列の追記マージは行わない。

### 既存挙動の保持

未指定時は現在と同じ request body になること。特に、既存 manifest で `top_p` / `top_k` / `stop_sequences` を省略している場合、wire request に新しい値が出ない。

## 範囲外

- `reasoning` / thinking 制御の manifest 露出。これは `tickets/model-reasoning-control.md` で扱う
- `presence_penalty` / `frequency_penalty` / `seed` / `response_format` / `tool_choice` / `parallel_tool_calls` 等、まだ `RequestConfig` に存在しない新規生成パラメータの追加
- provider ごとの値域検証や推奨値テーブル
- UI での設定編集画面

## 完了条件

- `[worker].top_p` / `[worker].top_k` / `[worker].stop_sequences` を manifest TOML で指定できる
- cascade merge 後の `WorkerManifest` がこれらの値を保持する
- `pod::apply_worker_manifest()` が `RequestConfig.top_p` / `top_k` / `stop_sequences` へ値を渡す
- 未指定時の既存挙動が変わらない
- manifest parse / merge / apply のテストが追加または更新されている

## Review
- 状態: Approve
- レビュー詳細: [./worker-generation-settings.review.md](./worker-generation-settings.review.md)
- 日付: 2026-04-28
