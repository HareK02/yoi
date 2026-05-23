# メモリ機構の方針

## Context

INSOMNIA がユーザーのプロジェクトに対して提供するメモリ機構。プロジェクトの暗黙知蓄積と同じ失敗を繰り返さないための記憶が目的。エージェントに連続するアイデンティティや自己意識を持たせる方向は対象外。

リサーチは `docs/ref/memory-systems.md`。前提として、**レポジトリがファイルシステム上にある**ケースで設計する（越境・バックエンド抽象は Scope 外）。

prompt 要件の整理は `docs/plan/memory-prompts.md` に切り出した。

Workflow（`/<slug>` で呼び出される制約付き作業フロー）は別 plan に切り出した。`docs/plan/workflow.md` 参照。

## 決定事項

### 記録対象の 4 種

本ドキュメント以下のパスはすべて **`<workspace_root>/.insomnia/`** からの相対表記。`.insomnia/` は manifest / prompts と同じく workspace に紐付く insomnia コンテンツのルートで、memory もこの規約に従う。`workspace_root` 既定は Pod の pwd。

| 種別             | パス                         | 備考                                                                                        |
| ---------------- | ---------------------------- | ------------------------------------------------------------------------------------------- |
| Always-on サマリ | `memory/summary.md`          | 1-5k tokens 目安                                                                            |
| Decisions        | `memory/decisions/<slug>.md` | `status: open \| resolved \| replaced` で未決議論も保持、置き換え時は `replaced_by: <slug>` |
| Requests         | `memory/requests/<slug>.md`  | ユーザー submit の構造化要約                                                                |
| Knowledge        | `knowledge/<slug>.md`        | `#slug` で注入。`kind` で大まかな型だけ持ち、本文は Markdown 自由記述 |

- `<slug>` は kebab-case（内容を要約した短い識別子）。**ファイル名そのものが ID**、frontmatter に別途 `id` field は持たない
- **1 件 1 ファイル**。append-only な複数エントリログファイルは作らない
- **同一 slug の衝突は新規作成禁止**。既存があれば update（Linter で検証、sub-Worker は read→edit に切り替え）
  - 同主題の content 進化 = 上書き update + git log で履歴追跡
  - 別主題が古い主題を置き換える場合のみ、別 slug で新規作成し古い方に `status: replaced` + `replaced_by: <新 slug>` を記録
- extract の中間ストアとして `memory/_staging/<id>.json` を使う（短命、consolidation 完了で cleanup。ここは衝突回避と順序のため UUIDv7 可）
- Raw session log は既存 `session-store` で保持する。memory 対象外、参照経路のみ

### Knowledge の呼び出し制御

agentskills.io の `SKILL.md` 形式は採用しない。Knowledge は `#<slug>` でユーザー / LLM から注入参照する。名前空間は**フラット**、slug は kebab-case（小文字英数とハイフン）。

| フラグ           | 意味                                                    | デフォルト |
| ---------------- | ------------------------------------------------------- | ---------- |
| `model_invokation` | description が model context に載り、モデルが自発的に参照判断できる | **OFF**    |
| `user_invocable` | ユーザーが `#<slug>` で明示的に呼べる                   | **ON**     |

Knowledge は consolidation が自律的に新規作成 / 更新 / フラグ切替を行う前提。毎回の人間承認 gate は設けない（実効性が低い）。保護は 3 段で担保:

- **採択 gate**: Knowledge 新規作成は使用頻度メトリクスの Knowledge 化候補レポート（後述）に載った source から派生する場合に限る。閾値未満のうちは decisions / requests に留める
- **Linter**: 構造違反を watch（詳細は後述）。意味破壊の自動検出は初期は持たず、挙動を見てから監査 LLM 層を追加する（将来検討）
- **OS ファイル権限**: 人間が書き換えさせたくない record は `-r--` にしてロック。consolidation の write は OS レベルで弾かれる

Workflow も同じフラグ仕様（`workflow.md` 参照）。per-record 保護フラグを提供する拡張は将来検討、初期は OS 権限で足りる。

### retrieval 経路

