# メモリ機構の方針

## Context

INSOMNIA がユーザーのプロジェクトに対して提供するメモリ機構。プロジェクトの暗黙知蓄積と同じ失敗を繰り返さないための記憶が目的。エージェントに連続するアイデンティティや自己意識を持たせる方向は対象外。

リサーチは `docs/ref/memory-systems.md`。前提として、**レポジトリがファイルシステム上にある**ケースで設計する（越境・バックエンド抽象は Scope 外）。

Workflow（`/workflow-name` で呼び出される制約付き作業フロー）は別 plan に切り出した。`docs/plan/workflow.md` 参照。

## 決定事項

### 記録対象の 5 種

| 種別             | パス                         | 備考                                                                                         |
| ---------------- | ---------------------------- | -------------------------------------------------------------------------------------------- |
| Always-on サマリ | `memory/summary.md`          | 1-5k tokens 目安                                                                             |
| Lessons          | `memory/lessons/<id>.md`     |                                                                                              |
| Decisions        | `memory/decisions/<id>.md`   | `status: open \| resolved \| superseded` で未決議論も保持、supersede は `superseded_by` 参照 |
| Requests         | `memory/requests/<id>.md`    | ユーザー submit の構造化要約                                                                 |
| Knowledge        | `memory/knowledge/<name>.md` | `#name` で注入。ノウハウ / 用語 / 運用方針 / ルール / 事実など型を設けず Markdown 自由記述   |

- `<id>` は UUIDv7（時系列順に並ぶ）、`<name>` は slug（小文字英数とハイフン）
- **1 件 1 ファイル**。append-only な複数エントリログファイルは作らない
- Phase 1 の中間ストアとして `memory/_staging/<id>.json` を使う（Phase 2 完了で cleanup、後述）
- Raw session log は既存 `llm-worker-persistence` で保持する。memory 対象外、参照経路のみ

### Knowledge の呼び出し制御

agentskills.io の `SKILL.md` 形式は採用しない。Knowledge は `#knowledge-name` でユーザー / LLM から注入参照する。名前空間は**フラット**、name は slug。

| フラグ           | 意味                                                    | デフォルト |
| ---------------- | ------------------------------------------------------- | ---------- |
| `auto_invoke`    | description が LLM context に載り、LLM が自発的に呼べる | **OFF**    |
| `user_invocable` | ユーザーが `#` で明示的に呼べる                         | **ON**     |

`auto_invoke` の ON 化は人間の判断、または consolidation が頻繁に `user_invoke` されているものを検出して offer する。自律的に新規エンティティを生成はしない。Workflow も同じフラグ仕様（`workflow.md` 参照）。

### 書き込み先の制約

`memory/workflow/` への自動書き込みは、consolidation Worker の structured output schema に `workflow` カテゴリを含めないことで担保する（後述、および `workflow.md`）。OS ファイル権限や Scope 上の特別な保護機構は設けず、`memory/` 配下を write allow する以上の細工はしない。衝突は git で解決する前提。

### 自動化（consolidation）メカニズム

**2 フェーズ構成**。Phase 1 は頻繁に発火して活動を raw event として抽出、Phase 2 は蓄積時のみ発火して永続化形式に統合する。参考: Codex Memories の Phase 1/2 構造。

#### Phase 1: 活動抽出

- **Trigger**: activity tokens の累積閾値。tool call カウントは不採用（ツールカスタマイズ非依存・大小重みづけのため）
- **実行主体**: 既存 compact と同じ Worker spawn 機構を再利用。Pod は立てない
- **入力**: 前回 Phase 1 以降の session log 範囲
- **出力**: JSON schema で**活動ログ**の候補配列を返す。Knowledge 等の派生物は Phase 2 が活動ログから導出するので、Phase 1 では純粋な「起きたこと」に絞る
  - `decisions`: 判断したこと（選択肢 + 選んだ + 根拠）
  - `discussions`: 議論したこと（トピック + 論点）
  - `attempts`: 試したこと（試行 + 結果 + 成否）
  - `requests`: ユーザー submit の構造化要約（意図 / 対象 / 要約）
  - **抽出対象がなければ空配列を返してよい**（Hermes の "Nothing to save." と同系。頻繁発火を許容する前提）
- **書き込み先**: `memory/_staging/<id>.json`
- **モデル**: `memory.extract_model`。軽量だが文脈理解できる中堅クラス（Haiku / 4o-mini / Flash 相当）を想定

