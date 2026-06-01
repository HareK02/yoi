# エージェント向けメモリ機構の外部事例

調査日: 2026-04-21。本ドキュメントはユーザー依頼の3ソース（OpenAI Codex Chronicle / Shann³ の "AI Knowledge Layer" スレッド / Nous Research Hermes Agent）を中心に、直近で公開されたメモリ機構をまとめ、yoi に入れる際の比較材料とすることを目的とする。2026年前半は各所からメモリ実装が同時多発的に登場しているので、「どの事例がどのレイヤを担っているか」を見失わないよう、各節で**何を記憶するか／いつ書くか／どう引き出すか／何に保存するか**を揃えて整理する。

数値・URLは一次ソースで再確認すること。挙動は研究プレビュー段階のものが多く、変わる前提で読む。

---

## 1. OpenAI Codex — Memories & Chronicle

Codex CLI に 2026-03 頃から追加された "Memories" 機能と、2026-04-15 頃に macOS 向け研究プレビューとして乗った "Chronicle"。スクリーン内容まで観測対象に広げた点が目新しいが、**コアは素朴な extract / consolidation の 2 段パイプライン**で、実装の示唆が多い。

### データフロー（memories 本体）

セッション開始時に走る 2-step パイプライン。`codex-rs/core/src/memories/` を直接読み確認した挙動:

- **extract — Extraction**（`extraction implementation`）: 対象 rollout ごとにモデル呼び出しで **JSON schema 強制**の `StageOneOutput { raw_memory, rollout_summary, rollout_slug }` を返させる（`#[serde(deny_unknown_fields)]`）。`CONCURRENCY_LIMIT = 8` で並列実行、結果は SQLite の `stage1_outputs` テーブルに格納。`StateRuntime` の job leasing で duplicate work 防止
  - モデル: `gpt-5.4-mini` / Low reasoning（`memories.extract_model` で override 可）
  - テンプレ: `codex-rs/core/templates/memories/stage_one_system.md` + `stage_one_input.md`
- **consolidation — Consolidation**（`consolidation implementation`）: **singleton** として走る（`jobs` テーブルで `kind = 'memory_consolidate_global'`, `job_key = 'global'` を `wx` 相当に claim）。`ConsolidationInputSelection` で前回 baseline との **added / retained / removed** 差分を計算し、その差分を prompt に埋めて sub-agent に投入
  - 出力は **自由形式 Markdown**（JSON schema なし）。sub-agent が memory_root を cwd に持ち、`MEMORY.md` / `memory_summary.md` / `skills/<name>/` を直接書き換える
  - モデル: `gpt-5.4` / Medium reasoning（`memories.consolidation_model` で override 可）
  - Heartbeat 90s / lease 3600s、失敗時 `retry_remaining` デフォルト 3
  - テンプレ: `codex-rs/core/templates/memories/consolidation.md`（836 行の詳細 instructions）
- 生成タイミングは「スレッドが十分アイドルになってから」（age + idle window で判定）
- `memory_summary.md` が **5,000 tokens cap** で system prompt に注入される（`MEMORY_TOOL_DEVELOPER_INSTRUCTIONS_SUMMARY_TOKEN_LIMIT`）

**非対称の肝**: 抽出 (extract) は structured output で分類を強制、統合 (consolidation) は sub-agent に自由書き込みさせる。分類ブレを extract で封じ、統合の柔軟性は consolidation の agentic 判断に委ねる、という役割分担。yoi の extract / consolidation 設計の直接の元ネタ。

### 保存場所 / 形式

- `$CODEX_HOME/memories/`（既定 `~/.codex/memories/`）配下に Markdown。
- `memory_summary.md` を中心に「summaries / durable entries / recent inputs / supporting evidence」の Markdown を束ねる構成。
- Chronicle 派生メモリは `$CODEX_HOME/memories_extensions/chronicle/` に分離。
- スクリーンキャプチャ中間物は `$TMPDIR/chronicle/screen_recording/` に置かれ、running 中 6h で自動削除。サーバー側には保存しない（法的義務時を除く）。
- **Extension resource retention は決定論的 7 日 hard-coded**（`EXTENSION_RESOURCE_RETENTION_DAYS = 7`, `memories/extensions.rs:11`）。filename embedded timestamp (`%Y-%m-%dT%H-%M-%S`) が cutoff より古い `.md` リソースを consolidation 直前に物理削除し、削除リストを `removed_extension_resources` として consolidation prompt に渡す（agent はそれを見て派生メモリを抹消）。

### 設定

- `memories.generate_memories` / `memories.use_memories`: 生成 / 注入の on-off。
- `memories.extract_model`: extract に使う軽量モデル。
- `memories.consolidation_model`: consolidation のマージ側モデル（既定で reasoning 系）。
- セッション内 `/memories` コマンドでスレッド単位に無効化可能。
- シークレットは生成時に自動 redaction。

### Chronicle 固有

- 画面スクリーンショット + OCR テキスト + タイミング + ローカルファイルパスを入力に、**サンドボックス化した Codex セッション**をバックグラウンドで回して Markdown メモリを吐く。
- 「Chronicle は rate limit を激しく消費する」「画面由来の prompt injection リスクが増える」「ローカル unencrypted」が公式に注意書きされている。Pro / macOS / 非EU+UK+CH 限定。
- 可搬性の観点では、生成物が単なる Markdown ファイルでユーザーが手編集できる点が重要。

### 設計上の示唆

- **圧縮は reasoning モデルで非同期に走らせる**: 会話ループの応答性を落とさず、コストも分離する。
- **中間テーブル（SQLite）と最終出力（Markdown）の 2 段構成**: 解析容易性と人間編集性を両立。
- **5k tokens 固定注入**: 動的 retrieval ではなく "常駐 summary" として使う素朴さ。
- **拡張はサブディレクトリで**: `memories_extensions/<name>/` というパターンで観測チャンネルを増やす構造。

一次ソース:
- https://developers.openai.com/codex/memories
- https://developers.openai.com/codex/memories/chronicle
- https://deepwiki.com/openai/codex/3.7-memory-system
- https://9to5mac.com/2026/04/20/codex-for-mac-gains-chronicle-for-enhancing-context-using-recent-screen-content/

