# Agent Skills を Workflow として ingest

## 背景

[agentskills.io](https://agentskills.io/) の "Agent Skills" は Anthropic 発のオープン標準で、Claude Code / Cursor / OpenCode / Gemini CLI / Goose / OpenHands など主要な coding agent 群が採用している。ユーザーが既に書いた skill を insomnia に持ち込めるようにすることで、他ツールと能力資産を共有できる。

insomnia 側の一級概念は **Workflow** (`/<slug>`、`tickets/workflow.md`)。SKILL.md 形式は Workflow と意味的にほぼ同型（procedural な指示本体 + メタデータ + 付随リソース）であり、別経路として並列管理するより **Workflow にマップして ingest する**方が呼び出し UX (`/<slug>`) と内部経路を一本化できる。

`docs/plan/memory.md` が「`SKILL.md` 形式は採用しない」と書いているのは Knowledge (`#<slug>`) の表現形式としての話で、本チケットの「Workflow への ingest 経路」とは矛盾しない。

### Skill の骨格 (仕様要旨)

```
skill-name/
├── SKILL.md          # 必須: YAML frontmatter + Markdown 本体
├── scripts/          # 任意: 実行コード
├── references/       # 任意: 詳細ドキュメント
└── assets/           # 任意: テンプレート等の静的資産
```

SKILL.md frontmatter:

| フィールド | 必須 | 制約 |
|---|---|---|
| `name` | ○ | 1-64 chars, `[a-z0-9-]+`, **ディレクトリ名と一致** |
| `description` | ○ | 1-1024 chars, 非空 |
| `license` | | 自由文 |
| `compatibility` | | 1-500 chars |
| `metadata` | | 任意の key-value map |
| `allowed-tools` | | experimental |

## 前提チケット

- `tickets/workflow.md` — Workflow loader / `/<slug>` resolve / `model_invokation` 注入の本実装。本チケットはその ingest 経路を増やすだけで、Workflow 側の意味論には手を入れない

## 方針

### ロードソース

skill は **manifest の `[skills] directories` で明示指定したパスのみ** ingest する。`$config_dir/skills/` の自動ロードや builtin skills は持たない — manifest に書かれていないディレクトリは決して読まれない。

```toml
[skills]
directories = ["~/skills", ".claude/skills", ".cursor/skills"]
```

各パスは manifest の base directory に対して resolve する（既存 `scope.allow.target` 等と同方針）。絶対パスでも相対パスでもよく:

- user manifest layer (`$config_dir/manifest.toml`) に書けば全 workspace 共通で使う skill 集を持てる
- project manifest layer に書けば workspace 固有の `.claude/skills/` `.cursor/skills/` をそのまま流用できる

ビルトイン `$insomnia/skills/` は不要になるまで作らない（前ガイドラインのまま）。

### SKILL → Workflow マッピング

| SKILL.md frontmatter | Workflow frontmatter | 備考 |
|---|---|---|
| `name` | （ファイル名 = slug として扱う） | `name` がディレクトリ名と一致することは仕様上の不変。slug としてはディレクトリ名を使用 |
| `description` | `description` | そのまま |
| —             | `model_invokation` | **`true` 固定**。agentskills の progressive disclosure（メタデータ常時注入）と整合 |
| —             | `user_invocable` | **`true` 固定** |
| —             | `requires` | **空配列**。SKILL 側に概念がない |
| `license` / `compatibility` / `metadata` | — | 保持はするが Workflow 実行には影響しない |
| `allowed-tools` | — | `permission-extension-point.md` が Permission 層を整備するまで `tracing::warn!` して無視 |

Workflow 本文には SKILL.md 本体（frontmatter 直下の Markdown）をそのまま使う。

### scripts / references / assets

skill ディレクトリ全体（`SKILL.md` 本体だけでなく `scripts/` `references/` `assets/`）を Pod の Scope に `permission = read` で自動 union する。`scripts/` の実行は Bash ツール + Permission 層（`permission-extension-point.md`）が整うまで Read 経由の閲覧に留める。

### 衝突解決

同一 slug が複数ソースから来た場合の優先順位:

1. `<workspace>/.insomnia/memory/workflow/<slug>.md`（内製 Workflow）
2. manifest `[skills] directories` の各エントリ（**列挙順**で解決。先に書かれたディレクトリが上位）

衝突時は上位を採用し、shadow した側について `Event::Notification` を発行する。「明示的に書かれた内製 Workflow が外部資産より強い」順に並べる。

### 検証

- frontmatter は SKILL 仕様通り検証。`name` ↔ ディレクトリ名一致、`description` 長さ、`name` の文字種制約を hard error
- 検証エラーは個別 skill 単位で skip + `tracing::warn!`。一個の壊れた SKILL が Pod 起動を止めない（内製 Workflow とは違う扱い: 外部資産は緩く受ける）
- 未知フィールドは無視

### 範囲外

- `allowed-tools` の実効化 → `permission-extension-point.md` 待ち
- skill 専用の実行機構（`scripts/` の自動実行）— Bash ツールと Permission 層で自然に扱う
- skill の自動生成（Hermes 風 Skill Library）— memory 系チケットで別途、本チケットの ingest 経路を再利用する側
- ビルトイン skill (`$insomnia/skills/`) — 必要になるまで追加しない
- skill 間依存の解決 — 独立単位、相互参照は本体の Markdown リンクで

## 完了条件

- manifest の `[skills] directories = [...]` で指定したパス配下の SKILL.md が Workflow として登録され、`/<name>` で呼び出せる
- `[skills]` セクションが無い manifest では skill 由来の Workflow は 0 件 (`$config_dir/skills/` 等の暗黙ロードはしない)
- 内製 Workflow と同 slug の skill は内製優先で shadow され、Notification が発行される
- skill ディレクトリ（SKILL.md 本体・`scripts/`・`references/`・`assets/`）が scope readable に含まれ、agent が Read ツールでアクセスできる
- frontmatter 違反の skill は warn でスキップされ、他の skill / Pod 起動は影響を受けない
- 単体テストで frontmatter 検証、Workflow へのマッピング、衝突解決（内製 > skill）、manifest 未指定時の skill 0 件が verify される

## 実装順序

1. SKILL.md パーサと frontmatter 検証を実装。Workflow frontmatter への変換器を含めてテスト完結
2. manifest に `[skills] directories: Vec<PathBuf>` を追加し、ingest 経路を実装
3. 衝突解決と Notification 発行を乗せる
4. skill ディレクトリの Scope union（read 自動 allow）

各ステップ終了時点でビルド通過・既存テスト合格を維持する。

## 参照

- 仕様本体: https://agentskills.io/specification
- 採用状況: https://agentskills.io/home
- Anthropic 公式サンプル: https://github.com/anthropics/skills
- 検証 CLI: https://github.com/agentskills/agentskills/tree/main/skills-ref
- 前提: `tickets/workflow.md`
- 関連: `tickets/permission-extension-point.md`（`allowed-tools` 実効化の受け皿）、`docs/plan/workflow.md`、`docs/plan/memory.md`、`docs/ref/memory-systems.md` §Skill Library

## Review

- 状態: Approve (Round 2)
- レビュー詳細: [./agent-skills.review.md](./agent-skills.review.md)
- 経緯:
  - 2026-05-04: 初回レビュー Approve (旧仕様: 2 ソース構成)
  - 2026-05-05: `$config_dir/skills/` の自動 ingest を廃止、skill ロードソースを manifest `[skills] directories` のみに統合する設計変更。実装も `WorkflowSource::WorkspaceSkill` / `UserSkill` 2 バリアントから単一の `Skill { dir }` に簡略化済み
  - 2026-05-04: Round 2 レビュー Approve。新設計の完了条件をすべて満たし、Round 1 で挙げた user_skills_dir 関連の懸念は設計変更で構造的に解消。non-blocking として manifest crate の stale doc コメント (`crates/manifest/src/lib.rs:47-62`) と受け皿チケット (`tickets/permission-extension-point.md:61`) の旧バリアント名参照が残るのみ
