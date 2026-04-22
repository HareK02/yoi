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
- **空出力を許容**する。保存価値が無ければ「何も追加しない」を正当な結果として扱う

### Phase 1: 活動抽出 prompt

Phase 1 は「派生物を作る」段階ではなく、「起きたことを抽出する」段階として縛る:

- 対象は `decisions`、`discussions`、`attempts`、`requests` の候補に限る
- Knowledge 化、summary rewrite、slug 命名、`auto_invoke` 判断は行わない
- 一回限りの雑談、浅い質問、長期参照価値の薄い進行ログは返さなくてよい
- 出力は schema 準拠の構造化データのみ。自由文の補足説明で schema 外情報を足さない
- 対象が無ければ空配列を返す

### Phase 2: 統合 prompt

Phase 2 は既存 `memory/*` と staging を見て、追加・更新・統合を agentic に判断する:

- 入力には staging の活動ログ、既存 `memory/*`、Knowledge 化候補レポートを含める
- 新規作成より update を優先し、既存 slug に自然に統合できる場合は新規 file を増やさない
- Decisions / Requests は staging の `source` をそのまま使い、LLM が `sources` を組み立てない
- summary は必要なときだけ rewrite し、常に 1-5k tokens 目安に圧縮する
- 削除は直接行わず、Decision の置き換えは `status: replaced` と `replaced_by` で表現する
- 人間編集との不整合が見える rewrite は避け、衝突しそうなら保守的に統合する

### Phase 2: Knowledge 書き込み prompt

Knowledge の新規作成 / 更新では、Phase 2 全体の原則に加えて以下を明示する:

- 採択ラインは「このプロジェクト / ユーザーに対して再度参照価値のある事実・ルール・ノウハウ」に限る
- 一回限りの判断や議論は Decisions に留め、繰り返し参照される抽象化だけを Knowledge に上げる
- 新規作成は Knowledge 化候補レポートに載った source から派生する場合に限る
- 既存 Knowledge の slug / description 一覧を見て、適合先があるなら必ず update を優先する
- 新規 slug は「既存に適合先が無い」と説明できるときだけ作る
- `last_sources` は入力で与えられた source を使い、推論で補完しない
- `description` は「何の知識か / いつ使うか」が短く分かる文にする
- `auto_invoke` ON/OFF は頻度・常駐コストの判断材料を踏まえて慎重に扱い、初期値は OFF とする
- `#<slug>` 参照を書く場合は、実在 record への参照だけを使う
- `AGENTS.md` や `docs/` に既に固定化されたルールの写しは作らない
- 保存価値が無ければ Knowledge を何も追加しない

### 監査 LLM prompt

監査 LLM は「よく書けているか」ではなく、「壊していないか」を見る:

- 入力には write 前後 diff、対象 record の直前内容、今回の source / staging 抜粋、採択基準を渡す
- rewrite / 圧縮で主張、根拠、参照が失われていないかを確認する
- 新規追加が source や活動ログに裏打ちされているかを確認する
- session 固有の進行状態や一時的事情が混入していないかを確認する
- `description` が本文のスコープと一致しているかを確認する
- `auto_invoke` の設定が本文の重要度や再利用性と不整合でないかを確認する
- 問題がある場合は `pass | fail` に加えて違反カテゴリと具体箇所を返し、Hook が差し戻せる形にする

### GC prompt

GC は Phase 2 より攻撃的に整理してよいが、可逆性と説明可能性を保つ:

- 入力には GC 対象 record 群に加えて、Linter Warn、使用頻度メトリクス、`replaced` chain、sources 過多情報を含める
- 明示 invoke 保護閾値を超える record は drop / 大幅圧縮の対象外とする
- `similar-slug`、`sources-overflow`、`replaced` 滞留、stale record を優先的に処理する
- merge / split / trim / drop の理由を diff から読める形で残す
- 直接削除してよいが、git で可逆である前提に甘えすぎず、誤判定しやすいものは merge / trim を優先する

## 関連

- `docs/plan/memory.md`: memory 全体方針
- `docs/plan/workflow.md`: workflow を自動書き込みの例外にしている理由