Knowledge / memory を LLM に渡す経路は以下で固定。採択基準（次節）と表裏で、引ける前提がないと採択しても無意味になる。

- **Knowledge 検索ツール**: frontmatter 含めた全文検索。通常 Pod と consolidation Pod の両方に渡す
  - Input: `query`（自由文字列）。オプションで `slug`（完全一致 1 件返し、`#<slug>` 解決に使う）、`kind` filter
  - Output: `{ slug, kind, description, model_invokation, excerpt }` の配列。`excerpt` はマッチ箇所の前後数行
  - 対象は `knowledge/*.md`。派生 index ファイルは持たず実ファイルを都度スキャン
  - ソートは初期 grep の出現順、FTS / vector 導入時に関連度へ切り替え（将来検討）
  - ヒット件数上限と excerpt 行数は設定で tune
- **memory 検索ツール**: `memory/{summary,decisions,requests}/*.md` 対象。spec は Knowledge 検索ツールと同型。§使用頻度メトリクスの観測経路と同一視する
- **更新は memory 専用 Tool + Linter**: Knowledge / memory への write/edit は `memory` クレートが提供する専用 Tool 経由のみ。汎用 CRUD（`tools` クレートの Write/Edit）は memory/knowledge 配下を触らない（Pod が Scope で deny）。Linter は memory tool 内で pre-write 検証として走り、違反は `ToolError::InvalidArgument` で LLM に返る。詳細は §書き込み経路と Linter
- **常駐注入**: メモリを消費する主体は通常 Pod。`model_invokation: ON` な record の description を通常 Pod の system prompt に常駐注入する。consolidation prompt には入れない
  - 予算はシステムプロンプト全体の予算に含める（`memory_summary.md` の 5k 枠とは別管理にしない）
  - 超過時の件数キャップ / 優先順位ルールは、description 1024 chars 上限で通常は収まる前提。ON record 数が増えたら追加する
- **consolidation の Knowledge アクセス**: 全 Knowledge 本文を prompt に埋めず、Knowledge 検索ツール + memory 専用 Tool を agent に渡して自律探索させる（詳細は §Consolidation）
- **`#<slug>` 補完 / 自動呼び出し（大枠のみ、実装は段階的）**:
  - `#<slug>` は検索ツールの slug 完全一致経路で本文が展開される
  - 補完 UI（slug サジェスト）は TUI 側。`user_invocable: false` は候補除外
  - 自動呼び出しは、常駐注入された description をモデルが見て必要と判断すれば検索ツールを呼ぶ形で成立する。専用の auto-invoke 経路は別途用意しない

### Knowledge の採択基準

Knowledge は「保存する価値があるか」だけでなく、「あとで見つけて再利用できるか」で評価する。最低限の基準は以下:

- **slug は入口**。短く、何の知識か推測でき、`#<slug>` や検索で指名しやすいものを優先する
- **description は discovery 面そのもの**。本文の要約ではなく、「何の知識で、どんな時に読むべきか」を短く示す
- **1 file = 1 主題**。description だけで対象範囲が分かる粒度に寄せる。細分化しすぎる slug 乱立も避ける
- **update 優先**。新情報は既存 slug に畳み込み、自然な適合先がない時だけ新規 slug を作る
- **昇格ライン**は「このプロジェクト / ユーザーで再度参照する価値のある事実・ルール・ノウハウ」。一回限りの判断や議論は decisions / requests に留める
- **`model_invokation` ON は別判断**。重要度だけでなく、description だけで「どんな時に読むべきか」が伝わるものに限る

### 書き込み経路と Linter

`memory/*` / `knowledge/*` への write/edit は **`memory` クレート提供の memory 専用 Tool（read / write / edit）** に集約する。汎用 CRUD（`tools` クレートの Write/Edit）はこれらのディレクトリに触らない（Pod が Scope で deny に落とすことで構造的に担保）。

Linter は memory tool 内で **pre-write 検証** として走り、違反は `ToolError::InvalidArgument` で返す。LLM は通常の tool error フローで違反内容を読み、自己修正する。Interceptor 拡張・retry message 注入・違反カウンタは持たない（worker 層の max iteration が暴走を止める）。Linter は frontmatter / slug / 参照整合などの機械的ルールを見る。

