# メモリ機構: Phase 2 consolidation

## 背景

`docs/plan/memory.md` §Phase 2 の実装。staging の活動ログ + 既存 `memory/*` + Knowledge 化候補レポートを入力に、consolidation Worker が汎用 CRUD + 検索ツール + Linter Hook で agentic に統合する。Phase 1 を終えた pod が spawn し、並走防止は staging 配下の進行状況ファイルで担保する。

Knowledge 新規作成は「候補レポート掲載の source から派生する場合」に限る。使用頻度メトリクス（候補レポートの集計元）が未完のうちは、レポートは空入力として動作し、Phase 2 は decisions / requests / summary / 既存 Knowledge update のみ行う。

## 要件

### Trigger

- staging の累積ファイル数 or bytes 閾値（設定で tune）
- compact 発火時に必ず flush（compact で失われる raw を漏らさない）

### 実行主体と入力

- Phase 1 を終えた pod が consolidation Worker を spawn
- 起動時スナップショットで consumed ID list を確定
- 入力:
  - consumed ID 分の staging エントリ（活動ログ + `source`）
  - 既存 `memory/*`（summary / decisions / requests）全文
  - Knowledge 化候補レポート（メトリクスチケットの成果物。未完のうちは空）
  - 既存 `knowledge/*` は prompt に埋めず、Knowledge 検索ツール経由で agent が引く

### 渡すツール

- 汎用 CRUD（file read / write / edit）
- memory 検索ツール
- Knowledge 検索ツール
- post-write Linter Hook（違反時 turn 戻し、N 回失敗 abort）

### 常駐注入の無効化

- consolidation Worker の Pod 起動直後に `Pod::set_resident_knowledge_injection(false)` を呼ぶ
- `model_invokation: ON` の Knowledge description を system prompt に載せず、Knowledge 検索ツール経由で agent に引かせる方針（本セクション「入力」と整合）
- 仕組み自体は `tickets/memory-resident-injection.md` で導入済み。lever は用意されているが Phase 2 spawn 経路がまだないので、本チケットの実装範囲で必ず呼ぶこと

### 処理内容

- 新規 decisions / requests を 1 件 1 ファイルで追加、`sources` は staging の `source` をコピー（LLM 推論ではない）
- 活動ログから派生する Knowledge を新規作成 or 既存 patch。**新規作成は候補レポート掲載の source 由来に限る**
- summary を必要に応じて rewrite（1-5k tokens 目安）
- 削除は `status: replaced` + `replaced_by: <slug>` で置き換え記録、直接削除しない
- 書き込み先: `memory/*`, `knowledge/*`。`memory/workflow/` は Linter で弾かれる

### 並走防止

- staging 配下に 1 ファイル（Pod 識別子 + consumed ID list）
- 存在し、示された Pod が動作している間、そのプロセスが排他占有
- 実行中に Phase 1 が追加した staging は触らず、次回 Phase 2（Coalesce）に委ねる
- 完了時は consumed ID list の staging のみ cleanup、追加分は残す
- Phase 2 完了時に staging 新着があれば次を発火（Coalesce）
- 占有の実現方法（pid 存在確認 / flock / 他）は実装判断

### モデル

- 設定 key `memory.consolidation_model`（reasoning 系）

### prompt

- `docs/plan/memory-prompts.md` §共通原則 / §Phase 2: 統合 prompt / §Phase 2: Knowledge 書き込み prompt に従う

## 範囲外

- 使用頻度メトリクスと Knowledge 化候補レポートの集計（別チケット。未完の間は空レポートで動作）
- GC（別チケット）
- Workflow 関連の offer（別チケット、Notification 経路が先）
- 意味破壊検出の監査 LLM 層（将来検討）

## 完了条件

- Phase 1 が staging に残した活動ログを Phase 2 が `memory/*` / `knowledge/*` に統合する
- Linter 違反時は turn が戻り、sub-Worker が自己修正する
- 並走防止ファイルが想定通り機能し、複数 Phase 2 の重複起動が防げる
- Coalesce で実行中追加分が次回に引き継がれる
- compact 発火時に Phase 2 が flush される
- 空レポートでも新規 Knowledge を作らずに動く（decisions / requests / summary / 既存 Knowledge update のみ）

## 参照

- `docs/plan/memory.md` §Phase 2 / §Phase 2 agent への原則 / §Compact との関係
- `docs/plan/memory-prompts.md` §Phase 2 関連
- `tickets/memory-file-format.md`（Linter）
- `tickets/memory-search-tools.md`（検索ツール）
- `tickets/memory-phase1-extract.md`（staging 生産）
