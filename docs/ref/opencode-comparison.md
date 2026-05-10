# Insomnia × OpenCode 比較レポート

## 概要

Insomnia（Rust製エージェントプラットフォーム、基礎実装段階）と OpenCode（TypeScript/Bun製AIコーディングアシスタント、本番稼働レベル）の設計を比較し、Insomniaの基礎設計に取り込めるパターンを特定する。

---

## 1. アーキテクチャ概観

### Insomnia（現状）

```
insomnia (stub)
  └─ insomnia-core          Pod / Controller / Protocol / SocketServer
       └─ llm-worker-persistence   Session永続化（JSONL + Blob）
            └─ llm-worker          Worker / Tool / Hook / Subscriber
                 └─ llm-worker-macros  #[tool] / #[tool_registry]
```

- 実行単位は **Pod**（独立したエージェント）
- Unix Domain Socket + JSONL で外部通信
- 状態遷移: Idle → Running → Paused/Idle
- 永続化: append-only JSONL ログ

### OpenCode

```
packages/opencode (server)    Session / Provider / Tool / Permission / Agent / Bus
packages/app (TUI)            SolidJS + OpenTUI（Terminal UI）
packages/sdk (client)         Hono OpenAPIから自動生成
packages/desktop (Tauri)      Web UIラッパー
```

- 実行単位は **Session**（会話）。Session内で **Agent** が切り替わる
- HTTP/SSE/WebSocket で外部通信
- SQLite + Drizzle ORM で永続化
- Effect.ts による依存注入・リソース管理

---

## 2. 設計判断の比較

| 観点 | Insomnia | OpenCode | 評価 |
|------|----------|----------|------|
| **DI** | ジェネリクス `<C: LlmClient, St: Store>` | Effect Service + Layer | Insomnia: コンパイル時保証。OpenCode: 実行時合成の柔軟性。方向性は正しい |
| **状態管理** | `RwLock<PodStatus>` + ファイル書き出し | SQLite + Event Bus + SSE | Insomnia: 軽量で正しい。DBは将来の選択肢 |
| **プロトコル** | 自前 JSONL (Method/Event) | Hono HTTP API + SSE | Insomnia: Unix Socketに最適化。目的が違う |
| **ツール** | `Tool` trait + マクロ生成 | Zod schema + execute関数 | 同等のアプローチ。マクロの方が型安全 |
| **フック** | `Hook<K: HookEventKind>` trait 10種 | Plugin hooks (before/after) | Insomnia: 型安全で粒度が細かい。OpenCode: 動的で拡張しやすい |
| **永続化** | JSONL append-only + Blob | SQLite + Drizzle ORM | 方向性が異なる。両方とも正当な選択 |
| **プロバイダ** | 4種（Anthropic/OpenAI/Gemini/Ollama） | 20種+（ai-sdk経由） | 数は後から追加できる。抽象は同レベル |

---

## 3. OpenCodeから取り込むべき設計パターン

### 3.1 パーミッションシステム（重要度: 高）

**OpenCodeの設計:**
- ツール実行前に **パターンベース** の権限チェック（`*.env` → deny、`src/**` → allow）
- 3段階: `deny` → `allow` → `ask`（ユーザーに確認）
- 「always」応答でパターンを永続的に許可

**Insomniaへの示唆:**

Insomniaには `Scope`（書き込みディレクトリ制約）があるが、これは静的な境界。
ツール単位の動的パーミッションが欠落している。

```
提案: PreToolCall Hook でパーミッション評価を行う
```

- **設計原則3（再発明しない）** に沿って、新しいtrait は作らない
- `PreToolCall` Hook として実装し、Podマニフェストにルールを宣言
- ルール定義は Scope の拡張ではなく、独立した概念として追加

```toml
# マニフェスト拡張案
[[permission]]
tool = "bash"
pattern = "rm *"
action = "deny"

[[permission]]
tool = "file_write"
pattern = "*.env"
action = "deny"
```

**今すぐ実装すべきか:** まだ。ツールが実装されてから。
ただし、**Hook で差し込める設計になっている** ことは確認済み。
拡張ポイントとして docs/pod.md の表に追加する価値あり。

---

### 3.2 ツール出力のトランケーション（重要度: 高）

