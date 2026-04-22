# メモリ機構の方針

## Context

INSOMNIA がユーザーのプロジェクトに対して提供するメモリ機構。プロジェクトの暗黙知蓄積と同じ失敗を繰り返さないための記憶が目的。エージェントに連続するアイデンティティや自己意識を持たせる方向は対象外。

リサーチは `docs/ref/memory-systems.md`。前提として、**レポジトリがファイルシステム上にある**ケースで設計する（越境・バックエンド抽象は Scope 外）。

prompt 要件の整理は `docs/plan/memory-prompts.md` に切り出した。

Workflow（`/<slug>` で呼び出される制約付き作業フロー）は別 plan に切り出した。`docs/plan/workflow.md` 参照。

## 決定事項

### 記録対象の 4 種

| 種別             | パス                         | 備考                                                                                        |
| ---------------- | ---------------------------- | ------------------------------------------------------------------------------------------- |
| Always-on サマリ | `memory/summary.md`          | 1-5k tokens 目安                                                                            |
| Decisions        | `memory/decisions/<slug>.md` | `status: open \| resolved \| replaced` で未決議論も保持、置き換え時は `replaced_by: <slug>` |
| Requests         | `memory/requests/<slug>.md`  | ユーザー submit の構造化要約                                                                |
| Knowledge        | `memory/knowledge/<slug>.md` | `#slug` で注入。ノウハウ / 用語 / 運用方針 / ルール / 事実など型を設けず Markdown 自由記述  |

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

Knowledge は Phase 2 が自律的に新規作成 / 更新 / フラグ切替を行う前提。毎回の人間承認 gate は設けない（実効性が低い）。保護は 3 段で担保:

- **採択 gate**: Knowledge 新規作成は使用頻度メトリクスの Knowledge 化候補レポート（後述）に載った source から派生する場合に限る。閾値未満のうちは decisions / requests に留める
- **Linter + 監査 LLM**: 構造違反と意味破壊を watch（詳細は後述）
- **OS ファイル権限**: 人間が書き換えさせたくない record は `-r--` にしてロック。Phase 2 / GC の write は OS レベルで弾かれる

Workflow も同じフラグ仕様（`workflow.md` 参照）。per-record 保護フラグを提供する拡張は将来検討、初期は OS 権限で足りる。

### 書き込み経路と Linter

人間も consolidation sub-Worker も**同じ CRUD tool（file read / write / edit）**で `memory/*` を触る。書き込み時の制約は 2 層で検証し、違反時は post-write Hook が turn を戻して sub-Worker に自己修正させる（N 回失敗で abort）:

1. **Linter（静的）**: frontmatter / slug / 参照整合などの機械的ルール
2. **監査 LLM（意味的）**: rewrite が元の情報を壊していないかを別 prompt で check。特に Knowledge の意味損壊を watch する主経路

Linter ルールは 2 系統:

**静的 error**（post-write Hook で turn 戻し、sub-Worker が自己修正）:

- frontmatter 必須 field
  - Decisions / Requests: `created_at`, `updated_at`, `sources`
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
- **入力**: 前回 Phase 1 以降の session log 範囲。処理済み境界の pointer は session 側に保持（寿命を session と揃える）
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
- **実行主体**: Phase 1 を終えた pod が consolidation Worker を spawn。並走防止は staging 配下の進行状況ファイル（後述）で担保
- **入力**: 起動時スナップショットで確定した consumed ID list 分の staging エントリ（活動ログ + `source`）+ 既存 `memory/*`（summary / decisions / requests / knowledge）+ **Knowledge 化候補レポート**（後述の使用頻度メトリクスから機械集計、閾値超過の source 一覧）
- **処理**: sub-Worker に**汎用 CRUD tool（file read / write / edit）+ post-write Linter Hook** を渡し、agentic に以下を自律判断:
  - 新規 decisions / requests を 1 件 1 ファイルで追加。`sources` は staging の `source` をコピー（LLM 推論ではない）
  - 活動ログから派生する Knowledge（用語定義 / 運用方針 / ルール / 事実 / ノウハウ）を新規作成 or 既存 patch。**新規作成は候補レポート掲載の source から派生する場合に限る**。`last_sources` を更新
  - summary を必要に応じて rewrite
- **書き込み先**: `memory/*` 配下。Workflow 禁止は Linter で担保（`workflow.md` 参照）
- **完了処理**: consumed ID list の staging のみ cleanup（実行中に Phase 1 が追加した分は残す）。Phase 2 完了時に staging に新着があれば次を発火（Coalesce）
- **モデル**: `memory.consolidation_model`。reasoning 系

##### 並走防止

- 場所: staging 配下に 1 ファイル（名前・形式は未定）
- 中身: 動作中の Pod 識別子 + **consumed ID list**（この Phase 2 run が起動時スナップショットで確定した staging エントリ ID の列）
- 占有ルール: そのファイルが存在し、示された Pod が動作している間、そのプロセスが排他占有
- 実行中に Phase 1 が staging に追加したエントリは触らず、次回 Phase 2（Coalesce）に委ねる
- cleanup は consumed ID list のエントリのみ削除、追加分は残す
- クラッシュ時は consumed ID list から処理途中を特定できる。重複作成は同一 slug update に自然収束
- 占有の実現方法（pid 存在確認 / flock / 他）は未定

