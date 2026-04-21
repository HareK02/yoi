# Claude Code コンテキスト管理リファレンス

調査日: 2026-04-12

## 概要

Claude Code は3層構造のコンテキスト管理を行う。
安価な局所操作から順に適用し、必要に応じてより重い操作にエスカレーションする。

```
Tier 1: MicroCompaction（ローカル、API コスト 0）
  ↓ それでもコンテキストが大きい場合
Tier 2: AutoCompact（API ベース要約、自動発動）
  ↓ ユーザーが明示的に要求
Tier 3: Full Compact（/compact コマンド）
```

---

## Tier 1: MicroCompaction

**API 呼び出しなし**のローカル操作。個別のアイテムを軽量に刈り込む。

### 対象

| 対象 | 操作 |
|------|------|
| 古いツール結果 | プレースホルダに置換（`"stored on disk, retrievable by path"`） |
| base64 画像 | 古いメッセージから除去 |
| 50K 文字超のツール出力 | ディスクに退避、パスで参照 |
| thinking ブロック | 直近ターン以外は除去 |

### 条件付き実行（cache-aware）

ツール結果クリア機能 `clear_tool_uses` には `clear_at_least` パラメータがある。

**「十分なトークンを削れる場合にだけ実行」** という判断を行い、
キャッシュ無効化コスト > 節約トークン数 となるケースを避ける。

これは Insomnia における条件付き Prune の直接的な先行事例。

### キャッシュへの影響

- ツール結果をクリアすると**プレフィクスが変わり、KV キャッシュが無効化される**
- Claude Code はこれを認識した上で、`clear_at_least` 閾値で損益を管理
- **14種のキャッシュ無効化ベクター**を `promptCacheBreakDetection.ts` で追跡
- system prompt を cached/uncached セクションに分離（`SYSTEM_PROMPT_DYNAMIC_BOUNDARY`）
- "sticky latch" パターンでモード切替によるキャッシュ破壊を防止

---

## Tier 2: AutoCompact

**API ベースの要約生成**。コンテキスト使用量が閾値を超えると自動発動。

### トリガー

- コンテキスト使用量が**ウィンドウの約 83.5%** に到達
  - 200K ウィンドウの場合、約 167K トークン
  - `CLAUDE_AUTOCOMPACT_PCT_OVERRIDE` 環境変数で調整可能（1-100）
- 約 33K トークンのバッファを確保（要約処理 + 応答生成用）

### 要約生成

- **最大 20,000 トークン**の構造化要約を生成
- 要約中は**ツール無効化**（`tools: []`）で副作用を防止
- `<analysis>` タグで chain-of-thought 推論を行い、最終要約から推論部分を除去

### 要約の構造（9セクション）

```
1. Primary Request and Intent（元の要求と意図）
2. Key Technical Concepts（重要な技術概念）
3. Files and Code Sections（ファイルとコードセクション）
4. Errors and Fixes（エラーと修正）
5. Problem Solving（問題解決の過程）
6. All User Messages（ツール結果以外の全ユーザーメッセージ）
7. Pending Tasks（未完了タスク）
8. Current Work（現在の作業）
9. Optional Next Steps（次のステップ案）
```

加えて「直近の会話からの直接引用」を要求し、作業の継続性を保証する。

### サーキットブレーカー

- 3回連続で AutoCompact が失敗すると、セッション残りで compaction を無効化
  - 定数: `MAX_CONSECUTIVE_AUTOCOMPACT_FAILURES = 3`
- コンテキスト再充填が3回連続で即座に発生（thrash loop）した場合もエラーで停止

### サーバーサイド Compaction API

Anthropic は `compact-2026-01-12` beta でサーバーサイド compaction を提供:

```
1. input_tokens が閾値超過
2. Claude が要約を生成（ツール無効）
3. 要約が `compaction` ブロックとして assistant メッセージに挿入
4. 次のリクエストで compaction ブロック以前のメッセージが自動ドロップ
```

`pause_after_compaction` オプション: compaction 後に `stop_reason: "compaction"` で一旦返し、
クライアントがファイル・プラン・メモリ等を再注入してから継続できる。

---

## Tier 3: Full Compact（`/compact`）

ユーザーが明示的に発動する完全圧縮。

### 圧縮後に再注入されるもの

| 再注入対象 | 上限 |
|------------|------|
| 要約 | 約 10K トークン |
| 直近アクセスファイル | 最大5ファイル、各 5,000 トークン |
| アクティブプラン | 全量 |
| CLAUDE.md / MEMORY.md | 全量 |
| 関連スキルスキーマ | 最大 25K トークン |
| フック結果 | 全量 |
| 直近メッセージ | 約 20 メッセージ |

圧縮後の作業バジェットは約 50,000 トークンにリセットされる。

---

## Anthropic Prompt Caching の仕様

Claude Code のコンテキスト管理を理解するために必要な、プロバイダ側のキャッシュ仕様。

### プレフィクスベース

- リクエストの先頭から `cache_control` ブレークポイントまでの内容がキャッシュ対象
- **1バイトでも変わるとそこから先は全てミス**
- キャッシュは因果的（causal）: アイテム N の KV が変わるとN 以降すべて再計算