人間編集（エディタ / git commit）は tool 層を経由しないため Linter を通らない。`memory::Linter` は pure 関数として export し、CLI / pre-commit hook 経路を後で別チケットで用意する。

意味破壊（rewrite で既存の主張・根拠が落ちる、Knowledge の記述主題がズレる等）の自動検出は初期範囲に含めない。consolidation prompt 側の情報損失最小化指示と git diff レビューで運用し、実使用で顕在化したら監査 LLM 層を後から挟む（将来検討）。

Linter ルールは 2 系統:

**静的 error**（memory tool が `ToolError::InvalidArgument` で返し、LLM が tool error フローで自己修正）:

- frontmatter 必須 field
  - Decisions / Requests: `created_at`, `updated_at`, `sources`
  - Knowledge: `kind`, `description`, `model_invokation`, `user_invocable`, `last_sources`, `created_at`, `updated_at`
  - Summary: `updated_at`（optional: `last_rewritten_from_range`）
- Workflow パス（`.insomnia/workflow/`）への書き込み禁止（sub-Worker context のみ、人間編集は除外）
- 同 slug での新規作成禁止（既存があれば update に切り替えるサイン）
- `#<slug>` 参照が実在ファイルを指す
- `replaced_by: <slug>` が実在 record を指す
- Decisions の `status` は enum `open | resolved | replaced`
- `model_invokation: true` の record は description 文字数上限（agentskills 準拠 1024 chars）
- 種別ごとの char 硬上限（具体値は運用で調整、設定ファイルで tune）

**膨張抑制 Warn**（error ではなく改善ヒント、sub-Worker は task 余力があれば対応）:

- 重要度 × char の天秤: 低重要度で char 使用過多な record に「圧縮余地あり」Warn
- sources 累積: 配列が閾値超過で「直近 N 件に絞り過去は git log に委ねる候補」Warn
- 類似 slug 乱立: 近似 slug の集合に「merge 候補」Warn

Workflow 保護は専用 tool schema のトリックではなく Linter ルールで担保するため、人間が規則を読んで理解できる。OS ファイル権限や Scope 上の特別な保護機構は設けず、`memory/` 配下を write allow する以上の細工はしない。衝突は git で解決する前提。

### 自動化（extract / consolidation）メカニズム

**extract / consolidation の 2 段構成**。extract は頻繁に発火して活動を raw event として抽出、consolidation は蓄積時のみ発火して永続化形式に統合する。参考: Codex Memories の extract/consolidation 構造。

#### Extract: 活動抽出

- **Trigger**: activity tokens の累積閾値（cumulative input tokens since last pointer）。tool call カウントは不採用（ツールカスタマイズ非依存・大小重みづけのため）
- **実行主体**: 既存 compact と同じ Worker spawn 機構を再利用。Pod は立てない
- **入力**: 前回 extract 以降の session log 範囲。処理済み境界の pointer は session log 側に保持し、寿命を session と揃える。session-store のドメイン純度を保つため、汎用拡張点 `LogEntry::Extension { domain, payload }`（domain = `"memory.extract"`）に寄せ、session-store は memory ドメインを知らない。Tool result は raw `content` ではなく表示用 `summary` だけを render し、巨大な tool output を extract input に載せない
- **出力**: JSON schema で**活動ログ**の候補配列を返す。Knowledge 等の派生物は consolidation が活動ログから導出するので、extract では純粋な「起きたこと」に絞る
  - `decisions`: 判断したこと（選択肢 + 選んだ + 根拠）
  - `discussions`: 議論したこと（トピック + 論点）
  - `attempts`: 試したこと（試行 + 結果 + 成否）
  - `requests`: ユーザー submit の構造化要約（意図 / 対象 / 要約）
  - **抽出対象がなければ空配列を返してよい**（Hermes の "Nothing to save." と同系。頻繁発火を許容する前提）
