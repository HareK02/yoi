# メモリ機構の prompt 要件

## Context

`docs/plan/memory.md` で決めた memory の挙動を、実装時に prompt 要件へ落とすための補助ページ。ここでは「個別タスクをこなすベーシックな system / developer 指示」の上に追加で必要になる、memory 固有の指示だけをまとめる。

前提は **tool-based + agentic**。専用 schema で挙動を閉じ込めるのではなく、基本は sub-Worker に CRUD tool を渡し、prompt と post-write 検証で振る舞いを制約する。例外は Workflow で、`docs/plan/workflow.md` の通り自動書き込みはしない。

## 決定事項

### 共通原則

memory 関連 prompt は種別を問わず、最低限以下を共有する:

- **source を推論しない**。`session_id` や entry range は wrapper / 呼び出し側が機械付与し、LLM が捏造しない
- **rewrite は許可**するが、情報損失は最小化する。既存の主張・根拠・参照を保ったまま整理・圧縮する
- **単純 append を優先しない**。既存 record に統合できるなら update を優先する
- **session 固有の進行状態を書かない**。長期参照価値のある内容だけを memory に残す
- **既存 docs と重複保存しない**。`AGENTS.md`、`docs/plan/*`、固定運用文書に既にある内容を再保存しない
- **git で追える事実を memory に書かない**。ticket file の作成・編集、TODO 更新、branch / worktree 操作、commit / merge / push、「commit X で実装した」「ticket Y を作った」「worker Pod を spawn した」等は git diff / log が真実で、memory に写すと陳腐化する。commit ハッシュ・branch 名・worktree パス・ticket file 名・PR 番号と組み合わせないと意味を成さない記録は採用しない
- **空出力を許容**する。保存価値が無ければ「何も追加しない」を正当な結果として扱う

### Phase 1: 活動抽出 prompt

Phase 1 は「派生物を作る」段階ではなく、「起きたことを抽出する」段階として縛る:

- 対象は `decisions`、`discussions`、`attempts`、`requests` の候補に限る
- Knowledge 化、summary rewrite、slug 命名、`model_invokation` 判断は行わない
- 一回限りの雑談、浅い質問、長期参照価値の薄い進行ログは返さなくてよい
- 出力は schema 準拠の構造化データのみ。自由文の補足説明で schema 外情報を足さない
- 対象が無ければ空配列を返す

ノイズ防御として、抽出時点で以下を除外する:

- `attempts`: git で追える操作 (ticket file / TODO 編集、branch / worktree 作成、commit / merge / push、既知 ticket への worker Pod spawn) は除外。残すのは git からは復元できない情報 (ビルド/テスト結果、外部 API 応答、観測されたバグ再現、後段の判断材料となる設計実験結果) に限る
- `discussions`: 当日中に陳腐化する一過性 triage (「次に着手するチケットはどれか」「いま review すべきか後でか」など) は除外。session を越えて意味を持つ論点 (アーキテクチャの trade-off、恒常的な制約、再来する問い) のみ残す
- `decisions`: rationale が「この session で X をした」になるものは除外。設計 / 方針 / 取り組み方の根拠でない記録は decision ではなく作業ログ
- 本文中に commit ハッシュ・branch 名・worktree パス・ticket file 名・PR 番号など陳腐化する identifier を埋め込まない

### Phase 2: 統合 + 整理 prompt

Phase 2 は既存 `memory/*`、`knowledge/*`、staging を見て、統合 phase と整理 phase を 1 セッション内で続けて回す。両 phase に共通する原則:

- 入力には staging の活動ログ、既存 `memory/*`（summary / decisions / requests）の全文、Knowledge 化候補レポート、整理材料（使用頻度メトリクス、Linter Warn、`replaced` chain、sources 過多情報）を含める
- 既存 `knowledge/*` は prompt に埋めず、Knowledge 検索ツール経由で agent が必要分を引く。まず候補レポートの source や staging の話題に近い slug を検索し、ヒットした slug / description / kind / `model_invokation` を見て適合先を探す
- 新規作成より update を優先し、既存 slug に自然に統合できる場合は新規 file を増やさない
- Decisions / Requests は staging の `source` をそのまま使い、LLM が `sources` を組み立てない
- summary は必要なときだけ rewrite し、常に 1-5k tokens 目安に圧縮する
- Decision の置き換えは `status: replaced` と `replaced_by` で表現する
- 人間編集との不整合が見える rewrite は避け、衝突しそうなら保守的に統合する

