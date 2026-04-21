# メモリ機構の方針

## Context

INSOMNIA がユーザーのプロジェクトに対して提供するメモリ機構。プロジェクトの暗黙知蓄積と同じ失敗を繰り返さないための記憶が目的。エージェントに連続するアイデンティティや自己意識を持たせる方向は対象外。

リサーチは `docs/ref/memory-systems.md`。前提として、**レポジトリがファイルシステム上にある**ケースで設計する（越境・バックエンド抽象は Scope 外）。

Workflow（`/<slug>` で呼び出される制約付き作業フロー）は別 plan に切り出した。`docs/plan/workflow.md` 参照。

## 決定事項

### 記録対象の 5 種

| 種別             | パス                           | 備考                                                                                            |
| ---------------- | ------------------------------ | ----------------------------------------------------------------------------------------------- |
| Always-on サマリ | `memory/summary.md`            | 1-5k tokens 目安                                                                                |
| Lessons          | `memory/lessons/<slug>.md`     |                                                                                                 |
| Decisions        | `memory/decisions/<slug>.md`   | `status: open \| resolved \| replaced` で未決議論も保持、置き換え時は `replaced_by: <slug>`     |
| Requests         | `memory/requests/<slug>.md`    | ユーザー submit の構造化要約                                                                    |
| Knowledge        | `memory/knowledge/<slug>.md`   | `#slug` で注入。ノウハウ / 用語 / 運用方針 / ルール / 事実など型を設けず Markdown 自由記述      |

- `<slug>` は kebab-case（内容を要約した短い識別子）。**ファイル名そのものが ID**、frontmatter に別途 `id` field は持たない
- **1 件 1 ファイル**。append-only な複数エントリログファイルは作らない
- **同一 slug の衝突は新規作成禁止**。既存があれば update（Linter で検証、sub-Worker は read→edit に切り替え）
  - 同主題の content 進化 = 上書き update + git log で履歴追跡
  - 別主題が古い主題を置き換える場合のみ、別 slug で新規作成し古い方に `status: replaced` + `replaced_by: <新 slug>` を記録
- Phase 1 の中間ストアとして `memory/_staging/<id>.json` を使う（短命、P2 完了で cleanup。ここは衝突回避と順序のため UUIDv7 可）
- Raw session log は既存 `llm-worker-persistence` で保持する。memory 対象外、参照経路のみ

### Knowledge の呼び出し制御

agentskills.io の `SKILL.md` 形式は採用しない。Knowledge は `#<slug>` でユーザー / LLM から注入参照する。名前空間は**フラット**、slug は kebab-case（小文字英数とハイフン）。

| フラグ           | 意味                                                    | デフォルト |
| ---------------- | ------------------------------------------------------- | ---------- |
| `auto_invoke`    | description が LLM context に載り、LLM が自発的に呼べる | **OFF**    |
| `user_invocable` | ユーザーが `#<slug>` で明示的に呼べる                   | **ON**     |

`auto_invoke` の ON 化は人間の判断、または consolidation が頻繁に `user_invoke` されているものを検出して offer する。自律的に新規エンティティを生成はしない。Workflow も同じフラグ仕様（`workflow.md` 参照）。

### 書き込み経路と Linter

人間も consolidation sub-Worker も**同じ CRUD tool（file read / write / edit）**で `memory/*` を触る。書き込み時の制約は Linter で検証し、違反時は post-write Hook が turn を戻して sub-Worker に自己修正させる（N 回失敗で abort）。

Linter ルールは 2 系統:

**静的 error**（post-write Hook で turn 戻し、sub-Worker が自己修正）:

- frontmatter 必須 field
  - Lessons / Decisions / Requests: `created_at`, `updated_at`, `sources`
  - Knowledge: `description`, `auto_invoke`, `user_invocable`, `last_sources`, `created_at`, `updated_at`
  - Summary: `updated_at`（optional: `last_rewritten_from_range`）
- `memory/workflow/` への書き込み禁止（sub-Worker context のみ、人間編集は除外）
- 同 slug での新規作成禁止（既存があれば update に切り替えるサイン）
- `#<slug>` 参照が実在ファイルを指す
- `replaced_by: <slug>` が実在 record を指す
- Decisions の `status` は enum `open | resolved | replaced`
- `auto_invoke: true` の record は description 文字数上限（agentskills 準拠 1024 chars）
- 種別ごとの char 硬上限（具体値は運用で調整、設定ファイルで tune）

**膨張抑制 Warn**（error ではなく改善ヒント、sub-Worker は task 余力があれば対応）:

- 重要度 × char の天秤: 低重要度で char 使用過多な record に「圧縮余地あり」Warn
- sources 累積: 配列が閾値超過で「直近 N 件に絞り過去は git log に委ねる候補」Warn
- 類似 slug 乱立: 近似 slug の集合に「merge 候補」Warn