---

## 2. Shann³ "AI Knowledge Layer"

https://x.com/shannholmberg/status/2044111115878326444 で提唱している、**エージェントより先に読ませる知識層**という枠組み。エンジニア向けというよりマーケター / コンテンツ運用者向けだが、構造はかなり yoi に転用しやすい。

### 2 層構造

- **Knowledge Base Layer (KBL) — 動的**
  - 生素材（tweet / 記事 / ブックマーク / PDF / ノート / ボイスメモ）を raw フォルダへ投げ込む。
  - 別エージェントが読んで種類別に分類、相互参照つきの構造化 wiki ページ化、マスターインデックスを維持。
  - 毎日少しずつ賢くなる前提。

- **Brand Foundation (BF) — 静的**
  - 声のルール / 視覚スタイル / ポジショニング / オーディエンス定義 / 禁則語などを**ユーザーだけが編集**。
  - エージェントは生成前に必ず BF を読む。書き換えはしない。
  - 別ソースの続編 ("Ronin runs 10 social accounts ... 17 markdown files") でも同じパターン。KBL + BF + エージェント 1 本で 10 アカウント運用している。

### ルーツ: Karpathy の llm-wiki

Shann³ の構成は Karpathy が 2026-04 に公開した `llm-wiki` gist をそのまま応用したもの。構造は 3 層：

- `raw/` — 人間のみ書き込む生ソース。
- `wiki/` — LLM 所有、page / index / glossary / log を自動維持。1 ソース取り込みで 10-15 ページが同時更新される。
- `CLAUDE.md`（または `AGENTS.md` / `GEMINI.md`）— ページ種別・ワークフロー・フォーマット・健全性チェックルールを定義するスキーマ。

運用は3種:

- **ingest**: 新規ソース投入 → 要約ページ作成 → index 更新 → entity/concept 更新 → `log.md` 追記。
- **query**: wiki を検索し citation 付きで答える。意義ある合成は `syntheses/` として保存。
- **lint**: 矛盾・stale claim・孤立ページ・参照抜け・データ欠落の定期点検。

`SamurAIGPT/llm-wiki-agent` がこのパターンを具体化しており、ディレクトリ構造が示唆的（なお `/wiki-lint` のプロダクション実装としては OpenClaw `memory-wiki` extension があり、§4 / §9 で触れる）：

```
wiki/
├── index.md          (全ページのカタログ)
├── log.md            (append-only 作業記録)
├── overview.md       (生きた合成ビュー)
├── sources/          (原文1つ = 1サマリ)
├── entities/         (人・会社・プロジェクト)
├── concepts/         (考え方・フレーム・手法)
└── syntheses/        (query 回答をページ化)
graph/
├── graph.json        (SHA256 キャッシュ付きノード/エッジ)
└── graph.html        (vis.js で可視化)
```

スラッシュコマンド例:
```
/wiki-ingest raw/papers/my-paper.md
/wiki-query "what are the main themes?"
/wiki-lint
/wiki-graph
```

### LLM Wiki vs RAG という論点

MindStudio の比較記事が要点をまとめている。

- **LLM Wiki が有利**: 文書 100 本未満、構造化コンテンツ、精度・監査性重視、速度（Git で versioning できる）。
- **RAG が必要**: 1,000 本以上、非構造テキスト、open-ended な質問、未知のドメイン。
- 決定論的 wikilink retrieval と、SHA256 キャッシュで差分のみ再処理。

### 設計上の示唆

- **可変 KBL と不変 BF の分離**が強い。yoi なら前者が Chronicle 的な自動メモリ、後者が `AGENTS.md` / 人間のガイド。
- **retrieval を埋め込みではなく決定論的 wikilink でやる**アプローチは、少量ドメインで意外と強い。
- エージェントに書かせる `log.md` と `index.md` の append-only 運用は、後で diff / git で検証しやすい。

一次ソース:
- https://x.com/shannholmberg/status/2044111115878326444
- https://x.com/shannholmberg/status/2043307903822844087 (Ronin / 17 markdown files)
- https://gist.github.com/karpathy/442a6bf555914893e9891c11519de94f
- https://github.com/SamurAIGPT/llm-wiki-agent
- https://www.mindstudio.ai/blog/llm-wiki-vs-rag-knowledge-base

---

## 3. Nous Research Hermes Agent

Self-improving agent を名乗るフレーム。メモリ周りは **3 層 + クローズドな学習ループ**。

### 3 層メモリ

- **Persistent Memory**: `MEMORY.md` / `USER.md` の Markdown + SQLite + FTS5 による過去セッション全文検索。char limit が hard-coded で **`MEMORY.md` 2,200 chars / `USER.md` 1,375 chars**（合計 ≒ 1,300 tokens）、system prompt 起動時スナップショット
- **Skill Library**: 複雑タスクの帰結として skill 化。`agentskills.io` 標準準拠（§5）、`~/.hermes/skills/<name>/SKILL.md` に保存。`skill_manage` tool (action: create/patch/edit/delete/write_file/remove_file) 経由で agent が自ら CRUD する
- **User Model (Honcho)**: "dialectic user modeling" でセッションを跨いでユーザー像を蓄積（外部 Honcho サービス連携）

### 学習ループ（`run_agent.py` 実装より）

既存 ref の「5 + tool call で自律生成」「15 tool call ごと reflection」は誤り。実装を読むと:

- `_skill_nudge_interval = 10`（デフォルト、設定可）で **skill review**、`_iters_since_skill` がツール呼び出しのたび increment
- memory nudge は別に **10 turns** で発火
- 閾値超過で **background thread に別 AIAgent を fork**（max_iterations = 8）、review prompt を与える
- レビュー本体は **1 LLM call** の自由形式、続いて agent が必要なら `skill_manage` / `memory` tool を呼んで書き戻す
- review prompt 原文（`_SKILL_REVIEW_PROMPT`, `run_agent.py:2738-2746`）:
  > Review the conversation above and consider saving or updating a skill if appropriate.
  > Focus on: was a non-trivial approach used to complete a task that required trial and error...
  > **If nothing is worth saving, just say 'Nothing to save.' and stop.**
