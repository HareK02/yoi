# メモリ機構: Phase 1 活動抽出

## 背景

`docs/plan/memory.md` §Phase 1 の実装。activity tokens の累積閾値で発火し、前回 Phase 1 以降の session log 範囲から「起きたこと」を 4 種の活動ログ候補として抽出、`memory/_staging/<id>.json` に書き出す。Knowledge 化や summary rewrite は Phase 2 に委ねる。

Pod を立てずに既存 compact と同じ Worker spawn 機構を再利用する。raw session log は `session-store` で保持されており、ここから range を切り出して入力に使う。

## 要件

### Trigger

- activity tokens 累積閾値（設定ファイルで tune）
- tool call カウントは不採用（ツールカスタマイズ非依存・大小重みづけのため）

### 実行主体と入出力

- 既存 compact の Worker spawn 機構を再利用、Pod は立てない
- 入力: 前回 Phase 1 以降の session log 範囲（処理済み境界 pointer は session 側に保持、寿命を session と揃える）
- 出力 JSON schema: `decisions`, `discussions`, `attempts`, `requests` の候補配列。抽出対象なしは空配列
- 出力に自由文の補足説明を入れさせない（schema 準拠のみ）

### 書き込み

- 書き込み先: `memory/_staging/<id>.json`（1 件 1 ファイル、UUIDv7 可）
- pod 側ラッパーが `source: { session_id, range: [start_entry, end_entry] }` を**機械付与**して LLM 出力と wrap
- LLM に source を推論させない

### モデル

- 設定 key `memory.extract_model`（軽量だが文脈理解できる中堅クラス想定）

### prompt

- prompt 要件は `docs/plan/memory-prompts.md` §Phase 1: 活動抽出 prompt に従う

## 範囲外

- Phase 2 による staging の消費・クリーンアップ（別チケット）
- staging の cleanup 戦略の詳細（Phase 2 で完了時に消す、実行中追加分は残す、という契約だけ本チケットで守る）
- compact Worker spawn 機構自体の拡張（既存をそのまま使う。共通化が必要になったら別途）

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