Workflow 保護は専用 tool schema のトリックではなく Linter ルールで担保するため、人間が規則を読んで理解できる。OS ファイル権限や Scope 上の特別な保護機構は設けず、`memory/` 配下を write allow する以上の細工はしない。衝突は git で解決する前提。

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
  - LLM 出力（活動ログ JSON）は pod 側ラッパーが `source: { session_id, range: [start_entry, end_entry] }` を**機械付与**して wrap。LLM には source を推論させない
- **モデル**: `memory.extract_model`。軽量だが文脈理解できる中堅クラス（Haiku / 4o-mini / Flash 相当）を想定

#### Phase 2: 永続化への統合

- **Trigger**: staging の累積ファイル数 or bytes が閾値超過、または compact 発火時（必ず flush）
- **実行主体**: Phase 1 を終えた pod が `memory/_staging/.lock` の advisory file lock を `LOCK_EX | LOCK_NB` で取得試行。取れたら consolidation Worker を spawn。取れなければ skip（他 pod が処理中）
- **Lock**: POSIX `flock(2)` / `fcntl` ベース。プロセスが生きている限り保持、落ちれば OS が自動解放。stale 検出・heartbeat 不要。reasoning 処理が数分〜十数分かかる前提
- **入力**: staging 全件（活動ログ + `source`）+ 既存 `memory/*`（summary / lessons / decisions / requests / knowledge）
- **処理**: sub-Worker に**汎用 CRUD tool（file read / write / edit）+ post-write Linter Hook** を渡し、agentic に以下を自律判断:
  - 新規 lessons / decisions / requests を 1 件 1 ファイルで追加。`sources` は staging の `source` をコピー（LLM 推論ではない）
  - 活動ログから派生する Knowledge（用語定義 / 運用方針 / ルール / 事実 / ノウハウ）を新規作成 or 既存 patch。`last_sources` を更新
  - summary を必要に応じて rewrite
- **書き込み先**: `memory/*` 配下。Workflow 禁止は Linter で担保（`workflow.md` 参照）
- **完了処理**: staging cleanup + lock release。Phase 2 完了時に staging に新着があれば次を発火（Coalesce）
- **モデル**: `memory.consolidation_model`。reasoning 系

#### Phase 2 agent への原則

`memory/` 配下は人間も git 経由で編集する。Phase 2 prompt で以下を明示:

- **rewrite は許可**。既存内容と新規情報を統合・再構成して情報密度を上げることを優先。単純 append（追記で増やすだけ）は避ける
- rewrite 時は**情報損失を最小化**する: 既存の主張・根拠・sources を保持。表現を整理・短縮しても、含まれている要素は落とさない
- 削除は置き換え記録（`status: replaced` + `replaced_by: <slug>`）で表現、直接削除しない
- 人間編集は git diff で顕在化する前提。整合しない rewrite は避け、衝突時は git で解決

#### Offer 経路

consolidation は自律生成しない。以下は Client に `Event::Notification` で提案し、人間承認で反映:

- Knowledge の `auto_invoke` ON 化
- Workflow 関連の offer（新規作成 / 改善 / `auto_invoke` ON 化）は `workflow.md` 参照

#### Compact との関係

基本分離（memory は独立トリガー、compact は `input_tokens` 既存閾値のまま）。ただし **compact 発火時は Phase 2 を必ず同時 flush**（compact で失われる raw を漏らさないため）。

### GC（定期再評価）

Phase 2 とは別経路で memory を再評価し、drop / merge / split / `replaced` chain の整理を行う。Shann³ llm-wiki の lint 相当。Phase 2 は rewrite 許可で情報統合寄りの働きをするが、それでも残る以下の課題の出口として機能する:

- 重要度の低い record が累積する
- 類似 slug が乱立する（Linter Warn で検出したものをまとめて処理）
- `replaced` が溜まり続けて grep / 注入時のノイズになる
- sources 累積
- 長期的に陳腐化した記録の drop

**詳細仕様は後で詰める**（scope 内、未決定）。方針として LLM 駆動の定期ジョブで、Linter Warn 群 + 人間 offer 承認を併用する前提。

### ファイル形式

- frontmatter + Markdown 本文。全 record 共通: `created_at`, `updated_at`
- ファイル名（slug）がそのまま識別子、frontmatter に `id` / `name` field は持たない
- source トレーサビリティ（session log への逆引き、粒度は `session_id` + entry range）:
  - Lessons / Decisions / Requests: `sources: [{session_id, range: [start, end]}, ...]` 永続化（update 時は追記累積）
  - Knowledge: `last_sources: [{session_id, range}, ...]`（最新更新時のみ、過去履歴は git log で追う）
  - Summary: optional `last_rewritten_from_range`（なしでも可）
- Knowledge 固有: `description`, `auto_invoke`, `user_invocable`
- Decisions 固有: `status: open | resolved | replaced`、置き換え時は `replaced_by: <slug>`
- Phase 1 staging: `memory/_staging/<id>.json`（JSON、1 件 1 ファイル、Phase 2 完了で削除。短命なので UUIDv7 可）。pod 側ラッパーが `source` を機械付与して LLM 出力と wrap
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

### 別 plan / ticket で扱う

- 具体クレート構成・API 境界