- 末尾の "Nothing to save." 指示が肝。頻繁発火でも中身ゼロの場合は NOP で抜ける設計。yoi extract の「空配列許容」の直接ソース

### 書き込み機構

- `skill_manage` tool: OpenAI function-calling schema。frontmatter YAML 検証、security scan、atomic write (temp + `os.replace()`)
- `memory` tool: target = `memory | user`、action = `add | replace | remove`。fcntl/msvcrt で file lock、entry-based rewrite
- **char limit 超過時は `add` を hard reject**（`memory_tool.py:248-259`）。エラーメッセージに `Memory at {current}/{limit} chars. Adding this entry would exceed the limit. Replace or remove existing entries first.` と明記し、LLM 側に「自分で削減してから再追加」を強制する。GC は完全に LLM agentic（決定論的 eviction は無い）、人間 offer や scoring も無い、非常にシンプルな設計
- FTS5: `messages_fts` virtual table + INSERT/DELETE/UPDATE trigger で auto-sync、CJK は LIKE フォールバック

### 実装観点

- 主要ディレクトリ: `agent/` `skills/` `tools/` `gateway/` `hermes_cli/` `tui_gateway/` `cron/` `plugins/` `optional-skills/` など
- バックエンド抽象: local / Docker / SSH / Daytona / Singularity / Modal。Daytona / Modal ではアイドル時 hibernation で永続化コストを削る
- OpenClaw からの移行: `hermes claw migrate`

### 設計上の示唆

- **"skill"＝ procedural memory として episodic / semantic から分離する**のは yoi にとっても綺麗。Claude Code の skill と接続できる余地がある。
- **FTS5 + LLM 要約ハイブリッド**で、ベクタを入れずにそこそこ回せる事例として参考価値が高い（少量ドメインなら LLM Wiki 論と同じ示唆）。
- **Honcho 的 user model** を semantic profile として固定注入する運用は、Codex の memory summary と形式的に同じ。

一次ソース:
- https://github.com/NousResearch/hermes-agent
- https://github.com/NousResearch/hermes-agent/blob/main/README.md
- https://hermes-agent.nousresearch.com/docs/
- https://hermes-agent.nousresearch.com/

---

## 4. 周辺事例（比較のため）

### Letta / MemGPT

- 4 区画: **Message Buffer**（直近）、**Core Memory**（in-context の編集可能 block: user prefs / persona）、**Recall Memory**（全履歴の検索）、**Archival Memory**（ベクトル or グラフ DB に外部化された宣言的知識）。
- OS メタファ: context window = RAM、外部ストア = Disk。エージェントが function call で階層間を移動させる。
- memory block = `{label, description, value, char_limit}`。
- **sleep-time agent** が idle 中に非同期で block を書き換える。主要 agent と **別 process で並列**に走り、`core_memory_replace` / `core_memory_append` / archival tool 群を呼んで **直接 memory block を書き換える**。主 agent のターン処理をブロックしないため、「mid-session で memory 品質を上げる」のが特徴。
- context 限界近くで要約による eviction → 再帰要約で古いメッセージを累進圧縮。**eviction 判断は context window fill の決定論的しきい値**で発火するが、要約の内容生成は LLM。evicted message は recursive summary として memory block に残り、古いほど重みが相対的に下がる。

### Cloudflare Agent Memory (2026-04 Agents Week)

- 分類: **Fact / Event / Instruction / Task**（Task のみベクトル索引除外）。
- 基盤: Durable Objects（プロファイル毎の SQLite、FTS・supersession chain・tx write を担う）、Vectorize（セマンティック）、Workers AI（Llama 4 Scout: 構造化、Nemotron 3: 自然文合成）。
- **Supersession chain は決定論的**（公式 blog 再確認）: Fact / Instruction は classification pipeline で **normalized topic key** が振られ、同 key の新 memory が来ると旧 memory を **supersede（物理削除せず forward pointer で版チェーン化）**。LLM 判断は入らない。superseded 側の vector は並行して削除され、新 vector が upsert される。SHA-256 content-addressed ID で `INSERT OR IGNORE` による冪等 dedupe も併用。
- API:
  ```javascript
  const profile = await env.MEMORY.getProfile("my-project");
  await profile.ingest([messages], { sessionId });
  await profile.remember({ content, sessionId });
  const results = await profile.recall("query");
  ```
- Ingest は 10k 文字チャンク + 2 メッセージ重複、並行 4 チャンク。Verification が 8 種類（entity / temporal / factual 支援）。
- Retrieval は 5 経路並行（FTS Porter stemming / exact fact-key / raw message / ベクトル直 / HyDE）を Reciprocal Rank Fusion でマージ。fact-key 優先、recency タイブレーク。時制クエリは regex + 算術で決定論的処理。
- SHA-256 content-addressed ID で冪等。
- 「メモリはあなたのもの」エクスポート前提。

### LinkedIn Cognitive Memory Agent (CMA)

- 3 層: **Episodic**（対話履歴）/ **Semantic**（構造化事実）/ **Procedural**（学習済みワークフロー）。
- 共有メモリ基盤として planner / reasoner / executor 複数エージェントが同じ層を参照。
- Recent context retrieval + semantic search + summarization compaction で階層管理。
- 高 stake 用途は human validation フローを挟む。

### OpenClaw

Peter Steinberger が 2025-11 に出したメッセージングファースト agent。2026-02 に本人は OpenAI 入社、プロジェクトは foundation へ移譲。Hermes Agent の `hermes claw migrate` はここからの移行導線。設計は **Markdown ファイルのみに全状態を持つ**という極端な透明性志向で、yoi の「ファイル + git」方針との親和性が最も高い。

Agent Workspace（`~/.openclaw/workspace/`）構成:

| ファイル | 役割 |
|----------|------|
| `AGENTS.md` | 運用指示とメモリ使用ガイドライン |
| `SOUL.md` | ペルソナ・口調・境界 |
| `USER.md` | ユーザー識別と呼称の好み |
| `IDENTITY.md` | エージェント名・絵文字 |
| `TOOLS.md` | ローカルツールのメモ |
| `MEMORY.md` | 長期記憶（durable facts / preferences / decisions）。DM セッション起動時に常駐ロード |
| `memory/YYYY-MM-DD.md` | 日次ノート。**当日 + 前日のみ**自動ロード |
| `DREAMS.md` | dreaming sweep の人間レビュー用サマリ（任意） |
| `skills/` | workspace 固有 skill override（最高優先） |

エージェントが触るツール:

- `memory_search`: 文言違いでも引ける semantic search
- `memory_get`: ファイル or 行範囲の読み取り

メモリ更新のタイミング（`extensions/memory-core/src/` 実装より）:

- **Compaction 前の silent turn**: `before_agent_reply` フックで `DREAMING_SYSTEM_EVENT_TEXT` を検出、`runShortTermDreamingPromotionIfTriggered` が発火。エージェントへの write-back 指示ではなく、**後述の dreaming pass をインラインで走らせる**仕組み
- **Dreaming pass (optional)**: 実体は **3 passes** (Light + Deep + REM)、cron default `"0 3 * * *"` UTC
  - **Light**: 最近の recall / daily / session signals を short-term store に ingest + narrative 生成（**subagent LLM call 1 回**、`NARRATIVE_SYSTEM_PROMPT` で poetic な dream diary）
  - **Deep**: `applyShortTermPromotions()` が promotion を決定。**LLM call ゼロ**、以下の 6 重みスコアと 3 ゲートで機械判断
    - 重み: frequency (0.24) / relevance (0.30) / diversity (0.15) / recency (0.15) / consolidation (0.10) / conceptual (0.06) + pass boost
    - ゲート: `minScore 0.8` / `minRecallCount 3` / `minUniqueQueries 3`
  - **REM**: テーマ反映 + narrative LLM call 1 回
  - promotion 通過項目のみ `MEMORY.md` に append、`DREAMS.md` に人間レビュー用 diary
- **書き込み制約**: `MEMORY.md` は hardcoded path、`memory/YYYY-MM-DD.md` のみ許可（regex `SHORT_TERM_PATH_RE`）。エージェントへ expose されているのは `memory_search` / `memory_get` の **read-only**。書き込みは系統内部のみ
- **Lock**: `memory/.dreams/short-term-promotion.lock` を `wx` フラグで exclusive create、60s stale 検出 + 10s wait timeout、in-process の Map も併用
- **モデルが "覚えている" のはディスクに書かれた内容だけ**、という明示ポリシー。隠れた state 無し

**yoi にとって重要**: consolidation を LLM 依存から切り離せる見本。narrative は subagent が生成するが、promotion の判断は純機械（scoring）。yoi の plan では Scope 外（consolidation は当面 agent 委任）だが、成熟したカテゴリから決定論的 promotion に差し替える upgrade path の参考になる。

**GC 観点の追加詳細**（`extensions/memory-core/src/short-term-promotion.ts:1518-1652` 実装より）:

- `applyShortTermPromotions()` は **append 専用**。gate 通過候補の snippet を `MEMORY.md` 末尾に `<!-- openclaw-memory-promotion:<hash> --> ` marker 付きで追記するだけで、既存 `MEMORY.md` ブロックの書き換えや削除は一切行わない
- **重複回避**: marker set を先読みし、既に書かれた key はスキップ
- **汚染検出**: `isContaminatedDreamingSnippet()` で dreaming narrative prompt が snippet に混入している候補を promotion 前に弾く（再帰汚染防止）
- **日次ノートの decay は物理削除ではなく search score 減衰のみ**（`memory/temporal-decay.ts`）。`halfLifeDays = 30` で `exp(-ln2/HL * age)` を score に乗じる。対象は `memory/YYYY-MM-DD.md` 形式のファイル限定で、`MEMORY.md` や topic ファイルは evergreen 扱い（減衰しない）
- **自己参照汚染の退避**: `dreaming-repair.ts` は dreaming narrative が session corpus や `DREAMS.md` に逆流した場合を検出し、該当ファイルを `.openclaw-repair/dreaming/<timestamp>/` に **rename で退避**（削除ではない）。lint と同じく audit-first の GC スタイル
- `MEMORY.md` 側の GC（重複 block の統合、stale record の drop 等）は **この extension には存在しない**。書き込みは promotion の append のみで、ユーザーが手で消すか `memory-wiki` lint のレポート経由で扱う

OpenClaw は「**削除は人間、script は append と退避まで**」という強い原則が貫かれている。

設計上の示唆:

- Workspace = git リポジトリ 1 本で完結、配置もフラット。yoi の pod workspace 概念にそのまま借用できる。
- 秘密は workspace **外**の `~/.openclaw/` 側（auth / credentials / session transcripts / managed skills）に退避する分離設計は、pod sandbox 境界を越えない運用の手本になる。
- `memory/YYYY-MM-DD.md` の日次切り分け + 「当日+前日のみ load」は、時系列の自然減衰を Markdown で素直に表現できる良い pattern。

### LangChain Agent Builder memory（参考）

- 記事は 301 redirect のため未取得。`https://www.langchain.com/blog/how-we-built-agent-builders-memory-system` に原文あり。semantic / episodic / procedural 三分類は共通。

一次ソース:
- https://www.letta.com/blog/agent-memory
- https://blog.cloudflare.com/introducing-agent-memory/
- https://www.infoq.com/news/2026/04/linkedin-cognitive-memory-agent/
- https://github.com/openclaw/openclaw/blob/main/docs/concepts/memory.md
- https://docs.openclaw.ai/concepts/agent-workspace
- https://en.wikipedia.org/wiki/OpenClaw

---

## 5. Agent Skills 標準（procedural memory の実装単位）

Hermes / OpenClaw / Claude Code / OpenAI Codex / Cursor / GitHub Copilot 等が揃って採用している **`agentskills.io` オープン標準**。Anthropic が 2025-12 に発表し、2026-03 時点で事実上の共通フォーマットになっている。memory 設計の「手続き的記憶 (procedural memory)」のほぼ全てがこの単位で流通するので、yoi でも独自フォーマットを避けて素直にこれに乗るのが合理的。