#### Phase 2: 永続化への統合

- **Trigger**: staging の累積ファイル数 or bytes が閾値超過、または compact 発火時（必ず flush）
- **実行主体**: Phase 1 を終えた pod が `memory/_staging/.lock` の advisory file lock を `LOCK_EX | LOCK_NB` で取得試行。取れたら consolidation Worker を spawn。取れなければ skip（他 pod が処理中）
- **Lock**: POSIX `flock(2)` / `fcntl` ベース。プロセスが生きている限り保持、落ちれば OS が自動解放。stale 検出・heartbeat 不要。reasoning 処理が数分〜十数分かかる前提
- **入力**: staging 全件（活動ログ）+ 既存 `memory/*`（summary / lessons / decisions / requests / knowledge）
- **処理**: sub-Worker に memory read/write tool を渡し、agentic に以下を自律判断:
  - 新規 lessons / decisions / requests を 1 件 1 ファイルで追加
  - 活動ログから派生する Knowledge（用語定義 / 運用方針 / ルール / 事実 / ノウハウ）を新規作成 or 既存 patch
  - summary を必要に応じて rewrite
- **書き込み先**: summary / lessons / decisions / requests / knowledge の 5 種
- **Workflow への書き込み禁止**: Phase 2 の write tool schema に workflow を含めないことで構造的に担保（`workflow.md` 参照）
- **完了処理**: staging cleanup + lock release。Phase 2 完了時に staging に新着があれば次を発火（Coalesce）
- **モデル**: `memory.consolidation_model`。reasoning 系

#### Phase 2 agent への原則

`memory/` 配下は人間も git 経由で編集する。Phase 2 prompt で以下を明示:

- 既存内容を尊重し、**差分追加を優先**する。全文 rewrite は `summary.md` のみ、かつ人間編集と整合する範囲で
- 削除は supersede 記録（`status: superseded` + `superseded_by: <id>`）で表現、直接削除しない

#### Offer 経路

consolidation は自律生成しない。以下は Client に `Event::Notification` で提案し、人間承認で反映:

- Knowledge の `auto_invoke` ON 化
- Workflow 関連の offer（新規作成 / 改善 / `auto_invoke` ON 化）は `workflow.md` 参照

#### Compact との関係

基本分離（memory は独立トリガー、compact は `input_tokens` 既存閾値のまま）。ただし **compact 発火時は Phase 2 を必ず同時 flush**（compact で失われる raw を漏らさないため）。

### ファイル形式

- frontmatter + Markdown 本文
- Knowledge の frontmatter: `name`, `description`, `auto_invoke`, `user_invocable`
- Lessons / Decisions / Requests の frontmatter: `id`, `created_at`, および種別固有フィールド
- Decisions は `status: open | resolved | superseded`、supersede 時は `superseded_by: <id>` を併記
- Phase 1 staging は `memory/_staging/<id>.json`（JSON、1 件 1 ファイル、Phase 2 完了で削除）
- Workflow の frontmatter は `workflow.md` 参照

## Scope 外

- ネットワーク越境での memory 同期 — `network-peering.md` で扱う範疇。本設計はプロジェクトスコープ固定

### 将来検討（運用で必要性が見えたら追加）

- Vector index / FTS5 等の検索索引 — 初期は grep で足りる想定。ファイル数増加で検索が重くなったら検討
- `auto_invoke` offer の自動判定ロジック — 初期は人間が手動で切り替え
- 過去 session を cross-session で検索する UI
- Phase 2 を担う常駐 daemon 化 — オンデマンド + lock 方式で始める。必要性が出たら upgrade path として daemon 化
- Deterministic promotion（OpenClaw 型 scoring + ゲート）— 初期は Phase 2 agent の LLM 判断に委ねる。運用実績で出力を評価してから、成熟カテゴリから scoring 導入
- Shallow request の自動除外判定 — 初期は Phase 1 prompt で「些細な質問は返さなくてよい」と指示する程度。精緻な filter は後
- Retention / GC — Phase 2 agent に直接削除禁止（supersede のみ）を課す一方、明示クリーンアップ経路は未定義。別途 LLM 駆動 GC を用意する前提で、初期は無制限蓄積 + summary rewrite で凌ぐ

### 別 plan / ticket で扱う

- 具体クレート構成・API 境界
