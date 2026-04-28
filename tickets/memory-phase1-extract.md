# メモリ機構: Phase 1 活動抽出

## 背景

`docs/plan/memory.md` §Phase 1 の実装。activity tokens の累積閾値で発火し、前回 Phase 1 以降の session log 範囲から「起きたこと」を 4 種の活動ログ候補として抽出、`memory/_staging/<id>.json` に書き出す。Knowledge 化や summary rewrite は Phase 2 に委ねる。

Pod を立てずに既存 compact と同じ Worker spawn 機構を再利用する。raw session log は `session-store` で保持されており、ここから range を切り出して入力に使う。

## 要件

### Trigger

- activity tokens 累積閾値（設定ファイルで tune）。input tokens cumulative since last pointer を使う
- tool call カウントは不採用（ツールカスタマイズ非依存・大小重みづけのため）
- 発火点は Pod の post-run hook で、**compact より前** に走らせる（compact は history を組み替えるため、extract の入力範囲を安定させたい）

### 実行主体と入出力

- 既存 compact の Worker spawn 機構を再利用、Pod は立てない
- 入力: 前回 Phase 1 以降の session log 範囲
- 出力 JSON schema: `decisions`, `discussions`, `attempts`, `requests` の候補配列。抽出対象なしは空配列
- 出力に自由文の補足説明を入れさせない（schema 準拠のみ）

### 処理境界の pointer 永続化

- pointer は session log に書き、寿命を session と揃える
- session-store のドメイン純度を保つため、汎用拡張点 `LogEntry::Extension { domain: String, payload: serde_json::Value }` を **本チケットで session-store に新設**し、`domain = "memory.extract"` で payload に `{ processed_through_entry: usize, staging_id: String }` を載せる
- `RestoredState` には `extensions: Vec<(String, serde_json::Value)>` 形で raw 集積し、memory crate 側が `domain` で fold して最新 pointer を取り出す（session-store は memory のことを知らない）

### 並走防止 (Phase 1 同士)

- Pod 上の `extract_in_flight: AtomicBool` で in-flight 中の新規 trigger を skip
- 完了時点で閾値再評価し、超過していれば直ちに次回を発火（新 pointer 以降の最大範囲を回収）
- pending 状態は別途保持しない（完了時の再評価で coalesce 相当が自然に成立）
- Phase 2 の進行状況ファイルとは別物（こちらは別チケット範囲外）

### 書き込み

- 書き込み先: `memory/_staging/<id>.json`（1 件 1 ファイル、UUIDv7 可）
- pod 側ラッパーが `source: { session_id, range: [start_entry, end_entry] }` を**機械付与**して LLM 出力と wrap
- LLM に source を推論させない

### モデル

- 設定 key `memory.extract_model`（軽量だが文脈理解できる中堅クラス想定）
- 副次設定: `memory.extract_threshold`（input tokens 累積閾値、未設定で disable）、`memory.extract_worker_max_input_tokens`（extract worker 自身の input cap）

### prompt

- prompt 要件は `docs/plan/memory-prompts.md` §Phase 1: 活動抽出 prompt に従う

## 範囲外

- Phase 2 による staging の消費・クリーンアップ（別チケット）
- staging の cleanup 戦略の詳細（Phase 2 で完了時に消す、実行中追加分は残す、という契約だけ本チケットで守る）
- compact Worker spawn 機構自体の拡張（既存をそのまま使う。共通化が必要になったら別途）
- Phase 2 並走防止ファイル（別チケット）

## 完了条件

- Pod 稼働中に閾値超過で Phase 1 が発火し、`memory/_staging/<id>.json` にファイルができる
- ファイルは schema に準拠、`source` が機械付与されている
- 抽出対象なしのときは空配列として書き出される（または発火そのものを skip、どちらでもよい）
- session 側の処理済み pointer が更新され、次回 Phase 1 は続きから走る
- 既存 compact の動作に回帰がない

## 参照

- `docs/plan/memory.md` §Phase 1: 活動抽出 / §ファイル形式（staging）
- `docs/plan/memory-prompts.md` §共通原則 / §Phase 1: 活動抽出 prompt
- 既存 `session-store` クレート（session log range 取得）
- 既存 compact の Worker spawn 経路