**OpenCodeの設計:**
- 50KB / 2000行を超える出力を切り捨て
- 切り捨て分はファイルに保存（7日間保持）
- LLMには「出力が大きすぎた。`grep` や `read` で絞り込め」とヒント

**Insomniaの現状:**
- `llm-worker` に Tool Output の Inline/Stored 閾値（800 bytes）がある
- Stored 出力は Blob Storage に退避し、要約を自動生成

**比較:**
Insomnia の方が洗練されている（要約生成まで組み込み済み）。
ただし OpenCode の「ヒント付きトランケーション」は追加の視点として有用。

**取り込み案:**
- Stored 出力時に「元データの場所と推奨アクション」を要約に含める規約
- これは llm-worker のツール出力設計に自然に統合できる
- 現状の `tool-output-design.md` の Auto-Summarization に、推奨アクションのヒント生成を追加する余地がある

---

### 3.3 コンテキスト圧縮（Compaction）（重要度: 高）

**OpenCodeの設計:**
1. **Prune**: 古いツール出力を除去（直近40Kトークンは保護）
2. **Compact**: オーバーフロー時に専用エージェントで要約を生成
   - 構造化要約: Goal / Instructions / Discoveries / Accomplished / Files
3. **Replay**: 圧縮後に前回のユーザーメッセージを再送して作業継続

**Insomniaの現状:**
- Worker は history をそのまま保持
- コンテキスト管理の仕組みは未実装

**これは重要な欠落。** 長時間実行エージェントである Insomnia にとって、コンテキスト管理はコア機能。

**取り込み案:**

```
extract: Prune（Hook ベース）
  PreLlmRequest Hook で古いツール出力を削除
  設計原則3に従い、新しい抽象は作らない

consolidation: Compact（Agent ベース）
  OnTurnEnd Hook でトークン数をチェック
  閾値超過時に要約生成を挿入
  Workerのresume機構で作業を継続
```

**設計の要点:**
- Prune は `PreLlmRequest` Hook で history を変更する（Mutable state で可能）
- Compact は Pod レベルの制御（Controller が要約 Pod を起動）
- OpenCode の「構造化要約フォーマット」は良い規約 → system prompt に含める

---

### 3.4 Event Bus / Typed Events（重要度: 中）

**OpenCodeの設計:**
- 型付きイベント定義（Zod スキーマ）
- Instance スコープ + Global スコープの二段バス
- `publish` / `subscribe` / `subscribeAll` の3操作

**Insomniaの現状:**
- `broadcast::Sender<Event>` による単一チャネル
- Event enum で型安全
- Pod 単位のスコープのみ

**比較:**
Insomnia の broadcast channel は Pod 単位では十分。
ただし、**複数 Pod の協調**（Supervisor）段階で Global Bus が必要になる。

**取り込み案:**
- 現時点では不要（設計原則4）
- Supervisor 実装時に参考にする設計として記録
- OpenCode の「Instance スコープ → Global 伝播」パターンは、Pod スコープ → Supervisor 伝播に自然に対応

---

### 3.5 スナップショットシステム（重要度: 中）

**OpenCodeの設計:**
- 内部 git リポジトリでファイル変更を追跡
- ツール実行前にスナップショット取得
- `restore` / `revert` / `diff` 操作

**Insomniaの現状:**
- Scope（書き込み制約）はあるが、変更追跡・復元は未実装

**取り込み案:**
- ファイル操作ツールの実装時に、git ベースのスナップショットを組み込む
- `PreToolCall` / `PostToolCall` Hook で自然に差し込める
- OpenCode の sparse checkout アプローチ（変更ファイルのみ追跡）は効率的

**今すぐ実装すべきか:** まだ。ファイル操作ツールの実装後。

---

### 3.6 Agent/Subagent パターン（重要度: 中）

**OpenCodeの設計:**
- Primary Agent（build, plan）と Subagent（explore, general）の区別
- Agent ごとにモデル・パーミッション・プロンプトを個別設定
- `steps` パラメータでサブエージェントの反復回数を制限
- 親セッションのコンテキストを子に渡す

**Insomniaの現状:**
- Pod は独立実行単位。Pod 間通信は未実装
- 拡張ポイント表に「Supervisor」として記載

**比較:**
OpenCode の Agent は Session 内のモード切り替え。
Insomnia の Pod は完全に独立したプロセス。

