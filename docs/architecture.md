# Insomnia アーキテクチャ

## プロジェクトの目的

複数の LLM エージェント（Pod）が独立プロセスとして並行動作し、自律的に分業・統括できるエンジン。シングルエージェントの LLM リグとの差別化は、タスクを別 Pod に委譲し結果を集約する**オーケストレーション**を LLM 自身に行わせる点にある。

## 設計原則

### 層の分離と方向性

各クレートは下位層に依存し、上位層を知らない。上位層は下位層の API を丸ごと隠蔽せず、制約が必要な部分だけをラップして提供し、それ以外は下位層を直接使わせる。

### 宣言した層が解決する

ある層が構成を宣言として受け取ったなら、その解決もその層の責務。マニフェストに `provider.kind = "anthropic"` と書いた以上、`ProviderConfig` → `LlmClient` の変換は insomnia が行う。逆に llm-worker が `LlmClient` trait だけを受け取るのは正しい — llm-worker はプロバイダの選択を宣言として受け取っていないから。

### 概念の追加は不在が問題になってから

概念を先に作らない。「これがないと今の問題が解けない」と言えるまで追加しない。拡張ポイントはドキュメントに記録するが、実装はしない。

### 最小の構造化で最大の自由度

insomnia は環境再現・コンテナ管理・VCS 統合などを自身の責務としない。Pod が動くホストの fs 上で活動する主体を提供し、それにコンテキストを与えてスポーンさせられる仕組みを付与する。構築する環境はユーザー次第。

## Pod

独立したエージェントの実行単位。llm-worker の Worker をラップし、マニフェストによる宣言的構成とディレクトリスコープを加える。

- 1 Pod = 1 プロセス = 1 セッション
- マニフェスト（TOML）から完結構築できる
- scope で書き込み可能なパスを制限（読み取りは自由）
- 独立した socket サーバーを持ち、Client (TUI / GUI) が接続して操作する

### 実行ループ

```
Client → Method::Run { input }
  → Worker: user message 追加 → LLM リクエスト
    → LLM 応答: text → Client に Event::TextDelta で配信
    → LLM 応答: tool_use → Interceptor (pre_tool_call) → Tool 実行 → Interceptor (post_tool_call) → Hook
    → tool result を履歴に追加 → 次の LLM リクエスト
  → ターン終了: Hook (on_turn_end) → Client に Event::RunEnd
```

### Interceptor と Hook の分離

- **Interceptor**: Worker の内部制御フロー。context / history の直接操作が可能。Pod の内部機構（compaction トリガー、notification 注入）が使う。外部に公開しない
- **Hook**: Pod の公開 API。read-only のサマリ情報のみを受け取り、制御フロー判断（continue / skip / abort / pause）を返す。将来スクリプト言語に安全に公開可能
- 実行順序: Interceptor → Hook。Interceptor が context を整えた結果を Hook が観測する

### コンテキスト管理

- **Prune**: 古い tool result の content を除去（summary は残す）。`pre_llm_request` で毎回判定
- **Compact**: 履歴全体を要約して圧縮。`input_tokens` が閾値を超えたとき、`PreRequestAction::Yield` で Worker を一旦中断し、Pod 側で要約 → 新セッションとして再開
- サーキットブレーカー: compact が3回連続失敗したら無効化

## Protocol

Pod の制御・監視に使う JSONL ベースのメッセージプロトコル。トランスポートに依存しない。

- **Method** (Client → Pod): `Run` / `Notify` / `Resume` / `Cancel` / `Shutdown` / `GetHistory`
- **Event** (Pod → Client, broadcast): `TurnStart` / `TurnEnd` / `TextDelta` / `ToolCallStart` / `ToolCallArgsDelta` / `ToolCallDone` / `ToolResult` / `Usage` / `RunEnd` / `Error` / `History` / `Notification` / `Shutdown`
- リクエストとレスポンスの紐付けはしない。Pod の状態遷移（イベント）を見れば何が起きているか分かる
- イベントは全リスナーに broadcast
- 操作の競合は先勝ち（run 中に別の run → AlreadyRunning エラー）

## マニフェストとファクトリ

### PodManifest

Pod の宣言的構成。TOML で記述。

```toml
[pod]
name = "agent"
pwd = "/abs/path"

[model]
scheme = "anthropic"
model_id = "claude-sonnet-4-20250514"

[worker]
instruction = "$insomnia/default"
max_tokens = 4096

[[scope.allow]]
target = "/abs/path"
permission = "write"
```

### PodFactory: カスケード設定

マニフェストを手書きせず、4 層のカスケードで `PodManifest` を組み立てる:

1. **ビルトインデフォルト** — `manifest::defaults` の定数値
2. **ユーザー manifest** — `$XDG_CONFIG_HOME/insomnia/manifest.toml`
3. **プロジェクト manifest** — `.insomnia/manifest.toml`（cwd から上方向に探索）
4. **プログラマティック overlay** — CLI / GUI / spawn 時のインライン指定

マージ規則: スカラーは上層が置換、Map はキー単位マージ、`scope.allow` / `scope.deny` は union。全パスは絶対パスのみ。

### Instruction とプロンプト資産

`worker.instruction` はファイル参照。3 層の prefix addressing でプロンプト資産を解決:

- `$insomnia/...` — バイナリ同梱（`resources/prompts/`、`include_dir!` で埋め込み）
- `$user/...` — `$XDG_CONFIG_HOME/insomnia/prompts/`
- `$workspace/...` — `<project>/.insomnia/prompts/`

テンプレートは minijinja で評価。`{% include "$insomnia/common/tool-usage" %}` のようにプロンプト間で参照可能（prefix なしの include は現在のファイルからの相対解決）。

レンダリング結果の末尾に scope summary と AGENTS.md（あれば）がコード側で固定付加される。ユーザーテンプレートからはこれらに触れない。

## Scope

Pod が操作できるファイルパスの制御。

- `allow` ルールで読み取り・書き込みを許可、`deny` ルールで制限
- effective permission = allow - deny
- `recursive = false` で直下のみに制限可能（summary に `[non-recursive]` マーカー）
- scope 排他: Pod 間の write 衝突は scope lock file (`$XDG_RUNTIME_DIR/insomnia/scope.lock`) で検出。scope 分譲（spawn 時に譲渡、終了時に返却）の記録にも使う

## セッション永続化

- append-only JSONL ログ。1 エントリ = 1 行
- SHA-256 チェーンでエントリの整合性を保証
- ログ再生で Worker の状態を完全復元（スナップショット不要）
- Compact 時に新セッションを開始し、旧セッションへのリンクを保持

## 組み込みツール

| ツール | 概要                                       |
| ------ | ------------------------------------------ |
| Read   | ファイル内容の読み取り                     |
| Write  | ファイルの新規作成・上書き                 |
| Edit   | 既存ファイルの部分編集（事前 Read が必要） |
| Glob   | ファイル名パターンマッチ                   |
| Grep   | ファイル内容の正規表現検索                 |

すべて scope の permission チェックを経由。`ScopedFs` が書き込み制限を、`Tracker` がセッション内のコンテンツハッシュ追跡を行う。
