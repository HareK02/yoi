# Agent Skills サポート

## 背景

[agentskills.io](https://agentskills.io/) で公開されているオープン標準 "Agent Skills" は、エージェントに手続き的知識・ドメイン能力をフォルダ単位で供給する形式。Anthropic 発で Claude Code / Cursor / OpenCode / Gemini CLI / Goose / OpenHands など主要な coding agent 群が採用しつつあり、ユーザーが既に書いた skill を insomnia に持ち込める／他ツールと共有できる価値が明確になっている。

`docs/ref/memory-systems.md` でも Hermes Agent の "Skill Library" が agentskills.io 互換として言及済みで、将来のセルフ生成メモリ経路 (procedural memory) の受け皿としてもこのフォーマットに合わせておく利がある。

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
| `name` | ○ | 1-64 chars, `[a-z0-9-]+`, 先頭末尾 `-` 不可, `--` 連続不可, **ディレクトリ名と一致** |
| `description` | ○ | 1-1024 chars, 非空 |
| `license` | | 自由文 |
| `compatibility` | | 1-500 chars |
| `metadata` | | 任意の key-value map |
| `allowed-tools` | | experimental (空白区切りのツールリスト) |

仕様の肝は **progressive disclosure**:

1. メタデータ (`name` + `description`) — セッション開始時に**全 skill 分**をプロンプトに載せる (~100 tokens/skill)
2. 指示本体 (SKILL.md body) — skill が必要になった時点で agent 側が読みに行く
3. リソース (`scripts/` / `references/` / `assets/`) — さらに on-demand

## 既存機構との関係

insomnia のシステムプロンプトは現状、`instruction` (テンプレ本体) + scope summary + AGENTS.md (trailing section) の 3 要素構成。Skill はこの並びに**任意個のオプショナル能力資産**として追加する位置取り。instruction / AGENTS.md を置き換えない。

配置は既存の prefix addressing と同じ軸:

- `$XDG_CONFIG_HOME/insomnia/skills/<name>/SKILL.md`
- `<project>/.insomnia/skills/<name>/SKILL.md`
- `$insomnia/skills/` (ビルトイン) は不要になるまで作らない

## 方針

### MVP スコープ

1. **Skill ディスカバリと検証**
   - 上記 2 経路のサブディレクトリを走査して `SKILL.md` を読む
   - frontmatter を仕様通り検証。name 不一致・必須欠落・制約違反は hard error (Pod 起動失敗)
   - 未知フィールドは `tracing::warn!` して無視 (pod-factory と同方針)
   - 重複 name は workspace 層優先で user 層を上書き + ユーザー向け notification 発行

2. **システムプロンプトへの注入 (progressive disclosure 準拠)**
   - `SystemPromptContext` に `skills: Vec<SkillSummary>` を追加 (`name`, `description`, 絶対パス)
   - ビルトイン `$insomnia/default.md` に skill 列挙ブロックを追加 (空なら一切出力しない)
   - ユーザーテンプレートからも `{{ skills }}` で参照可能
   - **本体は注入しない**。description の末尾に絶対パスを併記し、agent が Read ツールで取れるようにする

3. **本体への agent アクセス**
   - skill ディレクトリを `Scope` に `permission = read` で自動 allow
   - `scripts/` の実行は将来 Bash ツール + Permission 層で成立するまで Read 経由での閲覧に留める

### 範囲外

- `allowed-tools` フィールドの実効化 — `permission-extension-point.md` が Permission 層を整備するまで受信して無視
- skill 専用の実行機構 — `scripts/` は Bash / Read で自然に扱う
- skill の自動生成 (Hermes 風 Skill Library) — memory 系チケットで別途。本チケットで作る ingest 経路を再利用する側
- ビルトイン skill (`$insomnia/skills/`) — 必要になるまで追加しない
- skill 間依存の解決 — 独立単位、相互参照は本体の Markdown リンクで

## 完了条件

- `$user/skills/` と `$workspace/skills/` の両方から skill をロードし、frontmatter 違反は Pod 起動エラーになる
- ビルトインの `$insomnia/default.md` をレンダした結果、存在する skill の name + description + 絶対パスがシステムプロンプトに列挙される
- skill が 0 件のとき、既存の出力 (AGENTS.md / scope summary) が全く変わらない
- skill ディレクトリが scope の readable に含まれ、agent が Read ツールで `SKILL.md` 本体・`scripts/`・`references/` にアクセスできる
- 単体テストで frontmatter 検証の正常 / 異常系、progressive disclosure レンダリング、user/workspace 重複解決が verify される

## 実装順序

1. `manifest` または新設 `skill` クレートに `Skill` 構造体と `SkillDirectoryLoader` を置く。SKILL.md パースと検証のみでテスト完結
2. `pod::SystemPromptContext` に `skills` を生やし、`ensure_system_prompt_materialized` で loader を呼ぶ
3. `resources/prompts/default.md` に skills ブロックを追加、`Pod::from_manifest` で skill ディレクトリを scope readable に union
4. 重複 name 解決と notification 発行を乗せる

各ステップ終了時点でビルド通過・既存テスト合格を維持する。skill 0 件時の後方互換は Phase 2 で確認する。

## 参照

- 仕様本体: https://agentskills.io/specification
- 採用状況: https://agentskills.io/home
- Anthropic 公式サンプル: https://github.com/anthropics/skills
- 検証 CLI: https://github.com/agentskills/agentskills/tree/main/skills-ref (将来 `pod skills validate` 相当を作るなら参考)
- 類似機構: `crates/pod/src/prompt_loader.rs` (prefix addressing), `crates/pod/src/agents_md.rs` (cwd-relative ingest), `crates/pod/src/system_prompt.rs` (render pipeline)
- 後続接続先: `docs/ref/memory-systems.md` の Skill Library (自動生成 skill の保存先)