**取り込み案:**
- OpenCode の `steps`（最大反復回数）は Pod マニフェストに追加する価値あり
  - `[worker]` セクションに `max_turns` を追加
  - Worker の `OnTurnEnd` Hook で制御
- Agent テンプレート（名前 + モデル + プロンプト + パーミッション）は
  マニフェストがすでにこの役割を果たしている → 追加不要
- Subagent パターンは Supervisor の責務 → 設計原則5に従い Pod 内部には入れない

---

### 3.7 Config 階層（重要度: 低〜中）

**OpenCodeの設計:**
- Managed → Remote → Global → Env → Project → Workspace の6段階
- 配列フィールドはマージ（上書きではなく結合）
- Plugin の出自を追跡（PluginOrigin）

**Insomniaの現状:**
- マニフェスト（TOML）のみ。階層なし

**取り込み案:**
- Daemon 実装時にグローバル設定が必要になる
- OpenCode の「マージ戦略の明示」は参考になる
  - 単純上書き vs 配列結合 vs 深いマージ
- 現時点ではマニフェスト一本で正しい（設計原則4）

---

### 3.8 LSP 統合（重要度: 低）

**OpenCodeの設計:**
- ファイル拡張子でサーバーを自動選択
- ワークスペースルートを marker ファイルから検出
- 遅延初期化（必要時にのみ起動）
- graceful degradation（サーバーなし → 無視）

**Insomniaの現状:**
- LSP の言及なし

**取り込み案:**
- コーディングエージェントとして使う場合にのみ必要
- ツールとして実装する際に OpenCode のパターンを参考にする
- Hook ベースの差し込みではなく、Tool 実装として提供

---

## 4. 設計思想の根本的な違い

### Insomnia: 「Pod は独立した実行単位」

- 各 Pod が完結したプロセス
- 協調は外部（Supervisor）が行う
- Unix の哲学に近い

### OpenCode: 「Session が状態を持ち、Agent が振る舞いを切り替える」

- Session 内で Agent を動的に切り替え
- Session がコンテキストの境界
- アプリケーションの哲学に近い

**この違いは意図的であり、変える必要はない。**
Insomnia のアプローチは長時間自律実行に適しており、Pod の独立性がフォールトトレランスと拡張性の基盤になる。

---

## 5. 優先度付きアクション項目

### 今すぐ設計に反映（基礎段階で重要）

| # | 項目 | 理由 | 実装場所 |
|---|------|------|----------|
| 1 | **max_turns をマニフェストに追加** | 暴走防止。OpenCodeのstepsに相当 | WorkerManifest |
| 2 | **コンテキスト圧縮の設計文書** | 長時間実行のコア要件。Hook ベースの Prune + Compact 方針を固める | docs/ |
| 3 | **パーミッションの拡張ポイント記録** | Pod 設計の拡張ポイント表にパターンベースのパーミッションを追加 | docs/pod.md |

### ツール実装時に取り込む

| # | 項目 | 理由 |
|---|------|------|
| 4 | ツール出力ヒント生成 | Stored 出力時に推奨アクションを含める |
| 5 | スナップショット（git ベース） | ファイル操作ツールと組み合わせ |
| 6 | パーミッション Hook | PreToolCall で動的権限チェック |

### Supervisor/Daemon 実装時に取り込む

| # | 項目 | 理由 |
|---|------|------|
| 7 | Global Event Bus | 複数 Pod 間のイベント伝播 |
| 8 | Config 階層 | グローバル設定 + プロジェクト設定のマージ |

---

## 6. 取り込まないもの（と理由）

| 項目 | 理由 |
|------|------|
| Effect.ts 的な DI | Rust のジェネリクス + trait がすでにこの役割。再発明になる |
| SQLite 永続化 | JSONL append-only は Pod の独立性と相性が良い。変える理由がない |
| HTTP API サーバー | Unix Socket + JSONL は Pod のユースケースに最適。Daemon 層で HTTP を追加する選択肢はある |
| Session 内 Agent 切り替え | Pod = 独立実行単位 の設計方針と矛盾。Subagent は Supervisor の責務 |
| SolidJS TUI | Rust には ratatui 等がある。技術スタックの違い |
| Plugin システム | 設計原則4に反する。Hook で十分 |
