# パーミッション: パターンベースのツール実行制御

## 背景

現状の `Scope` はディレクトリ単位の書き込み制約で、静的な境界線。
実際のエージェント運用では、ツール単位・引数パターン単位の動的な権限制御が必要になる。

OpenCode はパターンベースのルール（tool × pattern → allow/deny/ask）を持ち、
`*.env` への書き込み拒否や `rm -rf` の実行拒否を宣言的に設定できる。

## 方針

Permission の評価点は `PreToolCall` Hook とする。マニフェストにルールを宣言し、
insomnia 層が built-in の `PreToolCall` Hook として登録してツール呼び出し時に評価する。

`deny` はターン全体の Cancel/Abort ではない。対象 tool call を実行せず、
permission denied を表す `is_error = true` の synthetic tool result を履歴に追加してターンを継続する。
これにより provider が要求する `tool_use` / `tool_result` の対応を壊さず、LLM は拒否結果を見て別手段の検討やユーザーへの説明に進める。

`ask` は `deny` の代替ではなく、ユーザー承認待ちを明示する action として扱う。承認されれば元の tool call を実行し、拒否されれば `deny` と同じ synthetic tool result に落とす。

```toml
[permissions]
default_action = "allow" # allow | deny | ask

[[permissions.rule]]
tool = "bash"
pattern = "rm *"
action = "deny"

[[permissions.rule]]
tool = "file_write"
pattern = "*.env"
action = "deny"
```

allowlist 型にしたい場合:

```toml
[permissions]
default_action = "deny"

[[permissions.rule]]
tool = "read"
pattern = "*"
action = "allow"

[[permissions.rule]]
tool = "grep"
pattern = "*"
action = "allow"
```

確認待ちを基本にしたい場合:

```toml
[permissions]
default_action = "ask"

[[permissions.rule]]
tool = "bash"
pattern = "rm *"
action = "deny"
```

評価順序:
1. `[permissions]` が無い場合、Permission 層は無効。従来通り実行する
2. `[permissions]` がある場合、`default_action` は必須
3. `[[permissions.rule]]` は宣言順に評価し、最初に `tool` と `pattern` が一致した rule の `action` を採用する
4. 一致する rule が無ければ `permissions.default_action` を採用する

## 設計ポイント

- 設計原則3: 新しい trait は作らない。`PreToolCall` Hook として実装
- 設計原則2: マニフェストに宣言した以上、insomnia 層が解決する
- Permission Hook は Pod が自動登録する built-in Hook とし、ユーザー追加 Hook より先に評価する
- `deny` は `PreToolAction::Abort` / 既存 `Skip` では表現しない。tool call 単位の拒否結果を履歴へ返すため、Worker 側に synthetic tool result を返せる action が必要
- `ask` アクションは Pod Protocol の拡張が必要（Event に `PermissionRequest`、Method に `PermissionReply` を追加）
- `ask` を処理できない実行環境では暗黙に待機しない。設定時に validation error とするか、fail closed で `deny` 相当の synthetic tool result に落とす
- `Scope` との関係: Scope は書き込みの物理的境界、Permission はツール実行のポリシー。補完関係
- ルール評価はパターンマッチのみ。コンテキスト依存の判断はしない（シンプルに保つ）

## 段階的実装

1. **拡張ポイントの記録**（今）: docs/pod.md の拡張ポイント表に追加
2. **deny/allow の実装**（ツール実装時）: `default_action` と rule 評価を manifest に追加し、built-in `PreToolCall` Hook でパターン評価
3. **拒否 tool result の実装**: `deny` が turn Abort ではなく synthetic error tool result として履歴に入るよう Worker の pre-tool action を拡張
4. **ask の実装**（Protocol 拡張時）: Method/Event に Permission 関連メッセージを追加し、承認後に元 tool call を実行、拒否時は synthetic error tool result を返す

## 受け皿になる外部仕様

### Agent Skills `allowed-tools`

`tickets/agent-skills.md` で ingest した SKILL.md の frontmatter には agent-skills 仕様の experimental field である `allowed-tools` (例: `["Read", "Bash"]`) が含まれる場合がある。`crates/memory/src/skill.rs::parse_skill_md` 時点では `tracing::warn!` で受け流しているだけで、実効化していない。

本チケットの Permission 層が固まった時点で、Skill 由来 Workflow を実行中のみ当該 skill の `allowed-tools` リストに含まれるツールしか走れない形で反映する。スコープは「Workflow 実行中」相当 (Workflow の system message が context に乗っているターン) に限定する想定。skill 単位で local な permission 集合を持つので、グローバルな `[[permissions.rule]]` ルールとは独立に評価する。

実装上の足がかり:
- `WorkflowRecord` の出所は `WorkflowSource::Skill { dir }` で識別済み (`crates/memory/src/workflow.rs`)。`dir` は manifest `[skills] directories` に書かれた skill ルートそのもの
- 受け皿実装時に `SkillFrontmatter::allowed_tools` の保持先を `WorkflowRecord` に伸ばすか、別の SkillRecord registry を持つかは本チケット内で決める
- 現状の `tracing::warn!` は受け皿実装と同時に消す

## 依存チケット

- ~~[remove-hook-module.md](remove-hook-module.md)~~ — 完了。PreToolCall は Pod 層の `hook::Hook<PreToolCall>` として利用可能