#### Phase 2 agent への原則

`memory/` 配下は人間も git 経由で編集する。Phase 2 prompt で以下を明示:

- **rewrite は許可**。既存内容と新規情報を統合・再構成して情報密度を上げることを優先。単純 append（追記で増やすだけ）は避ける
- rewrite 時は**情報損失を最小化**する: 既存の主張・根拠・sources を保持。表現を整理・短縮しても、含まれている要素は落とさない
- 削除は置き換え記録（`status: replaced` + `replaced_by: <slug>`）で表現、直接削除しない
- 人間編集は git diff で顕在化する前提。整合しない rewrite は避け、衝突時は git で解決

#### Offer 経路

Memory record の書き込みは Phase 2 が自律判断し、Offer は設けない（Knowledge 含む）。人間承認経路が必要なのは以下:

- Workflow 関連の offer（新規作成 / 改善 / `auto_invoke` ON 化）は `workflow.md` 参照

#### Compact との関係

基本分離（memory は独立トリガー、compact は `input_tokens` 既存閾値のまま）。ただし **compact 発火時は Phase 2 を必ず同時 flush**（compact で失われる raw を漏らさないため）。

### GC（定期再評価）

Phase 2 とは別経路で memory を再評価する定期ジョブ。Phase 2 は rewrite 許可で情報統合寄りの働きをするが、それでも残る以下の課題の出口として機能する:

- 重要度の低い record が累積する
- 類似 slug が乱立する（Linter Warn で検出したものをまとめて処理）
- `replaced` が溜まり続けて grep / 注入時のノイズになる
- sources 累積
- 長期的に陳腐化した記録の drop

他プロジェクトの GC 設計の横断比較は `docs/ref/memory-systems.md` §8。

#### 権限と操作粒度

GC Agent は **drop / merge / split を自律実行**（削除まで含む）。人間 offer はかけず、結果は git diff で検証する建て付け。operation 粒度は以下の両方:

- **ファイル単位**: 丸ごと drop、複数ファイルの merge、1 ファイルの分割（split）
- **ファイル内の部分削除**: 本文の一部節・箇条を削除 or 圧縮。frontmatter の `sources` 古いエントリの trim も含む

Phase 2 と同じ CRUD tool + Linter Hook を使うので、operation 粒度は自然にサポートされる（専用 API は用意しない）。

#### 使用頻度メトリクス

時間単位は実時間を使わない（LLM スループット向上で陳腐化の意味が変わるため）、累積 input token で正規化する。

**観測経路**: `memory/*` への読み取りは専用の memory 検索ツール（既存 built-in の grep / read とは別に用意）経由に揃える。invoke 計測はツール内でフックし、`#<slug>` / `/<slug>` / 明示検索呼び出しを同一経路に集約する。

**カウント対象**:

- **明示 invoke**: 検索ツール経由の読み取り / `#<slug>` / `/<slug>` を n回/Mtoken でスコア化
- **auto_invoke 注入**: 注入は context 常駐コストで、「載っているだけ」か「使われた」かを統計上区別不能。明示 invoke の分子には含めず、**コスト側（注入した record に対する消費 input tokens）として別途記録**する。使われ率 ratio や ON/OFF 判断の材料として後段で使う
- ファイル token 数

**記録先**: staging とは独立。invoke event を UUID + Stats 形式で workspace 側に記録し、session データが失われても統計が残るようにする。具体 schema・フォーマットは未定。

**累積方式**（後集計アプローチ）: 上記 invoke 記録に対して最大 10 回前の invoke から現在までの時系列窓でフィルタして集計する。

**Knowledge 化候補レポート**: Phase 2 が入力に受け取る、Knowledge 新規作成 gate 用の機械集計。対象は `memory/*` 配下の record（Phase 1 成果物である decisions / requests / 既存 knowledge）で、明示 invoke 頻度が閾値超過のものを列挙する。spike 除外のため、同一 session 内の連続参照は 1 count に丸め、複数 session での再参照を要件とする。閾値の具体値は運用で調整、設定ファイルで tune。

#### 判断ルール

- 保護閾値: **明示 invoke** の `frequency >= 1.0 invokes/Mtoken` の record は drop / 大幅圧縮の対象外（初期値 1.0、workspace 設定でカスタマイズ可）。auto_invoke 注入による常駐は計数対象外（別指標として後段で参照）

### ファイル形式

- frontmatter + Markdown 本文。全 record 共通: `created_at`, `updated_at`
- ファイル名（slug）がそのまま識別子、frontmatter に `id` / `name` field は持たない
- source トレーサビリティ（session log への逆引き、粒度は `session_id` + entry range）:
  - Decisions / Requests: `sources: [{session_id, range: [start, end]}, ...]` 永続化（update 時は追記累積）
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