- **書き込み先**: `memory/_staging/<id>.json`
  - LLM 出力（活動ログ JSON）は pod 側ラッパーが `source: { session_id, range: [start_entry, end_entry] }` を**機械付与**して wrap。LLM には source を推論させない
- **実行保証**: extract worker 自身の input occupancy cap は設けない。未処理 range が大きい場合でも pointer 以降の最大範囲を渡し、LLM/API/tool failure のときだけ pointer を進めない
- **モデル**: `memory.extract_model`。軽量だが文脈理解できる中堅クラス（Haiku / 4o-mini / Flash 相当）を想定
- **Compact との順序**: 同一 turn 完了後の post-run チェックで extract を **compact より前** に走らせる。compact は history を組み替えるので、extract の入力範囲（session log 上の entry index）は compact 前のほうが安定する
- **並走防止 (extract 同士)**: Pod 上の `extract_in_flight` フラグで in-flight 中の新規 trigger を skip。完了時点で閾値超過していれば直ちに次回を発火し、新 pointer 以降の最大範囲を回収する（pending 状態は保持しない＝完了時の閾値再評価で coalesce 相当の挙動を成立させる）

#### Consolidation: 永続化への統合 + 整理

- **Trigger**: staging の累積ファイル数 or bytes が閾値超過
- **実行主体**: extract を終えた pod が consolidation Worker を spawn。並走防止は staging 配下の進行状況ファイル（後述）で担保
- **入力**: 起動時スナップショットで確定した consumed ID list 分の staging エントリ（活動ログ + `source`）+ 既存 `memory/*`（summary / decisions / requests）の全文 + **Knowledge 化候補レポート**（後述の使用頻度メトリクスから機械集計、閾値超過の source 一覧）+ **整理材料**（明示 invoke の使用頻度メトリクス、Linter Warn、`replaced` chain、sources 過多情報）。既存 `knowledge/*` は全文を prompt に埋めず、Knowledge 検索ツール経由で agent が必要分を引く
- **処理**: sub-Worker に **memory 専用 Tool（read / write / edit、Linter 内蔵）+ Knowledge 検索ツール + memory 検索ツール** を渡し、agentic に以下を自律判断:
  - **統合**: 新規 decisions / requests を 1 件 1 ファイルで追加。`sources` は staging の `source` をコピー（LLM 推論ではない）
  - 活動ログから派生する Knowledge（用語定義 / 運用方針 / ルール / 事実 / ノウハウ）を新規作成 or 既存 patch。**新規作成は候補レポート掲載の source から派生する場合に限る**。`kind` を frontmatter に持ち、`last_sources` を更新
  - summary を必要に応じて rewrite
  - **整理（余力 step）**: 既存 record 群を §評価カテゴリ で評価し、保護閾値外の対象を drop / merge / split / trim / rewrite。Linter Warn で検出した類似 slug 乱立 / sources 過多 / `replaced` 滞留はここで収斂させる
- **書き込み先**: `memory/*` と `knowledge/*`。Workflow 禁止は Linter で担保（`workflow.md` 参照）
- **完了処理**: consumed ID list の staging のみ cleanup（実行中に extract が追加した分は残す）。consolidation 完了時に staging に新着があれば次を発火（Coalesce）
- **モデル**: `memory.consolidation_model`。reasoning 系

##### 並走防止

- 場所: staging 配下に 1 ファイル（名前・形式は未定）
- 中身: 動作中の Pod 識別子 + **consumed ID list**（この consolidation run が起動時スナップショットで確定した staging エントリ ID の列）
- 占有ルール: そのファイルが存在し、示された Pod が動作している間、そのプロセスが排他占有
- 実行中に extract が staging に追加したエントリは触らず、次回 consolidation（Coalesce）に委ねる
- cleanup は consumed ID list のエントリのみ削除、追加分は残す
- クラッシュ時は consumed ID list から処理途中を特定できる。重複作成は同一 slug update に自然収束
- 占有の実現方法（pid 存在確認 / flock / 他）は未定

#### consolidation agent への原則

`memory/` 配下は人間も git 経由で編集する。consolidation prompt で以下を明示:

- **rewrite は許可**。既存内容と新規情報を統合・再構成して情報密度を上げることを優先。単純 append（追記で増やすだけ）は避ける
- rewrite 時は**情報損失を最小化**する: 既存の主張・根拠・sources を保持。表現を整理・短縮しても、含まれている要素は落とさない
- Decision の置き換えは `status: replaced` + `replaced_by: <slug>` で表現、直接削除しない
- 整理 step での drop は許可。ただし保護閾値（§判断ルール）超過 record は drop / 大幅圧縮の対象外。誤判定しやすいものは merge / trim を優先
- 各 record の整理理由は `outdated | superseded | unused | noisy` の §評価カテゴリ で説明可能にし、git diff から読み取れる粒度の操作にする
- Knowledge は既存 record 群の slug / description / kind / `model_invokation` を入口に適合先を探し、自然に統合できるなら新規 slug を増やさない
- 人間編集は git diff で顕在化する前提。整合しない rewrite は避け、衝突時は git で解決

#### Offer 経路

Memory record の書き込みは consolidation が自律判断し、Offer は設けない（Knowledge 含む）。人間承認経路が必要なのは以下:

- Workflow 関連の offer（新規作成 / 改善 / `model_invokation` ON 化）は `workflow.md` 参照

#### Compact との関係

基本分離（memory は独立トリガー、compact は `input_tokens` 既存閾値のまま）。compact で失われる session log の raw は **extract が compact より前に走ることで staging に保全**される（§Extract §Compact との順序 参照）。consolidation を compact に同期させる義務はなく、staging 累積閾値で独立に発火する。

### 整理（GC 相当）の扱い

consolidation は rewrite 許可で情報統合寄りの働きをするが、それでも残る以下の課題は **consolidation の余力 step で同じ agent が処理**する（独立 trigger / 独立 Agent は持たない）:

- 重要度の低い record が累積する
- 類似 slug が乱立する（Linter Warn で検出したものをまとめて処理）
- `replaced` が溜まり続けて grep / 注入時のノイズになる
- sources 累積
- 現状と不整合になった record、実質的に置き換え済みの record、使われていない record、形がノイズ化した record の整理

他プロジェクトの GC 設計の横断比較は `docs/ref/memory-systems.md` §8。

#### 操作粒度

整理 step は consolidation 統合 step と同じ memory 専用 Tool（read / write / edit、内部で pre-write Linter）を使う。operation 粒度は自然にサポートされる（専用 API は用意しない）:

- **ファイル単位**: 丸ごと drop、複数ファイルの merge、1 ファイルの分割（split）
- **ファイル内の部分削除**: 本文の一部節・箇条を削除 or 圧縮。frontmatter の `sources` 古いエントリの trim も含む

#### 評価カテゴリ

整理対象 record は一律に「stale」とみなさず、少なくとも次の 4 カテゴリで評価する:

- `outdated`: 以前は妥当だったが、現在の実装・方針・運用と不整合になっている
- `superseded`: 別 record が実質的な正本になっており、元の record は置き換え済みに近い
- `unused`: 誤りではないが、明示 invoke や検索でほとんど参照されずノイズ化している
- `noisy`: 内容自体は有効でも、粒度・重複・冗長さ・sources 過多などで discovery / retrieval を悪化させている

これらは **保護条件ではなく整理理由の分類**。保護条件は別に持ち、その上で `drop / merge / split / trim / rewrite` のどれを選ぶかをこのカテゴリで説明可能にする。

#### 使用頻度メトリクス

時間単位は実時間を使わない（LLM スループット向上で陳腐化の意味が変わるため）、累積 input token で正規化する。

**観測経路**: `memory/*` / `knowledge/*` への読み取りは §retrieval 経路 で定義した memory 検索ツール / Knowledge 検索ツール（既存 built-in の grep / read とは別に用意）経由に揃える。invoke 計測はツール内でフックし、`#<slug>` / `/<slug>` / 明示検索呼び出しを同一経路に集約する。

**カウント対象**:

- **明示 invoke**: 検索ツール経由の読み取り / `#<slug>` / `/<slug>` を n回/Mtoken でスコア化
- **`model_invokation` 注入**: 注入は context 常駐コストで、「載っているだけ」か「使われた」かを統計上区別不能。明示 invoke の分子には含めず、**コスト側（注入した record に対する消費 input tokens）として別途記録**する。使われ率 ratio や ON/OFF 判断の材料として後段で使う
- ファイル token 数

**記録先**: staging とは独立。invoke event を UUID + Stats 形式で workspace 側に記録し、session データが失われても統計が残るようにする。具体 schema・フォーマットは未定。

**累積方式**（後集計アプローチ）: 上記 invoke 記録に対して最大 10 回前の invoke から現在までの時系列窓でフィルタして集計する。

**Knowledge 化候補レポート**: consolidation 統合 step が入力に受け取る、Knowledge 新規作成 gate 用の機械集計。対象は `memory/*` 配下の record（extract 成果物である decisions / requests / 既存 knowledge）で、明示 invoke 頻度が閾値超過のものを列挙する。spike 除外のため、同一 session 内の連続参照は 1 count に丸め、複数 session での再参照を要件とする。閾値の具体値は運用で調整、設定ファイルで tune。

#### 判断ルール

- 保護閾値: **明示 invoke** の `frequency >= 1.0 invokes/Mtoken` の record は drop / 大幅圧縮の対象外（初期値 1.0、workspace 設定でカスタマイズ可）。`model_invokation` 注入による常駐は計数対象外（別指標として後段で参照）
- 整理 step の評価カテゴリは `outdated | superseded | unused | noisy` を使う。単一 record が複数カテゴリに該当してもよい

### ファイル形式

- frontmatter + Markdown 本文。全 record 共通: `created_at`, `updated_at`
- ファイル名（slug）がそのまま識別子、frontmatter に `id` / `name` field は持たない
- source トレーサビリティ（session log への逆引き、粒度は `session_id` + entry range）:
  - Decisions / Requests: `sources: [{session_id, range: [start, end]}, ...]` 永続化（update 時は追記累積）
  - Knowledge: `last_sources: [{session_id, range}, ...]`（最新更新時のみ、過去履歴は git log で追う）
  - Summary: optional `last_rewritten_from_range`（なしでも可）
- Knowledge 固有: `kind`, `description`, `model_invokation`, `user_invocable`
- Knowledge の保存先は `knowledge/<slug>.md`。`memory/` とは兄弟ディレクトリに分ける
- Decisions 固有: `status: open | resolved | replaced`、置き換え時は `replaced_by: <slug>`
- extract staging: `memory/_staging/<id>.json`（JSON、1 件 1 ファイル、consolidation 完了で削除。短命なので UUIDv7 可）。pod 側ラッパーが `source` を機械付与して LLM 出力と wrap
- Workflow の frontmatter は `workflow.md` 参照

## Scope 外

- ネットワーク越境での memory 同期 — `network-peering.md` で扱う範疇。本設計はプロジェクトスコープ固定

### 将来検討（運用で必要性が見えたら追加）

- 監査 LLM 層（意味破壊検出）の導入 — 初期は静的 Linter のみで運用し、consolidation の rewrite で情報損失・主題ズレが実運用で顕在化したら memory tool 内の検証パイプラインに 2 層目として追加。入力 / check 項目 / pass-fail 返却形式は導入時に詰める
- Vector index / FTS5 等の検索索引 — 初期は grep で足りる想定。ファイル数増加で検索が重くなったら検討
- `model_invokation` offer の自動判定ロジック — 初期は人間が手動で切り替え
- 過去 session を cross-session で検索する UI
- consolidation を担う常駐 daemon 化 — オンデマンド + lock 方式で始める。必要性が出たら upgrade path として daemon 化
- Deterministic promotion（OpenClaw 型 scoring + ゲート）— 初期は consolidation agent の LLM 判断に委ねる。運用実績で出力を評価してから、成熟カテゴリから scoring 導入
- Shallow request の自動除外判定 — 初期は extract prompt で「些細な質問は返さなくてよい」と指示する程度。精緻な filter は後

### 別 plan / ticket で扱う

- 具体クレート構成・API 境界