### SKILL.md の最小仕様

```
skill-name/
├── SKILL.md          # 必須: frontmatter + Markdown 本文
├── scripts/          # 任意: 実行可能コード
├── references/       # 任意: 詳細ドキュメント
└── assets/           # 任意: テンプレ・画像・lookup など
```

```markdown
---
name: pdf-processing
description: Extract PDF text, fill forms, merge files. Use when handling PDFs.
license: Apache-2.0
metadata:
  author: example-org
  version: "1.0"
---
```

frontmatter 仕様:

| Field | 必須 | 制約 |
|-------|------|------|
| `name` | ○ | 64 字以下、小文字英数とハイフンのみ、親ディレクトリ名と一致 |
| `description` | ○ | 1024 字以下。**何をするか + いつ使うか**の両方を含める |
| `license` | ― | 任意 |
| `compatibility` | ― | 500 字以下。env 依存（特定 CLI / Python 3.14+ 等）がある場合 |
| `metadata` | ― | 任意 key/value |
| `allowed-tools` | ― | 実験的。space-separated `Bash(git:*) Read` 等 |

### Progressive disclosure（これが核）

Skill は 3 段でロードされる:

1. **Metadata (~100 tokens)**: `name` + `description` のみ、全 skill が起動時に常駐
2. **Instructions (<5k tokens 推奨)**: skill が発動した時に本文がロード
3. **Resources**: `scripts/` `references/` `assets/` は必要時のみ個別に読まれる

→ **常駐コストは「名前 + 一行説明」だけ**で、本体は遅延ロード。これがメモリ設計における宣言的知識 (LLM Wiki の `index.md` 常駐) と完全に同型で、**知識 / skill を同じ「目次のみ常駐・本体リンクで遅延」モデルに統一できる**。

### Claude Code の拡張

標準 + α として Claude Code が独自に足している項目:

- `disable-model-invocation: true`: ユーザーしか起動できない（deploy / commit 等の副作用ありタスクに使う）
- `user-invocable: false`: エージェントしか起動しない（背景知識用。`/` メニューから隠れる）
- `paths: ["src/**/*.ts"]`: **ファイル glob マッチ時のみ自動発動**。関心スコープを暗黙に絞る
- `context: fork` + `agent: Explore`: サブエージェントの fork context で実行（isolated context）
- `allowed-tools`: skill 有効時だけ事前承認されたツール
- `hooks`: skill ライフサイクルにフックを紐付け
- `$ARGUMENTS` / `${CLAUDE_SESSION_ID}` / `${CLAUDE_SKILL_DIR}` による文字列置換
- `` !`<cmd>` `` でプリプロセッサとしてシェルを走らせ、結果を本文に差し込む

Claude Code の skill 格納階層（上から優先）:

```
enterprise (managed) > personal (~/.claude/skills/) > project (.claude/skills/) > plugin (<plugin>/skills/)
```

プラグイン skill のみ `plugin-name:skill-name` の namespace を持つ。Live change detection（ディレクトリ監視）と monorepo 対応のネストディレクトリ自動検出もある。

Skill content のライフサイクルは重要で、**一度発動すると rendered 内容が会話に単一メッセージとして入り、以後再読み込みされない**。auto-compaction 時は各 skill の直近 invocation だけが冒頭 5,000 tokens 維持され、全 skill 合算 25,000 tokens の予算内で新しい順に残す。つまり「skill の本体は session 中ずっと context に居座る」前提で書く必要がある。

### yoi への示唆

- **procedural memory は SKILL.md で表現**する。`.claude/skills/` に人間手入れの skill を置き、エージェントが自動生成する手続きは別ディレクトリ（例: `memory/skills/`）で分離、混ざらないようにする。BF/KBL 分離の原則に一致。
- **`paths:` によるスコープ絞り**は、前回議論した「Pod の所属スコープ = ディレクトリ階層」と自然に噛む。`paths: ["crates/protocol/**"]` の skill は protocol スコープの pod でだけ発動、という運用が素直にできる。
- **progressive disclosure の 3 段構造**をメモリ本体にも適用する。知識ファイル先頭に 1 行 description を YAML で持たせ、`index.md` に description だけ集約、本体は wikilink で遅延ロード、という揃え方が可能。
- 独自フォーマットを作らない。Hermes / OpenClaw / Claude Code のエコシステム skill をそのまま引き込める。

一次ソース:
- https://agentskills.io/specification
- https://code.claude.com/docs/en/skills
- https://github.com/anthropics/skills
- https://developers.openai.com/codex/skills

---

## 6. プロンプト・スキルの継続的チューニング (empirical prompt tuning pattern)

メモリや skill の**中身を腐らせない**側の話。公開されている prompt tuning pattern として、agent-facing な指示を新規 subagent に実行させ、実行者の自己申告と指示側メトリクスを突き合わせて反復改善する方法がある。yoi のように skill を蓄積する設計では、**書いた直後に客観的に試す**仕組みが無いと品質が崩れていく。ここへの素直な当てはめ材料として記録。

### 基本思想

> Agent 向けテキスト指示（skill / slash command / task プロンプト / CLAUDE.md 節 / コード生成プロンプト）を、**バイアスを排した別の実行者**に動かしてもらい、**両面（実行者の自己申告 + 指示側メトリクス）**で評価して反復改善する。

書き手が自分で評価すると "こういう意図だったから" のバイアスで通ってしまう。だから毎反復ごとに **新規サブエージェント** に dispatch して、結果を構造化報告させる。

### 7 ステップ

1. description と body の整合性チェック（description 1024 字制約もここで効く）
2. baseline: 要件チェックリスト + シナリオ（典型 1 + edge 1-2）を固定
3. 新規 subagent で実行
4. 両面評価
5. 最小 patch で 1 箇所だけ直す
6. 新規 subagent で再実行
7. 収束判定

### 実行 AI に出させるレポート形式

- 成果物
- 要件達成（[critical] は最低ライン明記）
- **不明瞭だった点**
- **裁量で補完した箇所**
- 再試行回数