### A を送った後に A+B を送った場合

- **A のキャッシュは保持される**（独立した TTL）
- A+B のリクエストで A はキャッシュから読まれ、B だけ新規処理
- **複数のキャッシュエントリが共存**する

### TTL

| タイプ | 時間 | 書き込みコスト |
|--------|------|----------------|
| ephemeral（デフォルト） | 5分（ヒットで更新） | 基本入力の 1.25倍 |
| 拡張 | 1時間 | 基本入力の 2倍 |

読み取り: 基本入力の **0.1倍**（90% 割引）。1回のキャッシュヒットで書き込みコストを回収。

### 最小キャッシュ可能トークン

| モデル | 最小トークン |
|--------|-------------|
| Opus 4.6 / 4.5 | 4,096 |
| Sonnet 4.6 | 2,048 |
| Sonnet 4.5 / 4 / 3.7, Opus 4.1 / 4 | 1,024 |
| Haiku 4.5 / 3 | 4,096 |

### ブレークポイント

- 最大 **4個** の明示的ブレークポイント
- 自動キャッシング: 最後のキャッシュ可能ブロックにブレークポイントを自動配置
- **20ブロック lookback**: ブレークポイントから20ブロック以上離れたキャッシュは検索されない

### キャッシュ階層と無効化

```
tools → system → messages
```

上位の変更は下位すべてを無効化:
- tools 変更 → tools + system + messages 全て無効
- system 変更 → system + messages 無効
- messages 変更 → messages のみ無効

---

## OpenAI / Gemini との比較

| | Anthropic | OpenAI | Gemini |
|---|---|---|---|
| 制御 | 明示的 `cache_control` | 完全自動 | 明示 + 暗黙 |
| 書き込みコスト | +25% / +100% | **無料** | 標準入力レート |
| 読み取り割引 | 90% | 50-90% | 90% |
| TTL | 5分（更新あり）/ 1h | 5-10分 / 最大1h / 24h | デフォルト1h / 任意 |
| ヒット保証 | 確定的 | 確率的 | 明示=確定 / 暗黙=確率的 |
| 最小トークン | 1,024-4,096 | 1,024 | 1,024-4,096 |

### 共通原則

- **全プロバイダでプレフィクスベース**
- A を送った後に A+B を送ると、A のキャッシュは保持され再利用される
- プレフィクスの途中を変更するとそこから先は再計算
- これが Prune のキャッシュコストの根本原因

---

## OpenCode の Prune 関連 Issue

OpenCode（sst/opencode）でも Prune とキャッシュの問題は未解決。

| Issue | 内容 |
|-------|------|
| [#3917](https://github.com/sst/opencode/issues/3917) | Prune のドキュメント要求。**キャッシュ無効化が caveat として明記** |
| [#21208](https://github.com/sst/opencode/issues/21208) | 固定閾値（40k/20k）が 1M コンテキストモデルに対して不適切 |
| [#20826](https://github.com/sst/opencode/issues/20826) | Prune で tool output 消去 → 孤立した tool_call で API エラー |
| [#19081](https://github.com/sst/opencode/issues/19081) | thinking 除去で KV キャッシュが毎ターン無効化 |
| [#4416](https://github.com/sst/opencode/issues/4416) | キャッシュトークン二重カウントで premature compaction |
| [#2945](https://github.com/sst/opencode/issues/2945) | Compaction でワーキングコンテキスト全喪失 |
| [#6535](https://github.com/sst/opencode/issues/6535) | サブエージェントが compaction 後にシステムプロンプトを喪失 |

**「cache-aware な prune は未解決問題」** がコミュニティの共通認識。
サードパーティの Dynamic Context Pruning プラグインも「prune はキャッシュプレフィクスを壊す」と認めている。

---

## Insomnia 設計への示唆

### 1. Prune は条件付きで実行すべき

Claude Code の `clear_at_least` パターン:
- 「削れるトークン数がキャッシュ再計算コストを上回る場合にだけ prune」
- 無条件 prune はキャッシュを無駄に壊す

### 2. Compact は必須

- AutoCompact（83.5% 閾値）+ サーキットブレーカー（3回失敗で停止）は堅実な設計
- 要約後の再注入（ファイル、プラン、メモリ）で作業継続性を確保
- Anthropic のサーバーサイド compaction API は将来的に利用可能

### 3. キャッシュ階層を意識した設計

- system prompt は静的部分と動的部分を分離
- ツール定義の変更はキャッシュ全体を無効化する → 動的ツール登録/削除はキャッシュコストがある
- messages 部分の変更は messages キャッシュのみ影響

### 4. 既知の落とし穴

- Prune で ToolCall の対応する ToolResult を消すと API エラー（OpenCode #20826）
- キャッシュトークンの二重カウントで premature compaction（OpenCode #4416）
- Compaction 後のコンテキスト喪失（計画、サブエージェント、スキーマ）
- Thrash loop（compaction 直後に再び閾値超過）の検出と停止が必要