統合 phase の追加指示:

- staging の活動ログを decisions / requests / summary / Knowledge update に落とし込む
- staging の field ごとに宛先を分ける:
  - `decisions` (staging) → `memory/decisions/`。設計 / 方針 / 取り組み方の判断のみ。「この session で X をした」型は drop
  - `requests` (staging) → `memory/requests/`
  - `attempts` (staging) → 既定は drop。memory に `attempts/` フォルダは設けない。複数 attempts に通底する持続的な傾向だけ `summary.md` に 1 行で圧縮する例外あり
  - `discussions` (staging) → 設計 / 方針に決着していれば `decisions/` に統合、未決着でも問い自体が持続的なら `summary.md` に 1 行、それ以外は drop。`decisions/` に「議論した」だけの未決着メモを作らない
- Knowledge 新規作成は候補レポート掲載 source 由来に限る（詳細は §Phase 2: Knowledge 書き込み prompt）

整理 phase の追加指示（統合 phase 完了後、余力で実行）:

- 既存 record 群を `outdated`、`superseded`、`unused`、`noisy` の観点で評価し、なぜ整理対象なのかを分類する
- 明示 invoke の保護閾値超過 record は drop / 大幅圧縮の対象外とする
- `similar-slug`、`sources-overflow`、`replaced` 滞留は主に `superseded` または `noisy` の材料として扱う
- merge / split / trim / drop の理由を git diff から読める形で残す
- 直接削除してよいが、git で可逆である前提に甘えすぎず、誤判定しやすいものは merge / trim を優先する

### Phase 2: Knowledge 書き込み prompt

Knowledge の新規作成 / 更新では、Phase 2 全体の原則に加えて以下を明示する:

- 採択ラインは「このプロジェクト / ユーザーに対して再度参照価値のある事実・ルール・ノウハウ」に限る
- 一回限りの判断や議論は Decisions に留め、繰り返し参照される抽象化だけを Knowledge に上げる
- 新規作成は Knowledge 化候補レポートに載った source から派生する場合に限る
- 既存 Knowledge は Knowledge 検索ツールで検索し、ヒットした slug / description / kind / `model_invokation` を見て、適合先があるなら必ず update を優先する
- 新規 slug は「既存に適合先が無い」と説明できるときだけ作る
- Knowledge は `kind` を frontmatter に持ち、少なくとも「用語 / 運用方針 / ルール / 事実 / ノウハウ」のどこに寄るかを明示する
- `last_sources` は入力で与えられた source を使い、推論で補完しない
- `description` は「何の知識か / いつ使うか」が短く分かる文にする
- description だけで対象範囲が分からない粒度や、複数主題を抱えた file は避ける
- `model_invokation` ON/OFF は頻度・常駐コストの判断材料を踏まえて慎重に扱い、初期値は OFF とする
- `#<slug>` 参照を書く場合は、実在 record への参照だけを使う
- `AGENTS.md` や `docs/` に既に固定化されたルールの写しは作らない
- 保存価値が無ければ Knowledge を何も追加しない

### 監査 LLM prompt

初期範囲では専用の監査 LLM は持たない（`memory.md` §書き込み経路と Linter / §将来検討 参照）。意味破壊の抑制は Phase 2 prompt 側の情報損失最小化指示と git diff レビューに寄せる。後から 2 層目として挟む際の入力・check 項目・pass-fail 返却形式はそのときに詰める。

## 関連

- `docs/plan/memory.md`: memory 全体方針
- `docs/plan/workflow.md`: workflow を自動書き込みの例外にしている理由