### 指示側メトリクス（機械抽出）

Claude Code の Task tool 戻り値から:
- `tool_uses` 数
- `duration_ms`
- 要件チェックリスト達成率（critical / non-critical 別）

### 収束判定

> 連続 2 回で新規不明瞭点ゼロ、精度の伸び ≤3pt、step 分散 ±10%、実行時間 ±15%

シナリオは hold-out を用意して過適合検出、評価基準の後付け修正は無効化扱い。

### クリティカルな制約

> 毎回**新規** AI を dispatch すること。同じセッション再利用は前回の指摘を学習してしまい、指標が腐る。

### yoi への示唆

- **skill や lessons を新規追加した直後に、同じ yoi ハーネス内の別 pod で実行して評価**する自動フロー（"skill doctor" 的な存在）を作れる。これは yoi が pod factory を持っている点と相性がいい。
- 失敗ログを書いた後、「同じ失敗が再現しないか」を新規 pod で試走する検証ステップが、構造的に**メモリ整備の一部**に組み込める。skill 化しない失敗ログでも有効。
- 評価指標を自前で定義しておくと、後で他人（or 未来の自分）が skill を更新した時に腐敗検知できる。
- 実体は skill 自身として配布されている例がある。yoi のメンテ用 skill セットのテンプレにも応用できる。

一次ソースは公開 sanitize branch では省略する。

---

## 7. パターン抽出

上記を一枚にすると、2026 年現在のメモリ実装はだいたい以下の次元の組み合わせに収まる。

| 次元 | 主な選択肢 |
|------|-----------|
| 分類軸 | 宣言的知識 / エピソード / 手続き(skill) / プロファイル の 2〜4 層 |
| 抽出タイミング | ターン内（write-back）/ セッション終了（rollup）/ 完全非同期（sleep-time agent, bg chronicle） |
| 圧縮方式 | 要約 eviction / recursive summarization / consolidation pass / 差分更新 wiki |
| 保存形式 | Markdown ファイル群 / SQLite (+FTS) / Vector store / Graph DB |
| 取り出し方式 | 常駐注入(summary fixed) / 決定論的 wikilink / FTS / ベクトル / HyDE / RRF 融合 |
| ユーザー編集性 | 不可視 / 読取のみ / 手編集前提 |
| スコープ | per-user / per-project / shared across agents |

yoi で意思決定すべきポイントはこの対応表：

- **Pod / Agent の「skill」概念を Hermes 風に明示すべきか**。現状の controller.rs には "sub-agent spawn" はあるが、skill を書き出して再利用する仕組みは無い。
- **Codex Chronicle 風の "consolidation モデルを別途設定" 構成**は、yoi の llm provider policy（Ollama / Codex OAuth / Anthropic）と相性が良い。軽量 extract と重い consolidation を別プロバイダに張れる。
- **LLM Wiki パターンを採用する場合**、既に `docs/` と `tickets/` が Markdown + git で運用されているので、`memory/` ディレクトリを足して Git で可観測にしておくのが自然。RAG やベクトル化より先に、wikilink / index.md / log.md で足りるか見極めるべき。
- **storage 層**: SQLite は既存 crate 構成にもフィット。Cloudflare Agent Memory / Codex の SQLite + 最終 Markdown の 2 段は移植しやすい。
- **prompt injection 対策**: Chronicle が注意書きしている通り、観測チャンネルを増やすと攻撃面が広がる。yoi では pod の sandbox 境界とメモリ生成を同じ境界で括る必要がある。

---

## 8. GC 機構の横断比較

`docs/plan/memory.md` §GC は「consolidation とは別経路で memory を再評価し、drop / merge / split / `replaced` chain 整理を行う」ことを決めた段階で、判断主体と処理種別の仕様をこれから詰める。本節は他プロジェクトの GC 設計を共通の 6 軸で並べて、yoi で採るべき型の材料とする。

### 8.1 比較表

対象の行は「その system が実装している GC 機構」単位で、同じプロジェクトが複数機構を持つ場合は複数行にした。

| # | プロジェクト / 機構 | 対象 | トリガー | 判断主体 | 処理種別 | 人間介入点 | 履歴保持 |
|---|---------------------|------|----------|----------|----------|-----------|---------|
| 1 | Codex consolidation | `MEMORY.md` block / `memory_summary.md` / `skills/*` | session idle + age | **LLM agentic**（sub-agent） | drop / merge / split / rewrite、removed thread id に紐づく block を **部分削除**（block 丸ごとは消さない、thread-local 記述のみ除去） | 無し（`/memories` で thread 単位 opt-out のみ） | git 任意、memory_root は単なる Markdown |
| 2 | Codex extension resource retention | `memories_extensions/<name>/resources/*.md` | consolidation 実行直前に cron 相当 | **決定論** | **物理削除** （filename timestamp cutoff） | 無し | 完全消去、consolidation prompt に removed list を通知 |
| 3 | Codex stage1 pruning | `stage1_outputs` SQLite row | consolidation 後 / 容量超過 | **決定論** | `selected_for_consolidation = 0` の古い row を cutoff + `LIMIT` で DELETE | 無し | SQL 完全削除 |
| 4 | Hermes `memory` tool | `MEMORY.md` / `USER.md` のエントリ | **char limit (2,200 / 1,375) 超過時の add 拒否** | **LLM agentic**（エラー受けて自分で `replace` / `remove` を呼ぶ） | drop / rewrite | 無し（all-agent） | ディスク消去（file lock で tx 化） |
| 5 | Hermes background review | entry / skill | turn / iter カウンタ（10 デフォルト） | **LLM agentic**（fork agent、max_iterations = 8） | add / update / delete をレビュー判断、`Nothing to save.` で no-op | 無し | same as 4 |
| 6 | OpenClaw `applyShortTermPromotions` | promotion candidate → `MEMORY.md` | Deep dreaming phase | **決定論**（6 重み合算 + 3 ゲート、LLM ゼロ） | **append のみ**（`<!-- openclaw-memory-promotion:<hash> -->` marker、既存 block は触らない） | 無し | 追記のみ、削除系統は別 |
| 7 | OpenClaw temporal decay | `memory/YYYY-MM-DD.md` | search 呼び出し毎 | **決定論** | **物理削除せず search score 減衰**（half-life 30d） | 無し | ファイル残置、検索順位だけ沈む |
| 8 | OpenClaw dreaming-repair | `DREAMS.md` / session-corpus / session-ingestion | 手動 + audit CLI | **決定論** | **archive 退避**（`.openclaw-repair/dreaming/<timestamp>/` に rename）、完全削除しない | 退避後は手動判断 | archive ディレクトリに rename で保持 |
| 9 | memory-wiki `/wiki-lint`（OpenClaw extension、SamurAIGPT/llm-wiki-agent と Karpathy llm-wiki の production 実装） | wiki page / claim | `/wiki-lint` 手動 or cron 想定 | **決定論**（issue 検出ロジック） + その後の **human/LLM 任意判断** | **削除ゼロ**、`reports/lint.md` に issue を書き出すのみ。修正は別コマンド / 人間が行う | 全出力点が人間承認 | 原ファイル無変更、report は上書き |
| 10 | Cloudflare Agent Memory supersession | Fact / Instruction row（Durable Object SQLite） | 新 memory ingest 時 | **決定論**（normalized topic key が一致した瞬間） | **supersede**（旧 row を forward pointer で版チェーン化、物理削除せず）、vector は非同期 delete + new upsert | 無し（全自動） | SQLite に旧版残置、version chain で参照可能 |
| 11 | Letta sleep-time agent | in-context memory block | 非同期 idle process | **LLM agentic** | `core_memory_replace` / `core_memory_append` / archival 移動で block 書き換え | 無し | 書き換え後の block が保存、旧 block は失われる |
| 12 | Letta recursive summarization | message buffer | **context window fill の決定論閾値** | 発火は決定論 / 要約内容は LLM | evict + 再帰要約（古いほど重み減） | 無し | 要約に畳み込まれる（原メッセージは消失） |
| 13 | LinkedIn CMA（公開情報レベル） | Episodic / Semantic / Procedural 各層 | 未公開 | 未公開 | **summarization による compaction**（階層別の drop 仕様は非公開、"cache invalidation is one of the hardest problems" と明示） | **高 stake 用途は human validation** を挟む | 非公開 |

### 8.2 軸別の知見

**対象（何が GC されるか）の粒度差**:

- 最小粒度の Hermes（entry）と Cloudflare（row）は、LLM がエントリ単位で `remove` / `replace` する設計に寄る。
- 中粒度の Codex（`MEMORY.md` block）は、block 内を thread id で部分削除しながら必要なら split / rewrite する agentic 寄り。
- 最大粒度の OpenClaw / memory-wiki は **ファイル単位でのみ削除 / 退避** し、file 中身は LLM / 人間に委ねる。`docs/plan/memory.md` の「1 件 1 ファイル」方針とは **OpenClaw の粒度が最も近い**。

**トリガー（いつ GC するか）の 4 パターン**:

1. **容量超過 hard reject**（Hermes）: 追加要求を失敗で返して LLM に自律対処を強制。**決定論的 trigger + agentic 処理**で、設計最小コスト。
2. **session idle + age**（Codex consolidation）: 人間の活動終了を待って非同期、最もユーザー体感を壊しにくい。
3. **cron / scheduled sweep**（OpenClaw dreaming default `0 3 * * *`, Codex extension retention）: 定期的・予測可能。人間 review との組み合わせがしやすい。
4. **ingest 時の即時**（Cloudflare supersession）: 書き込みの tx 内で完結、後続 GC 走査が要らない。topic key 設計が前提。

yoi の plan は (2) consolidation で rewrite 許可を置きつつ、GC は (3) 方向で別経路という構造。これは Codex / OpenClaw の両方と整合する。

**判断主体の 3 系統**:

- **決定論のみ**: Codex retention / stage1 pruning / OpenClaw temporal decay / Cloudflare supersession / lint 検出。条件がはっきりしているもの（age / key 一致 / 構造的 issue）は決定論が強い。
- **決定論 scoring → 閾値 gate → 機械適用**: OpenClaw Deep promotion。LLM の揺れを除き、コストも LLM コールゼロ。ただし対象が append 側のみで、削除には使われていない。
- **LLM agentic**: Codex consolidation / Hermes review / Letta sleep-time。判断の柔軟性（block 内部分削除、context 依存の merge）を LLM に委ねる。

`docs/plan/memory.md` は consolidation が LLM agentic、GC も暫定的に **LLM agentic + Linter Warn 併用**としている。完全に一致する事例は **Codex consolidation の consolidation prompt**（836 行）で、「removed thread id を `MEMORY.md` から部分削除し、blockに他の thread が残っている場合は split / rewrite して保持」という手続きを自然言語で指示している。**yoi は Linter 側に警告カテゴリ（類似 slug / `replaced` 滞留 / sources 過多 / stale）を先に定義し、GC 実行の agent プロンプトはそれを入力にする**構造が素直。

**処理種別の選択肢**:

- `drop / merge / split / rewrite` の組み合わせは Codex consolidation が最も自由度高く、Hermes もそれに近い（entry 粒度）。
- `replaced` chain の整理は **Cloudflare だけが自動で版チェーン維持**、他は LLM 任せ。yoi は decision record に `replaced_by` を入れているので、Cloudflare 方式の forward pointer 概念を **人間可読な `replaced_by:` frontmatter** で既に踏襲している。GC 時に chain をどこまで短く畳むか（長大な `a → b → c → d` を `a → d` に圧縮するか）は未決定論点で、Cloudflare は圧縮せず chain を保持する設計。
- **`split` は Codex だけが明示**。block 内に複数 thread id が混ざった場合に thread id 単位で分ける。yoi の「1 件 1 ファイル」方針では split = ファイル分割となり、主題の粒度判断は GC agent に委ねる必要がある。

**人間介入点の 3 段**:

- 完全無介入: Codex / Cloudflare / Letta / Hermes
- audit-first（issue を surface し、人間が決断）: memory-wiki lint / OpenClaw dreaming-repair
- high-stake 限定 gate: LinkedIn CMA

yoi の plan は「人間 offer 承認を併用」なので **audit-first に寄る**のが自然。lint 相当の Warn を Linter で出し、LLM consolidation / GC がそれを消費する前に人間が承認 / 拒否できる UI を提供する構造。memory-wiki lint は `reports/lint.md` というシンプルな Markdown 出力なので、そのまま `memory/reports/gc-lint.md` 相当を tick off する実装が参考になる。

**履歴保持の 3 モデル**:

1. **物理削除**: Codex retention / stage1 / Hermes / Letta（要約で畳む分は実質消失）
2. **archive 退避（rename）**: OpenClaw dreaming-repair
3. **forward pointer / tombstone**: Cloudflare supersession

yoi は **git 管理下に memory を置く**前提なので、物理削除を選んでも git log で復元できるのが強み。`replaced_by:` frontmatter が forward pointer の役割を果たしているので、Cloudflare 型と git を足した「**現物は物理削除、frontmatter pointer で chain を参照、git で history**」が最も設計コストに合う。archive 退避は git を前提にすると冗長。

### 8.3 yoi の GC 仕様を詰めるときの示唆

1. **GC trigger は 2 系統に割る**。(a) 決定論: Linter Warn 群 + age / count / size 閾値の sweep、(b) LLM 判定: consolidation とは別 prompt で Linter の issue リストを入力に渡す。両方が `memory/reports/gc-*.md` 相当を書き、それを次回の GC run が読む、というフィードバックループが OpenClaw lint / Codex consolidation input selection の両方と整合する。
2. **Linter に「GC 候補検出」カテゴリを足す**。memory-wiki の lint issue code が参考になる: `stale-page`（90d 超）/ `stale-claim` / `low-confidence` / `orphan` / `duplicate-id` / `broken-wikilink` / `contradiction-present` / `open-question`。yoi 固有の追加候補: `similar-slug`（類似 slug 乱立、既に plan に記載）/ `replaced-chain-long`（`replaced_by` が 3 段以上）/ `sources-overflow`（1 record の sources が閾値超）/ `knowledge-invoke-frequency-low`（`user_invoke` が一定期間ゼロ）。
3. **処理は rewrite 優先、削除は `status: replaced` 経由**（既に plan 方針と一致）。forward pointer は Cloudflare 流、ただし chain 圧縮ルール（例: 「chain が n 段超えたら中間を drop、端のみ残す」）を決めるかは別論点。Cloudflare は圧縮しない、yoi は git があるので圧縮してよい。
4. **char limit は採用しない方が筋が良い**。Hermes の hard limit + LLM self-rewrite は設計最小だが、yoi は 1 record 1 file なのでファイル内 size 制約は薄く、file 数による grep コストの方が支配的になる。file 数閾値 → GC trigger の方が yoi の形に合う。
5. **決定論 scoring を後から差し込む余地を残す**。OpenClaw Deep pass のような「頻度 / 関連度 / 多様性 / 時間減衰 / 整合性 / 概念」の 6 重み + 閾値は、agent LLM の出力が運用で評価可能になった段階で部分的に差し替える upgrade path として最適。初期は consolidation LLM + Linter Warn で十分。
6. **削除は git commit 単位で可逆**という前提を明示する。プロジェクトメモリは git 管理下なので、GC が誤って drop してもユーザーは revert できる。これは Codex が持っていない利点で、GC agent の判断を多少攻めても安全マージンがある。

一次ソース:
- Codex consolidation: `github.com/openai/codex/codex-rs/core/src/memories/consolidation implementation`, `core/templates/memories/consolidation.md`
- Codex retention / stage1 pruning: `github.com/openai/codex/codex-rs/core/src/memories/extensions.rs:11-139`, `codex-rs/state/src/runtime/memories.rs:290-331, 333-464`
- Hermes char limit reject: `github.com/NousResearch/hermes-agent/tools/memory_tool.py:211-266`
- Hermes review spawn: `github.com/NousResearch/hermes-agent/run_agent.py:2727-2830`
- OpenClaw promotion apply: `github.com/openclaw/openclaw/extensions/memory-core/src/short-term-promotion.ts:1518-1652`
- OpenClaw temporal decay: `github.com/openclaw/openclaw/extensions/memory-core/src/memory/temporal-decay.ts`
- OpenClaw dreaming-repair: `github.com/openclaw/openclaw/extensions/memory-core/src/dreaming-repair.ts:70-165`
- memory-wiki lint: `github.com/openclaw/openclaw/extensions/memory-wiki/src/lint.ts`, `claim-health.ts:6-7`（`WIKI_AGING_DAYS = 30`, `WIKI_STALE_DAYS = 90`）
- Cloudflare supersession: https://blog.cloudflare.com/introducing-agent-memory/
- Letta sleep-time: https://docs.letta.com/guides/agents/memory/, https://www.letta.com/blog/sleep-time-compute, arxiv 2504.13171
- Letta recursive summary: https://www.letta.com/blog/agent-memory
- Karpathy llm-wiki lint: https://gist.github.com/karpathy/442a6bf555914893e9891c11519de94f
- SamurAIGPT/llm-wiki-agent `/wiki-lint`: https://github.com/SamurAIGPT/llm-wiki-agent
- LinkedIn CMA: https://www.infoq.com/news/2026/04/linkedin-cognitive-memory-agent/

---

## 9. 未調査 / 次に掘るべき項目

- Letta `memory block` の Rust 実装例・永続化形式。
- `agentskills.io` の CRDT 的バージョニング方針（標準は version metadata を任意 key にしている。実運用でどう衝突解決するかは未整備）。
- Hermes の Honcho 連携の実体（外部サービス API 越しの dialectic user modeling、repo には prompt と API 呼び出しのみ）。
- LinkedIn CMA の層別 GC 仕様（公開情報では compaction-by-summarization までしか開示されていない）。
- Cloudflare supersession chain の圧縮ポリシー（chain 長の上限、vector GC と row GC の tx 境界）。
